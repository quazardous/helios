//! Faces: the ways of drawing a value, shared by every control.
//!
//! A [`crate::model::Control`] is a value (its type, range, unit,
//! default) and a behaviour (its connector: jacks, gestures, intents,
//! signals). It says nothing about drawing. A [`Face`] is a way to draw
//! a value type: its slot forms per representation (the size policies),
//! its orientations, its HUD chip and who draws it on each surface. The
//! library ([`library`]) holds the faces `egui_hud` provides:
//!
//! | value | faces |
//! |---|---|
//! | bool | `checkbox`, `switch`, `push-button`, `lamp` (read-only) |
//! | number | `slider`, `fader`, `knob`, `endless-knob`, `scrub`, `meter` (read-only), `readout` (read-only) |
//! | one-of | `menu`, `segmented`, `radio` |
//! | points | `curve`, `point-list` |
//! | colour | `colour-wheel` |
//! | text | `text-field` |
//!
//! A face declares which values it [`Fit`]s, not only their type: a
//! bounded knob, slider or fader needs a bounded number; the endless knob
//! takes a cyclic (`wraps`) or open number only, moved by relative
//! [`Intent::Nudge`]s. A read-only face fits an indicator; a writing
//! face never does.
//!
//! A control may restrict and order the faces it accepts (a gain: fader
//! then knob); otherwise every fitting face of its value type is
//! accepted ([`fitting`]), the library's order giving the default. Which
//! face is shown is presentation: the kit's default (the first
//! accepted), the context (a slot too small for the default takes the
//! next accepted face that fits it, [`crate::resolve`]), or the person
//! on the bench.

use std::sync::LazyLock;

use crate::model::{Control, Intent, Kind, Representation, ValueSpec, ValueType, Variant};
use crate::registry::slider_policy;
use crate::slot::{Fallback, Grow, Integration, Orientation, Size, SizePolicy, Slot};

/// Which values of its type a face can draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Every value of the type.
    Any,
    /// A number with a closed range that does not wrap: a bar or a
    /// knob's arc has two ends.
    Bounded,
    /// A number that wraps (an angle, a hue) or has no bounds: it has no
    /// ends to show, only a relative movement.
    Endless,
}

/// A way to draw a value type.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// Stable, as kits name it (`fader`).
    pub id: &'static str,
    pub label: &'static str,
    /// The value type it draws.
    pub value: ValueType,
    /// Which values of that type.
    pub fits: Fit,
    /// It shows the value and never writes it: an indicator's face.
    pub read_only: bool,
    /// The intents it moves the value with, which the control's
    /// behaviour must offer (the endless knob nudges).
    pub needs: &'static [Intent],
    /// The renderer that draws it until it has its own: the nearest
    /// existing face, or `None` when it has its own.
    pub stand_in: Option<&'static str>,
    /// Its slot forms, orientations, chip and integration, as a control
    /// takes it.
    pub variant: Variant,
}

impl Face {
    /// Whether it can draw `control`'s value: its type, its range (bounded
    /// or endless) and its behaviour (an indicator only in a read-only
    /// face; the intents it needs).
    pub fn fits(&self, control: &Control) -> Result<(), String> {
        let ty = control.value.value_type();
        if ty != self.value {
            return Err(format!(
                "the {} face draws {:?}, not {ty:?}",
                self.id, self.value
            ));
        }
        if control.kind == Kind::Indicator && !self.read_only {
            return Err(format!(
                "the {} face writes; an indicator only reads",
                self.id
            ));
        }
        if let ValueSpec::Number(n) = &control.value {
            match self.fits {
                Fit::Bounded if !n.bounded() => {
                    return Err(format!(
                        "the {} face needs a bounded number; this one wraps or is open",
                        self.id
                    ));
                }
                Fit::Endless if n.bounded() => {
                    return Err(format!(
                        "the {} face needs a number that wraps or is open",
                        self.id
                    ));
                }
                _ => {}
            }
        }
        if let Some(i) = self
            .needs
            .iter()
            .find(|i| !control.connector.intents.contains(i))
        {
            return Err(format!(
                "the {} face needs the {i:?} intent, which the control does not offer",
                self.id
            ));
        }
        Ok(())
    }
}

/// The library's face of that id.
pub fn face(id: &str) -> Option<&'static Face> {
    library().iter().find(|f| f.id == id)
}

/// The faces `control` accepts when it names none: every fitting face of
/// its value type, in the library's order; for a control, its writing
/// faces only, for an indicator its read-only ones.
pub fn fitting(control: &Control) -> Vec<&'static Face> {
    library()
        .iter()
        .filter(|f| f.fits(control).is_ok())
        .filter(|f| f.read_only == (control.kind == Kind::Indicator))
        .collect()
}

