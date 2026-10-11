//! The scopes panel: a histogram, a waveform or an RGB parade of the image
//! it is given (`helios-scopes` counts, this draws). Until the viewport
//! feeds it, the image is set from outside (`Shell::set_scope_image`).
//!
//! The picture is computed once per image, mode and scale, into a texture;
//! counts are shown on a log scale so faint tails stay visible.

use egui::{Color32, ColorImage, TextureHandle, Ui};
use helios_core::Image;
use helios_scopes::{Counts, Scale, histogram, parade, waveform};

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
    generation: u64,
}

impl Default for Scopes {
    fn default() -> Self {
        Scopes {
            image: None,
            mode: Mode::Histogram,
            stops: true,
            texture: None,
            generation: 0,
        }
    }
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

    pub fn ui(&mut self, ui: &mut Ui) {
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
        let Some(image) = &self.image else {
            ui.weak("No image yet");
            return;
        };
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
}
