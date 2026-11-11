//! The registry: every control and case, registered and discoverable,
//! with the patches that plug them into node parameters (sections 3.4 and
//! 3.10). What agents read through `controls.catalogue` and
//! `controls.describe`.
//!
//! - A [`Patch`] is a module's: its parameter `param` is edited by control
//!   `control` (jack `jack`). The node is not in it: the patch holds for
//!   every node of that module.
//! - A parameter with no patch gets an implicit control, built from its
//!   descriptor ([`implicit`]): what the generated panel drew before.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::model::{
    Control, Feel, Gesture, Intent, Jack, NumberSpec, Representation, Response, ValueSpec,
    ValueType,
};
use crate::slot::{Grow, Size, SizePolicy, Slot};
use crate::value::{ParamDescriptor, ParamType, Value, Widget};

/// A slider's full form: a row, growing across up to four units (the
/// rule for sliders, range sliders and drop-downs), shrinking to three
/// quarters of a unit so a side panel a little narrower than a column
/// keeps its labelled sliders. Below that it steps down to its compact
/// form.
pub fn slider_policy() -> SizePolicy {
    SizePolicy::growable(FULL_MIN, Slot::ROW, Slot::wide(4), Grow::Width)
}

/// The least width of a full slider or field: three quarters of a unit.
const FULL_MIN: Slot = Slot::new(Size::THREE_QUARTERS, Size::ONE);

/// A module's parameter plugged into a control's jack.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch {
    /// The module (`exposure`).
    pub module: String,
    /// The parameter (`exposure`).
    pub param: String,
    /// The control (`exposure`).
    pub control: String,
    /// The control's jack (`value`).
    pub jack: String,
}

impl Patch {
    pub fn new(module: &str, param: &str, control: &str, jack: &str) -> Self {
        Self {
            module: module.into(),
            param: param.into(),
            control: control.into(),
            jack: jack.into(),
        }
    }
}

/// The catalogue of controls and the patches onto parameters.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    controls: BTreeMap<String, Control>,
    patches: Vec<Patch>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry of these controls, with no patches; an invalid one is
    /// refused.
    pub fn with(controls: impl IntoIterator<Item = Control>) -> Result<Self, String> {
        let mut r = Self::new();
        for c in controls {
            r.register(c)?;
        }
        Ok(r)
    }

    /// Adds a control, with every fitting face of the library when it
    /// names none; an invalid one or a second one with the same id is
    /// refused.
    pub fn register(&mut self, control: Control) -> Result<(), String> {
        let control = control.with_faces();
        control.check()?;
        if self.controls.contains_key(&control.id) {
            return Err(format!("control `{}` is already registered", control.id));
        }
        self.controls.insert(control.id.clone(), control);
        Ok(())
    }

    /// Adds a module's patches; one naming an unknown control or jack, or
    /// a parameter already patched, is refused with the others kept.
    pub fn patch(&mut self, patches: impl IntoIterator<Item = Patch>) -> Vec<String> {
        let mut errors = Vec::new();
        for p in patches {
            let Some(c) = self.controls.get(&p.control) else {
                errors.push(format!(
                    "patch {}/{}: no control `{}`",
                    p.module, p.param, p.control
                ));
                continue;
            };
            if !c.connector.jacks.iter().any(|j| j.name == p.jack) {
                errors.push(format!(
                    "patch {}/{}: control `{}` has no jack `{}`",
                    p.module, p.param, p.control, p.jack
                ));
                continue;
            }
            if self.patch_of(&p.module, &p.param).is_some() {
                errors.push(format!("patch {}/{}: already patched", p.module, p.param));
                continue;
            }
            self.patches.push(p);
        }
        errors
    }

    pub fn get(&self, id: &str) -> Option<&Control> {
        self.controls.get(id)
    }

    pub fn controls(&self) -> impl Iterator<Item = &Control> {
        self.controls.values()
    }

    pub fn patches(&self) -> &[Patch] {
        &self.patches
    }

    /// The patch of a module's parameter.
    pub fn patch_of(&self, module: &str, param: &str) -> Option<&Patch> {
        self.patches
            .iter()
            .find(|p| p.module == module && p.param == param)
    }

    /// The control patched to a store path (`node/<node>/<param>`) of a
    /// parameter of `module`.
    pub fn control_at(&self, path: &str, module: Option<&str>) -> Option<&Control> {
        let module = module?;
        let param = path.strip_prefix("node/")?.rsplit_once('/')?.1;
        self.get(&self.patch_of(module, param)?.control)
    }

    /// Every registered control's warnings (an integrated variant with no
    /// managed fallback): the exceptions the inspector counts.
    pub fn warnings(&self) -> Vec<String> {
        self.controls.values().flat_map(Control::warnings).collect()
    }

    /// One line per control: what `controls.catalogue` lists.
    pub fn catalogue(&self) -> Json {
        Json::Array(self.controls.values().map(summary).collect())
    }

    /// Everything about a control, machine-readable: what
    /// `controls.describe` answers and the gallery's card shows.
    pub fn describe(&self, id: &str) -> Option<Json> {
        let c = self.get(id)?;
        let patches: Vec<&Patch> = self.patches.iter().filter(|p| p.control == id).collect();
        Some(describe(c, &patches))
    }
}

