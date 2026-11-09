//! The value flow (`egui_hudget::flow`) on `helios_values::Store`: the
//! same begin, preview, commit and cancel, with Helios's origins and the
//! store's gestures (held against other origins, one undo step each).

use helios_shell::commands::Origin;
use helios_values::{GestureId, Store};

pub use egui_hudget::flow::{
    CHANGE_STEP, Cause, ChangeEvent, Outcome, SharedSteps, Steps, Verdict, node_of,
};

use crate::model::Control;
use crate::values::StoreMut;

/// A one-shot change: the value asked, who asks and why.
pub type OneShot<'a> = egui_hudget::flow::OneShot<'a, Origin>;

/// An open gesture of one control on one path.
#[derive(Debug)]
pub struct Gesture(egui_hudget::flow::Gesture<GestureId, Origin>);

/// Begins a gesture of `control` on `path`, as `origin`.
pub fn begin(
    store: &mut Store,
    control: &Control,
    path: &str,
    origin: Origin,
) -> Result<Gesture, String> {
    egui_hudget::flow::begin(&mut StoreMut(store), control, path, origin).map(Gesture)
}

impl Gesture {
    /// The value before the gesture.
    pub fn from(&self) -> f64 {
        self.0.from()
    }

    pub fn path(&self) -> &str {
        self.0.path()
    }

    /// Writes a value inside the gesture (forced past the soft range when
    /// asked); returns what the store holds.
    pub fn preview(&self, store: &mut Store, v: f64, force: bool) -> Result<f64, String> {
        self.0.preview(&mut StoreMut(store), v, force)
    }

    /// Ends the gesture: runs the `controls.change` step, then makes one
    /// undo step, or puts the values back when vetoed or unchanged.
    pub fn commit(self, store: &mut Store, steps: &Steps, cause: Cause, device: &str) -> Outcome {
        self.0.commit(&mut StoreMut(store), steps, cause, device)
    }

    /// Gives the gesture up: the values before it come back, no undo step.
    pub fn cancel(self, store: &mut Store) -> usize {
        self.0.cancel(&mut StoreMut(store))
    }
}

/// A one-shot change: begin, write its value, commit.
pub fn apply(
    store: &mut Store,
    steps: &Steps,
    control: &Control,
    path: &str,
    ask: OneShot<'_>,
) -> Result<Outcome, String> {
    egui_hudget::flow::apply(&mut StoreMut(store), steps, control, path, ask)
}
