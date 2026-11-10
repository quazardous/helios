//! The controls model, without egui: what a control is, declared with
//! Rust builders that produce serialisable data (`docs/design/controls.md`,
//! sections 3.1 to 3.3, question Q4).
//!
//! - A [`Control`] is built for one purpose (exposure, feather): its value
//!   ([`ValueSpec`]), units and ranges, [`Feel`] (sensitivity, fine
//!   factor, response curve), [`Detent`]s, [`Readout`], [`Connector`] and
//!   [`Variant`]s.
//! - The rack standard: slots and size policies per [`Representation`]
//!   on boxed [`Surface`]s, orientations and integration levels
//!   ([`crate::slot`]); the HUD's chip and the image's gizmo; typed
//!   [`Connector`]s (jacks, gestures, intents, signals); [`Kind`]
//!   (a control or an indicator, whose jacks are read-only); [`Case`]s
//!   (composites: parts and chrome); variants; and the ergonomic contract
//!   ([`HIT_AREA`], [`Feel`], detents).
//! - Where a control lives ([`Mode`]) and how much it shows
//!   ([`Representation`]); which of them a context picks is
//!   [`crate::context`] and [`crate::resolve`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::context::{Rule, Tier};
use crate::slot::{Chrome, Fallback, Integration, Orientation, SizePolicy, SlotForm};

/// The smallest hit area of anything a control lets the hand grab, in
/// points, whatever its drawn size (the ergonomic contract).
pub const HIT_AREA: f32 = 24.0;

/// What a value is stored as. Types describe storage; they are never
/// offered as controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValueType {
    Number,
    Bool,
    OneOf,
    Point,
    Angle,
    Range,
    Colour,
    Points,
    Box,
    Text,
}

/// Where a control appears. Panel, dock, toolbar and float are boxed:
/// they lay controls out in slots. The HUD shows a chip, the image a
/// gizmo: forms of those surfaces, not slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Surface {
    /// The module's card.
    Panel,
    /// A rack docked on its own (favourites, a quick strip, a bank strip).
    Dock,
    /// A single line of slots along a toolbar.
    Toolbar,
    /// A rack in a window.
    Float,
    /// Read-outs and small controls over the image.
    Hud,
    /// Handles on the image.
    Image,
}

impl Surface {
    pub const ALL: [Surface; 6] = [
        Surface::Panel,
        Surface::Dock,
        Surface::Toolbar,
        Surface::Float,
        Surface::Hud,
        Surface::Image,
    ];

    /// Whether it lays controls out in slots.
    pub fn is_boxed(self) -> bool {
        !matches!(self, Surface::Hud | Surface::Image)
    }
}

/// A control, or an indicator: a control whose jacks are all read-only
/// (a histogram, a meter, a read-out). Both get a slot, a place in the
/// order and an accessibility node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[default]
    Control,
    Indicator,
}

impl Kind {
    #[allow(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde's skip_serializing_if passes a reference"
    )]
    fn is_control(&self) -> bool {
        *self == Kind::Control
    }
}

/// Where a control lives. The host mounts it; a placement says which
/// mode it is in and which one a rule offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Panel,
    Docked,
    Floating,
    Hud,
    Toolbar,
}

/// How much a control shows in its slot, the same variant either way.
/// In that order they are the slot engine's forms of a variant: full
/// first, then the smaller ones it declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Representation {
    /// Everything: the label, the bar, the value, the marks.
    Full,
    /// The value and its gestures in less room.
    Compact,
    /// A read-out, at most a nudge.
    Simplified,
}

impl Representation {
    pub const ALL: [Representation; 3] = [
        Representation::Full,
        Representation::Compact,
        Representation::Simplified,
    ];
}

/// A gesture a control accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Gesture {
    /// A relative drag along a bar.
    Drag,
    /// A relative drag on a field or on the image, at the control's
    /// sensitivity.
    Scrub,
    /// Wheel notches step it.
    Wheel,
    /// Arrows step it, Page Up and Page Down take the coarse step.
    Keys,
    /// Enter types a value or an expression.
    Typed,
    /// A double click resets it.
    DoubleClickReset,
    /// A pick on the image.
    Pick,
}

