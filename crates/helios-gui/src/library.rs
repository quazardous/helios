//! The library: the vault's images as a grid (the Vault panel) and as a
//! filmstrip (one row, bottom), with their rating, reject and color labels.
//!
//! The shell does not hold the vault: the host gives it the images
//! ([`Shell::set_library`](crate::Shell::set_library)), sends their
//! thumbnails as they are read (from any thread, through
//! [`Library::thumbnails`]), and applies what the user changes, collected
//! with [`Shell::take_library_changes`](crate::Shell::take_library_changes)
//! as vault field commands. The shown values change at once; the host
//! puts back what the vault holds ([`Library::show_field`]) when a write
//! fails or another writer changes a field.
//!
//! A change says what the user did, never "here is my copy": a color label
//! is added or removed, so a label another writer put on meanwhile stays,
//! and a reject is a field of its own, so taking it back keeps the stars.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, ColorImage, TextureHandle, Ui};
use helios_core::{Image, Message, Region};
pub use helios_vault::fields::Op;
use helios_vault::fields::{LABELS as LABELS_FIELD, RATING, REJECTED};
use serde_json::Value;

/// The labels, in the order darktable gives them F1 to F5.
pub const LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

/// An image of the library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    /// The vault's id (hex).
    pub id: String,
    /// What to call it: its file name.
    pub name: String,
    /// 0 to 5 stars.
    pub rating: i64,
    /// Rejected; the stars stay beside it.
    pub rejected: bool,
    pub labels: Vec<String>,
    /// The file's size in bytes, when it is on disk.
    pub bytes: Option<u64>,
    /// When the file was last modified, in seconds since 1970 (UTC); shown
    /// in the system's time zone.
    pub modified: Option<i64>,
}

/// A file size as the interface's language writes it: `12.3 MiB`,
/// `12,3 Mio` in French.
pub fn size_text(ctx: &egui::Context, bytes: u64) -> String {
    let locale = crate::i18n::locale(ctx);
    let b = bytes as f64;
    let (unit, value) = if bytes < 1024 {
        (Message::new("gui-size-bytes", "{ $size } B"), b)
    } else if b < 1024.0 * 1024.0 {
        (Message::new("gui-size-kib", "{ $size } KiB"), b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        (
            Message::new("gui-size-mib", "{ $size } MiB"),
            b / (1024.0 * 1024.0),
        )
    } else {
        (
            Message::new("gui-size-gib", "{ $size } GiB"),
            b / (1024.0 * 1024.0 * 1024.0),
        )
    };
    let size = crate::i18n::number(&locale, value, 1);
    crate::i18n::text(ctx, unit.with("size", size))
}

/// A moment (seconds since 1970) as the calendar day and time it was in
/// time zone `tz`, its offset at that moment (summer time included).
pub fn day_in<Tz: chrono::TimeZone>(tz: &Tz, seconds: i64) -> egui_pop::i18n::format::Date {
    use chrono::Offset;
    let offset = chrono::DateTime::from_timestamp(seconds, 0).map_or(0, |utc| {
        i64::from(
            tz.offset_from_utc_datetime(&utc.naive_utc())
                .fix()
                .local_minus_utc(),
        )
    });
    egui_pop::i18n::format::Date::from_unix(seconds + offset)
}

/// What the library says of an image's file beside its name: its size and
/// the day it was last modified, in the interface's language and the
/// system's time zone.
pub fn file_details(ctx: &egui::Context, entry: &Entry) -> Option<String> {
    file_details_in(ctx, entry, &chrono::Local)
}

/// [`file_details`] with the day as it was in time zone `tz`.
pub fn file_details_in<Tz: chrono::TimeZone>(
    ctx: &egui::Context,
    entry: &Entry,
    tz: &Tz,
) -> Option<String> {
    use egui_pop::i18n::format::{DateStyle, date};
    let locale = crate::i18n::locale(ctx);
    let size = entry.bytes.map(|b| size_text(ctx, b));
    let day = entry
        .modified
        .map(|s| date(&locale, &day_in(tz, s), DateStyle::Medium));
    match (size, day) {
        (Some(size), Some(day)) => Some(crate::i18n::text(
            ctx,
            Message::new("gui-library-file", "{ $size } · modified { $day }")
                .with("size", size)
                .with("day", day),
        )),
        (Some(one), None) | (None, Some(one)) => Some(one),
        (None, None) => None,
    }
}

impl Entry {
    /// Shows a field's stored value (`None`: the field is empty).
    pub fn show(&mut self, field: &str, value: Option<&Value>) {
        match field {
            RATING => self.rating = value.and_then(Value::as_i64).unwrap_or(0),
            REJECTED => self.rejected = value.and_then(Value::as_bool).unwrap_or(false),
            LABELS_FIELD => {
                self.labels = value
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                self.labels
                    .sort_by_key(|l| LABELS.iter().position(|k| k == l));
            }
            _ => {}
        }
    }
}

/// A command on one field of one image, for the vault.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub image: String,
    pub field: String,
    pub op: Op,
}

