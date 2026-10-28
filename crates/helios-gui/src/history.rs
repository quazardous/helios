//! Helios's undo behind egui-pop's history contract
//! (`egui_pop::api::history`, egui-pop's ADR 0014).
//!
//! Helios's kernel owns the history: the steps of its value store
//! (`helios_values`), which the vault saves as versions. [`HeliosHistory`]
//! shows them to egui-pop as one scope, [`EDIT`], always active: egui-pop
//! routes Ctrl+Z to it (after a focused text field and a component's own
//! history, the theme editor's or the settings window's), labels the
//! Edit menu from its steps, and runs `edit.undo`, `edit.redo` and
//! `edit.jump` on it.
//!
//! - **Steps** are numbered by their place in the scope's line, from 1
//!   (the oldest undo step) to the newest redo step: a step keeps its id
//!   while it is undone and redone, and [`Line::rev`] (the store's
//!   [`helios_values::Store::history_rev`]) says when the numbering moved
//!   on.
//! - **A gesture in progress** (a drag) refuses undo, redo and jump until
//!   it ends; a value held by another origin's gesture refuses them too.
//! - **Groups** are the store's groups: what an origin writes in one is a
//!   single step.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use egui_pop::api::history::{
    Answer, History, Label, Line, Refusal, RefusalKind, Request, ScopeId, ScopeInfo, StepInfo,
    Walked, plan_jump,
};
use egui_pop::model::commands::Origin as PopOrigin;
use helios_values::{GestureId, Refused, Step, Store};

use crate::host::{to_helios, to_pop};

/// The scope of Helios's value store: the open image's edit.
pub const EDIT: ScopeId = ScopeId::from_static("edit");

/// Helios's value store as egui-pop's history owner.
#[derive(Clone)]
pub struct HeliosHistory {
    values: Arc<Mutex<Store>>,
}

impl std::fmt::Debug for HeliosHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeliosHistory").finish_non_exhaustive()
    }
}

impl HeliosHistory {
    /// The history of `values`.
    pub fn new(values: Arc<Mutex<Store>>) -> Self {
        Self { values }
    }

    fn lock(&self) -> MutexGuard<'_, Store> {
        self.values.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Checks a request against the store: the scope, its revision, a
    /// gesture in progress.
    fn check(store: &Store, request: &Request) -> Result<(), Refusal> {
        if request.scope != EDIT {
            return Err(Refusal::new(
                RefusalKind::Unknown,
                format!("no history `{}`", request.scope),
            ));
        }
        request.check(store.history_rev())?;
        if let Some(label) = store.open_gesture() {
            return Err(Refusal::new(
                RefusalKind::Held,
                format!("“{label}” is in progress"),
            ));
        }
        Ok(())
    }

    /// Answers a request: checked, then `f` walks the store.
    fn answer(
        &self,
        request: &Request,
        f: impl FnOnce(&mut Store) -> Result<Vec<StepInfo>, Refusal>,
    ) -> Answer {
        let mut store = self.lock();
        Answer::Done(Self::check(&store, request).and_then(|()| {
            let steps = f(&mut store)?;
            let line = line_of(&store);
            Ok(Walked {
                scope: EDIT,
                steps,
                rev: line.rev,
                head: line.head_id(),
                dry_run: request.dry_run,
            })
        }))
    }
}

/// A step of the store as egui-pop sees it, numbered `id`.
fn step_info(id: u64, step: &Step) -> StepInfo {
    StepInfo::new(id, EDIT, Label::text(&step.label), to_pop(&step.origin))
}

/// The store's steps as a line: undo steps numbered from 1, oldest
/// first, then the redo steps on from the newest undo step.
fn line_of(store: &Store) -> Line {
    let undo = store.undo_steps();
    let redo = store.redo_steps();
    let n = undo.len() as u64;
    let m = redo.len() as u64;
    Line {
        undo: undo
            .iter()
            .zip(1..)
            .map(|(s, id)| step_info(id, s))
            .collect(),
        // The next redo (the last) is n + 1; the one after it n + 2...
        redo: redo
            .iter()
            .zip((n + 1..=n + m).rev())
            .map(|(s, id)| step_info(id, s))
            .collect(),
        rev: store.history_rev(),
    }
}

