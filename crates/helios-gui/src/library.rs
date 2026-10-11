//! The library: the vault's images as a grid (the Vault panel) and as a
//! filmstrip (one row, bottom), with their rating, reject and color labels.
//!
//! The shell does not hold the vault: the host gives it the images
//! ([`Shell::set_library`](crate::Shell::set_library)), sends their
//! thumbnails as they are read (from any thread, through
//! [`Library::thumbnails`]), and applies what the user changes, collected
//! with [`Shell::take_library_changes`](crate::Shell::take_library_changes)
//! as vault field changes. The shown values change at once.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, ColorImage, TextureHandle, Ui};
use helios_core::{Image, Region};
use serde_json::Value;

/// The labels, in the order darktable gives them F1 to F5.
pub const LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

/// The rating meaning "rejected" (as in darktable and the vault).
pub const REJECTED: i64 = -1;

/// An image of the library.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The vault's id (hex).
    pub id: String,
    /// What to call it: its file name.
    pub name: String,
    pub rating: i64,
    pub labels: Vec<String>,
}

/// A change to one field of one image, for the vault.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub image: String,
    pub field: String,
    pub value: Option<Value>,
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

/// An image in the working space (linear Rec.2020) as an sRGB picture no
/// larger than [`THUMB`].
pub fn thumbnail(image: &Image) -> ColorImage {
    use helios_photo::color;
    let r = image.region();
    let scale = (f64::from(THUMB) / f64::from(r.width.max(r.height).max(1))).min(1.0);
    let w = ((f64::from(r.width) * scale).round() as u32).max(1);
    let h = ((f64::from(r.height) * scale).round() as u32).max(1);
    let at = |v: i64| (v as f64 * scale).floor() as i64;
    let small = image.sample_at(
        Region {
            x: at(r.x),
            y: at(r.y),
            width: w,
            height: h,
        },
        scale,
    );
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

    fn change(&mut self, field: &str, value: Option<Value>) -> Result<(), String> {
        let i = self.selected.ok_or("no image selected")?;
        let entry = &mut self.entries[i];
        match field {
            "rating" => entry.rating = value.as_ref().and_then(Value::as_i64).unwrap_or(0),
            _ => {
                entry.labels = value
                    .as_ref()
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
            }
        }
        let image = entry.id.clone();
        // The last change to a field wins; earlier ones need not be sent.
        self.changes
            .retain(|c| !(c.image == image && c.field == field));
        self.changes.push(Change {
            image,
            field: field.into(),
            value,
        });
        Ok(())
    }

    /// Rates the selected image, 0 to 5 stars or [`REJECTED`].
    pub fn rate(&mut self, rating: i64) -> Result<(), String> {
        if !(REJECTED..=5).contains(&rating) {
            return Err(format!("a rating is {REJECTED} to 5, not {rating}"));
        }
        self.change("rating", Some(Value::from(rating)))
    }

    /// Rejects the selected image, or takes the reject back.
    pub fn toggle_reject(&mut self) -> Result<(), String> {
        let i = self.selected.ok_or("no image selected")?;
        let rating = if self.entries[i].rating == REJECTED {
            0
        } else {
            REJECTED
        };
        self.rate(rating)
    }

    /// Puts a color label on the selected image, or takes it off.
    pub fn toggle_label(&mut self, label: &str) -> Result<(), String> {
        if !LABELS.contains(&label) {
            return Err(format!("`{label}` is not one of {}", LABELS.join(", ")));
        }
        let i = self.selected.ok_or("no image selected")?;
        let mut labels = self.entries[i].labels.clone();
        if let Some(at) = labels.iter().position(|l| l == label) {
            labels.remove(at);
        } else {
            labels.push(label.into());
            labels.sort_by_key(|l| LABELS.iter().position(|k| k == l));
        }
        let value = (!labels.is_empty()).then(|| Value::from(labels));
        self.change("labels", value)
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
            let tint = if entry.rating == REJECTED {
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
        let stars = if entry.rating == REJECTED {
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
        response.on_hover_text(&entry.name)
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
