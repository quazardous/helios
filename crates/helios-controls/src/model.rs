//! The controls model, without egui: what a control is, declared with
//! Rust builders that produce serialisable data (`docs/design/controls.md`,
//! sections 3.1 to 3.3, question Q4).
//!
//! - A [`Control`] is built for one purpose (exposure, feather): its value
//!   ([`ValueSpec`]), units and ranges, [`Feel`] (sensitivity, fine
//!   factor, response curve), [`Detent`]s, [`Readout`], [`Connector`] and
//!   [`Variant`]s.
//! - The rack standard: [`Footprint`]s per [`Surface`], typed
//!   [`Connector`]s (jacks, gestures, intents, signals), [`Case`]s
//!   (composites, a skeleton for now), variants, and the ergonomic
//!   contract ([`HIT_AREA`], [`Feel`], detents).
//! - Where a control lives ([`Mode`]) and how much it shows
//!   ([`Representation`]): hooks only, the context rules wait for a usage
//!   study.

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

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

/// Where a control appears.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Surface {
    /// The module's card.
    Panel,
    /// Read-outs and small controls over the image.
    Hud,
    /// Handles on the image.
    Image,
}

/// The standard sizes a control fits on a surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Footprint {
    /// Half a panel row: two side by side.
    Half,
    /// One panel row.
    Row,
    /// So many panel rows, 2 to 8.
    Block(u8),
    /// One line in the HUD.
    Chip,
    /// No box: handles in the image's space.
    Gizmo,
}

impl Footprint {
    /// The surface it belongs to.
    pub fn surface(self) -> Surface {
        match self {
            Footprint::Half | Footprint::Row | Footprint::Block(_) => Surface::Panel,
            Footprint::Chip => Surface::Hud,
            Footprint::Gizmo => Surface::Image,
        }
    }

    /// Whether it is a valid footprint (a block of 2 to 8 rows).
    pub fn is_valid(self) -> bool {
        !matches!(self, Footprint::Block(n) if !(2..=8).contains(&n))
    }
}

/// Where a control lives (hooks only: which mode a context picks is not
/// settled).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Panel,
    Docked,
    Floating,
    Hud,
    Toolbar,
}

/// How much a control shows (hooks only, as [`Mode`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl Jack {
    pub fn new(name: &str, ty: ValueType) -> Self {
        Self {
            name: name.into(),
            ty,
            unit: None,
            space: None,
        }
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub id: String,
    pub label: String,
    /// Its footprint on each surface it shows on.
    pub footprints: Vec<Footprint>,
}

impl Variant {
    pub fn new(id: &str, label: &str, footprints: &[Footprint]) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            footprints: footprints.to_vec(),
        }
    }

    /// Its footprint on a surface.
    pub fn footprint(&self, surface: Surface) -> Option<Footprint> {
        self.footprints
            .iter()
            .copied()
            .find(|f| f.surface() == surface)
    }
}

/// The HUD read-out: when it shows (always the value as the control
/// formats it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Readout {
    /// `active`: while the control is handled.
    pub when: String,
}

/// A part of a case: another control, by id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    pub control: String,
}

/// A composite's skeleton: its parts. Derived parts and their wiring come
/// with the first case (the gradient mask).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub parts: Vec<Part>,
}

/// A business control.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Control {
    /// The purpose, stable (`exposure`).
    pub id: String,
    pub label: String,
    /// What it is for, one sentence.
    pub purpose: String,
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
            if let Some(f) = v.footprints.iter().find(|f| !f.is_valid()) {
                return Err(format!(
                    "control `{}`, variant `{}`: footprint {f:?} is not standard",
                    self.id, v.id
                ));
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
