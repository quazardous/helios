//! Helios's catalogue of business controls (`docs/design/controls.md`,
//! section 5). Curated: each control is decided in the note and its
//! follow-ups.

use crate::model::{
    Control, Feel, Footprint, Gesture, Intent, Jack, NumberSpec, Response, ValueSpec, ValueType,
    Variant,
};

/// The exposure control's id.
pub const EXPOSURE: &str = "exposure";

/// Every control of the catalogue.
pub fn all() -> Vec<Control> {
    vec![exposure()]
}

/// Exposure, in stops: -18 to +18 EV, the bar spanning -3 to +4; steps of
/// 0.01 EV (0.001 in fine mode), a third of a stop for the coarse step; a
/// detent at 0 EV; a scrub moves 1 EV per 200 points; double click
/// resets to 0 EV; the HUD reads `+0.67 EV` while handled.
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
    .variant(Variant::new(
        "slider",
        "Slider",
        &[Footprint::Row, Footprint::Chip],
    ))
    .variant(Variant::new(
        "scrub",
        "Scrub field",
        &[Footprint::Half, Footprint::Chip],
    ))
    .feel(Feel {
        sensitivity: 1.0 / 200.0,
        fine: 0.1,
        response: Response::Perceptual,
    })
    // Held at 0 EV over a twentieth of a stop (10 points of a scrub).
    .detent(0.0, 0.05)
    .readout("active")
}