/// Every face `egui_hud` provides, per value type, the default first.
pub fn library() -> &'static [Face] {
    static LIBRARY: LazyLock<Vec<Face>> = LazyLock::new(build);
    &LIBRARY
}

const THIRD: Slot = Slot::new(Size::THIRD, Size::ONE);
const QUARTER: Slot = Slot::new(Size::QUARTER, Size::ONE);
/// The least width of a full field: three quarters of a unit.
const FULL_MIN: Slot = Slot::new(Size::THREE_QUARTERS, Size::ONE);

fn row_growing() -> SizePolicy {
    SizePolicy::growable(FULL_MIN, Slot::ROW, Slot::wide(4), Grow::Width)
}

/// An editor that draws itself in a block of `rows`, a row when compact,
/// its values as the value type's managed rows where it cannot go.
fn drawn(id: &str, label: &str, rows: u32) -> Variant {
    Variant::new(id, label)
        .slot(
            Representation::Full,
            SizePolicy::growable(
                Slot::new(Size::THREE_QUARTERS, Slot::block(rows).height),
                Slot::block(rows),
                Slot::block(rows),
                Grow::Width,
            ),
        )
        .slot(Representation::Compact, SizePolicy::fixed(Slot::ROW))
        .integrated(
            crate::model::Surface::Panel,
            Integration::integrated(Fallback::ValueType),
        )
        .integrated(
            crate::model::Surface::Dock,
            Integration::integrated(Fallback::ValueType),
        )
        .integrated(
            crate::model::Surface::Float,
            Integration::integrated(Fallback::ValueType),
        )
}

