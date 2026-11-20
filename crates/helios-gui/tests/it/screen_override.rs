//! The person's correction of what a screen shows (Settings > Display):
//! a key of its own per screen, which wins over what the system says,
//! which wins over nothing said; the system's read (at start, on
//! `screen.added`) never overwrites it. Settings > Display lists each
//! screen known with what the system says and the correction, a screen
//! not connected included while it has one; agents correct through
//! `helios.display.screen_override`.

use std::time::{Duration, Instant};

use egui::{pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::api::window::{Screen, set_screens};
use helios_display::{Chromaticities, ScreenDescription};
use helios_gui::display::{self, ScreenColour};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

const PLAIN: &str = "DP-1";
const WIDE: &str = "eDP-1";
const AWAY: &str = "HDMI-A-1";

/// A Display P3 panel's primaries, as the compositor gives them.
const DISPLAY_P3: Chromaticities = Chromaticities {
    red: (680_000, 320_000),
    green: (265_000, 690_000),
    blue: (150_000, 60_000),
    white: (312_700, 329_000),
};

/// The image description a compositor gives the output `name`: sRGB
/// encoded, on a panel of `panel` primaries.
fn described(name: &str, panel: Chromaticities) -> ScreenDescription {
    ScreenDescription {
        name: Some(name.into()),
        primaries_named: Some("srgb".into()),
        primaries: Some(Chromaticities::SRGB),
        target_primaries: Some(panel),
        transfer: Some("gamma22".into()),
        ..ScreenDescription::default()
    }
}

/// The system as a compositor with colour management describes it: an
/// sRGB panel and a Display P3 one.
fn the_system() -> Vec<ScreenDescription> {
    vec![
        described(PLAIN, Chromaticities::SRGB),
        described(WIDE, DISPLAY_P3),
    ]
}

fn desktop() -> Harness<'static, Shell> {
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
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 900.0));
    h.run();
    h
}

fn correct(h: &mut Harness<'static, Shell>, screen: &str, value: &str) -> Result<Value, String> {
    let r = h.state_mut().run(
        display::SCREEN_OVERRIDE_COMMAND,
        json!({ "screen": screen, "value": value }),
        Origin::Agent("helper".into()),
    );
    h.run_steps(2);
    r
}

#[test]
fn the_correction_wins_over_the_system_which_wins_over_nothing() {
    let mut h = desktop();
    let app = h.state().app().handle().clone();
    display::tell_described_screens(&app, &the_system());
    assert_eq!(
        display::screen_colour(&app, WIDE),
        Some(ScreenColour::WideGamut)
    );
    // Nothing said of a screen: none, the route follows the display.
    assert_eq!(display::screen_colour(&app, AWAY), None);

    // An agent corrects the wide screen to sRGB: the correction wins,
    // the system's word stays beside it.
    let answer = correct(&mut h, WIDE, "srgb").unwrap();
    assert_eq!(answer["screen"]["override"], "srgb", "{answer}");
    assert_eq!(answer["screen"]["system"], "wide-gamut", "{answer}");
    assert_eq!(display::screen_colour(&app, WIDE), Some(ScreenColour::Srgb));
    assert_eq!(
        display::system_screen_colour(&app, WIDE),
        Some(ScreenColour::WideGamut)
    );
    // A window there wants sRGB, the Wide-gamut preview on.
    assert_eq!(
        display::wanted_on(true, display::screen_colour(&app, WIDE)),
        display::wanted(false)
    );

    // A screen the system says nothing of takes the correction too.
    correct(&mut h, AWAY, "hdr").unwrap();
    assert_eq!(display::screen_colour(&app, AWAY), Some(ScreenColour::Hdr));
    assert_eq!(display::system_screen_colour(&app, AWAY), None);

    // Auto: the system's word again.
    correct(&mut h, WIDE, "auto").unwrap();
    assert_eq!(
        display::screen_colour(&app, WIDE),
        Some(ScreenColour::WideGamut)
    );
    assert_eq!(
        display::overrides(&app).into_iter().collect::<Vec<_>>(),
        [(AWAY.to_owned(), "hdr".to_owned())]
    );

    // A value outside the choices is refused before the handler.
    assert!(correct(&mut h, WIDE, "p3").is_err());
    set_screens(Vec::new());
}

#[test]
fn the_systems_read_leaves_the_correction_alone() {
    let mut h = desktop();
    let app = h.state().app().handle().clone();
    display::tell_described_screens(&app, &the_system());
    correct(&mut h, PLAIN, "wide-gamut").unwrap();
    // The system reads the screens again (a screen plugged in), and
    // says the same as before, or that the screen is unknown now.
    display::read_system_screens(&app, || Ok(the_system()));
    assert_eq!(
        display::screen_colour(&app, PLAIN),
        Some(ScreenColour::WideGamut)
    );
    display::tell_screen_colour(&app, PLAIN, None).unwrap();
    assert_eq!(
        display::screen_colour(&app, PLAIN),
        Some(ScreenColour::WideGamut)
    );
    assert_eq!(display::system_screen_colour(&app, PLAIN), None);
    assert_eq!(
        display::screen_override(&app, PLAIN),
        Some(ScreenColour::WideGamut)
    );
    set_screens(Vec::new());
}

fn until(
    h: &mut Harness<'static, Shell>,
    what: &str,
    done: impl Fn(&Harness<'static, Shell>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("never got there: {what}");
}

/// Settings > Display: a row per screen known (the two connected, the
/// one away with its correction), each with what the system says and
/// the correction; as `tests/snapshots/display-screens-*.png` keeps it
/// (regenerated by the box's `gui-snapshots` job with
/// `test=screen_override`).
#[test]
fn settings_display_lists_each_screen_with_the_system_and_the_correction() {
    use crate::shot::Shots as _;
    let mut h = desktop();
    let app = h.state().app().handle().clone();
    display::tell_described_screens(&app, &the_system());
    correct(&mut h, AWAY, "srgb").unwrap();

    let rows = h
        .state_mut()
        .run(display::SCREENS_COMMAND, Value::Null, Origin::User)
        .unwrap();
    let names: Vec<&str> = rows["screens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, [PLAIN, AWAY, WIDE], "{rows}");
    let away = &rows["screens"][1];
    assert_eq!(away["connected"], false, "{rows}");
    assert_eq!(away["systemText"], "Not described", "{rows}");
    assert_eq!(away["override"], "srgb", "{rows}");
    assert_eq!(rows["screens"][2]["systemText"], "Wide gamut", "{rows}");
    assert_eq!(rows["screens"][2]["connected"], true, "{rows}");

    h.state_mut()
        .run(
            "admin.open",
            json!({ "path": "hardware/display" }),
            Origin::User,
        )
        .unwrap();
    until(&mut h, "the screens table", |h| {
        h.query_by_label(AWAY).is_some() && h.query_by_label(WIDE).is_some()
    });
    h.run_steps(4);
    let mut results = egui_kittest::SnapshotResults::new();
    results.add(h.try_body_shot("display-screens-pop-dark-1x"));
    set_screens(Vec::new());
}
