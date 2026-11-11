//! Helios's catalogue of business controls (`docs/design/controls.md`,
//! section 5). Curated: each control is decided in the note and its
//! follow-ups.

use egui_hud::model::{
    Control, Feel, Gesture, Intent, Jack, NumberSpec, Response, ValueSpec, ValueType,
};

pub use egui_hud::registry::slider_policy;

/// The exposure control's id.
pub const EXPOSURE: &str = "exposure";

/// The black level control's id.
pub const BLACK_LEVEL: &str = "black-level";

/// Every control of the catalogue.
pub fn all() -> Vec<Control> {
    vec![exposure(), black_level()]
}

/// A registry of the catalogue, with no patches.
pub fn registry() -> egui_hud::Registry {
    egui_hud::Registry::with(all()).expect("the standard catalogue is valid")
}

/// Exposure, in stops: -18 to +18 EV, the bar spanning -3 to +4; steps of
/// 0.01 EV (0.001 in fine mode), a third of a stop for the coarse step; a
/// detent at 0 EV; a scrub moves 1 EV per 200 points; double click
/// resets to 0 EV; the HUD reads `+0.67 EV` while handled.
///
/// Its faces, from `egui_hud`'s library: the slider (the default), the
/// knob and the scrub field.
pub fn exposure() -> Control {
    Control::new(
        EXPOSURE,
        "Exposure",
        ValueSpec::Number(NumberSpec {
            unit: Some("EV".into()),
            hard: (-18.0, 18.0),
            soft: (-3.0, 4.0),
            step: 0.01,
            coarse: 1.0 / 3.0,
            default: 0.0,
            precision: 2,
            display_scale: 1.0,
            signed: true,
            integer: false,
            wraps: false,
            open: false,
        }),
    )
    .purpose("Brighten or darken the image by a number of stops")
    .jack(Jack::new("value", ValueType::Number).unit("EV"))
    .gestures(&[
        Gesture::Drag,
        Gesture::Scrub,
        Gesture::Wheel,
        Gesture::Keys,
        Gesture::Typed,
        Gesture::DoubleClickReset,
    ])
    .intents(&[
        Intent::Nudge,
        Intent::Set,
        Intent::Reset,
        Intent::Begin,
        Intent::End,
        Intent::Cancel,
    ])
    .face("slider")
    .face("knob")
    .face("scrub")
    .feel(Feel {
        sensitivity: 1.0 / 200.0,
        fine: 0.1,
        response: Response::Perceptual,
    })
    // Held at 0 EV over a twentieth of a stop (10 points of a scrub).
    .detent(0.0, 0.05)
    .readout("active")
}

/// The black level, a share of white: -100 % to +100 %, the bar spanning
/// -10 % to +10 %, shown in percent with two decimals; steps of 0.05 %
/// (0.005 % in fine mode), half a percent for the coarse step; a detent
/// at 0; a linear scrub of the bar's span per 200 points; double click
/// resets to 0; the HUD reads `Black level 0.50 %` while handled.
///
/// Its faces, from `egui_hud`'s library: the slider (the default) and the
/// knob.
pub fn black_level() -> Control {
    Control::new(
        BLACK_LEVEL,
        "Black level",
        ValueSpec::Number(NumberSpec {
            unit: Some("%".into()),
            hard: (-1.0, 1.0),
            soft: (-0.1, 0.1),
            step: 0.0005,
            coarse: 0.005,
            default: 0.0,
            precision: 2,
            display_scale: 100.0,
            signed: false,
            integer: false,
            wraps: false,
            open: false,
        }),
    )
    .purpose("Set the level taken as black, as a share of white")
    .jack(Jack::new("value", ValueType::Number).unit("relative"))
    .gestures(&[
        Gesture::Drag,
        Gesture::Scrub,
        Gesture::Wheel,
        Gesture::Keys,
        Gesture::Typed,
        Gesture::DoubleClickReset,
    ])
    .intents(&[
        Intent::Nudge,
        Intent::Set,
        Intent::Reset,
        Intent::Begin,
        Intent::End,
        Intent::Cancel,
    ])
    .face("slider")
    .face("knob")
    .feel(Feel {
        // The bar's span (20 %) over 200 points, a tenth of it in fine mode.
        sensitivity: 0.2 / 200.0,
        fine: 0.1,
        response: Response::Linear,
    })
    // Held at 0 over a quarter of a percent.
    .detent(0.0, 0.0025)
    .readout("active")
}