fn build() -> Vec<Face> {
    use Representation::{Compact, Full, Simplified};
    let h = [Orientation::Horizontal];
    let hv = [Orientation::Horizontal, Orientation::Vertical];
    let face = |id: &'static str,
                label: &'static str,
                value: ValueType,
                fits: Fit,
                read_only: bool,
                stand_in: Option<&'static str>,
                variant: Variant| Face {
        id,
        label,
        value,
        fits,
        read_only,
        needs: &[],
        stand_in,
        variant,
    };
    let toggle = |id: &str, label: &str| {
        Variant::new(id, label)
            .slot(Full, SizePolicy::fixed(Slot::HALF))
            .slot(Simplified, SizePolicy::fixed(QUARTER))
            .chip()
    };
    let knob = |id: &str, label: &str| {
        Variant::new(id, label)
            .slot(Full, SizePolicy::fixed(Slot::HALF))
            .slot(Simplified, SizePolicy::fixed(THIRD))
            .orientations(&h)
            .chip()
    };
    vec![
        // bool
        face(
            "checkbox",
            "Checkbox",
            ValueType::Bool,
            Fit::Any,
            false,
            None,
            toggle("checkbox", "Checkbox"),
        ),
        face(
            "switch",
            "Switch",
            ValueType::Bool,
            Fit::Any,
            false,
            Some("checkbox"),
            toggle("switch", "Switch"),
        ),
        face(
            "push-button",
            "Push button",
            ValueType::Bool,
            Fit::Any,
            false,
            None,
            toggle("push-button", "Push button"),
        ),
        face(
            "lamp",
            "Lamp",
            ValueType::Bool,
            Fit::Any,
            true,
            None,
            toggle("lamp", "Lamp"),
        ),
        // number
        face(
            "slider",
            "Slider",
            ValueType::Number,
            Fit::Bounded,
            false,
            None,
            Variant::new("slider", "Slider")
                .slot(Full, slider_policy())
                .slot(Compact, SizePolicy::fixed(Slot::HALF))
                .slot(Simplified, SizePolicy::fixed(THIRD))
                .orientations(&hv)
                .chip(),
        ),
        face(
            "fader",
            "Fader",
            ValueType::Number,
            Fit::Bounded,
            false,
            None,
            Variant::new("fader", "Fader")
                .slot(Full, SizePolicy::fixed(Slot::wide(3)))
                .slot(Compact, SizePolicy::fixed(FULL_MIN))
                .slot(Simplified, SizePolicy::fixed(Slot::HALF))
                .orientations(&[Orientation::Vertical, Orientation::Horizontal])
                .chip(),
        ),
        face(
            "knob",
            "Knob",
            ValueType::Number,
            Fit::Bounded,
            false,
            None,
            knob("knob", "Knob"),
        ),
        Face {
            needs: &[Intent::Nudge],
            ..face(
                "endless-knob",
                "Endless knob",
                ValueType::Number,
                Fit::Endless,
                false,
                Some("knob"),
                knob("endless-knob", "Endless knob"),
            )
        },
        face(
            "scrub",
            "Scrub field",
            ValueType::Number,
            Fit::Any,
            false,
            None,
            Variant::new("scrub", "Scrub field")
                .slot(Full, SizePolicy::fixed(Slot::HALF))
                .slot(Simplified, SizePolicy::fixed(THIRD))
                .orientations(&h)
                .chip(),
        ),
        face(
            "meter",
            "Meter",
            ValueType::Number,
            Fit::Bounded,
            true,
            None,
            Variant::new("meter", "Meter")
                .slot(
                    Full,
                    SizePolicy::fixed(Slot::new(Size::units(2), Size::HALF)),
                )
                .slot(Simplified, SizePolicy::fixed(THIRD))
                .orientations(&[Orientation::Vertical, Orientation::Horizontal])
                .chip(),
        ),
        face(
            "readout",
            "Read-out",
            ValueType::Number,
            Fit::Any,
            true,
            None,
            Variant::new("readout", "Read-out")
                .slot(Full, SizePolicy::fixed(Slot::HALF))
                .slot(Simplified, SizePolicy::fixed(THIRD))
                .orientations(&h)
                .chip(),
        ),
        // one-of
        face(
            "menu",
            "Menu",
            ValueType::OneOf,
            Fit::Any,
            false,
            None,
            Variant::new("menu", "Menu")
                .slot(Full, slider_policy())
                .slot(Compact, SizePolicy::fixed(Slot::HALF))
                .orientations(&h),
        ),
        face(
            "segmented",
            "Segmented buttons",
            ValueType::OneOf,
            Fit::Any,
            false,
            None,
            Variant::new("segmented", "Segmented buttons")
                .slot(
                    Full,
                    SizePolicy::growable(Slot::ROW, Slot::wide(2), Slot::wide(4), Grow::Width),
                )
                .orientations(&h),
        ),
        face(
            "radio",
            "Radio list",
            ValueType::OneOf,
            Fit::Any,
            false,
            None,
            Variant::new("radio", "Radio list")
                .slot(
                    Full,
                    SizePolicy::growable(
                        Slot::new(Size::THREE_QUARTERS, Size::units(3)),
                        Slot::block(3),
                        Slot::new(Size::units(2), Size::units(3)),
                        Grow::Width,
                    ),
                )
                .orientations(&[Orientation::Vertical]),
        ),
        // points, colour, text: their editors
        face(
            "curve",
            "Curve",
            ValueType::Points,
            Fit::Any,
            false,
            None,
            drawn("curve", "Curve", 6),
        ),
        face(
            "point-list",
            "Point list",
            ValueType::Points,
            Fit::Any,
            false,
            None,
            Variant::new("point-list", "Point list")
                .slot(Full, row_growing())
                .orientations(&h),
        ),
        face(
            "colour-wheel",
            "Colour wheel",
            ValueType::Colour,
            Fit::Any,
            false,
            None,
            drawn("colour-wheel", "Colour wheel", 4),
        ),
        face(
            "text-field",
            "Text field",
            ValueType::Text,
            Fit::Any,
            false,
            None,
            Variant::new("text-field", "Text field")
                .slot(Full, row_growing())
                .orientations(&h),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NumberSpec;

    fn number(wraps: bool, open: bool) -> Control {
        Control::new(
            "n",
            "N",
            ValueSpec::Number(NumberSpec {
                unit: None,
                hard: (0.0, 360.0),
                soft: (0.0, 360.0),
                step: 1.0,
                coarse: 10.0,
                default: 0.0,
                precision: 0,
                display_scale: 1.0,
                signed: false,
                integer: false,
                wraps,
                open,
            }),
        )
        .intents(&[Intent::Nudge, Intent::Set])
    }

    #[test]
    fn every_face_is_valid_and_unique() {
        let mut seen = Vec::new();
        for f in library() {
            assert_eq!(f.id, f.variant.id);
            assert!(!seen.contains(&f.id), "{} twice", f.id);
            seen.push(f.id);
            assert!(!f.variant.surfaces().is_empty(), "{} shows nowhere", f.id);
        }
    }

    #[test]
    fn bounded_faces_and_the_endless_knob_split_numbers() {
        let ids = |c: &Control| fitting(c).iter().map(|f| f.id).collect::<Vec<_>>();
        let bounded = ids(&number(false, false));
        assert_eq!(bounded, ["slider", "fader", "knob", "scrub"]);
        let cyclic = ids(&number(true, false));
        assert_eq!(cyclic, ["endless-knob", "scrub"]);
        assert_eq!(ids(&number(false, true)), cyclic);
        let no_nudge = number(true, false).intents(&[Intent::Set]);
        assert_eq!(ids(&no_nudge), ["scrub"]);
        let meter = number(false, false).indicator();
        assert_eq!(ids(&meter), ["meter", "readout"]);
    }
}
