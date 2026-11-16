//! The screens Helios's event loop tells egui-pop: the main window kept
//! on a monitor now gone (X11, Windows) opens on a screen left, as
//! eframe creates it (`shell::place_main_window`'s hook); where no
//! position was kept (Wayland) nothing moves.

use egui::{ViewportBuilder, vec2};
use egui_pop::api::window::{self, Screen};
use helios_gui::shell::place_main_window;

/// A laptop's panel and, at its right, a monitor, named as winit names
/// them.
fn panel() -> Screen {
    Screen::from_physical(Some("eDP-1".into()), (0, 0), (2560, 1600), 2.0)
}

fn monitor() -> Screen {
    Screen::from_physical(Some("DP-1".into()), (2560, 0), (1920, 1080), 1.0)
}

/// The builder eframe creates the main window with, from `viewport`, the
/// screens told being `screens`.
fn created(viewport: ViewportBuilder, screens: Vec<Screen>) -> ViewportBuilder {
    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    place_main_window(&mut options, None);
    window::set_screens(screens);
    let vb = match options.window_builder.take() {
        Some(hook) => hook(options.viewport.clone()),
        None => options.viewport.clone(),
    };
    window::set_screens(Vec::new());
    vb
}

#[test]
fn the_main_window_kept_on_a_monitor_now_gone_opens_on_a_screen_left() {
    let kept = ViewportBuilder::default()
        .with_inner_size([1000.0, 700.0])
        .with_position(monitor().rect.min + vec2(100.0, 50.0));
    let vb = created(kept.clone(), vec![panel()]);
    let p = vb.position.expect("a position");
    assert!(panel().rect.contains(p), "{p:?} is on the panel");
    assert_eq!(vb.inner_size, Some(vec2(1000.0, 700.0)));
    // The monitor plugged in again: the window opens where it was.
    let vb = created(kept.clone(), vec![panel(), monitor()]);
    assert_eq!(vb.position, kept.position);
}

#[test]
fn where_no_position_was_kept_nothing_moves() {
    let kept = ViewportBuilder::default().with_inner_size([1000.0, 700.0]);
    let vb = created(kept, vec![panel()]);
    assert_eq!(vb.position, None, "the system places it");
    assert_eq!(vb.inner_size, Some(vec2(1000.0, 700.0)));
}
