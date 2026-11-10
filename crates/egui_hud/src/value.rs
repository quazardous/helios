//! What the framework asks of the application's values: a value type of
//! its own ([`Value`]), a parameter descriptor for the controls built
//! from it ([`ParamDescriptor`], see [`crate::registry::implicit`]), and
//! the store the value flow writes through ([`ValueSource`] to read,
//! [`ValueStore`] to write inside gestures).
//!
//! Paths name parameters; a card's parameters share a prefix
//! (`node/<node>/<param>`, see [`crate::flow::node_of`]).
//! [`MemoryStore`] is a small store in memory, for tests and benches.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

/// A parameter's value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Float(f64),
    Int(i64),
    Bool(bool),
    Choice(String),
    /// Numbers that belong together (a curve's points, a matrix).
    Floats(Vec<f64>),
    /// Whole numbers that belong together.
    Ints(Vec<i64>),
    Text(String),
}

impl Value {
    /// The value as a number: a float or an integer.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(v) => Some(*v),
            Value::Int(v) => Some(*v as f64),
            _ => None,
        }
    }

    /// The value as plain JSON (a number, a boolean, a string, an array).
    pub fn to_json(&self) -> Json {
        match self {
            Value::Float(v) => json!(v),
            Value::Int(v) => json!(v),
            Value::Bool(v) => json!(v),
            Value::Choice(v) | Value::Text(v) => json!(v),
            Value::Floats(v) => json!(v),
            Value::Ints(v) => json!(v),
        }
    }
}

/// A parameter's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParamType {
    Float,
    Int,
    Bool,
    Choice,
    Floats,
    Ints,
    Text,
}

/// An editor that draws itself, asked for a list of numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Widget {
    /// A curve, drawn in a block six rows high.
    Curve,
    /// A colour wheel, drawn in a block four rows high.
    Wheel,
}

/// What a parameter is: its type, range, steps, choices and display.
/// What an implicit control is built from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamDescriptor {
    pub ty: ParamType,
    pub label: String,
    pub description: String,
    pub default: Value,
    /// What a write is held to, always.
    pub hard: Option<(f64, f64)>,
    /// What a slider spans; past it up to `hard` only when forced.
    pub soft: Option<(f64, f64)>,
    pub step: f64,
    pub coarse: f64,
    pub unit: Option<String>,
    /// Shown value = stored value times this.
    pub display_scale: f64,
    pub precision: u32,
    pub choices: Vec<String>,
    pub logarithmic: bool,
    pub widget: Option<Widget>,
    /// The module (or any owner) the parameter belongs to: what a patch
    /// names ([`crate::registry::Patch`]).
    pub module: Option<String>,
    /// Value units per point of a scrub, when the parameter states one;
    /// otherwise its implicit control derives one from the soft range
    /// ([`crate::registry::scrub_sensitivity`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitivity: Option<f64>,
}

impl ParamDescriptor {
    /// A parameter of type `ty` with its default, no range and no unit.
    pub fn new(ty: ParamType, label: &str, default: Value) -> Self {
        Self {
            ty,
            label: label.into(),
            description: String::new(),
            default,
            hard: None,
            soft: None,
            step: 0.01,
            coarse: 0.1,
            unit: None,
            display_scale: 1.0,
            precision: 2,
            choices: Vec::new(),
            logarithmic: false,
            widget: None,
            module: None,
            sensitivity: None,
        }
    }

    /// A float from `min` to `max` (the hard and the soft range).
    pub fn float(label: &str, min: f64, max: f64, default: f64) -> Self {
        let mut d = Self::new(ParamType::Float, label, Value::Float(default));
        d.hard = Some((min, max));
        d.soft = Some((min, max));
        d
    }

    /// The range a write is held to: the soft one unless forced.
    pub fn range(&self, force: bool) -> Option<(f64, f64)> {
        if force {
            self.hard
        } else {
            self.soft.or(self.hard)
        }
    }
}

/// One parameter as a store lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub path: String,
    pub value: Value,
    pub descriptor: ParamDescriptor,
}

/// What the framework reads from the application's values.
pub trait ValueSource {
    /// The value at `path`; `None` when nothing is there.
    fn get(&self, path: &str) -> Option<Value>;

    /// The default of the value at `path`.
    fn default_value(&self, path: &str) -> Option<Value>;

    /// Every parameter whose path starts with `prefix`, in path order.
    fn params(&self, prefix: &str) -> Vec<Param>;
}

/// What the value flow writes through ([`crate::flow`]): gestures, each
/// one undo step when committed and no trace when cancelled.
///
/// - [`begin`](Self::begin) opens a gesture labelled by the control,
///   holding its paths against other origins until it ends.
/// - [`preview`](Self::preview) writes inside the gesture, held to the
///   soft range unless forced, to the hard one always; it answers what
///   the store holds.
/// - [`commit`](Self::commit) ends the gesture as one undo step (none
///   when nothing changed).
/// - [`cancel`](Self::cancel) puts back the values from before the
///   gesture, leaves no step, and answers how many values went back.
pub trait ValueStore: ValueSource {
    /// Who writes (the person, an agent, a script, a device).
    type Origin: Clone + fmt::Debug;
    /// An open gesture's handle.
    type Gesture: Copy + fmt::Debug;

    fn begin(&mut self, origin: &Self::Origin, label: &str) -> Self::Gesture;

    fn preview(
        &mut self,
        gesture: Self::Gesture,
        path: &str,
        value: Value,
        origin: &Self::Origin,
        force: bool,
    ) -> Result<Value, String>;

    fn commit(&mut self, gesture: Self::Gesture);

    fn cancel(&mut self, gesture: Self::Gesture) -> usize;