/// What a device asks a control (section 3.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Intent {
    Nudge,
    Set,
    Reset,
    Begin,
    End,
    Cancel,
    Pick,
}

/// What a control emits while handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Signal {
    /// A value inside a gesture.
    Preview,
    /// The gesture's end: one undo step.
    Commit,
    /// The gesture given up: no trace.
    Cancel,
}

/// A typed value a control reads and writes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Jack {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ValueType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// The coordinate space of a point or an angle (`image`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// Read, never written: an indicator's jacks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
}

impl Jack {
    pub fn new(name: &str, ty: ValueType) -> Self {
        Self {
            name: name.into(),
            ty,
            unit: None,
            space: None,
            read_only: false,
        }
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub fn unit(mut self, unit: &str) -> Self {
        self.unit = Some(unit.into());
        self
    }

    pub fn space(mut self, space: &str) -> Self {
        self.space = Some(space.into());
        self
    }

    /// `number<EV>`, as the note writes it.
    pub fn signature(&self) -> String {
        let ty = serde_json::to_value(self.ty)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        match (&self.unit, &self.space) {
            (Some(u), _) => format!("{ty}<{u}>"),
            (None, Some(s)) => format!("{ty}<{s}>"),
            (None, None) => ty,
        }
    }
}

/// A control's typed interface: two controls with the same connector are
/// variants of each other.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Connector {
    pub jacks: Vec<Jack>,
    pub gestures: Vec<Gesture>,
    pub intents: Vec<Intent>,
    pub signals: Vec<Signal>,
}

/// How a number moves under the hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Response {
    Linear,
    Logarithmic,
    /// Even to the eye (exposure in stops).
    Perceptual,
}

/// The feel of a control: numbers a person may tune (question Q7), never
/// the gestures' meanings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feel {
    /// Value units per point of a scrub.
    pub sensitivity: f64,
    /// The fine mode's factor (Ctrl).
    pub fine: f64,
    pub response: Response,
}

impl Default for Feel {
    fn default() -> Self {
        Self {
            sensitivity: 0.01,
            fine: 0.1,
            response: Response::Linear,
        }
    }
}

/// A short resistance at a meaningful value, crossed by moving on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Detent {
    pub value: f64,
    /// How far from it, in value units, a moving value is held on it.
    pub window: f64,
}

/// A number's ranges, steps and how it is shown.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NumberSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub hard: (f64, f64),
    pub soft: (f64, f64),
    pub step: f64,
    pub coarse: f64,
    pub default: f64,
    /// Decimals shown.
    pub precision: u32,
    /// Shown value = value × scale.
    pub display_scale: f64,
    /// Shown with its sign (`+0.70 EV`).
    #[serde(default)]
    pub signed: bool,
    #[serde(default)]
    pub integer: bool,
}

impl NumberSpec {
    /// Kept to the soft range, or to the hard one when forced.
    pub fn clamp(&self, v: f64, force: bool) -> f64 {
        let (lo, hi) = if force { self.hard } else { self.soft };
        let v = v.clamp(lo.min(hi), hi.max(lo));
        if self.integer { v.round() } else { v }
    }

    /// `+0.67 EV`.
    pub fn format(&self, v: f64) -> String {
        let shown = v * self.display_scale;
        let p = self.precision as usize;
        let mut s = if self.signed && shown > 0.0 {
            format!("+{shown:.p$}")
        } else {
            format!("{shown:.p$}")
        };
        // No `-0.00`.
        if s.trim_start_matches(['-', '+'])
            .chars()
            .all(|c| c == '0' || c == '.')
        {
            s = format!("{:.p$}", 0.0);
        }
        match &self.unit {
            Some(u) if !u.is_empty() => format!("{s} {u}"),
            _ => s,
        }
    }
}

/// What a control's value is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ValueSpec {
    Number(NumberSpec),
    Bool {
        default: bool,
    },
    OneOf {
        choices: Vec<String>,
        default: String,
    },
    /// Any other stored value, kept as the store has it (a list, a text).
    Other {
        stored: ValueType,
    },
}

