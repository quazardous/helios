//! The scopes follow their panel: laid out alone in a small and in a large
//! panel, the histogram fills it, and the waveform and the parade are
//! computed at the panel's size in pixels (up to a cap) and drawn one
//! texel per pixel, never a small picture stretched. A resize drag does
//! not recompute every frame: the picture follows once it is let go.

use std::collections::BTreeMap;

use egui::{Event, Id, Modifiers, PointerButton, Rect, Vec2, ViewportId, pos2, vec2};
use egui_kittest::Harness;
use helios_core::{Image, Region};
use helios_gui::scopes::{Mode, Scopes};

/// The largest picture the scopes compute, in pixels (both sides).
const CAP: f32 = 2048.0;

/// A ramp, each channel at its own slope, so every scope draws something.
fn image() -> Image {
    Image::from_fn(Region::sized(640, 400), 4, |x, y, c| {
        if c == 3 {
            1.0
        } else {
            (x as f32 / 640.0) * (0.4 + 0.2 * c as f32) + (y as f32 / 4000.0)
        }
    })
}

fn harness(mode: Mode, size: Vec2, ppp: f32) -> Harness<'static, Scopes> {
    let mut scopes = Scopes::default();
    scopes.set_image(image());
    scopes.set_mode(mode);
    let mut h = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(ppp)
        .build_ui_state(|ui, s: &mut Scopes| s.ui(ui), scopes);
    h.run();
    h
}

/// Where the scope was drawn (the shell's named rect).
fn drawn(h: &Harness<'_, Scopes>) -> Rect {
    h.ctx
        .data(|d| d.get_temp::<BTreeMap<String, (Rect, ViewportId)>>(Id::unique("helios.named")))
        .and_then(|m| m.get("helios.scopes.picture").map(|(r, _)| *r))
        .expect("the scope names its picture")
}

/// The size of the scopes' texture, in pixels.
fn texture(h: &Harness<'_, Scopes>) -> [usize; 2] {
    let tm = h.ctx.tex_manager();
    let tm = tm.read();
    let sizes: Vec<[usize; 2]> = tm
        .allocated()
        .filter(|(_, m)| m.name == "helios.scopes")
        .map(|(_, m)| m.size)
        .collect();
    assert_eq!(sizes.len(), 1, "one scopes texture: {sizes:?}");
    sizes[0]
}

/// The scope fills the panel (the harness's frame keeps 8 points each
/// side; the mode bar takes the top).
fn assert_fills(r: Rect, size: Vec2, what: &str) {
    assert!(r.width() >= size.x - 20.0, "{what}: {r:?} in {size:?}");
    assert!(r.bottom() >= size.y - 20.0, "{what}: {r:?} in {size:?}");
    assert!(r.height() >= size.y - 60.0, "{what}: {r:?} in {size:?}");
}

#[test]
fn the_histogram_fills_a_small_and_a_large_panel() {
    for size in [vec2(320.0, 220.0), vec2(1600.0, 1000.0)] {
        let h = harness(Mode::Histogram, size, 1.0);
        assert_fills(drawn(&h), size, "histogram");
    }
}

#[test]
fn the_waveform_and_the_parade_are_computed_at_the_panel_s_pixel_size() {
    let cases = [
        (vec2(320.0, 220.0), 1.0),
        (vec2(1400.0, 900.0), 1.0),
        (vec2(700.0, 450.0), 2.0),
    ];
    for mode in [Mode::Waveform, Mode::Parade] {
        for (size, ppp) in cases {
            let h = harness(mode, size, ppp);
            let r = drawn(&h);
            assert_fills(r, size, &format!("{mode:?}"));
            let [w, t] = texture(&h);
            let px = r.size() * ppp;
            let what = format!("{mode:?} at {size:?} x{ppp}: texture {w}x{t}, drawn {px:?} px");
            // One texel per pixel: neither upscaled nor wasted.
            assert!((w as f32 - px.x.min(CAP)).abs() <= 1.0, "{what}");
            assert!((t as f32 - px.y.min(CAP)).abs() <= 1.0, "{what}");
        }
    }
}

#[test]
fn a_resize_drag_recomputes_the_waveform_once_let_go() {
    let mut h = harness(Mode::Waveform, vec2(400.0, 300.0), 1.0);
    let before = texture(&h);
    // A drag under way (a splitter held): the panel grows frame by frame.
    let at = pos2(2.0, 2.0);
    h.hover_at(at);
    h.input_mut().events.push(Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    for (i, w) in [500.0, 650.0, 800.0, 900.0].into_iter().enumerate() {
        h.set_size(vec2(w, 300.0 + 50.0 * i as f32));
        h.step();
    }
    assert_eq!(texture(&h), before, "not recomputed while the drag is held");
    h.input_mut().events.push(Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
    let r = drawn(&h);
    let [w, t] = texture(&h);
    assert!((w as f32 - r.width()).abs() <= 1.0, "{w}x{t} for {r:?}");
    assert!((t as f32 - r.height()).abs() <= 1.0, "{w}x{t} for {r:?}");
}