impl Change {
    /// What a later change replaces: a set replaces a set, adding or
    /// removing a label replaces a change of the same label.
    fn replaces(&self, other: &Change) -> bool {
        self.image == other.image
            && self.field == other.field
            && match (&self.op, &other.op) {
                (Op::Add(a) | Op::Remove(a), Op::Add(b) | Op::Remove(b)) => a == b,
                (Op::Set(_), Op::Set(_)) => true,
                _ => false,
            }
    }
}

/// The longest side of a thumbnail, in pixels.
pub const THUMB: u32 = 256;

pub struct Library {
    pub entries: Vec<Entry>,
    pub selected: Option<usize>,
    /// The selection moved: the filmstrip scrolls to it once.
    reveal: bool,
    changes: Vec<Change>,
    sender: Sender<(String, Image)>,
    receiver: Receiver<(String, Image)>,
    pictures: HashMap<String, ColorImage>,
    textures: HashMap<String, TextureHandle>,
}

impl Default for Library {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Library {
            entries: Vec::new(),
            selected: None,
            reveal: false,
            changes: Vec::new(),
            sender,
            receiver,
            pictures: HashMap::new(),
            textures: HashMap::new(),
        }
    }
}

/// An image scaled to no larger than [`THUMB`]: what a thread reading
/// thumbnails sends, so the interface thread never scales a full-size
/// preview (scaling a small one again costs a copy).
pub fn small(image: &Image) -> Image {
    let r = image.region();
    let scale = (f64::from(THUMB) / f64::from(r.width.max(r.height).max(1))).min(1.0);
    let w = ((f64::from(r.width) * scale).round() as u32).max(1);
    let h = ((f64::from(r.height) * scale).round() as u32).max(1);
    let at = |v: i64| (v as f64 * scale).floor() as i64;
    image.sample_at(
        Region {
            x: at(r.x),
            y: at(r.y),
            width: w,
            height: h,
        },
        scale,
    )
}