    /// The device an origin reports in a change (`pointer`, `agent`,
    /// `midi`…).
    fn device(origin: &Self::Origin) -> &'static str;
}

/// An open gesture of a [`MemoryStore`].
#[derive(Debug)]
struct Open {
    id: u64,
    label: String,
    origin: &'static str,
    before: BTreeMap<String, Value>,
}

/// One undo step of a [`MemoryStore`]: each path's value before and
/// after.
#[derive(Debug, Clone, PartialEq)]
struct Step {
    label: String,
    changes: Vec<(String, Value, Value)>,
}

/// A value store in memory: parameters declared with their descriptor,
/// one gesture open at a time, undo steps with their labels, undone and
/// redone. Its origin is the device's name (`pointer`, `agent`…).
#[derive(Debug, Default)]
pub struct MemoryStore {
    params: BTreeMap<String, (Value, ParamDescriptor)>,
    open: Option<Open>,
    next: u64,
    steps: Vec<String>,
    done: Vec<Step>,
    undone: Vec<Step>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares a parameter at its default.
    pub fn declare(&mut self, path: &str, d: ParamDescriptor) {
        self.params.insert(path.into(), (d.default.clone(), d));
    }

    /// The undo steps, by label, oldest first.
    pub fn undo_steps(&self) -> &[String] {
        &self.steps
    }

    /// The open gesture's label.
    pub fn open_gesture(&self) -> Option<&str> {
        self.open.as_ref().map(|o| o.label.as_str())
    }

    /// The steps undone, newest last: what [`redo`](Self::redo) replays.
    pub fn redo_steps(&self) -> Vec<&str> {
        self.undone.iter().map(|s| s.label.as_str()).collect()
    }

    /// Puts back the values from before the last step, and answers its
    /// label; `None` with nothing to undo or a gesture open.
    pub fn undo(&mut self) -> Option<String> {
        if self.open.is_some() {
            return None;
        }
        let step = self.done.pop()?;
        self.steps.pop();
        for (path, before, _) in &step.changes {
            if let Some((now, _)) = self.params.get_mut(path) {
                *now = before.clone();
            }
        }
        let label = step.label.clone();
        self.undone.push(step);
        Some(label)
    }

    /// Writes again the values of the last step undone, and answers its
    /// label; `None` with nothing to redo or a gesture open.
    pub fn redo(&mut self) -> Option<String> {
        if self.open.is_some() {
            return None;
        }
        let step = self.undone.pop()?;
        for (path, _, after) in &step.changes {
            if let Some((now, _)) = self.params.get_mut(path) {
                *now = after.clone();
            }
        }
        let label = step.label.clone();
        self.steps.push(label.clone());
        self.done.push(step);
        Some(label)
    }
}

impl ValueSource for MemoryStore {
    fn get(&self, path: &str) -> Option<Value> {
        self.params.get(path).map(|(v, _)| v.clone())
    }

    fn default_value(&self, path: &str) -> Option<Value> {
        self.params.get(path).map(|(_, d)| d.default.clone())
    }

    fn params(&self, prefix: &str) -> Vec<Param> {
        self.params
            .range(prefix.to_owned()..)
            .take_while(|(p, _)| p.starts_with(prefix))
            .map(|(p, (v, d))| Param {
                path: p.clone(),
                value: v.clone(),
                descriptor: d.clone(),
            })
            .collect()
    }
}

impl ValueStore for MemoryStore {
    type Origin = &'static str;
    type Gesture = u64;

    fn begin(&mut self, origin: &&'static str, label: &str) -> u64 {
        self.next += 1;
        self.open = Some(Open {
            id: self.next,
            label: label.into(),
            origin,
            before: BTreeMap::new(),
        });
        self.next
    }

    fn preview(
        &mut self,
        gesture: u64,
        path: &str,
        value: Value,
        origin: &&'static str,
        force: bool,
    ) -> Result<Value, String> {
        let open = self
            .open
            .as_mut()
            .filter(|o| o.id == gesture)
            .ok_or("no such gesture")?;
        if open.origin != *origin {
            return Err(format!("`{path}` is held by `{}`", open.origin));
        }
        let (now, d) = self
            .params
            .get_mut(path)
            .ok_or_else(|| format!("no value at `{path}`"))?;
        let value = match (value.as_f64(), d.range(force)) {
            (Some(v), Some((lo, hi))) => {
                let v = v.clamp(lo, hi);
                if d.ty == ParamType::Int {
                    Value::Int(v.round() as i64)
                } else {
                    Value::Float(v)
                }
            }
            _ => value,
        };
        open.before
            .entry(path.into())
            .or_insert_with(|| now.clone());
        *now = value.clone();
        Ok(value)
    }

    fn commit(&mut self, gesture: u64) {
        let Some(o) = self.open.take_if(|o| o.id == gesture) else {
            return;
        };
        let changes: Vec<(String, Value, Value)> = o
            .before
            .into_iter()
            .filter_map(|(p, before)| {
                let now = self.params.get(&p).map(|(v, _)| v.clone())?;
                (now != before).then_some((p, before, now))
            })
            .collect();
        if !changes.is_empty() {
            self.steps.push(o.label.clone());
            self.done.push(Step {
                label: o.label,
                changes,
            });
            self.undone.clear();
        }
    }

    fn cancel(&mut self, gesture: u64) -> usize {
        let Some(o) = self.open.take_if(|o| o.id == gesture) else {
            return 0;
        };
        let n = o.before.len();
        for (p, v) in o.before {
            if let Some((now, _)) = self.params.get_mut(&p) {
                *now = v;
            }
        }
        n
    }

    fn device(origin: &&'static str) -> &'static str {
        origin
    }
}
