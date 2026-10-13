//! The value store: every value a widget or an agent edits lives here, at a
//! stable path, never in a widget (see `docs/widgets.md`).
//!
//! - Paths: `node/<node>/<param>` for a module's parameters (mirrored onto
//!   the graph, which renders), `settings/<key>`, `view/<key>`.
//! - Each path has a [`Descriptor`]: type, hard and soft range, fine and
//!   coarse steps, unit, display scale and precision, default, label,
//!   choices. A module's come from its manifest and its parameter hints.
//!   A text may name a path on disk ([`PathHint`]): forms then offer to
//!   browse for it.
//! - Every write is a [`Set`]: an absolute value, a relative step, reset,
//!   top or bottom, with its origin. The answer says what was applied and
//!   whether it was clamped; a refusal says why and gives the current value.
//! - During a gesture (a drag), the paths it touched belong to it: a write
//!   from another origin is refused. Otherwise the last writer wins; a
//!   writer may ask to write only if nothing changed since it read
//!   (`if_seq`).
//! - Every change is announced on the bus (`param.changed`: path, old, new,
//!   origin, sequence); a write equal to the value changes nothing and
//!   announces nothing, so a change never loops back.
//! - Undo: one step per gesture or group, else one per write; a cancelled
//!   gesture puts its values back and is no step.

mod commands;
mod graph;

pub use commands::{register, value_from_json, value_to_json};
pub use graph::{bind_node, node_path, sync};

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use helios_core::{Bus, Event, ParamHint, ParamKind, ParamSpec, Value};
use helios_shell::commands::Origin;
use serde::Serialize;

/// The type of a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Type {
    Float,
    Int,
    Bool,
    Choice,
    Floats,
    Ints,
    Text,
}

/// What a path names: a directory or a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PathKind {
    Dir,
    File,
}

/// A text that names a path on disk: what it names and, for a file, the
/// extensions accepted (none: any). A form shows it with a Browse button.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PathHint {
    pub kind: PathKind,
    /// Without the dot, lowercase (`jpg`, `xmp`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,
}

impl PathHint {
    /// Whether a file's name has an accepted extension.
    pub fn accepts(&self, path: &std::path::Path) -> bool {
        self.extensions.is_empty()
            || path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| self.extensions.iter().any(|x| x.eq_ignore_ascii_case(e)))
    }
}

/// What a value is and how it is shown and moved.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Descriptor {
    #[serde(rename = "type")]
    pub ty: Type,
    pub label: String,
    pub description: String,
    pub default: Value,
    /// What the value can be, at most.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hard: Option<(f64, f64)>,
    /// What a slider spans; past it up to `hard` only when forced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soft: Option<(f64, f64)>,
    pub step: f64,
    pub coarse: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Shown value = value × scale.
    pub display_scale: f64,
    /// Decimals shown.
    pub precision: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// How many values a list holds at most.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// How many bytes a text holds at most.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity: Option<u32>,
    /// A text that names a path on disk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathHint>,
    /// Kept out of the history (a view's zoom and position): a write is
    /// announced like any other but never makes an undo step, so it is
    /// never undone and never commits (nothing to save).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub transient: bool,
    /// The module whose parameter this is (a node's value), so an
    /// interface finds its translations (`param-<module>-<name>`); `None`
    /// for every other value.
    #[serde(skip)]
    pub module: Option<String>,
}

/// Decimals enough to show one step.
fn precision_of(step: f64) -> u32 {
    if step >= 1.0 || step <= 0.0 {
        0
    } else {
        (-step.log10()).ceil() as u32
    }
}

impl Descriptor {
    fn base(ty: Type, label: &str, default: Value) -> Self {
        Self {
            ty,
            label: label.into(),
            description: String::new(),
            default,
            hard: None,
            soft: None,
            step: 1.0,
            coarse: 10.0,
            unit: None,
            display_scale: 1.0,
            precision: 0,
            choices: Vec::new(),
            count: None,
            capacity: None,
            path: None,
            transient: false,
            module: None,
        }
    }

    /// The same value kept out of the history (see
    /// [`Descriptor::transient`]).
    pub fn transient(mut self) -> Self {
        self.transient = true;
        self
    }

    /// A float in `[min, max]`, its fine step a hundredth of the range.
    pub fn float(label: &str, min: f64, max: f64, default: f64) -> Self {
        let step = if max > min { (max - min) / 100.0 } else { 0.01 };
        Self {
            hard: Some((min, max)),
            step,
            coarse: step * 10.0,
            precision: precision_of(step),
            ..Self::base(Type::Float, label, Value::Float(default))
        }
    }