fn refused(r: &Refused) -> Refusal {
    let kind = match r {
        Refused::Held { .. } => RefusalKind::Held,
        _ => RefusalKind::Owner,
    };
    Refusal::new(kind, r.to_string())
}

/// A step's values held by a gesture of another origin.
fn held(store: &Store, step: &Step) -> Option<Refusal> {
    step.changes.iter().find_map(|(path, _, _)| {
        store.held_by(path).map(|_| {
            Refusal::new(
                RefusalKind::Held,
                format!("`{path}` is held by a gesture in progress"),
            )
        })
    })
}

/// Undoes (or redoes) the head of the store, as `origin`: the step
/// walked, none when there was nothing.
fn walk(
    store: &mut Store,
    origin: &PopOrigin,
    redo: bool,
    dry_run: bool,
) -> Result<Vec<StepInfo>, Refusal> {
    let line = line_of(store);
    let Some(info) = (if redo { line.next() } else { line.head() }).cloned() else {
        return Ok(Vec::new());
    };
    if dry_run {
        let list = if redo {
            store.redo_steps()
        } else {
            store.undo_steps()
        };
        if let Some(r) = list.last().and_then(|s| held(store, s)) {
            return Err(r);
        }
        return Ok(vec![info]);
    }
    let origin = to_helios(origin);
    if redo {
        store.redo(&origin)
    } else {
        store.undo(&origin)
    }
    .map_err(|e| refused(&e))?;
    Ok(vec![info])
}

impl History for HeliosHistory {
    fn scopes(&self) -> Vec<ScopeInfo> {
        vec![ScopeInfo::new(EDIT, "Edit")]
    }

    fn line(&self, scope: &ScopeId) -> Option<Line> {
        (*scope == EDIT).then(|| line_of(&self.lock()))
    }

    fn rev(&self, scope: &ScopeId) -> u64 {
        if *scope == EDIT {
            self.lock().history_rev()
        } else {
            0
        }
    }

    fn undo(&mut self, request: &Request, origin: &PopOrigin) -> Answer {
        self.answer(request, |s| walk(s, origin, false, request.dry_run))
    }

    fn redo(&mut self, request: &Request, origin: &PopOrigin) -> Answer {
        self.answer(request, |s| walk(s, origin, true, request.dry_run))
    }

    fn jump(&mut self, request: &Request, to: u64, origin: &PopOrigin) -> Answer {
        self.answer(request, |s| {
            let line = line_of(s);
            let jump = plan_jump(&line, to)?;
            let list = if jump.redo { &line.redo } else { &line.undo };
            let walked: Vec<StepInfo> = list.iter().rev().take(jump.count).cloned().collect();
            let steps = if jump.redo {
                s.redo_steps()
            } else {
                s.undo_steps()
            };
            if let Some(r) = steps
                .iter()
                .rev()
                .take(jump.count)
                .find_map(|st| held(s, st))
            {
                return Err(r);
            }
            if request.dry_run {
                return Ok(walked);
            }
            // All or nothing: a step refused puts back the ones walked.
            let o = to_helios(origin);
            for done in 0..jump.count {
                let r = if jump.redo { s.redo(&o) } else { s.undo(&o) };
                if let Err(e) = r {
                    for _ in 0..done {
                        let _ = if jump.redo { s.undo(&o) } else { s.redo(&o) };
                    }
                    return Err(refused(&e));
                }
            }
            Ok(walked)
        })
    }

    fn group(&mut self, origin: &PopOrigin, label: &Label) -> Option<u64> {
        Some(self.lock().begin_group(to_helios(origin), &label.text).0)
    }

