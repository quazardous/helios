//! The shell at every scale egui-pop's test kit knows: the layout stays
//! in points, the pictures follow the physical pixels, the interface zoom
//! is egui-pop's, and a `hidpi` profile variant follows the screen.

use egui::{Key, Modifiers, vec2};
use egui_kittest::kittest::Queryable;
use egui_pop_test::at_scales;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

fn framed() -> Shell {
    let mut shell = Shell::new(helios_panels(), None);
    shell.draw_frame(true);
    shell.set_title("photos");
    shell
}

#[test]
fn the_frame_and_the_areas_keep_their_place_in_points_at_every_scale() {
    let mut at_one = None;
    at_scales(|s| {
        let mut h = headless::harness_at(framed(), vec2(1280.0, 800.0), s);
        h.run();
        let title = h.get_by_label("photos — Helios").rect();
        let vault = h.state().target_rect("helios.vault").unwrap();
        let params = h.state().target_rect("helios.params").unwrap();
        let here = (title, vault, params);
        let one = *at_one.get_or_insert(here);
        let near = |a: egui::Rect, b: egui::Rect| (a.min - b.min).length() < 1.0;
        assert!(
            near(here.1, one.1) && near(here.2, one.2),
            "{here:?} {one:?}"
        );
        assert!((here.0.center().y - one.0.center().y).abs() < 1.0);
        // Pictures are in physical pixels.
        let window = headless::snapshot(&mut h, "window").unwrap();
        assert_eq!(
            window.dimensions(),
            ((1280.0 * s).round() as u32, (800.0 * s).round() as u32)
        );
    });
}

#[test]
fn the_interface_zoom_is_egui_pop_s_and_its_change_is_journaled() {
    use egui_pop::api::zoom;
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1024.0, 700.0));
    h.run();
    // egui's own zoom keys are off: Ctrl and `=` leave the interface.
    h.key_press_modifiers(Modifiers::CTRL, Key::Equals);
    h.run();
    assert!((h.ctx.zoom_factor() - 1.0).abs() < 1e-6);
    // egui-pop's keys move it a step, in every window.
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::ALT, Key::Equals);
    h.run();
    let app = h.state().app().handle().clone();
    assert!((zoom::current(&app) - 1.1).abs() < 1e-6);
    assert!((h.ctx.zoom_factor() - 1.1).abs() < 1e-6);
    assert!((h.state().pixels_per_point() - 1.1).abs() < 1e-6);
    let changed: Vec<_> = app
        .events_since(0)
        .into_iter()
        .filter(|e| e.name == "scale.changed")
        .collect();
    assert_eq!(changed.len(), 1, "{changed:?}");
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::ALT, Key::Num0);
    h.run();
    assert!((h.ctx.zoom_factor() - 1.0).abs() < 1e-6);
}

#[test]
fn a_hidpi_variant_follows_the_screen_not_the_interface_zoom() {
    let dir = std::env::temp_dir().join(format!("helios-hidpi-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("work.helios-profile"),
        "helios_profile = 1\nname = \"work\"\n\
         [theme]\nname = \"helios-dark\"\n\
         [on.hidpi.theme]\nname = \"helios-light\"\n",
    )
    .unwrap();
    let shell = |dir: &std::path::Path| {
        let mut s = Shell::new(helios_panels(), None);
        s.set_profiles_dir(dir.to_path_buf());
        s.use_profile("work").unwrap();
        s
    };
    // A dense screen: the variant applies from the first frame.
    let mut h = headless::harness_at(shell(&dir), vec2(800.0, 500.0), 2.0);
    h.run();
    assert_eq!(h.state().theme_name(), "helios-light");
    // A plain screen zoomed to 200 %: still a plain screen.
    let mut h = headless::harness(shell(&dir), vec2(800.0, 500.0));
    h.run();
    let s = h.state_mut();
    s.run(
        egui_pop::api::zoom::CMD_ZOOM_IN.as_str(),
        json!(null),
        Origin::User,
    )
    .unwrap();
    h.run();
    h.run();
    assert!(h.state().pixels_per_point() > 1.0);
    assert_eq!(h.state().theme_name(), "helios-dark");
    // Moved to a dense screen: applied again.
    egui_pop_test::set_native_scale(&mut h, 2.0);
    h.run();
    h.run();
    assert_eq!(h.state().theme_name(), "helios-light");
    std::fs::remove_dir_all(&dir).unwrap();
}