fn summary(c: &Control) -> Json {
    json!({
        "id": c.id,
        "label": c.label,
        "purpose": c.purpose,
        "kind": c.kind,
        "slot": c
            .faces
            .first()
            .and_then(|v| v.form(Representation::Full))
            .map(|f| f.policy.size),
        "warnings": c.warnings().len(),
        "connector": c.connector.jacks.iter().map(Jack::signature).collect::<Vec<_>>(),
        "faces": c.faces.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        "case": c.case.is_some(),
    })
}

/// A control's full description: the declaration as data, its config
/// schema and its patches.
pub fn describe(c: &Control, patches: &[&Patch]) -> Json {
    json!({
        "id": c.id,
        "label": c.label,
        "purpose": c.purpose,
        "control": c,
        "connector": c.connector.jacks.iter().map(Jack::signature).collect::<Vec<_>>(),
        "layout": c.layout_report(),
        "surfaces": c
            .faces
            .iter()
            .map(|v| (v.id.clone(), v.surfaces()))
            .collect::<BTreeMap<_, _>>(),
        "gestures": c.connector.gestures,
        "detents": c.detents,
        "faces": c.faces.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        "config_schema": c.config_schema(),
        "patches": patches,
    })
}

/// The faces an implicit control accepts: every fitting one of its value
/// type, or one editor for a list of numbers or a text.
enum Shape {
    /// Every face of the library that fits its value.
    Fitting,
    /// One face of the library (`curve`, `colour-wheel`, `point-list`,
    /// `text-field`).
    Face(&'static str),
}
/// The points of drag a scrub takes to cross a parameter's soft range.
pub const SCRUB_TRAVEL: f64 = 300.0;

/// An integer moves by one at least every this many points of a scrub.
pub const INTEGER_POINTS: f64 = 4.0;

/// The scrub speed (value units per point) of an implicit control over
/// `range`: the range crossed in 150 to [`SCRUB_TRAVEL`] points, rounded
/// up to a clean step (1, 2, 2.5 or 5 times a power of ten). An integer
/// moves by one at least every [`INTEGER_POINTS`] points. A range with no
/// bounds keeps [`Feel::default`]'s speed. The fine mode (Ctrl) still
/// divides it by the feel's `fine` factor.
pub fn scrub_sensitivity(range: Option<(f64, f64)>, integer: bool) -> f64 {
    let fallback = Feel::default().sensitivity;
    let speed = match range {
        Some((lo, hi))
            if lo.is_finite() && hi.is_finite() && hi > lo && hi - lo < f64::MAX / 4.0 =>
        {
            let raw = (hi - lo) / SCRUB_TRAVEL;
            let decade = 10f64.powf(raw.log10().floor());
            [1.0, 2.0, 2.5, 5.0, 10.0]
                .into_iter()
                .map(|m| m * decade)
                .find(|s| *s >= raw * (1.0 - 1e-9))
                .unwrap_or(10.0 * decade)
        }
        _ => fallback,
    };
    if integer {
        let s = speed.max(1.0 / INTEGER_POINTS);
        // Past one unit per point, a whole number of units.
        if s > 1.0 { s.round() } else { s }
    } else {
        speed
    }
}

/// The implicit control of a parameter no control is patched to, from
/// its descriptor and hints: the row the panel draws for it (a slider, a
/// toggle, a list, a curve).
pub fn implicit(path: &str, d: &ParamDescriptor) -> Control {
    let id = format!("implicit:{path}");
    let (value, shape, gestures): (ValueSpec, Shape, &[Gesture]) = match d.ty {
        ParamType::Float | ParamType::Int => {
            let hard = d.hard.unwrap_or((f64::MIN, f64::MAX));
            let default = d.default.as_f64().unwrap_or(0.0);
            (
                ValueSpec::Number(NumberSpec {
                    unit: d.unit.clone(),
                    hard,
                    soft: d.soft.unwrap_or(hard),
                    step: d.step,
                    coarse: d.coarse,
                    default,
                    precision: d.precision,
                    display_scale: d.display_scale,
                    signed: d.unit.as_deref() == Some("EV"),
                    integer: d.ty == ParamType::Int,
                    wraps: false,
                    // No range declared at all: nothing to draw a bar on.
                    open: d.hard.is_none() && d.soft.is_none(),
                }),
                Shape::Fitting,
                &[
                    Gesture::Drag,
                    Gesture::Wheel,
                    Gesture::Keys,
                    Gesture::Typed,
                    Gesture::DoubleClickReset,
                ],
            )
        }
        ParamType::Bool => (
            ValueSpec::Bool {
                default: d.default == Value::Bool(true),
            },
            Shape::Fitting,
            &[],
        ),
        ParamType::Choice => (
            ValueSpec::OneOf {
                choices: d.choices.clone(),
                default: match &d.default {
                    Value::Choice(c) => c.clone(),
                    _ => String::new(),
                },
            },
            Shape::Fitting,
            &[Gesture::Keys],
        ),
        ParamType::Floats => (
            ValueSpec::Other {
                // A wheel's numbers are a colour (hue, chroma, luminance).
                stored: if d.widget == Some(Widget::Wheel) {
                    ValueType::Colour
                } else {
                    ValueType::Points
                },
            },
            Shape::Face(match d.widget {
                Some(Widget::Curve) => "curve",
                Some(Widget::Wheel) => "colour-wheel",
                None => "point-list",
            }),
            &[],
        ),
        ParamType::Ints => (
            ValueSpec::Other {
                stored: ValueType::Points,
            },
            Shape::Face("point-list"),
            &[],
        ),
        ParamType::Text => (
            ValueSpec::Other {
                stored: ValueType::Text,
            },
            Shape::Face("text-field"),
            &[Gesture::Typed],
        ),
    };
    let ty = value.value_type();
    let mut jack = Jack::new("value", ty);
    jack.unit.clone_from(&d.unit);
    let mut c = Control::new(&id, &d.label, value)
        .purpose(&d.description)
        .jack(jack)
        .gestures(gestures)
        .intents(&[Intent::Nudge, Intent::Set, Intent::Reset])
        .feel(Feel {
            sensitivity: d
                .sensitivity
                .unwrap_or_else(|| scrub_sensitivity(d.soft.or(d.hard), d.ty == ParamType::Int)),
            response: if d.logarithmic {
                Response::Logarithmic
            } else {
                Response::Linear
            },
            ..Feel::default()
        });
    if let Shape::Face(id) = shape {
        c = c.face(id);
    } else if c.value.number().is_some_and(|n| n.open) {
        // No range to draw: a field first, the endless knob as the other.
        c = c.face("scrub").face("endless-knob");
    }
    c.implicit = true;
    let mut c = c.with_faces();
    // A parameter nothing is patched to stays off the HUD: no chip.
    for f in &mut c.faces {
        f.chip = false;
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The points a scrub takes to cross `range`.
    fn travel(range: (f64, f64), integer: bool) -> f64 {
        (range.1 - range.0) / scrub_sensitivity(Some(range), integer)
    }

    #[test]
    fn a_scrub_crosses_the_soft_range_in_a_few_hundred_points() {
        // Feather, 0 to 200 px: one pixel a point, 40 px in 40 points.
        assert_eq!(scrub_sensitivity(Some((0.0, 200.0)), false), 1.0);
        assert_eq!(travel((0.0, 200.0), false), 200.0);
        // Exposure, -3 to +4 EV: a fortieth of a stop a point.
        assert!((scrub_sensitivity(Some((-3.0, 4.0)), false) - 0.025).abs() < 1e-12);
        assert!((travel((-3.0, 4.0), false) - 280.0).abs() < 1e-9);
        for range in [
            (0.0, 1.0),
            (0.0, 100.0),
            (-1.0, 1.0),
            (0.0, 7.5),
            (0.0, 1e6),
        ] {
            let t = travel(range, false);
            assert!((150.0..=300.0 + 1e-6).contains(&t), "{range:?}: {t} pt");
        }
    }

    #[test]
    fn whole_numbers_move_by_one_every_few_points() {
        // A small range: one unit every four points, not a hundred.
        assert_eq!(scrub_sensitivity(Some((0.0, 10.0)), true), 0.25);
        assert_eq!(scrub_sensitivity(Some((1.0, 64.0)), true), 0.25);
        // A large one: a whole number of units a point.
        assert_eq!(scrub_sensitivity(Some((0.0, 1000.0)), true), 5.0);
        let s = scrub_sensitivity(Some((0.0, 750.0)), true);
        assert_eq!(s, s.round());
    }

    #[test]
    fn unbounded_ranges_keep_the_fallback_and_a_stated_speed_wins() {
        let fallback = Feel::default().sensitivity;
        assert_eq!(scrub_sensitivity(None, false), fallback);
        assert_eq!(
            scrub_sensitivity(Some((f64::MIN, f64::MAX)), false),
            fallback
        );
        assert_eq!(scrub_sensitivity(Some((1.0, 1.0)), false), fallback);
        assert_eq!(scrub_sensitivity(None, true), 1.0 / INTEGER_POINTS);
        let mut d = ParamDescriptor::float("Feather", 0.0, 200.0, 0.0);
        assert_eq!(implicit("n/feather", &d).feel.sensitivity, 1.0);
        d.sensitivity = Some(0.5);
        assert_eq!(implicit("n/feather", &d).feel.sensitivity, 0.5);
        // The fine mode still divides it.
        let c = implicit("n/feather", &ParamDescriptor::float("F", 0.0, 200.0, 0.0));
        assert_eq!(c.scrubbed(0.0, 40.0, false, true), 40.0);
        assert!((c.scrubbed(0.0, 40.0, true, true) - 4.0).abs() < 1e-9);
    }
}