impl ValueSpec {
    pub fn value_type(&self) -> ValueType {
        match self {
            ValueSpec::Number(_) => ValueType::Number,
            ValueSpec::Bool { .. } => ValueType::Bool,
            ValueSpec::OneOf { .. } => ValueType::OneOf,
            ValueSpec::Other { stored } => *stored,
        }
    }

    pub fn number(&self) -> Option<&NumberSpec> {
        match self {
            ValueSpec::Number(n) => Some(n),
            _ => None,
        }
    }
}

/// A form of a control; variants of one control share its connector.
/// A variant declares what it asks of each surface: its slot forms on
/// boxed surfaces (a size policy per representation, largest first), the
/// orientations it supports (preferred first), whether it has a HUD chip
/// or an image gizmo, and who draws it where (managed unless said).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub id: String,
    pub label: String,
    /// Its forms on boxed surfaces: full, then the smaller ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<SlotForm>,
    /// Supported orientations, preferred first; empty is horizontal only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orientations: Vec<Orientation>,
    /// A one-line chip in the HUD.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chip: bool,
    /// Handles on the image.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gizmo: bool,
    /// Its integration level on the surfaces where it is not managed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub integration: BTreeMap<Surface, Integration>,
}

impl Variant {
    /// A variant with no surface yet; the builders below add them.
    pub fn new(id: &str, label: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            slots: Vec::new(),
            orientations: Vec::new(),
            chip: false,
            gizmo: false,
            integration: BTreeMap::new(),
        }
    }

    /// Its slot policy in a representation (declare full first).
    pub fn slot(mut self, representation: Representation, policy: SizePolicy) -> Self {
        self.slots.push(SlotForm {
            representation,
            policy,
        });
        self
    }

    /// Its supported orientations, preferred first.
    pub fn orientations(mut self, orientations: &[Orientation]) -> Self {
        self.orientations = orientations.to_vec();
        self
    }

    pub fn chip(mut self) -> Self {
        self.chip = true;
        self
    }

    pub fn gizmo(mut self) -> Self {
        self.gizmo = true;
        self
    }

    /// Its integration level on `surface`.
    pub fn integrated(mut self, surface: Surface, integration: Integration) -> Self {
        self.integration.insert(surface, integration);
        self
    }

    /// Whether it shows on a surface.
    pub fn shows_on(&self, surface: Surface) -> bool {
        match surface {
            Surface::Hud => self.chip,
            Surface::Image => self.gizmo,
            _ => !self.slots.is_empty(),
        }
    }

    /// The surfaces it shows on.
    pub fn surfaces(&self) -> Vec<Surface> {
        Surface::ALL
            .into_iter()
            .filter(|s| self.shows_on(*s))
            .collect()
    }

    /// Who draws it on `surface`: managed unless declared.
    pub fn integration(&self, surface: Surface) -> Integration {
        self.integration.get(&surface).cloned().unwrap_or_default()
    }

    /// Its slot form in a representation.
    pub fn form(&self, representation: Representation) -> Option<&SlotForm> {
        self.slots
            .iter()
            .find(|f| f.representation == representation)
    }

    /// Its supported orientations, preferred first.
    pub fn orientation_order(&self) -> Vec<Orientation> {
        if self.orientations.is_empty() {
            vec![Orientation::Horizontal]
        } else {
            self.orientations.clone()
        }
    }
}

/// The HUD read-out: when it shows (always the value as the control
/// formats it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Readout {
    /// `active`: while the control is handled.
    pub when: String,
}

/// A part of a case: another control, by id, at a tier (how rare it
/// is: folded under "More" below its detail level).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    pub control: String,
    #[serde(default, skip_serializing_if = "Tier::is_standard")]
    pub tier: Tier,
}

/// What a case holds, in its order: a part (a control) or chrome (a
/// legend, a separator, help text, an icon: no value, no jack).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaseItem {
    Part(Part),
    Chrome(Chrome),
}

