//! The value flow (section 3.5): every change, from any surface or
//! device, is the same sequence on a [`ValueStore`].
//!
//! 1. **Begin** ([`begin`]): a gesture on the store, labelled by the
//!    control, holding the path against other origins.
//! 2. **Preview** ([`Gesture::preview`]): each move writes inside the
//!    gesture; the engine and every surface redraw from the store.
//! 3. **Commit** ([`Gesture::commit`]): the `controls.change` step runs
//!    ([`Steps`]); a hook may veto (the values go back, no trace) or
//!    redirect (snap, clamp); then the gesture ends as one undo step.
//! 4. **Cancel** ([`Gesture::cancel`]): the values from before the
//!    gesture are put back, and nothing is left in the history.
//!
//! One-shot inputs (an agent's set, a nudge, a reset) are a gesture begun
//! and committed at once ([`apply`]).

use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::model::Control;
use crate::value::{Value, ValueStore};

/// What moved a control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cause {
    Drag,
    Scrub,
    Key,
    Wheel,
    Typed,
    Reset,
    /// An absolute value asked (an agent, a fader).
    Set,
    /// Steps asked (an agent, an encoder).
    Nudge,
    Access,
}

/// The `controls.change` step's arguments: one finished change.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChangeEvent {
    pub control: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    pub path: String,
    pub from: f64,
    pub to: f64,
    pub cause: Cause,
    /// `pointer`, `keys`, `agent`, `midi`…
    pub device: String,
}

/// What a `controls.change` hook decides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Go,
    /// Nothing written: the values go back as they were before the gesture.
    Veto,
    /// This value instead.
    Redirect(f64),
}

type Hook = Box<dyn Fn(&ChangeEvent) -> Verdict + Send + Sync>;

/// The `controls.change` step: hooks that see each finished change before
/// it becomes an undo step, and may veto or redirect it.
#[derive(Default)]
pub struct Steps {
    hooks: Vec<(String, Hook)>,
}

impl std::fmt::Debug for Steps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.hooks.iter().map(|(n, _)| n))
            .finish()
    }
}

/// The step's name.
pub const CHANGE_STEP: &str = "controls.change";

impl Steps {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a hook, by name (who added it).
    pub fn before(
        &mut self,
        name: &str,
        hook: impl Fn(&ChangeEvent) -> Verdict + Send + Sync + 'static,
    ) {
        self.hooks.push((name.into(), Box::new(hook)));
    }

    /// Runs the hooks in order: a veto stops; a redirect is what the next
    /// hooks see.
    pub fn run(&self, event: &ChangeEvent) -> Verdict {
        let mut e = event.clone();
        let mut redirected = None;
        for (_, h) in &self.hooks {
            match h(&e) {
                Verdict::Go => {}
                Verdict::Veto => return Verdict::Veto,
                Verdict::Redirect(v) => {
                    e.to = v;
                    redirected = Some(v);
                }
            }
        }
        redirected.map_or(Verdict::Go, Verdict::Redirect)
    }
}

/// Shared steps, as the commands and the panel hold them.
pub type SharedSteps = Arc<Mutex<Steps>>;

/// How a gesture ended.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum Outcome {
    /// One undo step, from `from` to `to`.
    Committed { from: f64, to: f64 },
    /// Vetoed by a hook: the values went back.
    Vetoed,
    /// Nothing changed: no step.
    Unchanged,
}

/// An open gesture of one control on one path, on a store whose gesture
/// handle is `G` and origin `O`.
#[derive(Debug)]
pub struct Gesture<G, O> {
    id: G,
    control: String,
    path: String,
    from: f64,
    origin: O,
}

/// The node of a path (`node/<node>/<param>`).
pub fn node_of(path: &str) -> Option<String> {
    Some(path.strip_prefix("node/")?.rsplit_once('/')?.0.to_owned())
}

/// The gesture type of a store.
pub type GestureOf<S> = Gesture<<S as ValueStore>::Gesture, <S as ValueStore>::Origin>;

