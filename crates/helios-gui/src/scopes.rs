//! The scopes panel: a histogram, a waveform or an RGB parade of the
//! pipeline's output (`helios-scopes` counts). The image is the
//! viewport's preview (`crate::preview`), or one set from outside
//! (`Shell::set_scope_image`).
//!
//! - The histogram is egui-pop's (`egui_pop::viewport::histogram`): the
//!   red, green and blue counts on a log scale, the clipping markers lit
//!   by the share of pixels at or below black and at or above white, and
//!   darktable's drag of the exposure: dragging it moves the exposure of
//!   the network shown by as many stops as the histogram moved (one undo
//!   step, the `histogram.drag` step first, Escape giving it up).
//! - The waveform and the parade are pictures computed once per image,
//!   mode and scale, into a texture.

use std::sync::Mutex;

use egui::{Color32, ColorImage, TextureHandle, Ui};
use egui_pop::api::AppHandle;
use egui_pop::viewport::histogram::{Channel, Clipping, Histogram, HistogramOutput};
use helios_core::{Image, Message, Value};
use helios_scopes::{Counts, Scale, histogram, parade, waveform};
use helios_shell::commands::Origin;
use helios_values::{Change, GestureId, Set, Store};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Histogram,
    Waveform,
    Parade,
}

pub struct Scopes {
    pub image: Option<Image>,
    pub mode: Mode,
    /// Stops (log2 around 1.0) rather than linear.
    pub stops: bool,
    texture: Option<(u64, TextureHandle)>,
    /// The histogram's counts and clipping, per generation.
    counts: Option<(u64, Counts3, Clipping)>,
    generation: u64,
    /// The preview last taken from the viewport.
    preview_seen: u64,
    /// A drag of the exposure under way: its path, its gesture, the
    /// value it started from.
    drag: Option<(String, GestureId, f64)>,
}

/// The red, green and blue bins, as egui-pop's histogram takes them.
type Counts3 = [Vec<f32>; 3];

impl Default for Scopes {
    fn default() -> Self {
        Scopes {
            image: None,
            mode: Mode::Histogram,
            stops: true,
            texture: None,
            counts: None,
            generation: 0,
            preview_seen: 0,
            drag: None,
        }
    }
}

/// The histogram's bins.
const BINS: usize = 256;

/// The share of pixels at or below black (any channel at or under 0) and
/// at or above white (any channel at or over 1).
pub fn clipping(image: &Image) -> Clipping {
    let n = image.channels() as usize;
    let (mut low, mut high, mut all) = (0usize, 0usize, 0usize);
    for p in image.data().chunks_exact(n.max(1)) {
        let rgb = &p[..n.min(3)];
        all += 1;
        if rgb.iter().any(|v| *v <= 0.0) {
            low += 1;
        }
        if rgb.iter().any(|v| *v >= 1.0) {
            high += 1;
        }
    }
    let share = |k: usize| if all == 0 { 0.0 } else { k as f32 / all as f32 };
    Clipping {
        shadows: share(low),
        highlights: share(high),
    }
}

/// How many stops one width of the histogram spans: the stops scale's
/// span, or four stops on the linear one.
pub fn stops_per_width(stops: bool) -> f64 {
    match scale(stops) {
        Scale::Stops { min, max } => f64::from(max - min),
        Scale::Linear { .. } => 4.0,
    }
}

/// The exposure the histogram drags: the `exposure` parameter of the
/// selected node when it is an exposure module, else of the first one
/// in the network's order.
pub fn exposure_path(store: &Store) -> Option<String> {
    let selected = match store.get(crate::params::SELECTION).map(|e| &e.value) {
        Some(Value::Text(t)) if !t.is_empty() => Some(format!("node/{t}/exposure")),
        _ => None,
    };
    let mut found: Vec<(&str, u64)> = store
        .list("node/")
        .into_iter()
        .filter(|(p, e)| {
            p.ends_with("/exposure") && e.descriptor.module.as_deref() == Some("exposure")
        })
        .map(|(p, e)| (p, e.order))
        .collect();
    found.sort_by_key(|(_, order)| *order);
    found
        .iter()
        .find(|(p, _)| Some(*p) == selected.as_deref())
        .or_else(|| found.first())
        .map(|(p, _)| (*p).to_string())
}

const WIDTH: usize = 256;
const HEIGHT: usize = 128;

fn scale(stops: bool) -> Scale {
    if stops {
        Scale::default()
    } else {
        Scale::Linear { max: 1.0 }
    }
}