/// A composite: its parts and its chrome, in order, and the rules it
/// declares over the context keys (`docs/design/control-contexts.md`).
/// Derived parts and their wiring come with the first case (the gradient
/// mask).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub items: Vec<CaseItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
}

impl Case {
    /// A part at the standard tier.
    pub fn part(self, name: &str, control: &str) -> Self {
        self.tiered_part(name, control, Tier::Standard)
    }

    /// A part at a tier.
    pub fn tiered_part(mut self, name: &str, control: &str, tier: Tier) -> Self {
        self.items.push(CaseItem::Part(Part {
            name: name.into(),
            control: control.into(),
            tier,
        }));
        self
    }

    /// A rule over the context keys.
    pub fn rule(mut self, rule: Rule) -> Self {
        self.rules.push(rule);
        self
    }

    pub fn chrome(mut self, chrome: Chrome) -> Self {
        self.items.push(CaseItem::Chrome(chrome));
        self
    }

    /// Its parts, without the chrome.
    pub fn parts(&self) -> impl Iterator<Item = &Part> {
        self.items.iter().filter_map(|i| match i {
            CaseItem::Part(p) => Some(p),
            CaseItem::Chrome(_) => None,
        })
    }
}

/// A business control.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Control {
    /// The purpose, stable (`exposure`).
    pub id: String,
    pub label: String,
    /// What it is for, one sentence.
    pub purpose: String,
    /// A control, or an indicator (read-only jacks).
    #[serde(default, skip_serializing_if = "Kind::is_control")]
    pub kind: Kind,
    pub value: ValueSpec,
    pub connector: Connector,
    /// The first is the default.
    pub variants: Vec<Variant>,
    pub feel: Feel,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detents: Vec<Detent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readout: Option<Readout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<Case>,
    /// Built from a parameter's descriptor and hints, not declared.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub implicit: bool,
}