/// Begins a gesture of `control` on `path`, as `origin`.
pub fn begin<S: ValueStore>(
    store: &mut S,
    control: &Control,
    path: &str,
    origin: S::Origin,
) -> Result<GestureOf<S>, String> {
    if control.kind == crate::model::Kind::Indicator {
        return Err(format!("`{}` is an indicator: it is read-only", control.id));
    }
    let from = store
        .get(path)
        .ok_or_else(|| format!("no value at `{path}`"))?
        .as_f64()
        .ok_or_else(|| format!("`{path}` is not a number"))?;
    let id = store.begin(&origin, &control.label);
    Ok(Gesture {
        id,
        control: control.id.clone(),
        path: path.into(),
        from,
        origin,
    })
}

fn to_value<S: ValueStore>(store: &S, path: &str, v: f64) -> Value {
    match store.get(path) {
        Some(Value::Int(_)) => Value::Int(v.round() as i64),
        _ => Value::Float(v),
    }
}

impl<G: Copy, O: Clone> Gesture<G, O> {
    /// The value before the gesture.
    pub fn from(&self) -> f64 {
        self.from
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Writes a value inside the gesture (forced past the soft range when
    /// asked); returns what the store holds.
    pub fn preview<S: ValueStore<Gesture = G, Origin = O>>(
        &self,
        store: &mut S,
        v: f64,
        force: bool,
    ) -> Result<f64, String> {
        let value = to_value(store, &self.path, v);
        let applied = store.preview(self.id, &self.path, value, &self.origin, force)?;
        Ok(applied.as_f64().unwrap_or(v))
    }

    /// Ends the gesture: runs the `controls.change` step, then makes one
    /// undo step, or puts the values back when vetoed or unchanged.
    pub fn commit<S: ValueStore<Gesture = G, Origin = O>>(
        self,
        store: &mut S,
        steps: &Steps,
        cause: Cause,
        device: &str,
    ) -> Outcome {
        let to = store
            .get(&self.path)
            .and_then(|v| v.as_f64())
            .unwrap_or(self.from);
        if to == self.from {
            store.cancel(self.id);
            return Outcome::Unchanged;
        }
        let event = ChangeEvent {
            control: self.control.clone(),
            node: node_of(&self.path),
            path: self.path.clone(),
            from: self.from,
            to,
            cause,
            device: device.into(),
        };
        let to = match steps.run(&event) {
            Verdict::Veto => {
                store.cancel(self.id);
                return Outcome::Vetoed;
            }
            Verdict::Go => to,
            Verdict::Redirect(v) => match self.preview(store, v, false) {
                Ok(v) => v,
                Err(_) => to,
            },
        };
        if to == self.from {
            store.cancel(self.id);
            return Outcome::Unchanged;
        }
        store.commit(self.id);
        Outcome::Committed {
            from: self.from,
            to,
        }
    }

    /// Gives the gesture up: the values before it come back, no undo step.
    pub fn cancel<S: ValueStore<Gesture = G, Origin = O>>(self, store: &mut S) -> usize {
        store.cancel(self.id)
    }
}

/// A one-shot change: the value asked, who asks and why.
#[derive(Clone, Debug, PartialEq)]
pub struct OneShot<'a, O> {
    pub to: f64,
    /// Past the soft range, up to the hard one.
    pub force: bool,
    pub origin: O,
    pub cause: Cause,
    pub device: &'a str,
}

/// A one-shot change: begin, write its value, commit.
pub fn apply<S: ValueStore>(
    store: &mut S,
    steps: &Steps,
    control: &Control,
    path: &str,
    ask: OneShot<'_, S::Origin>,
) -> Result<Outcome, String> {
    let g = begin(store, control, path, ask.origin)?;
    if let Err(e) = g.preview(store, ask.to, ask.force) {
        g.cancel(store);
        return Err(e);
    }
    Ok(g.commit(store, steps, ask.cause, ask.device))
}