    fn close(&mut self, group: u64, keep: bool) {
        let mut s = self.lock();
        if keep {
            s.end(GestureId(group));
        } else {
            s.cancel(GestureId(group));
        }
    }

    fn in_progress(&self, scope: &ScopeId) -> Option<Label> {
        if *scope != EDIT {
            return None;
        }
        self.lock().open_gesture().map(Label::text)
    }
}

#[cfg(test)]
mod tests {
    use helios_core::Value;
    use helios_values::{Change, Descriptor, Set};

    use super::*;

    fn store() -> Arc<Mutex<Store>> {
        let mut s = Store::new();
        s.declare(
            "node/exposure",
            Descriptor::float("Exposure", -3.0, 3.0, 0.0),
        );
        Arc::new(Mutex::new(s))
    }

    fn set(values: &Arc<Mutex<Store>>, v: f64) {
        values
            .lock()
            .unwrap()
            .set(Set::new(
                "node/exposure",
                Change::To(Value::Float(v)),
                helios_shell::commands::Origin::User,
            ))
            .unwrap();
    }

    fn value(values: &Arc<Mutex<Store>>) -> Value {
        values
            .lock()
            .unwrap()
            .get("node/exposure")
            .unwrap()
            .value
            .clone()
    }

    #[test]
    fn steps_keep_their_ids_while_undone_and_redone() {
        let values = store();
        set(&values, 1.0);
        set(&values, 2.0);
        let mut h = HeliosHistory::new(values.clone());
        let line = h.line(&EDIT).unwrap();
        assert_eq!(line.undo.iter().map(|s| s.id).collect::<Vec<_>>(), [1, 2]);
        let Answer::Done(Ok(w)) = h.undo(&Request::new(EDIT), &PopOrigin::User) else {
            panic!("undone");
        };
        assert_eq!(w.steps[0].id, 2);
        assert_eq!(w.head, 1);
        let line = h.line(&EDIT).unwrap();
        assert_eq!(line.next().unwrap().id, 2);
        assert_eq!(value(&values), Value::Float(1.0));
    }

    #[test]
    fn a_jump_walks_to_the_step() {
        let values = store();
        for v in [1.0, 2.0, 3.0] {
            set(&values, v);
        }
        let mut h = HeliosHistory::new(values.clone());
        let Answer::Done(Ok(w)) = h.jump(&Request::new(EDIT), 0, &PopOrigin::User) else {
            panic!("jumped");
        };
        assert_eq!(w.steps.len(), 3);
        assert_eq!(value(&values), Value::Float(0.0));
        let Answer::Done(Ok(_)) = h.jump(&Request::new(EDIT), 2, &PopOrigin::User) else {
            panic!("jumped back");
        };
        assert_eq!(value(&values), Value::Float(2.0));
    }

    #[test]
    fn a_stale_request_or_a_drag_refuses() {
        let values = store();
        set(&values, 1.0);
        let mut h = HeliosHistory::new(values.clone());
        let rev = h.rev(&EDIT);
        set(&values, 2.0);
        let Answer::Done(Err(r)) = h.undo(&Request::new(EDIT).if_rev(rev), &PopOrigin::User) else {
            panic!("refused");
        };
        assert_eq!(r.kind, RefusalKind::Stale);
        let g = values
            .lock()
            .unwrap()
            .begin_gesture(helios_shell::commands::Origin::User, "drag exposure");
        values
            .lock()
            .unwrap()
            .set(
                Set::new(
                    "node/exposure",
                    Change::To(Value::Float(2.5)),
                    helios_shell::commands::Origin::User,
                )
                .gesture(g),
            )
            .unwrap();
        let Answer::Done(Err(r)) = h.undo(&Request::new(EDIT), &PopOrigin::User) else {
            panic!("refused");
        };
        assert_eq!(r.kind, RefusalKind::Held);
        assert_eq!(h.in_progress(&EDIT).unwrap().text, "drag exposure");
    }
}