/// How bright a count draws, on a log scale against the peak.
fn level(count: u32, peak: u32) -> f32 {
    if count == 0 || peak == 0 {
        0.0
    } else {
        ((count as f32).ln_1p() / (peak as f32).ln_1p()).clamp(0.0, 1.0)
    }
}

fn column(img: &mut ColorImage, x: usize, counts: &Counts, peak: u32, tint: [f32; 3]) {
    let levels = counts.bins.len();
    for (i, &c) in counts.bins.iter().enumerate() {
        let v = level(c, peak);
        if v == 0.0 {
            continue;
        }
        // Level 0 at the bottom.
        let y0 = HEIGHT - 1 - i * HEIGHT / levels;
        let y1 = HEIGHT - (i + 1) * HEIGHT / levels;
        for y in y1..=y0 {
            let p = &mut img.pixels[y * img.size[0] + x];
            let add = |old: u8, t: f32| (f32::from(old) + v * t * 255.0).min(255.0) as u8;
            *p = Color32::from_rgb(
                add(p.r(), tint[0]),
                add(p.g(), tint[1]),
                add(p.b(), tint[2]),
            );
        }
    }
}

/// The scope of an image as a picture, `WIDTH` × `HEIGHT`.
pub fn picture(image: &Image, mode: Mode, stops: bool) -> ColorImage {
    let mut img = ColorImage::filled([WIDTH, HEIGHT], Color32::from_rgb(12, 12, 12));
    let s = scale(stops);
    let tints = [[1.0, 0.25, 0.25], [0.25, 1.0, 0.25], [0.3, 0.45, 1.0]];
    match mode {
        Mode::Histogram => {
            let h = histogram(image, WIDTH, s);
            for (c, counts) in h.channels.iter().enumerate() {
                let peak = counts.peak();
                for (x, &n) in counts.bins.iter().enumerate() {
                    let tall = (level(n, peak) * HEIGHT as f32) as usize;
                    for y in HEIGHT - tall.min(HEIGHT)..HEIGHT {
                        let p = &mut img.pixels[y * WIDTH + x];
                        let t = tints[c];
                        let add = |old: u8, t: f32| (f32::from(old) + t * 110.0).min(255.0) as u8;
                        *p =
                            Color32::from_rgb(add(p.r(), t[0]), add(p.g(), t[1]), add(p.b(), t[2]));
                    }
                }
            }
        }
        Mode::Waveform => {
            // No more bands than the image has columns; each drawn wide.
            let bands = WIDTH.min(image.region().width.max(1) as usize);
            let w = waveform(image, bands, HEIGHT, s);
            let peak = w.columns.iter().map(Counts::peak).max().unwrap_or(0);
            for (b, counts) in w.columns.iter().enumerate() {
                for x in b * WIDTH / bands..(b + 1) * WIDTH / bands {
                    column(&mut img, x, counts, peak, [0.9, 0.9, 0.9]);
                }
            }
        }
        Mode::Parade => {
            let third = WIDTH / 3;
            let bands = third.min(image.region().width.max(1) as usize);
            let channels = parade(image, bands, HEIGHT, s);
            for (c, w) in channels.iter().enumerate() {
                let peak = w.columns.iter().map(Counts::peak).max().unwrap_or(0);
                for (b, counts) in w.columns.iter().enumerate() {
                    for x in b * third / bands..(b + 1) * third / bands {
                        column(&mut img, c * third + x, counts, peak, tints[c]);
                    }
                }
            }
        }
    }
    img
}

impl Scopes {
    pub fn set_image(&mut self, image: Image) {
        self.image = Some(image);
        self.generation += 1;
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.generation += 1;
    }

    pub fn set_stops(&mut self, stops: bool) {
        self.stops = stops;
        self.generation += 1;
    }

    /// Takes the viewport's preview when a newer one landed.
    pub fn follow(&mut self, preview: &crate::preview::Preview) {
        if let Some((n, image)) = preview.latest()
            && n != self.preview_seen
        {
            self.preview_seen = n;
            self.set_image((*image).clone());
        }
    }

    /// The panel, its histogram not draggable.
    pub fn ui(&mut self, ui: &mut Ui) {
        self.ui_with(ui, None);
    }