    pub fn int(label: &str, min: i64, max: i64, default: i64) -> Self {
        Self {
            hard: Some((min as f64, max as f64)),
            ..Self::base(Type::Int, label, Value::Int(default))
        }
    }

    pub fn bool(label: &str, default: bool) -> Self {
        Self::base(Type::Bool, label, Value::Bool(default))
    }

    pub fn choice(label: &str, choices: &[&str], default: &str) -> Self {
        Self {
            choices: choices
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            ..Self::base(Type::Choice, label, Value::Choice(default.into()))
        }
    }

    pub fn text(label: &str, default: &str) -> Self {
        Self::base(Type::Text, label, Value::Text(default.into()))
    }

    /// A text naming a directory or a file, empty by default; for a file,
    /// the extensions accepted (none: any).
    pub fn path(label: &str, kind: PathKind, extensions: &[&str]) -> Self {
        Self {
            path: Some(PathHint {
                kind,
                extensions: extensions.iter().map(|e| e.to_lowercase()).collect(),
            }),
            ..Self::base(Type::Text, label, Value::Text(String::new()))
        }
    }

    /// From a module's parameter and its hint, if any.
    pub fn from_spec(spec: &ParamSpec, hint: Option<&ParamHint>) -> Self {
        let mut d = match &spec.kind {
            ParamKind::Float { min, max, default } => Self::float(&spec.name, *min, *max, *default),
            ParamKind::Int { min, max, default } => Self::int(&spec.name, *min, *max, *default),
            ParamKind::Bool { default } => Self::bool(&spec.name, *default),
            ParamKind::Choice { options, default } => {
                let options: Vec<&str> = options.iter().map(String::as_str).collect();
                Self::choice(&spec.name, &options, default)
            }
            ParamKind::Floats {
                count, min, max, ..
            } => {
                let step = if max > min { (max - min) / 100.0 } else { 0.01 };
                Self {
                    hard: Some((*min, *max)),
                    count: Some(*count),
                    step,
                    coarse: step * 10.0,
                    precision: precision_of(step),
                    ..Self::base(Type::Floats, &spec.name, spec.kind.default_value())
                }
            }
            ParamKind::Ints {
                count, min, max, ..
            } => Self {
                hard: Some((*min as f64, *max as f64)),
                count: Some(*count),
                ..Self::base(Type::Ints, &spec.name, spec.kind.default_value())
            },
            ParamKind::Text { capacity, .. } => Self {
                capacity: Some(*capacity),
                ..Self::base(Type::Text, &spec.name, spec.kind.default_value())
            },
        };
        d.description.clone_from(&spec.description);
        if let Some(h) = hint {
            if let Some((lo, hi)) = h.soft {
                // Within the hard range, whatever the hint says.
                d.soft = Some(match d.hard {
                    Some((min, max)) => (lo.max(min), hi.min(max)),
                    None => (lo, hi),
                });
            }
            if let Some(s) = h.step {
                d.step = s;
                d.coarse = s * 10.0;
                d.precision = precision_of(s);
            }
            if let Some(c) = h.coarse {
                d.coarse = c;
            }
            d.unit.clone_from(&h.unit);
            if let Some(s) = h.display_scale {
                d.display_scale = s;
            }
            if let Some(p) = h.precision {
                d.precision = p;
            }
            if let Some(l) = &h.label {
                d.label.clone_from(l);
            }
        }
        d
    }

    /// The range a write is held to: the soft one unless forced.
    fn range(&self, force: bool) -> Option<(f64, f64)> {
        if force {
            self.hard
        } else {
            self.soft.or(self.hard)
        }
    }
}

/// A value at its path.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    pub descriptor: Descriptor,
    pub value: Value,
    /// The store's sequence number when it last changed (0: never).
    pub seq: u64,
    /// Who changed it last; `None` until someone does.
    pub origin: Option<Origin>,
    /// The order it was declared in (a manifest's order).
    pub order: u64,
}

/// What a write asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    To(Value),
    /// So many fine steps (negative: down).
    Step(i64),
    /// So many coarse steps.
    Coarse(i64),
    Reset,
    Top,
    Bottom,
}

