//! The display route is chosen per window, from the screen the window is
//! on: a detached viewer moved from an sRGB screen to a wide-gamut one
//! follows it, the main window keeps its own; on a compositor with colour
//! management the viewer's own image surface carries it. Its window is
//! built for one on Wayland (transparent); elsewhere it stays opaque and
//! the route says why.

use egui::{pos2, vec2};
use egui_kittest::Harness;
use egui_pop::api::window::{Screen, set_screens};

use egui_pop::model::values::{Change, Set, Value as PopValue};
use egui_pop::viewport::display::{ColorSpace, SurfaceFormat};
use egui_pop::viewport::image_surface::{self, Offer, raw_window_handle};
use egui_pop_wayland::{Capabilities, Feature, NamedPrimaries as P, NamedTransfer as T};
use helios_gui::display::ScreenColour;
use helios_gui::{Shell, display, headless, helios_panels};
use serde_json::{Value as Json, json};

const PLAIN: &str = "Plain sRGB";
const WIDE: &str = "Wide gamut";

fn set(h: &mut Harness<'static, Shell>, key: &str, value: PopValue) {
    let store = h.state().app().handle().store();
    store
        .lock()
        .unwrap()
        .set(
            Set::new(
                key,
                Change::To(value),
                egui_pop::model::commands::Origin::User,
            )
            .untracked(),
        )
        .unwrap();
}

/// The `helios.display` snapshot's entry for each window: the main one
/// and the other one (the detached viewer's).
fn windows(h: &Harness<'static, Shell>) -> (Json, Json) {
    let s = display::snapshot(&h.ctx, true);
    let all = s["windows"].as_array().cloned().unwrap_or_default();
    let main = all.iter().find(|w| w["window"] == "main").cloned();
    let other = all.iter().find(|w| w["window"] != "main").cloned();
    (
        main.unwrap_or_else(|| panic!("no main window in {s}")),
        other.unwrap_or_else(|| panic!("no detached window in {s}")),
    )
}

/// A KWin-like compositor's offer: BT.2020 with a 2.2 power taken, no
/// sRGB transfer announced (the image surface is tagged).
fn kwin() -> Offer {
    let mut wsi = Vec::new();
    for f in [SurfaceFormat::Rgba8Unorm, SurfaceFormat::Rgb10a2Unorm] {
        wsi.push((f, ColorSpace::Srgb));
    }
    Offer {
        unavailable: None,
        colour: Some(Capabilities {
            version: 1,
            features: vec![Feature::Parametric],
            primaries: vec![P::Srgb, P::Bt2020],
            transfers: vec![T::Gamma22],
            intents: vec![0],
        }),
        wsi,
        may_tag: true,
    }
}

/// The main window's pass lists `viewer` among the open windows, as
/// eframe's does (egui-pop frees the image surface of a window it does
/// not list).
fn list_open(h: &mut Harness<'static, Shell>, viewer: egui::ViewportId) {
    h.input_mut()
        .viewports
        .insert(viewer, egui::ViewportInfo::default());
    h.step();
}

fn steps(h: &mut Harness<'static, Shell>) {
    for _ in 0..4 {
        h.step();
    }
}

#[test]
fn a_detached_viewer_takes_the_route_of_the_screen_it_is_on() {
    set_screens(vec![
        Screen {
            name: Some(PLAIN.into()),
            rect: egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0)),
            scale: 1.0,
        },
        Screen {
            name: Some(WIDE.into()),
            rect: egui::Rect::from_min_size(pos2(1280.0, 0.0), vec2(1280.0, 800.0)),
            scale: 1.0,
        },
    ]);
    let (mut h, windows_of) =
        headless::harness_with_windows(Shell::new(helios_panels(), None), vec2(1024.0, 700.0), 1.0);
    h.step();
    // What each screen shows, and the Wide-gamut preview on.
    let app = h.state().app().handle().clone();
    display::tell_screen_colour(&app, PLAIN, Some(ScreenColour::Srgb)).unwrap();
    display::tell_screen_colour(&app, WIDE, Some(ScreenColour::WideGamut)).unwrap();
    set(
        &mut h,
        &format!("settings/{}", display::SETTING_WIDE_GAMUT),
        PopValue::Bool(true),
    );
    h.state_mut()
        .run(
            "helios.viewport.detach",
            json!(null),
            helios_shell::commands::Origin::User,
        )
        .unwrap();
    windows_of.set_position("helios.viewport", pos2(100.0, 100.0));
    steps(&mut h);
    let (main_before, on_plain) = windows(&h);
    assert_eq!(on_plain["screen"], PLAIN, "{on_plain}");
    assert_eq!(on_plain["screen_colour"], "srgb", "{on_plain}");
    assert_eq!(on_plain["route"]["route"], "srgb", "{on_plain}");
    assert!(
        on_plain["route"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("sRGB only")),
        "{on_plain}"
    );
    assert!(
        on_plain["wanted"].as_str().unwrap().contains("sRGB"),
        "{on_plain}"
    );

    // Moved onto the wide-gamut screen: the window wants Rec.2020 there.
    windows_of.set_position("helios.viewport", pos2(1400.0, 100.0));
    steps(&mut h);
    let (main_after, on_wide) = windows(&h);
    assert_eq!(on_wide["window"], on_plain["window"]);
    assert_eq!(on_wide["screen"], WIDE, "{on_wide}");
    assert_eq!(on_wide["screen_colour"], "wide-gamut", "{on_wide}");
    assert!(
        on_wide["wanted"].as_str().unwrap().contains("2020"),
        "{on_wide}"
    );
    assert_ne!(on_wide["route"], on_plain["route"]);
    // The main window is unchanged.
    assert_eq!(main_before, main_after);

    // Its own image surface: offered by a compositor with colour
    // management (a fake, KWin-like), it is tagged Rec.2020 for that
    // window alone; the main window's stays as it was.
    let viewer = app
        .windows_list()
        .into_iter()
        .find(|w| w.id != "main")
        .and_then(|w| w.viewport_id())
        .expect("the detached viewer's viewport");
    list_open(&mut h, viewer);
    image_surface::install_offer(&h.ctx, viewer, kwin());
    let n = image_surface::negotiate_window(&h.ctx, viewer);
    assert_eq!(n.route.name(), "tagged", "{n:?}");
    let (main_tagged, own) = windows(&h);
    assert_eq!(own["route"]["route"], "tagged", "{own}");
    assert!(
        own["route"]["encoding"].as_str().unwrap().contains("2020"),
        "{own}"
    );
    assert_eq!(main_tagged, main_after);
    // `windows.list` shows the viewer's image surface.
    let listed = app
        .windows_list()
        .into_iter()
        .find(|w| w.viewport_id() == Some(viewer))
        .and_then(|w| w.surface)
        .expect("the viewer's image surface in windows.list");
    assert_eq!(listed["route"], "tagged", "{listed}");
    assert_eq!(listed["format"], "Rgb10a2Unorm", "{listed}");
    image_surface::close_window(&h.ctx, viewer);

    // And back: sRGB again.
    windows_of.set_position("helios.viewport", pos2(200.0, 100.0));
    steps(&mut h);
    let (_, back) = windows(&h);
    assert_eq!(back["screen"], PLAIN);
    assert_eq!(back["route"], on_plain["route"]);
    set_screens(Vec::new());
}

