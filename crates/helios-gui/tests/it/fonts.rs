//! Text on the desktop across themes: the text size and the fonts the
//! user picks (egui-pop's Settings › Appearance › Fonts) stay when Helios
//! switches themes, and the dock tabs' labels come back drawn as before
//! after a light and dark round trip (a theme installing fonts rebuilds
//! the font atlas).

use egui::{TextStyle, vec2};
use egui_kittest::Harness;
use egui_pop::api::text;
use egui_pop::core::fonts::Role;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

fn harness() -> Harness<'static, Shell> {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1000.0, 640.0));
    h.run();
    h
}

fn body_size(h: &Harness<'_, Shell>) -> f32 {
    h.ctx.global_style().text_styles[&TextStyle::Body].size
}

fn theme(h: &mut Harness<'_, Shell>, name: &str) {
    h.state_mut()
        .run(&format!("theme.{name}"), json!(null), Origin::User)
        .unwrap();
    h.run();
}

#[test]
fn the_text_size_and_fonts_stay_across_themes() {
    let mut h = harness();
    let normal = body_size(&h);
    let handle = h.state().app().handle().clone();
    text::set_text_size(&handle, 1.5);
    text::set_font(&handle, Role::Mono, Some("DejaVu Sans Mono"));
    h.run();
    let large = body_size(&h);
    assert!(large > normal * 1.4, "{large} after {normal}");
    for name in ["helios-light", "helios-high-contrast", "helios-dark"] {
        theme(&mut h, name);
        let spec = handle.theme().spec();
        assert!(
            (spec.text_size - 1.5).abs() < 1e-6,
            "{name}: {}",
            spec.text_size
        );
        assert_eq!(
            spec.font_mono.as_deref(),
            Some("DejaVu Sans Mono"),
            "{name}"
        );
        // The size is a factor of the theme's own: high contrast has
        // larger body text of its own, scaled the same.
        let scaled = body_size(&h);
        text::set_text_size(&handle, 1.0);
        h.run();
        let own = body_size(&h);
        assert!(
            (scaled - own * 1.5).abs() < 0.6,
            "{name}: {scaled}, its own size {own}"
        );
        text::set_text_size(&handle, 1.5);
        h.run();
    }
    // Back to 100 %: the theme's own size.
    text::set_text_size(&handle, 1.0);
    h.run();
    assert!((body_size(&h) - normal).abs() < 1e-3, "{}", body_size(&h));
}

/// The window drawn in the dark theme, then again after a switch to the
/// light one and back: the same pixels, dock tab labels included (they
/// once kept glyphs of the atlas the switch rebuilt).
#[test]
fn dock_tabs_draw_the_same_after_a_light_and_dark_round_trip() {
    let mut h = harness();
    let before = headless::snapshot(&mut h, "window").unwrap();
    theme(&mut h, "helios-light");
    let light = headless::snapshot(&mut h, "window").unwrap();
    assert_ne!(
        before.as_raw(),
        light.as_raw(),
        "the light theme drew nothing new"
    );
    theme(&mut h, "helios-dark");
    let after = headless::snapshot(&mut h, "window").unwrap();
    assert_eq!(before.dimensions(), after.dimensions());
    let differ = before
        .pixels()
        .zip(after.pixels())
        .filter(|(a, b)| a.0.iter().zip(b.0.iter()).any(|(x, y)| x.abs_diff(*y) > 8))
        .count();
    let total = (before.width() * before.height()) as usize;
    assert!(
        differ * 1000 < total,
        "{differ} of {total} pixels differ after the round trip"
    );
}