/// A write.
#[derive(Clone, Debug, PartialEq)]
pub struct Set {
    pub path: String,
    pub change: Change,
    pub origin: Origin,
    /// Past the soft range, up to the hard one.
    pub force: bool,
    pub gesture: Option<GestureId>,
    /// Only if the value has not changed since this sequence number.
    pub if_seq: Option<u64>,
}

impl Set {
    pub fn new(path: &str, change: Change, origin: Origin) -> Self {
        Self {
            path: path.into(),
            change,
            origin,
            force: false,
            gesture: None,
            if_seq: None,
        }
    }

    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }

    pub fn gesture(mut self, gesture: GestureId) -> Self {
        self.gesture = Some(gesture);
        self
    }

    pub fn if_seq(mut self, seq: u64) -> Self {
        self.if_seq = Some(seq);
        self
    }
}

/// Which range held a write back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Clamped {
    None,
    Soft,
    Hard,
}

/// What a write did.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Applied {
    pub value: Value,
    pub clamped: Clamped,
    /// The value's sequence number now.
    pub seq: u64,
    /// False when the value was already that.
    pub changed: bool,
}

/// Why a write was refused; each gives the current value.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "refused", rename_all = "snake_case")]
pub enum Refused {
    Unknown {
        path: String,
    },
    Invalid {
        reason: String,
        current: Value,
    },
    /// It changed since the writer read it.
    Stale {
        current: Value,
        seq: u64,
    },
    /// Someone else's gesture holds it.
    Held {
        current: Value,
        by: Origin,
    },
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Unknown { path } => write!(f, "no value at `{path}`"),
            Refused::Invalid { reason, current } => {
                write!(f, "{reason} (the value stays {current:?})")
            }
            Refused::Stale { current, seq } => write!(
                f,
                "it changed since it was read (now {current:?}, sequence {seq})"
            ),
            Refused::Held { current, by } => write!(
                f,
                "a gesture of {by:?} holds it (the value stays {current:?})"
            ),
        }
    }
}

/// A value changed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParamChanged {
    pub path: String,
    pub old: Value,
    pub new: Value,
    pub origin: Origin,
    pub seq: u64,
}

impl Event for ParamChanged {
    const KIND: &'static str = "param.changed";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct GestureId(pub u64);

struct Gesture {
    origin: Origin,
    label: String,
    /// Whether the paths it touched are held against other origins: a
    /// drag holds them, a group of an agent's writes does not.
    holds: bool,
    paths: BTreeSet<String>,
    changes: Vec<(String, Value, Value)>,
}

/// One undo step.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Step {
    pub label: String,
    pub origin: Origin,
    /// Path, old value, new value, in the order written.
    pub changes: Vec<(String, Value, Value)>,
}

#[derive(Default)]
pub struct Store {
    entries: BTreeMap<String, Entry>,
    seq: u64,
    bus: Option<Arc<Bus>>,
    gestures: BTreeMap<GestureId, Gesture>,
    next_gesture: u64,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// `node/…` paths changed since the graph was last synced.
    dirty: BTreeSet<String>,
    /// Steps committed so far, and what the last one was.
    committed: u64,
    last_committed: String,
}