/// An image in the working space (linear Rec.2020) as an sRGB picture no
/// larger than [`THUMB`].
pub fn thumbnail(image: &Image) -> ColorImage {
    use helios_photo::color;
    let small = small(image);
    let (w, h) = (small.region().width, small.region().height);
    let to_srgb = color::mul(
        &color::inverse(&color::SRGB_TO_XYZ).expect("invertible primaries"),
        &color::REC2020_TO_XYZ,
    );
    let n = small.channels() as usize;
    let pixels = small
        .data()
        .chunks_exact(n)
        .map(|p| {
            let rgb = [0, 1, 2].map(|c| f64::from(p[c.min(n - 1)]));
            let v = color::apply(&to_srgb, rgb)
                .map(|v| (color::srgb_encode(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
            Color32::from_rgb(v[0], v[1], v[2])
        })
        .collect();
    ColorImage::new([w as usize, h as usize], pixels)
}

impl Library {
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        let selected = self
            .selected
            .and_then(|i| self.entries.get(i))
            .map(|e| e.id.clone());
        self.entries = entries;
        self.selected = selected.and_then(|id| self.entries.iter().position(|e| e.id == id));
    }

    /// Where thumbnails are sent, by image id, from any thread.
    pub fn thumbnails(&self) -> Sender<(String, Image)> {
        self.sender.clone()
    }

    pub fn take_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.changes)
    }

    pub fn select(&mut self, index: usize) {
        if index < self.entries.len() {
            self.selected = Some(index);
            self.reveal = true;
        }
    }

    /// Moves the selection by `step`, within bounds.
    pub fn step(&mut self, step: i64) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as i64 - 1;
        let at = self
            .selected
            .map_or(0, |i| (i as i64 + step).clamp(0, last));
        self.selected = Some(at as usize);
        self.reveal = true;
    }

    /// Shows a change on the selected image at once, and queues it.
    fn change(&mut self, field: &str, op: Op) -> Result<(), String> {
        let i = self.selected.ok_or("no image selected")?;
        let entry = &mut self.entries[i];
        match &op {
            Op::Set(v) => entry.show(field, v.as_ref()),
            Op::SetIfAbsent(_) => {}
            Op::Add(label) => {
                if !entry.labels.contains(label) {
                    entry.labels.push(label.clone());
                    entry
                        .labels
                        .sort_by_key(|l| LABELS.iter().position(|k| k == l));
                }
            }
            Op::Remove(label) => entry.labels.retain(|l| l != label),
        }
        let change = Change {
            image: entry.id.clone(),
            field: field.into(),
            op,
        };
        // A later change replaces an earlier one of the same thing.
        self.changes.retain(|c| !change.replaces(c));
        self.changes.push(change);
        Ok(())
    }

    /// Rates the selected image, 0 to 5 stars; a rejected image is taken
    /// back from the reject, as in darktable.
    pub fn rate(&mut self, rating: i64) -> Result<(), String> {
        if !(0..=5).contains(&rating) {
            return Err(format!("a rating is 0 to 5 stars, not {rating}"));
        }
        let i = self.selected.ok_or("no image selected")?;
        if self.entries[i].rejected {
            self.change(REJECTED, Op::Set(None))?;
        }
        self.change(RATING, Op::Set(Some(Value::from(rating))))
    }

    /// Rejects the selected image, or takes the reject back; the stars
    /// are kept either way.
    pub fn toggle_reject(&mut self) -> Result<(), String> {
        let i = self.selected.ok_or("no image selected")?;
        let op = if self.entries[i].rejected {
            Op::Set(None)
        } else {
            Op::Set(Some(Value::from(true)))
        };
        self.change(REJECTED, op)
    }

    /// Puts a color label on the selected image, or takes it off: one
    /// label added or removed, whatever else the set holds in the vault.
    pub fn toggle_label(&mut self, label: &str) -> Result<(), String> {
        if !LABELS.contains(&label) {
            return Err(format!("`{label}` is not one of {}", LABELS.join(", ")));
        }
        let i = self.selected.ok_or("no image selected")?;
        let op = if self.entries[i].labels.iter().any(|l| l == label) {
            Op::Remove(label.into())
        } else {
            Op::Add(label.into())
        };
        self.change(LABELS_FIELD, op)
    }

    /// Shows a field as the vault holds it, for an image of the library:
    /// after another writer changed it, or after a write failed.
    pub fn show_field(&mut self, image: &str, field: &str, value: Option<&Value>) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == image) {
            entry.show(field, value);
        }
    }

    fn receive(&mut self, ui: &Ui) {
        let mut any = false;
        while let Ok((id, image)) = self.receiver.try_recv() {
            self.pictures.insert(id.clone(), thumbnail(&image));
            self.textures.remove(&id);
            any = true;
        }
        if any {
            ui.ctx().request_repaint();
        }
    }

    fn texture(&mut self, ui: &Ui, id: &str) -> Option<TextureHandle> {
        if let Some(t) = self.textures.get(id) {
            return Some(t.clone());
        }
        let picture = self.pictures.get(id)?.clone();
        let t = ui.ctx().load_texture(
            format!("helios.library.{id}"),
            picture,
            egui::TextureOptions::LINEAR,
        );
        self.textures.insert(id.into(), t.clone());
        Some(t)
    }

    /// One cell: the thumbnail fitted in `size`, then the stars and labels.
    fn cell(&mut self, ui: &mut Ui, index: usize, size: f32) -> egui::Response {
        let entry = self.entries[index].clone();
        let selected = self.selected == Some(index);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(size, size + 16.0), egui::Sense::click());
        let visuals = ui.visuals();
        let painter = ui.painter_at(rect);
        let frame = if selected {
            visuals.selection.bg_fill
        } else {
            visuals.faint_bg_color
        };
        painter.rect_filled(rect, 3.0, frame);
        let area = egui::Rect::from_min_size(rect.min, egui::vec2(size, size)).shrink(4.0);
        if let Some(t) = self.texture(ui, &entry.id) {
            let [w, h] = t.size().map(|v| v as f32);
            let fit = (area.width() / w).min(area.height() / h);
            let shown = egui::Rect::from_center_size(area.center(), egui::vec2(w, h) * fit);
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            let tint = if entry.rejected {
                Color32::from_gray(90)
            } else {
                Color32::WHITE
            };
            painter.image(t.id(), shown, uv, tint);
        } else {
            painter.text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                &entry.name,
                egui::FontId::proportional(11.0),
                visuals.weak_text_color(),
            );
        }
        let line = egui::pos2(rect.min.x + 4.0, rect.max.y - 9.0);
        let stars = if entry.rejected {
            "✕".to_string()
        } else {
            "★".repeat(entry.rating.clamp(0, 5) as usize)
        };
        painter.text(
            line,
            egui::Align2::LEFT_CENTER,
            stars,
            egui::FontId::proportional(11.0),
            visuals.text_color(),
        );
        for (k, label) in entry.labels.iter().enumerate() {
            let color = match label.as_str() {
                "red" => Color32::from_rgb(220, 60, 60),
                "yellow" => Color32::from_rgb(230, 200, 50),
                "green" => Color32::from_rgb(70, 180, 80),
                "blue" => Color32::from_rgb(60, 120, 230),
                _ => Color32::from_rgb(160, 80, 200),
            };
            let c = egui::pos2(rect.max.x - 8.0 - k as f32 * 10.0, line.y);
            painter.circle_filled(c, 4.0, color);
        }
        let hover = match file_details(ui.ctx(), &entry) {
            Some(details) => format!("{}\n{details}", entry.name),
            None => entry.name.clone(),
        };
        response.on_hover_text(hover)
    }

    /// The grid: as many columns as fit.
    pub fn grid(&mut self, ui: &mut Ui) {
        self.receive(ui);
        if self.entries.is_empty() {
            ui.weak("No images: import a folder into the vault");
            return;
        }
        let size = 120.0;
        let columns = ((ui.available_width() / (size + 6.0)).floor() as usize).max(1);
        egui::ScrollArea::vertical()
            .id_salt("helios.library.grid")
            .show(ui, |ui| {
                for row in 0..self.entries.len().div_ceil(columns) {
                    ui.horizontal(|ui| {
                        for i in row * columns..((row + 1) * columns).min(self.entries.len()) {
                            let r = self.cell(ui, i, size);
                            crate::name_rect(ui.ctx(), &format!("helios.library.grid.{i}"), r.rect);
                            if r.clicked() {
                                self.select(i);
                            }
                        }
                    });
                }
            });
    }

    /// The filmstrip: one row, the selection kept in view.
    pub fn filmstrip(&mut self, ui: &mut Ui) {
        self.receive(ui);
        if self.entries.is_empty() {
            ui.weak("No images");
            return;
        }
        let size = (ui.available_height() - 16.0).clamp(48.0, 160.0);
        egui::ScrollArea::horizontal()
            .id_salt("helios.library.strip")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for i in 0..self.entries.len() {
                        let r = self.cell(ui, i, size);
                        crate::name_rect(ui.ctx(), &format!("helios.library.strip.{i}"), r.rect);
                        if r.clicked() {
                            self.select(i);
                        }
                        if self.reveal && self.selected == Some(i) {
                            r.scroll_to_me(None);
                        }
                    }
                });
                self.reveal = false;
            });
    }
}
