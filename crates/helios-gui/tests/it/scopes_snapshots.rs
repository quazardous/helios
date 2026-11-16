//! The scopes' snapshots (`tests/snapshots/`, thresholds in the
//! workspace's `kittest.toml`): the histogram, the waveform and the parade
//! of a ramp, each in a small and in a large panel, filling it and drawn
//! sharp at both sizes. Regenerate with
//! `UPDATE_SNAPSHOTS=1 cargo test -p helios-gui --test it scopes_snapshots::`
//! (the box's `gui-snapshots` job).

use egui::{Vec2, vec2};
use egui_kittest::Harness;
use helios_core::{Image, Region};
use helios_gui::scopes::{Mode, Scopes};

/// A ramp, each channel at its own slope.
fn image() -> Image {
    Image::from_fn(Region::sized(640, 400), 4, |x, y, c| {
        if c == 3 {
            1.0
        } else {
            (x as f32 / 640.0) * (0.4 + 0.2 * c as f32) + (y as f32 / 4000.0)
        }
    })
}

fn shoot(mode: Mode, size: Vec2, name: &str) {
    let mut scopes = Scopes::default();
    scopes.set_image(image());
    scopes.set_mode(mode);
    let mut h = Harness::builder()
        .with_size(size)
        .build_ui_state(|ui, s: &mut Scopes| s.ui(ui), scopes);
    h.run();
    h.snapshot(name);
}

const SMALL: Vec2 = vec2(360.0, 240.0);
const LARGE: Vec2 = vec2(1000.0, 620.0);

#[test]
fn the_histogram_small() {
    shoot(Mode::Histogram, SMALL, "scopes_histogram_small");
}

#[test]
fn the_histogram_large() {
    shoot(Mode::Histogram, LARGE, "scopes_histogram_large");
}

#[test]
fn the_waveform_small() {
    shoot(Mode::Waveform, SMALL, "scopes_waveform_small");
}

#[test]
fn the_waveform_large() {
    shoot(Mode::Waveform, LARGE, "scopes_waveform_large");
}

#[test]
fn the_parade_small() {
    shoot(Mode::Parade, SMALL, "scopes_parade_small");
}

#[test]
fn the_parade_large() {
    shoot(Mode::Parade, LARGE, "scopes_parade_large");
}