fn clamp(v: f64, range: Option<(f64, f64)>, soft: bool) -> (f64, Clamped) {
    match range {
        Some((lo, hi)) if v < lo || v > hi => (
            v.clamp(lo, hi),
            if soft { Clamped::Soft } else { Clamped::Hard },
        ),
        _ => (v, Clamped::None),
    }
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    /// Announces every change on `bus`.
    pub fn with_bus(mut self, bus: Arc<Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Declares a path at its default value.
    pub fn declare(&mut self, path: &str, descriptor: Descriptor) {
        let value = descriptor.default.clone();
        self.declare_with(path, descriptor, value);
    }

    /// Declares a path at a value (a node's current parameter).
    pub fn declare_with(&mut self, path: &str, descriptor: Descriptor, value: Value) {
        let order = self
            .entries
            .get(path)
            .map_or(self.entries.len() as u64, |e| e.order);
        self.entries.insert(
            path.into(),
            Entry {
                descriptor,
                value,
                seq: 0,
                origin: None,
                order,
            },
        );
    }

    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.entries.get(path)
    }

    /// The paths under a prefix (`node/exposure/`), in order.
    pub fn list(&self, prefix: &str) -> Vec<(&str, &Entry)> {
        self.entries
            .range(prefix.to_string()..)
            .take_while(|(p, _)| p.starts_with(prefix))
            .map(|(p, e)| (p.as_str(), e))
            .collect()
    }

    /// The store's sequence number: the last change's.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// What a change makes of the value, before ranges are checked.
    fn target(entry: &Entry, set: &Set) -> Result<(Value, Clamped), String> {
        let d = &entry.descriptor;
        let range = d.range(set.force);
        let soft = !set.force && d.soft.is_some();
        let number = |v: f64| -> Result<(Value, Clamped), String> {
            if !v.is_finite() {
                return Err(format!("{v} is not a number"));
            }
            let (v, c) = clamp(v, range, soft);
            Ok(match d.ty {
                Type::Int => (Value::Int(v.round() as i64), c),
                _ => (Value::Float(v), c),
            })
        };
        let current = entry.value.as_f64();
        match (&set.change, d.ty) {
            (Change::Reset, _) => Ok((d.default.clone(), Clamped::None)),
            (Change::To(Value::Float(v)), Type::Float | Type::Int) => number(*v),
            (Change::To(Value::Int(v)), Type::Float | Type::Int) => number(*v as f64),
            (Change::To(Value::List(v)), Type::Floats) => {
                if let Some(n) = d.count
                    && v.len() > n as usize
                {
                    return Err(format!("{} values, at most {n}", v.len()));
                }
                let mut clamped = Clamped::None;
                let mut out = Vec::with_capacity(v.len());
                for x in v {
                    if !x.is_finite() {
                        return Err(format!("{x} is not a number"));
                    }
                    let (x, c) = clamp(*x, d.hard, false);
                    if c != Clamped::None {
                        clamped = c;
                    }
                    out.push(x);
                }
                Ok((Value::List(out), clamped))
            }
            (Change::To(Value::Ints(v)), Type::Ints) => {
                if let Some(n) = d.count
                    && v.len() > n as usize
                {
                    return Err(format!("{} values, at most {n}", v.len()));
                }
                let mut clamped = Clamped::None;
                let out = v
                    .iter()
                    .map(|x| {
                        let (x, c) = clamp(*x as f64, d.hard, false);
                        if c != Clamped::None {
                            clamped = c;
                        }
                        x as i64
                    })
                    .collect();
                Ok((Value::Ints(out), clamped))
            }
            (Change::To(Value::Bool(b)), Type::Bool) => Ok((Value::Bool(*b), Clamped::None)),
            (Change::To(Value::Choice(c)), Type::Choice) => {
                if d.choices.contains(c) {
                    Ok((Value::Choice(c.clone()), Clamped::None))
                } else {
                    Err(format!("`{c}` is not one of {}", d.choices.join(", ")))
                }
            }
            (Change::To(Value::Text(t)), Type::Text) => match d.capacity {
                Some(n) if t.len() > n as usize => Err(format!("{} bytes, at most {n}", t.len())),
                _ => Ok((Value::Text(t.clone()), Clamped::None)),
            },
            (Change::To(v), _) => Err(format!("{v:?} is not a {:?}", d.ty)),
            (Change::Step(n) | Change::Coarse(n), Type::Float | Type::Int) => {
                let step = if matches!(set.change, Change::Coarse(_)) {
                    d.coarse
                } else {
                    d.step
                };
                number(current.unwrap_or(0.0) + *n as f64 * step)
            }
            (Change::Step(n) | Change::Coarse(n), Type::Bool) => {
                let b = entry.value.as_bool().unwrap_or(false);
                Ok((Value::Bool(b ^ (n % 2 != 0)), Clamped::None))
            }
            (Change::Step(n) | Change::Coarse(n), Type::Choice) => {
                let at = entry
                    .value
                    .as_choice()
                    .and_then(|c| d.choices.iter().position(|o| o == c))
                    .unwrap_or(0) as i64;
                let last = d.choices.len() as i64 - 1;
                let to = at + n;
                let clamped = if to < 0 || to > last {
                    Clamped::Hard
                } else {
                    Clamped::None
                };
                Ok((
                    Value::Choice(d.choices[to.clamp(0, last.max(0)) as usize].clone()),
                    clamped,
                ))
            }
            (Change::Top | Change::Bottom, Type::Float | Type::Int) => {
                let (lo, hi) = range.ok_or("no range to go to the end of")?;
                number(if set.change == Change::Top { hi } else { lo })
            }
            (Change::Top | Change::Bottom, Type::Bool) => {
                Ok((Value::Bool(set.change == Change::Top), Clamped::None))
            }
            (Change::Top | Change::Bottom, Type::Choice) => {
                let c = if set.change == Change::Top {
                    d.choices.last()
                } else {
                    d.choices.first()
                };
                Ok((Value::Choice(c.cloned().unwrap_or_default()), Clamped::None))
            }
            (_, ty) => Err(format!("a {ty:?} cannot be stepped")),
        }
    }

    /// The origin of the gesture holding a path, if one does: a widget
    /// shows it, an agent knows to wait.
    pub fn held_by(&self, path: &str) -> Option<&Origin> {
        self.gestures
            .values()
            .find(|g| g.holds && g.paths.contains(path))
            .map(|g| &g.origin)
    }

    /// Who holds a path against `origin`, if anyone.
    fn holder(&self, path: &str, origin: &Origin, gesture: Option<GestureId>) -> Option<&Origin> {
        self.gestures
            .iter()
            .find(|(id, g)| {
                g.holds && Some(**id) != gesture && g.origin != *origin && g.paths.contains(path)
            })
            .map(|(_, g)| &g.origin)
    }

    /// Writes a value already checked; announces it.
    fn write(&mut self, path: &str, value: Value, origin: &Origin) -> (Value, u64) {
        self.seq += 1;
        let seq = self.seq;
        let entry = self.entries.get_mut(path).expect("a declared path");
        let old = std::mem::replace(&mut entry.value, value.clone());
        entry.seq = seq;
        entry.origin = Some(origin.clone());
        if path.starts_with("node/") {
            self.dirty.insert(path.into());
        }
        if let Some(bus) = &self.bus {
            let event = ParamChanged {
                path: path.into(),
                old: old.clone(),
                new: value,
                origin: origin.clone(),
                seq,
            };
            bus.publish("values", None, &event);
        }
        (old, seq)
    }

    /// Applies a write.
    pub fn set(&mut self, set: Set) -> Result<Applied, Refused> {
        let entry = self
            .entries
            .get(&set.path)
            .ok_or_else(|| Refused::Unknown {
                path: set.path.clone(),
            })?;
        let current = entry.value.clone();
        let transient = entry.descriptor.transient;
        if let Some(seq) = set.if_seq
            && entry.seq != seq
        {
            return Err(Refused::Stale {
                current,
                seq: entry.seq,
            });
        }
        if let Some(by) = self.holder(&set.path, &set.origin, set.gesture) {
            return Err(Refused::Held {
                current,
                by: by.clone(),
            });
        }
        if let Some(g) = set.gesture
            && !self.gestures.contains_key(&g)
        {
            return Err(Refused::Invalid {
                reason: format!("no gesture {}", g.0),
                current,
            });
        }
        let (value, clamped) = Self::target(entry, &set).map_err(|reason| Refused::Invalid {
            reason,
            current: current.clone(),
        })?;
        if value == current {
            return Ok(Applied {
                value,
                clamped,
                seq: entry.seq,
                changed: false,
            });
        }
        let (old, seq) = self.write(&set.path, value.clone(), &set.origin);
        let change = (set.path.clone(), old, value.clone());
        match set.gesture.and_then(|g| self.gestures.get_mut(&g)) {
            Some(g) => {
                g.paths.insert(set.path.clone());
                if !transient {
                    g.changes.push(change);
                }
            }
            None if transient => {}
            None => self.push_step(Step {
                label: format!("set {}", set.path),
                origin: set.origin,
                changes: vec![change],
            }),
        }
        Ok(Applied {
            value,
            clamped,
            seq,
            changed: true,
        })
    }

    fn push_step(&mut self, step: Step) {
        self.commit(step.label.clone());
        self.undo.push(step);
        self.redo.clear();
    }

    fn commit(&mut self, label: String) {
        self.committed += 1;
        self.last_committed = label;
    }

    /// How many steps were committed: a write outside a gesture, the end
    /// of a gesture or a group that changed something, an undo, a redo.
    /// A host that keeps the edit saves it when this moves.
    pub fn committed(&self) -> u64 {
        self.committed
    }

    /// What the last committed step was: its label, or `undo <label>`,
    /// `redo <label>`.
    pub fn last_committed(&self) -> &str {
        &self.last_committed
    }

    /// Whether an open gesture or group has changed values that are not
    /// committed yet (a drag in progress).
    pub fn uncommitted(&self) -> bool {
        self.gestures.values().any(|g| !g.changes.is_empty())
    }

    /// The open gesture with changes, by label, if there is one.
    pub fn open_gesture(&self) -> Option<&str> {
        self.gestures
            .values()
            .find(|g| !g.changes.is_empty())
            .map(|g| g.label.as_str())
    }

    fn open(&mut self, origin: Origin, label: &str, holds: bool) -> GestureId {
        self.next_gesture += 1;
        let id = GestureId(self.next_gesture);
        self.gestures.insert(
            id,
            Gesture {
                origin,
                label: label.into(),
                holds,
                paths: BTreeSet::new(),
                changes: Vec::new(),
            },
        );
        id
    }

    /// Starts a gesture (a drag): its writes are one undo step, and the
    /// paths it touches are held against other origins until it ends.
    pub fn begin_gesture(&mut self, origin: Origin, label: &str) -> GestureId {
        self.open(origin, label, true)
    }

    /// Starts a group: its writes are one undo step, nothing is held.
    pub fn begin_group(&mut self, origin: Origin, label: &str) -> GestureId {
        self.open(origin, label, false)
    }

    /// Ends a gesture or a group; what it changed becomes one undo step.
    pub fn end(&mut self, id: GestureId) {
        if let Some(g) = self.gestures.remove(&id)
            && !g.changes.is_empty()
        {
            self.push_step(Step {
                label: g.label,
                origin: g.origin,
                changes: g.changes,
            });
        }
    }

    /// Cancels a gesture or a group (Escape during a drag): every value it
    /// changed is put back as it was before it began, announced like any
    /// write, and nothing becomes an undo step. Returns how many values
    /// were put back.
    pub fn cancel(&mut self, id: GestureId) -> usize {
        let Some(g) = self.gestures.remove(&id) else {
            return 0;
        };
        // The first old value of each path is what it was before.
        let mut before: Vec<(String, Value)> = Vec::new();
        for (path, old, _) in g.changes {
            if !before.iter().any(|(p, _)| *p == path) {
                before.push((path, old));
            }
        }
        let mut n = 0;
        for (path, value) in before {
            if self.entries.get(&path).is_some_and(|e| e.value != value) {
                self.write(&path, value, &g.origin);
                n += 1;
            }
        }
        n
    }

    /// The undo steps, oldest first.
    pub fn undo_steps(&self) -> &[Step] {
        &self.undo
    }

    /// The redo steps, oldest first: the next redo is the last.
    pub fn redo_steps(&self) -> &[Step] {
        &self.redo
    }

    /// Puts back the values a step changed, as a write by `origin`.
    fn replay(&mut self, step: &Step, origin: &Origin, back: bool) -> Result<(), Refused> {
        let changes: Vec<(String, Value)> = if back {
            step.changes
                .iter()
                .rev()
                .map(|(p, old, _)| (p.clone(), old.clone()))
                .collect()
        } else {
            step.changes
                .iter()
                .map(|(p, _, new)| (p.clone(), new.clone()))
                .collect()
        };
        for (path, _) in &changes {
            let current = self
                .entries
                .get(path)
                .map(|e| e.value.clone())
                .ok_or_else(|| Refused::Unknown { path: path.clone() })?;
            if let Some(by) = self.holder(path, origin, None) {
                return Err(Refused::Held {
                    current,
                    by: by.clone(),
                });
            }
        }
        for (path, value) in changes {
            if self.entries[&path].value != value {
                self.write(&path, value, origin);
            }
        }
        Ok(())
    }

    /// Undoes the last step; its label, or `None` when there is none.
    pub fn undo(&mut self, origin: &Origin) -> Result<Option<String>, Refused> {
        let Some(step) = self.undo.pop() else {
            return Ok(None);
        };
        if let Err(e) = self.replay(&step, origin, true) {
            self.undo.push(step);
            return Err(e);
        }
        let label = step.label.clone();
        self.commit(format!("undo {label}"));
        self.redo.push(step);
        Ok(Some(label))
    }

    /// Redoes the last undone step.
    pub fn redo(&mut self, origin: &Origin) -> Result<Option<String>, Refused> {
        let Some(step) = self.redo.pop() else {
            return Ok(None);
        };
        if let Err(e) = self.replay(&step, origin, false) {
            self.redo.push(step);
            return Err(e);
        }
        let label = step.label.clone();
        self.commit(format!("redo {label}"));
        self.undo.push(step);
        Ok(Some(label))
    }

    /// The `node/…` paths changed since the last call, for [`sync`].
    pub fn take_dirty(&mut self) -> Vec<String> {
        std::mem::take(&mut self.dirty).into_iter().collect()
    }
}
