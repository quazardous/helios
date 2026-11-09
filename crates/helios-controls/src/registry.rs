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

use helios_values::{Descriptor, Type};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::catalogue::slider_policy;
use crate::model::{
    Control, Feel, Gesture, Intent, Jack, NumberSpec, Representation, Response, Surface, ValueSpec,
    ValueType, Variant,
};
use crate::slot::{Fallback, Grow, Integration, Orientation, Size, SizePolicy, Slot};

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

    /// Helios's own catalogue (`crate::catalogue`), with no patches.
    pub fn standard() -> Self {
        let mut r = Self::new();
        for c in crate::catalogue::all() {
            r.register(c).expect("the standard catalogue is valid");
        }
        r
    }

    /// Adds a control; an invalid one or a second one with the same id
    /// is refused.
    pub fn register(&mut self, control: Control) -> Result<(), String> {
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

    /// The control patched to a store path (`node/<node>/<param>`), from
    /// its descriptor's module.
    pub fn control_for(&self, path: &str, d: &Descriptor) -> Option<&Control> {
        let module = d.module.as_deref()?;
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
            .variants
            .first()
            .and_then(|v| v.form(Representation::Full))
            .map(|f| f.policy.size),
        "warnings": c.warnings().len(),
        "connector": c.connector.jacks.iter().map(Jack::signature).collect::<Vec<_>>(),
        "variants": c.variants.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
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
            .variants
            .iter()
            .map(|v| (v.id.clone(), v.surfaces()))
            .collect::<BTreeMap<_, _>>(),
        "gestures": c.connector.gestures,
        "detents": c.detents,
        "variants": c.variants.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        "config_schema": c.config_schema(),
        "patches": patches,
    })
}

/// What an implicit control's one variant asks of the surfaces.
enum Shape {
    /// A slider: a row growing across, a value field, a read-out; turns
    /// vertical in a strip.
    Slider,
    /// A toggle: half a unit, a quarter when simplified.
    Toggle,
    /// A drop-down: a row growing across, half a unit compact.
    Choice,
    /// A field one row high, growing across.
    Field,
    /// An editor that draws itself in a block of `n` rows (a curve in
    /// six, a colour wheel in four), a row when compact, its values as
    /// the value type's managed rows where its drawing cannot go.
    Drawn(u32),
}

impl Shape {
    fn variant(self) -> Variant {
        let v = Variant::new("default", "Default");
        let third = Slot::new(Size::THIRD, Size::ONE);
        let quarter = Slot::new(Size::QUARTER, Size::ONE);
        match self {
            Shape::Slider => v
                .slot(Representation::Full, slider_policy())
                .slot(Representation::Compact, SizePolicy::fixed(Slot::HALF))
                .slot(Representation::Simplified, SizePolicy::fixed(third))
                .orientations(&[Orientation::Horizontal, Orientation::Vertical]),
            Shape::Toggle => v
                .slot(Representation::Full, SizePolicy::fixed(Slot::HALF))
                .slot(Representation::Simplified, SizePolicy::fixed(quarter)),
            Shape::Choice => v
                .slot(Representation::Full, slider_policy())
                .slot(Representation::Compact, SizePolicy::fixed(Slot::HALF))
                .orientations(&[Orientation::Horizontal]),
            Shape::Field => v
                .slot(
                    Representation::Full,
                    SizePolicy::growable(Slot::ROW, Slot::ROW, Slot::wide(4), Grow::Width),
                )
                .orientations(&[Orientation::Horizontal]),
            Shape::Drawn(rows) => v
                .slot(Representation::Full, SizePolicy::fixed(Slot::block(rows)))
                .slot(Representation::Compact, SizePolicy::fixed(Slot::ROW))
                .integrated(Surface::Panel, Integration::integrated(Fallback::ValueType))
                .integrated(Surface::Dock, Integration::integrated(Fallback::ValueType))
                .integrated(Surface::Float, Integration::integrated(Fallback::ValueType)),
        }
    }
}

/// The implicit control of a parameter no control is patched to, from
/// its descriptor and hints: the row the panel draws for it (a slider, a
/// toggle, a list, a curve).
pub fn implicit(path: &str, d: &Descriptor) -> Control {
    let id = format!("implicit:{path}");
    let (value, shape, gestures): (ValueSpec, Shape, &[Gesture]) = match d.ty {
        Type::Float | Type::Int => {
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
                    integer: d.ty == Type::Int,
                }),
                Shape::Slider,
                &[
                    Gesture::Drag,
                    Gesture::Wheel,
                    Gesture::Keys,
                    Gesture::Typed,
                    Gesture::DoubleClickReset,
                ],
            )
        }
        Type::Bool => (
            ValueSpec::Bool {
                default: d.default == helios_core::Value::Bool(true),
            },
            Shape::Toggle,
            &[],
        ),
        Type::Choice => (
            ValueSpec::OneOf {
                choices: d.choices.clone(),
                default: match &d.default {
                    helios_core::Value::Choice(c) => c.clone(),
                    _ => String::new(),
                },
            },
            Shape::Choice,
            &[Gesture::Keys],
        ),
        Type::Floats => (
            ValueSpec::Other {
                stored: ValueType::Points,
            },
            match d.widget {
                Some(helios_core::ParamWidget::Curve) => Shape::Drawn(6),
                Some(helios_core::ParamWidget::Wheel) => Shape::Drawn(4),
                None => Shape::Field,
            },
            &[],
        ),
        Type::Ints => (
            ValueSpec::Other {
                stored: ValueType::Points,
            },
            Shape::Field,
            &[],
        ),
        Type::Text => (
            ValueSpec::Other {
                stored: ValueType::Text,
            },
            Shape::Field,
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
        .variant(shape.variant())
        .feel(Feel {
            response: if d.logarithmic {
                Response::Logarithmic
            } else {
                Response::Linear
            },
            ..Feel::default()
        });
    c.implicit = true;
    c
}