/// A detached viewer on the wide-gamut screen, the Wide-gamut preview on,
/// the windows' handles read where Helios reads them
/// (`display::set_window_source`), on the windowing system `display`
/// names (what Helios's event loop tells egui-pop as it resumes).
fn viewer_on_wide_screen(
    display: raw_window_handle::RawDisplayHandle,
) -> (Harness<'static, Shell>, egui::ViewportId) {
    egui_pop::api::runner::host::set_display(display);
    set_screens(vec![Screen {
        name: Some(WIDE.into()),
        rect: egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0)),
        scale: 1.0,
    }]);
    let (mut h, windows_of) =
        headless::harness_with_windows(Shell::new(helios_panels(), None), vec2(1024.0, 700.0), 1.0);
    h.step();
    let app = h.state().app().handle().clone();
    display::tell_screen_colour(&app, WIDE, Some(ScreenColour::WideGamut)).unwrap();
    set(
        &mut h,
        &format!("settings/{}", display::SETTING_WIDE_GAMUT),
        PopValue::Bool(true),
    );
    display::set_window_source(&h.ctx);
    h.state_mut()
        .run(
            "helios.viewport.detach",
            json!(null),
            helios_shell::commands::Origin::User,
        )
        .unwrap();
    windows_of.set_position("helios.viewport", pos2(100.0, 100.0));
    steps(&mut h);
    let viewer = app
        .windows_list()
        .into_iter()
        .find(|w| w.id != "main")
        .and_then(|w| w.viewport_id())
        .expect("the detached viewer's viewport");
    list_open(&mut h, viewer);
    (h, viewer)
}

#[test]
fn on_wayland_a_detached_viewers_window_is_built_for_its_own_image_surface() {
    let wayland = raw_window_handle::RawDisplayHandle::Wayland(
        raw_window_handle::WaylandDisplayHandle::new(std::ptr::NonNull::dangling()),
    );
    let (h, viewer) = viewer_on_wide_screen(wayland);
    // Helios asks its panels' windows transparent: on Wayland the
    // viewer's is, so a surface below it shows through.
    assert!(h.state().app().handle().transparent_panel_windows());
    let built = egui_pop::core::window_surface::transparency(&h.ctx, viewer);
    assert_eq!(
        built,
        Some(egui_pop::core::window_surface::Transparency::Transparent)
    );
    // Its handles come from the event loop's pairing; none in a headless
    // run, and the route says so (not "opaque", not the main window's).
    let n = image_surface::negotiate_window(&h.ctx, viewer);
    assert_eq!(n.route.name(), "window", "{n:?}");
    assert!(
        n.why.as_deref().is_some_and(|w| w.contains("handles")),
        "{n:?}"
    );
    let (_, own) = windows(&h);
    assert_eq!(own["route"]["route"], "srgb", "{own}");
    assert!(
        own["route"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("handles")),
        "{own}"
    );
    set_screens(Vec::new());
}

#[test]
fn on_x11_a_detached_viewer_draws_srgb_and_says_why() {
    let x11 = raw_window_handle::RawDisplayHandle::Xlib(raw_window_handle::XlibDisplayHandle::new(
        None, 0,
    ));
    let (h, viewer) = viewer_on_wide_screen(x11);
    // Opaque: the image surface is a Wayland subsurface.
    let built = egui_pop::core::window_surface::transparency(&h.ctx, viewer)
        .expect("the viewer's window says how it was built");
    assert!(
        built.why_opaque().is_some_and(|w| w.contains("X11")),
        "{built:?}"
    );
    let n = image_surface::negotiate_window(&h.ctx, viewer);
    assert_eq!(n.route.name(), "window", "{n:?}");
    let (_, own) = windows(&h);
    assert_eq!(own["route"]["route"], "srgb", "{own}");
    assert!(
        own["route"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("X11")),
        "{own}"
    );
    set_screens(Vec::new());
}
