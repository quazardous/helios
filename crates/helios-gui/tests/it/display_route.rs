//! The display route is chosen per window, from the screen the window is
//! on: a detached viewer moved from an sRGB screen to a wide-gamut one
//! follows it, the main window keeps its own.

use egui::{pos2, vec2};
use egui_kittest::Harness;
use egui_pop::api::window::{Screen, set_screens};

use egui_pop::model::values::{Change, Set, Value as PopValue};
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
    set(
        &mut h,
        &format!("display/screens/{PLAIN}"),
        PopValue::Text("srgb".into()),
    );
    set(
        &mut h,
        &format!("display/screens/{WIDE}"),
        PopValue::Text("wide-gamut".into()),
    );
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

    // And back: sRGB again.
    windows_of.set_position("helios.viewport", pos2(200.0, 100.0));
    steps(&mut h);
    let (_, back) = windows(&h);
    assert_eq!(back["screen"], PLAIN);
    assert_eq!(back["route"], on_plain["route"]);
    set_screens(Vec::new());
}