    /// The panel; with the application and the value store, the
    /// histogram drags the exposure of the network shown.
    pub fn ui_with(&mut self, ui: &mut Ui, edit: Option<(&AppHandle, &Mutex<Store>)>) {
        ui.horizontal(|ui| {
            for (mode, label) in [
                (Mode::Histogram, "Histogram"),
                (Mode::Waveform, "Waveform"),
                (Mode::Parade, "Parade"),
            ] {
                if ui.selectable_label(self.mode == mode, label).clicked() {
                    self.set_mode(mode);
                }
            }
            ui.separator();
            if ui.selectable_label(self.stops, "Stops").clicked() {
                self.set_stops(!self.stops);
            }
        });
        if self.image.is_none() {
            ui.weak("No image yet");
            return;
        }
        if self.mode == Mode::Histogram {
            self.histogram_ui(ui, edit);
            return;
        }
        let Some(image) = &self.image else { return };
        let stale = self
            .texture
            .as_ref()
            .is_none_or(|(g, _)| *g != self.generation);
        if stale {
            let picture = picture(image, self.mode, self.stops);
            let texture =
                ui.ctx()
                    .load_texture("helios.scopes", picture, egui::TextureOptions::LINEAR);
            self.texture = Some((self.generation, texture));
        }
        if let Some((_, texture)) = &self.texture {
            let width = ui.available_width();
            let size = egui::vec2(width, width * HEIGHT as f32 / WIDTH as f32);
            let response = ui.add(egui::Image::new((texture.id(), size)));
            crate::name_rect(ui.ctx(), "helios.scopes.picture", response.rect);
        }
    }

    /// egui-pop's histogram of the image, draggable when there is an
    /// exposure to drag.
    fn histogram_ui(&mut self, ui: &mut Ui, edit: Option<(&AppHandle, &Mutex<Store>)>) {
        let Some(image) = &self.image else { return };
        if self
            .counts
            .as_ref()
            .is_none_or(|(g, _, _)| *g != self.generation)
        {
            let h = histogram(image, BINS, scale(self.stops));
            let bins = h
                .channels
                .map(|c| c.bins.iter().map(|&n| n as f32).collect::<Vec<f32>>());
            self.counts = Some((self.generation, bins, clipping(image)));
        }
        let Some((_, bins, clip)) = &self.counts else {
            return;
        };
        let ctx = ui.ctx().clone();
        let names = [
            crate::i18n::text(&ctx, Message::new("gui-scopes-red", "Red")),
            crate::i18n::text(&ctx, Message::new("gui-scopes-green", "Green")),
            crate::i18n::text(&ctx, Message::new("gui-scopes-blue", "Blue")),
        ];
        let colors = [
            Color32::from_rgb(220, 70, 60),
            Color32::from_rgb(80, 190, 90),
            Color32::from_rgb(70, 120, 230),
        ];
        let channels = (0..3).map(|c| Channel::new(&names[c], &bins[c], colors[c]));
        let widget = Histogram::new(channels).log(true).clipping(*clip);
        let target = edit.and_then(|(_, store)| {
            exposure_path(
                &store
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )
        });
        let out = match (edit, &target) {
            (Some((app, _)), Some(_)) => widget
                .on_change(egui_pop::imaging::histogram(app, "helios.scopes"))
                .show(ui),
            _ => widget.draggable(false).show(ui),
        };
        crate::name_rect(ui.ctx(), "helios.scopes.picture", out.response.rect);
        if let (Some((_, store)), Some(path)) = (edit, target) {
            self.drag_exposure(store, &path, &out);
        }
    }

    /// Moves the exposure as the histogram's drag moved: a drag as one
    /// gesture (one undo step), a key or an assistive action as a step of
    /// its own; a drag given up leaves no trace.
    fn drag_exposure(&mut self, store: &Mutex<Store>, path: &str, out: &HistogramOutput) {
        let per_width = stops_per_width(self.stops);
        let mut s = store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = s.get(path).and_then(|e| e.value.as_f64()).unwrap_or(0.0);
        let write = |s: &mut Store, v: f64, g: Option<GestureId>| {
            let mut set = Set::new(path, Change::To(Value::Float(v)), Origin::User);
            set.gesture = g;
            let _ = s.set(set);
        };
        for c in &out.changes {
            let to = f64::from(c.to.delta) * per_width;
            match (c.done, self.drag.take()) {
                (false, held) => {
                    let (g, start) = match held {
                        Some((p, g, start)) if p == path => (g, start),
                        _ => (
                            s.begin_gesture(Origin::User, "drag the exposure on the histogram"),
                            current,
                        ),
                    };
                    write(&mut s, start + to, Some(g));
                    self.drag = Some((path.to_string(), g, start));
                }
                (true, Some((p, g, start))) if p == path => {
                    write(&mut s, start + to, Some(g));
                    s.end(g);
                }
                (true, _) => {
                    let from = f64::from(c.from.delta) * per_width;
                    write(&mut s, current + to - from, None);
                }
            }
        }
        // Given up (Escape, a veto at the release): put back, no step.
        if out.response.drag_stopped()
            && let Some((_, g, _)) = self.drag.take()
        {
            s.cancel(g);
        }
    }
}