impl Control {
    /// A control for a purpose, with a value; the rest from the builders.
    pub fn new(id: &str, label: &str, value: ValueSpec) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            purpose: String::new(),
            kind: Kind::Control,
            value,
            connector: Connector {
                signals: vec![Signal::Preview, Signal::Commit, Signal::Cancel],
                ..Connector::default()
            },
            variants: Vec::new(),
            feel: Feel::default(),
            detents: Vec::new(),
            readout: None,
            case: None,
            implicit: false,
        }
    }

    pub fn purpose(mut self, purpose: &str) -> Self {
        self.purpose = purpose.into();
        self
    }

    /// Makes it an indicator: declare its jacks read-only.
    pub fn indicator(mut self) -> Self {
        self.kind = Kind::Indicator;
        self
    }

    pub fn jack(mut self, jack: Jack) -> Self {
        self.connector.jacks.push(jack);
        self
    }

    pub fn gestures(mut self, gestures: &[Gesture]) -> Self {
        self.connector.gestures = gestures.to_vec();
        self
    }

    pub fn intents(mut self, intents: &[Intent]) -> Self {
        self.connector.intents = intents.to_vec();
        self
    }

    pub fn variant(mut self, variant: Variant) -> Self {
        self.variants.push(variant);
        self
    }

    pub fn feel(mut self, feel: Feel) -> Self {
        self.feel = feel;
        self
    }

    pub fn detent(mut self, value: f64, window: f64) -> Self {
        self.detents.push(Detent { value, window });
        self
    }

    pub fn readout(mut self, when: &str) -> Self {
        self.readout = Some(Readout { when: when.into() });
        self
    }

    pub fn case(mut self, case: Case) -> Self {
        self.case = Some(case);
        self
    }

    /// The variant by id, or the default one.
    pub fn variant_or_default(&self, id: Option<&str>) -> Option<&Variant> {
        id.and_then(|id| self.variants.iter().find(|v| v.id == id))
            .or_else(|| self.variants.first())
    }

    /// What is wrong with the declaration, if anything.
    pub fn check(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("a control needs an id".into());
        }
        if self.variants.is_empty() {
            return Err(format!("control `{}` has no variant", self.id));
        }
        for v in &self.variants {
            self.check_variant(v)
                .map_err(|e| format!("control `{}`, variant `{}`: {e}", self.id, v.id))?;
        }
        let read_only = self.connector.jacks.iter().filter(|j| j.read_only).count();
        match self.kind {
            Kind::Indicator if read_only < self.connector.jacks.len() => {
                return Err(format!(
                    "indicator `{}` has a jack it writes; an indicator's jacks are read-only",
                    self.id
                ));
            }
            Kind::Control if read_only > 0 && read_only == self.connector.jacks.len() => {
                return Err(format!(
                    "control `{}` writes no jack: declare it an indicator",
                    self.id
                ));
            }
            _ => {}
        }
        if let Some(case) = &self.case {
            let names: Vec<&str> = case.parts().map(|p| p.name.as_str()).collect();
            for r in &case.rules {
                r.check(&names)
                    .map_err(|e| format!("control `{}`: {e}", self.id))?;
            }
        }
        if let Some(n) = self.value.number() {
            if !(n.hard.0 <= n.soft.0 && n.soft.1 <= n.hard.1) {
                return Err(format!(
                    "control `{}`: soft range outside the hard one",
                    self.id
                ));
            }
            if let Some(d) = self
                .detents
                .iter()
                .find(|d| d.value < n.hard.0 || d.value > n.hard.1)
            {
                return Err(format!(
                    "control `{}`: detent {} outside the hard range",
                    self.id, d.value
                ));
            }
        }
        Ok(())
    }

    fn check_variant(&self, v: &Variant) -> Result<(), String> {
        if v.surfaces().is_empty() {
            return Err("it shows on no surface".into());
        }
        let mut last = None;
        for f in &v.slots {
            if last.is_some_and(|r| r >= f.representation) {
                return Err(format!(
                    "slot forms go full, compact, simplified, each once; {:?} is out of order",
                    f.representation
                ));
            }
            last = Some(f.representation);
            f.policy
                .check()
                .map_err(|e| format!("{:?} slot: {e}", f.representation))?;
        }
        if v.slots
            .first()
            .is_some_and(|f| f.representation != Representation::Full)
        {
            return Err("its first slot form is not the full one".into());
        }
        let mut seen = Vec::new();
        for o in &v.orientations {
            if seen.contains(o) {
                return Err(format!("orientation {o} given twice"));
            }
            seen.push(*o);
        }
        for (surface, level) in &v.integration {
            if !v.shows_on(*surface) {
                return Err(format!("integrated on {surface:?}, where it does not show"));
            }
            match level {
                Integration::Integrated {
                    fallback: Some(Fallback::Variant(id)),
                } => {
                    let Some(f) = self.variants.iter().find(|o| &o.id == id) else {
                        return Err(format!("its fallback variant `{id}` does not exist"));
                    };
                    if f.id == v.id
                        || !f.shows_on(*surface)
                        || f.integration(*surface) != Integration::Managed
                    {
                        return Err(format!(
                            "its fallback variant `{id}` is not managed on {surface:?}"
                        ));
                    }
                }
                Integration::Integrated {
                    fallback: Some(Fallback::Parts),
                } if self.case.is_none() => {
                    return Err("its fallback is its parts, but it is not a case".into());
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// What the inspector counts but does not refuse: an integrated
    /// variant with no managed fallback, per surface.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for v in &self.variants {
            for (surface, level) in &v.integration {
                if *level == (Integration::Integrated { fallback: None }) {
                    out.push(format!(
                        "control `{}`, variant `{}`: integrated on {} with no managed fallback",
                        self.id,
                        v.id,
                        surface_name(*surface)
                    ));
                }
            }
        }
        out
    }

    /// How each variant lays out: its slot policies per representation,
    /// orientations, chip and gizmo, and its integration level per
    /// surface with the fallback. What `controls.describe` reports.
    pub fn layout_report(&self) -> Json {
        let variants: serde_json::Map<String, Json> = self
            .variants
            .iter()
            .map(|v| {
                let slots: serde_json::Map<String, Json> = v
                    .slots
                    .iter()
                    .map(|f| {
                        (
                            representation_name(f.representation).to_owned(),
                            json!({
                                "policy": f.policy,
                                "summary": f.policy.to_string(),
                            }),
                        )
                    })
                    .collect();
                let integration: serde_json::Map<String, Json> = v
                    .surfaces()
                    .into_iter()
                    .map(|s| {
                        let level = v.integration(s);
                        let (name, fallback) = match &level {
                            Integration::Managed => ("managed", Json::Null),
                            Integration::Integrated { fallback } => ("integrated", json!(fallback)),
                        };
                        (
                            surface_name(s).to_owned(),
                            json!({"level": name, "fallback": fallback}),
                        )
                    })
                    .collect();
                (
                    v.id.clone(),
                    json!({
                        "slots": slots,
                        "orientations": v.orientation_order(),
                        "preferred_orientation": v.orientation_order()[0],
                        "surfaces": v.surfaces().into_iter().map(surface_name).collect::<Vec<_>>(),
                        "integration": integration,
                    }),
                )
            })
            .collect();
        json!({
            "kind": self.kind,
            "variants": variants,
            "warnings": self.warnings(),
        })
    }

    /// A value moved by a hand, held on a detent within its window
    /// (unless `free`, the modifier that turns detents off).
    pub fn detented(&self, v: f64, free: bool) -> f64 {
        if free {
            return v;
        }
        self.detents
            .iter()
            .find(|d| (v - d.value).abs() <= d.window)
            .map_or(v, |d| d.value)
    }

    /// Where a scrub of `points` from `start` leads: the control's
    /// sensitivity, the fine factor when asked, its detents, its soft
    /// range (past it only if it started past).
    pub fn scrubbed(&self, start: f64, points: f64, fine: bool, free: bool) -> f64 {
        let Some(n) = self.value.number() else {
            return start;
        };
        let speed = self.feel.sensitivity * if fine { self.feel.fine } else { 1.0 };
        let v = self.detented(start + points * speed, free);
        let lo = n.soft.0.min(start);
        let hi = n.soft.1.max(start);
        let v = v.clamp(lo, hi);
        if n.integer { v.round() } else { v }
    }

    /// Where `n` steps from `v` lead (fine or coarse when asked), on the
    /// step's grid, held to the soft range.
    pub fn nudged(&self, v: f64, n: i64, fine: bool, coarse: bool) -> f64 {
        let Some(spec) = self.value.number() else {
            return v;
        };
        let step = if coarse {
            spec.coarse
        } else if fine {
            spec.step * self.feel.fine
        } else {
            spec.step
        };
        let to = v + n as f64 * step;
        // Back on the grid of the step, so 0 is reached exactly.
        let to = (to / step).round() * step;
        spec.clamp(to, false)
    }

    /// The JSON schema of what a person may set for this control (its
    /// variant, its feel, its detents on or off): customisation within
    /// its purpose.
    pub fn config_schema(&self) -> Json {
        let variants: Vec<&str> = self.variants.iter().map(|v| v.id.as_str()).collect();
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": format!("{} settings", self.label),
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "variant": {
                    "enum": variants,
                    "default": variants.first(),
                },
                "sensitivity": {
                    "type": "number",
                    "exclusiveMinimum": 0,
                    "default": self.feel.sensitivity,
                },
                "fine": {
                    "type": "number",
                    "exclusiveMinimum": 0,
                    "maximum": 1,
                    "default": self.feel.fine,
                },
                "detents": {
                    "type": "boolean",
                    "default": !self.detents.is_empty(),
                },
            },
        })
    }
}

/// A surface's name as data (`panel`, `hud`).
pub fn surface_name(s: Surface) -> &'static str {
    match s {
        Surface::Panel => "panel",
        Surface::Dock => "dock",
        Surface::Toolbar => "toolbar",
        Surface::Float => "float",
        Surface::Hud => "hud",
        Surface::Image => "image",
    }
}

/// A representation's name as data (`full`).
pub fn representation_name(r: Representation) -> &'static str {
    match r {
        Representation::Full => "full",
        Representation::Compact => "compact",
        Representation::Simplified => "simplified",
    }
}
