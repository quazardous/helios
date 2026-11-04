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
//! - **Steps** keep the id the store gave them
//!   ([`helios_values::Step::id`]) while they are undone, redone or
//!   replaced; [`Line::rev`] (the store's
//!   [`helios_values::Store::history_rev`]) says when the line moved on.
//! - **A gesture in progress** (a drag) refuses undo, redo and jump until
//!   it ends; a value held by another origin's gesture refuses them too.
//! - **Groups** are the store's groups: what an origin writes in one is a
//!   single step. An agent's request (a tool call, or the calls it names
//!   with one `group`) is a group of the store's too
//!   ([`helios_values::Store::begin_request`]): its step carries the
//!   group's number ([`StepInfo::group`]) and the agent as its origin,
//!   which the History panel names after the label; a step that changed
//!   several values says how many.
//! - **An agent's lane**: an agent's undo, redo or jump (egui-pop's
//!   `history.*` run as an agent) walks only its own steps; a step of the
//!   user's or of another origin refuses it, as the workspace's undo for
//!   agents does. [`HeliosHistory::undo_group`] undoes one of its batches
//!   alone, even under later steps, unless one of them changed the same
//!   values (the store's [`helios_values::Store::undo_group`]).
//! - **Undo this step**: the History panel's context menu (egui-pop's
//!   `edit.undo_step`, `history.undo_step` for an agent) undoes one step
//!   alone by the same rule (the store's
//!   [`helios_values::Store::undo_step`]); it goes on the redo list.
//! - **Replaced steps**: the redo list a new step drops stays in the
//!   store as a line leaving from the step it came after; the History
//!   panel shows it behind its "replaced" toggle and a jump reaches it
//!   (back to where the line left, then along it; the redo list there
//!   becomes a replaced line in turn).
//! - **The place** of each step is the context stack's place it was made
//!   in (the store's [`helios_values::Store::set_place`], which
//!   `crate::contexts::follow_place` keeps on the place shown): an undo,
//!   a redo or a jump that walks a step made out of sight makes egui-pop
//!   say "Undone in <place>", with Go there.
//! - **Snapshots** (`crate::edit_snapshots`): taking one saves nothing,
//!   it records a name and the head step; the next save of the edit pins
//!   it to that version, which the vault keeps. Until then it is shown
//!   "not saved yet", and lost if the app quits first. Restoring one
//!   writes its values back as one new step, which an undo takes back;
//!   egui-pop's `edit.restore` step, vetoed like an undo during a drag.
//! - **The hover preview**: the pointer on a row of the History panel
//!   (a step, a replaced step, a snapshot) shows the image as it was
//!   there, and the panel lists what differs (egui-pop's
//!   [`History::preview`]). The values there are an overlay of the store
//!   the graph renders from ([`helios_values::Store::set_preview`]):
//!   nothing is written, so no step, no save and no vault write, and the
//!   live values come back exactly when the pointer leaves. Asked once
//!   per row entered, it costs one render per row, none while the
//!   pointer rests. A drag going on gets no preview.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use egui_pop::api::contexts::Entry;
use egui_pop::api::history::{
    Answer, At, Features, History, Label, Line, Preview, PreviewChange, Refusal, RefusalKind,
    Replaced, Request, ScopeId, ScopeInfo, Snapshot, StepInfo, Walked, plan_jump,
};
use egui_pop::api::host::Waker;
use egui_pop::i18n::I18n;
use egui_pop::model::commands::Origin as PopOrigin;
use egui_pop::model::ids::FrameId;
use helios_core::{Message, Value};
use helios_values::{GestureId, Refused, Set, Step, Store};

use crate::edit_snapshots::{self, Shared, Taken};
use crate::host::{to_helios, to_pop};

/// The scope of Helios's value store: the open image's edit.
pub const EDIT: ScopeId = ScopeId::from_static("edit");

/// Helios's value store as egui-pop's history owner.
#[derive(Clone)]
pub struct HeliosHistory {
    values: Arc<Mutex<Store>>,
    /// The open image's snapshots, shared with the vault's link.
    snapshots: Shared,
    /// Wakes the window when a preview changes what the image shows.
    waker: Option<Arc<Mutex<Waker>>>,
    /// The catalogues a preview's list is said through.
    i18n: Option<I18n>,
}

impl std::fmt::Debug for HeliosHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeliosHistory").finish_non_exhaustive()
    }
}

impl HeliosHistory {
    /// The history of `values`.
    pub fn new(values: Arc<Mutex<Store>>) -> Self {
        Self {
            values,
            snapshots: Shared::default(),
            waker: None,
            i18n: None,
        }
    }

    /// Wakes the window through `waker` when a preview changes what the
    /// image shows (the host's: the graph follows at the next frame).
    pub fn with_waker(mut self, waker: Arc<Mutex<Waker>>) -> Self {
        self.waker = Some(waker);
        self
    }

    /// Says a preview's list through `i18n` (English without).
    pub fn with_i18n(mut self, i18n: I18n) -> Self {
        self.i18n = Some(i18n);
        self
    }

    /// Keeps its snapshots in `book` (the shell's, which the vault's link
    /// pins and keeps).
    pub fn with_snapshots(mut self, book: Shared) -> Self {
        self.snapshots = book;
        self
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

/// A step of the store as egui-pop sees it.
fn step_info(step: &Step) -> StepInfo {
    let mut info = StepInfo::new(step.id, EDIT, label_of(step), to_pop(&step.origin));
    info.group = step.group;
    info.place = step
        .place
        .iter()
        .filter_map(|p| {
            Some(Entry {
                frame: FrameId::new(p.frame.as_str()).ok()?,
                payload: p.payload.clone(),
            })
        })
        .collect();
    info
}

/// A step's label: an agent's batch that changed several values says
/// how many, in the interface's language.
fn label_of(step: &Step) -> Label {
    let n = step.paths().len();
    if step.group.is_none() || n < 2 {
        return Label::text(&step.label);
    }
    let count = n.to_string();
    let m = Message::new("gui-history-batch", "{ $label } ({ $count } values)")
        .with("label", step.label.clone())
        .with("count", count.clone());
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: [
            ("label".to_string(), step.label.clone()),
            ("count".to_string(), count),
        ]
        .into(),
    }
}

/// The store's steps as a line.
fn line_of(store: &Store) -> Line {
    Line {
        undo: store.undo_steps().iter().map(step_info).collect(),
        redo: store.redo_steps().iter().map(step_info).collect(),
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

/// Refused when an agent would walk a step that is not its own.
fn lane(origin: &PopOrigin, steps: &[StepInfo]) -> Result<(), Refusal> {
    if !matches!(origin, PopOrigin::Agent(_)) {
        return Ok(());
    }
    match steps.iter().find(|s| s.origin != *origin) {
        Some(s) => Err(Refusal::new(
            RefusalKind::Owner,
            format!(
                "“{}” is not this agent's step: the user undoes it, or the agent that made it",
                s.label
            ),
        )),
        None => Ok(()),
    }
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
    lane(origin, std::slice::from_ref(&info))?;
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

/// A jump to a replaced step `to`: back to where its line left, then
/// along it, all or nothing; `None` when `to` is no replaced step.
fn jump_replaced(
    store: &mut Store,
    to: u64,
    origin: &PopOrigin,
    dry_run: bool,
) -> Option<Result<Vec<StepInfo>, Refusal>> {
    let o = to_helios(origin);
    let walked = match store.jump_replaced(to, &o, true) {
        Ok(Some(w)) => w,
        Ok(None) => {
            return store
                .replaced_steps()
                .iter()
                .any(|(_, s)| s.id == to)
                .then(|| {
                    Err(Refusal::new(
                        RefusalKind::Owner,
                        "its line left from a step that is replaced too: go to that line first",
                    ))
                });
        }
        Err(e) => return Some(Err(refused(&e))),
    };
    let steps: Vec<StepInfo> = walked.iter().map(step_info).collect();
    Some(lane(origin, &steps).and_then(|()| {
        if !dry_run {
            store
                .jump_replaced(to, &o, false)
                .map_err(|e| refused(&e))?;
        }
        Ok(steps)
    }))
}

/// A snapshot as egui-pop sees it, under the name the panel shows.
fn snapshot_info(t: &Taken, name: String) -> Snapshot {
    Snapshot {
        id: t.id,
        scope: EDIT,
        name,
        step: t.step,
        origin: to_pop(&t.origin),
        at: t.at,
        pending: false,
    }
}

/// Seconds since the epoch: when a snapshot is taken.
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Brings the store back to a snapshot's values as one new step, as
/// `origin`: the step made (none: nothing differed). All or nothing: a
/// value it may not write (outside the sub-workspace entered) refuses
/// it before anything is written.
fn restore(
    store: &mut Store,
    taken: &Taken,
    origin: &PopOrigin,
    dry_run: bool,
) -> Result<Vec<StepInfo>, Refusal> {
    let writes = edit_snapshots::restore_writes(store, &taken.values);
    if writes.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(node) = writes
        .iter()
        .filter_map(|(p, _)| helios_values::node_of(p))
        .find(|n| !store.in_scope(n))
    {
        return Err(Refusal::new(
            RefusalKind::Owner,
            format!(
                "“{}” sets `{node}`, outside the sub-workspace entered: leave it first",
                taken.name
            ),
        ));
    }
    let label = format!("Restore “{}”", taken.name);
    let o = to_helios(origin);
    if dry_run {
        return Ok(vec![StepInfo::new(
            0,
            EDIT,
            Label::text(&label),
            origin.clone(),
        )]);
    }
    let g = store.begin_group(o.clone(), &label);
    for (path, value) in writes {
        if store.get(&path).is_none() {
            // A label or a lock the network's nodes do not have yet.
            let node = path.split_once('/').map_or("", |(_, n)| n);
            let d = if path.starts_with("label/") {
                crate::mask_tree::label_descriptor(node)
            } else {
                crate::mask_tree::lock_descriptor(node)
            };
            store.declare(&path, d);
        }
        if let Err(e) =
            store.set(Set::new(&path, helios_values::Change::To(value), o.clone()).gesture(g))
        {
            store.cancel(g);
            return Err(refused(&e));
        }
    }
    store.end(g);
    Ok(store
        .undo_steps()
        .last()
        .map(step_info)
        .into_iter()
        .collect())
}

/// A message through `i18n`, else its English.
fn say(i18n: Option<&I18n>, m: &Message) -> String {
    i18n.map_or_else(|| m.english.clone(), |i| helios_i18n::message(i, m))
}

/// One value that differs at the point previewed, as the History panel
/// lists it: a parameter by its label (and its node, when that is not
/// named after its module), a bypass by its node.
fn preview_change(store: &Store, i18n: Option<&I18n>, path: &str, then: &Value) -> PreviewChange {
    let entry = store.get(path);
    let now = entry.map_or(Value::Bool(false), |e| e.value.clone());
    if let Some(node) = path.strip_prefix(helios_values::BYPASS) {
        let state = |v: &Value| {
            say(
                i18n,
                &if v.as_bool() == Some(true) {
                    Message::new("gui-history-preview-bypassed", "bypassed")
                } else {
                    Message::new("gui-history-preview-applied", "applied")
                },
            )
        };
        return PreviewChange {
            what: node.to_string(),
            then: state(then),
            now: state(&now),
        };
    }
    let Some(d) = entry.map(|e| &e.descriptor) else {
        return PreviewChange {
            what: path.to_string(),
            then: format!("{then:?}"),
            now: format!("{now:?}"),
        };
    };
    let locale = i18n.map_or_else(|| "en".to_string(), I18n::locale);
    let shown = |v: &Value| crate::params::shown_in(&locale, d, v);
    let what = match (
        path.strip_prefix("node/").and_then(|r| r.rsplit_once('/')),
        &d.module,
    ) {
        (Some((node, param)), Some(module)) => {
            let key = helios_i18n::ids::param_label(module, param);
            let label = i18n
                .and_then(|i| i.get(&key.key(), &[]))
                .unwrap_or_else(|| d.label.clone());
            // The node named only when it is not the module's only one.
            if node == module {
                return PreviewChange {
                    what: label,
                    then: shown(then),
                    now: shown(&now),
                };
            }
            say(
                i18n,
                &Message::new("gui-history-preview-param", "{ $label } ({ $node })")
                    .with("label", label)
                    .with("node", node.to_string()),
            )
        }
        _ => d.label.clone(),
    };
    PreviewChange {
        what,
        then: shown(then),
        now: shown(&now),
    }
}

impl HeliosHistory {
    /// Undoes the steps of `group` (an agent's batch) alone, newest
    /// first, even under later steps (the answer to "undo only the
    /// agent's last batch"): refused when a later step changed one of its
    /// values, and, asked by an agent, unless the batch is its own. The
    /// steps undone go on the redo list. `history.undo` with a `group`.
    pub fn undo_group(
        &self,
        request: &Request,
        group: u64,
        origin: &PopOrigin,
    ) -> Result<Walked, Refusal> {
        let mut store = self.lock();
        Self::check(&store, request)?;
        let line = line_of(&store);
        let steps: Vec<StepInfo> = line
            .undo
            .iter()
            .rev()
            .filter(|s| s.group == Some(group))
            .cloned()
            .collect();
        if steps.is_empty() {
            return Err(Refusal::new(
                RefusalKind::Nothing,
                format!("no step of group {group} left to undo"),
            ));
        }
        lane(origin, &steps)?;
        let o = to_helios(origin);
        store.check_undo_group(group, &o).map_err(|e| refused(&e))?;
        if !request.dry_run {
            store.undo_group(group, &o).map_err(|e| refused(&e))?;
        }
        let line = line_of(&store);
        Ok(Walked {
            scope: EDIT,
            steps,
            rev: line.rev,
            head: line.head_id(),
            dry_run: request.dry_run,
        })
    }
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
            let jump = match plan_jump(&line, to) {
                Ok(j) => j,
                Err(e) => return jump_replaced(s, to, origin, request.dry_run).unwrap_or(Err(e)),
            };
            let list = if jump.redo { &line.redo } else { &line.undo };
            let walked: Vec<StepInfo> = list.iter().rev().take(jump.count).cloned().collect();
            lane(origin, &walked)?;
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

    fn replaced(&self, scope: &ScopeId) -> Vec<Replaced> {
        if *scope != EDIT {
            return Vec::new();
        }
        self.lock()
            .replaced_steps()
            .into_iter()
            .map(|(after, s)| Replaced {
                step: step_info(s),
                after,
            })
            .collect()
    }

    fn features(&self, scope: &ScopeId) -> Features {
        let mut f = Features::default();
        if *scope == EDIT {
            f.undo_step = true;
            f.snapshots = true;
        }
        f
    }

    fn snapshots(&self, scope: &ScopeId) -> Vec<Snapshot> {
        if *scope != EDIT {
            return Vec::new();
        }
        let book = edit_snapshots::lock(&self.snapshots);
        book.list()
            .iter()
            .map(|t| snapshot_info(t, book.shown_name(t)))
            .collect()
    }

    fn snapshot(
        &mut self,
        request: &Request,
        name: &str,
        origin: &PopOrigin,
    ) -> Result<Snapshot, Refusal> {
        let store = self.lock();
        Self::check(&store, request)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(Refusal::new(RefusalKind::Owner, "a snapshot needs a name"));
        }
        let mut book = edit_snapshots::lock(&self.snapshots);
        let taken = book.take(&store, name, to_helios(origin), now(), request.dry_run);
        Ok(snapshot_info(&taken, book.shown_name(&taken)))
    }

    fn restore(&mut self, request: &Request, snapshot: u64, origin: &PopOrigin) -> Answer {
        let Some(taken) = edit_snapshots::lock(&self.snapshots).get(snapshot).cloned() else {
            return Answer::Done(Err(Refusal::new(
                RefusalKind::Nothing,
                format!("no snapshot {snapshot} in `{EDIT}`"),
            )));
        };
        let answer = self.answer(request, |s| restore(s, &taken, origin, request.dry_run));
        if let Answer::Done(Ok(w)) = &answer
            && !w.dry_run
        {
            edit_snapshots::lock(&self.snapshots).restored(&taken.name);
        }
        answer
    }

    fn forget(&mut self, request: &Request, snapshot: u64) -> Result<(), Refusal> {
        if request.scope != EDIT {
            return Err(Refusal::new(
                RefusalKind::Unknown,
                format!("no history `{}`", request.scope),
            ));
        }
        request.check(self.lock().history_rev())?;
        let mut book = edit_snapshots::lock(&self.snapshots);
        if book.get(snapshot).is_none() {
            return Err(Refusal::new(
                RefusalKind::Nothing,
                format!("no snapshot {snapshot} in `{EDIT}`"),
            ));
        }
        if !request.dry_run {
            book.forget(snapshot);
        }
        Ok(())
    }

    fn undo_step(&mut self, request: &Request, step: u64, origin: &PopOrigin) -> Answer {
        self.answer(request, |s| {
            let line = line_of(s);
            let Some(info) = line.undo.iter().find(|x| x.id == step).cloned() else {
                let why = if line.redo.iter().any(|x| x.id == step) {
                    format!("step {step} is undone already")
                } else {
                    format!("no step {step} in `{EDIT}`")
                };
                return Err(Refusal::new(RefusalKind::Nothing, why));
            };
            lane(origin, std::slice::from_ref(&info))?;
            let o = to_helios(origin);
            s.check_undo_step(step, &o).map_err(|e| refused(&e))?;
            if !request.dry_run {
                s.undo_step(step, &o).map_err(|e| refused(&e))?;
            }
            Ok(vec![info])
        })
    }

    fn preview(&mut self, scope: &ScopeId, at: Option<At>) -> Option<Preview> {
        if *scope != EDIT {
            return None;
        }
        let mut store = self.lock();
        // The values there, where they differ from now; `None`: nothing
        // to show (the pointer left, a drag goes on, a point no jump
        // reaches).
        let values = match at {
            None => None,
            // A drag shows its own values: they stay what the image shows.
            Some(_) if store.open_gesture().is_some() => None,
            Some(At::Step(id)) => store.values_at(id),
            Some(At::Snapshot(id)) => edit_snapshots::lock(&self.snapshots)
                .get(id)
                .map(|t| edit_snapshots::restore_writes(&store, &t.values)),
        };
        if store.set_preview(values.clone().unwrap_or_default())
            && let Some(w) = &self.waker
        {
            w.lock().unwrap_or_else(PoisonError::into_inner).wake();
        }
        let values = values?;
        let i18n = self.i18n.as_ref();
        Some(Preview {
            shown: !store.previewed().is_empty(),
            // Labels and locks the store does not hold are not shown.
            changes: values
                .iter()
                .filter(|(p, _)| store.previewed().contains_key(p))
                .map(|(p, v)| preview_change(&store, i18n, p, v))
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use helios_core::Value;
    use helios_values::{Change, Descriptor, Set};

    use super::*;

    /// A parameter, as an edit saves it (`node/<node>/<param>`): only
    /// those are in a snapshot.
    const EV: &str = "node/exposure/exposure";

    fn store() -> Arc<Mutex<Store>> {
        let mut s = Store::new();
        s.declare(EV, Descriptor::float("Exposure", -3.0, 3.0, 0.0));
        Arc::new(Mutex::new(s))
    }

    fn set(values: &Arc<Mutex<Store>>, v: f64) {
        values
            .lock()
            .unwrap()
            .set(Set::new(
                EV,
                Change::To(Value::Float(v)),
                helios_shell::commands::Origin::User,
            ))
            .unwrap();
    }

    fn value(values: &Arc<Mutex<Store>>) -> Value {
        values.lock().unwrap().get(EV).unwrap().value.clone()
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
    fn a_step_says_the_place_it_was_made_in() {
        let values = store();
        values.lock().unwrap().set_place(vec![helios_values::Place {
            frame: "helios.darkroom".into(),
            payload: serde_json::json!({ "image": "i1" }),
        }]);
        set(&values, 1.0);
        values.lock().unwrap().set_place(Vec::new());
        set(&values, 2.0);
        let mut h = HeliosHistory::new(values.clone());
        let line = h.line(&EDIT).unwrap();
        assert_eq!(line.undo[0].place.len(), 1);
        assert_eq!(line.undo[0].place[0].frame.as_str(), "helios.darkroom");
        assert_eq!(line.undo[0].place[0].payload["image"], "i1");
        assert!(line.undo[1].place.is_empty());
        // The walked step carries its place to egui-pop's toast.
        let Answer::Done(Ok(w)) = h.jump(&Request::new(EDIT), 0, &PopOrigin::User) else {
            panic!("jumped");
        };
        assert_eq!(w.steps.last().unwrap().place.len(), 1);
    }

    #[test]
    fn an_agent_s_request_is_one_step_with_its_group_and_origin() {
        let values = store();
        values
            .lock()
            .unwrap()
            .declare("node/black", Descriptor::float("Black", -1.0, 1.0, 0.0));
        let agent = helios_shell::commands::Origin::Agent("retouch".into());
        let g = {
            let mut s = values.lock().unwrap();
            let g = s.begin_request(agent.clone(), Some("Retouch skin"));
            for (path, v) in [(EV, 0.5), ("node/black", -0.1)] {
                s.set(Set::new(path, Change::To(Value::Float(v)), agent.clone()))
                    .unwrap();
            }
            assert_eq!(s.end_request(), Some(g));
            g
        };
        set(&values, 1.0);
        let h = HeliosHistory::new(values.clone());
        let line = h.line(&EDIT).unwrap();
        assert_eq!(line.undo.len(), 2);
        let batch = &line.undo[0];
        assert_eq!(batch.group, Some(g));
        assert_eq!(batch.origin, PopOrigin::Agent("retouch".into()));
        assert_eq!(batch.label.key.as_deref(), Some("gui-history-batch"));
        assert_eq!(batch.label.text, "Retouch skin (2 values)");
        // The user's write is no group, its label as the store has it.
        assert_eq!(line.undo[1].group, None);
        assert_eq!(line.undo[1].label.key, None);
    }

    #[test]
    fn an_agent_walks_its_own_steps_and_undoes_its_batch_alone() {
        let values = store();
        values
            .lock()
            .unwrap()
            .declare("node/black", Descriptor::float("Black", -1.0, 1.0, 0.0));
        let agent = helios_shell::commands::Origin::Agent("retouch".into());
        let pop_agent = PopOrigin::Agent("retouch".into());
        let g = {
            let mut s = values.lock().unwrap();
            let g = s.begin_request(agent.clone(), None);
            s.set(Set::new(
                "node/black",
                Change::To(Value::Float(-0.1)),
                agent.clone(),
            ))
            .unwrap();
            s.end_request();
            g
        };
        set(&values, 1.0);
        let mut h = HeliosHistory::new(values.clone());
        // The head is the user's: the agent's undo is refused, the user's
        // goes.
        let Answer::Done(Err(r)) = h.undo(&Request::new(EDIT), &pop_agent) else {
            panic!("refused");
        };
        assert_eq!(r.kind, RefusalKind::Owner);
        let Answer::Done(Err(_)) = h.jump(&Request::new(EDIT), 0, &pop_agent) else {
            panic!("refused");
        };
        // Its batch alone: a dry run says what, then it goes, under the
        // user's step, which stays.
        let rev = h.rev(&EDIT);
        let dry = h
            .undo_group(&Request::new(EDIT).dry_run(), g, &pop_agent)
            .unwrap();
        assert_eq!(dry.steps.len(), 1);
        assert_eq!(h.rev(&EDIT), rev);
        let done = h
            .undo_group(&Request::new(EDIT).if_rev(rev), g, &pop_agent)
            .unwrap();
        assert_eq!(done.steps[0].group, Some(g));
        assert_eq!(value(&values), Value::Float(1.0));
        assert_eq!(
            values.lock().unwrap().get("node/black").unwrap().value,
            Value::Float(0.0)
        );
        // Redone: its own step again, which the agent may undo when it is
        // the head.
        let Answer::Done(Ok(_)) = h.redo(&Request::new(EDIT), &PopOrigin::User) else {
            panic!("redone");
        };
        let Answer::Done(Ok(w)) = h.undo(&Request::new(EDIT), &pop_agent) else {
            panic!("its own head undone");
        };
        assert_eq!(w.steps[0].group, Some(g));
        // Another agent's batch is not this one's.
        let other = PopOrigin::Agent("other".into());
        let Answer::Done(Ok(_)) = h.redo(&Request::new(EDIT), &PopOrigin::User) else {
            panic!("redone");
        };
        assert_eq!(
            h.undo_group(&Request::new(EDIT), g, &other)
                .unwrap_err()
                .kind,
            RefusalKind::Owner
        );
    }

    #[test]
    fn one_step_is_undone_alone_unless_a_later_step_changed_its_values() {
        let values = store();
        values
            .lock()
            .unwrap()
            .declare("node/black", Descriptor::float("Black", -1.0, 1.0, 0.0));
        set(&values, 1.0);
        values
            .lock()
            .unwrap()
            .set(Set::new(
                "node/black",
                Change::To(Value::Float(-0.1)),
                helios_shell::commands::Origin::User,
            ))
            .unwrap();
        let mut h = HeliosHistory::new(values.clone());
        assert!(h.features(&EDIT).undo_step);
        // The exposure, under the black: a dry run changes nothing, then
        // it goes alone and waits on the redo list.
        let rev = h.rev(&EDIT);
        let Answer::Done(Ok(w)) = h.undo_step(&Request::new(EDIT).dry_run(), 1, &PopOrigin::User)
        else {
            panic!("allowed");
        };
        assert_eq!((w.steps[0].id, h.rev(&EDIT)), (1, rev));
        let Answer::Done(Ok(_)) = h.undo_step(&Request::new(EDIT), 1, &PopOrigin::User) else {
            panic!("undone");
        };
        assert_eq!(value(&values), Value::Float(0.0));
        let line = h.line(&EDIT).unwrap();
        assert_eq!(line.undo.iter().map(|s| s.id).collect::<Vec<_>>(), [2]);
        assert_eq!(line.next().unwrap().id, 1);
        let Answer::Done(Err(r)) = h.undo_step(&Request::new(EDIT), 1, &PopOrigin::User) else {
            panic!("refused");
        };
        assert_eq!(r.kind, RefusalKind::Nothing);
        // A later step on the same value refuses it.
        let Answer::Done(Ok(_)) = h.redo(&Request::new(EDIT), &PopOrigin::User) else {
            panic!("redone");
        };
        set(&values, 2.0);
        let Answer::Done(Err(r)) = h.undo_step(&Request::new(EDIT), 1, &PopOrigin::User) else {
            panic!("refused");
        };
        assert_eq!(r.kind, RefusalKind::Owner);
        assert_eq!(value(&values), Value::Float(2.0));
        // An agent may not undo the user's step.
        let agent = PopOrigin::Agent("retouch".into());
        let Answer::Done(Err(r)) = h.undo_step(&Request::new(EDIT), 2, &agent) else {
            panic!("refused");
        };
        assert_eq!(r.kind, RefusalKind::Owner);
    }

    #[test]
    fn a_replaced_step_is_listed_and_a_jump_reaches_it() {
        let values = store();
        for v in [1.0, 2.0] {
            set(&values, v);
        }
        let mut h = HeliosHistory::new(values.clone());
        let Answer::Done(Ok(_)) = h.undo(&Request::new(EDIT), &PopOrigin::User) else {
            panic!("undone");
        };
        // A new step drops the redo list: step 2 is replaced, after 1.
        set(&values, 3.0);
        let replaced = h.replaced(&EDIT);
        assert_eq!(replaced.len(), 1);
        assert_eq!((replaced[0].step.id, replaced[0].after), (2, 1));
        assert!(h.line(&EDIT).unwrap().redo.is_empty());
        // A dry run says the walk: step 3 undone, step 2 redone.
        let rev = h.rev(&EDIT);
        let Answer::Done(Ok(w)) = h.jump(&Request::new(EDIT).dry_run(), 2, &PopOrigin::User) else {
            panic!("allowed");
        };
        assert_eq!(w.steps.iter().map(|s| s.id).collect::<Vec<_>>(), [3, 2]);
        assert_eq!((h.rev(&EDIT), value(&values)), (rev, Value::Float(3.0)));
        // The jump: step 2 is the head again, and step 3 replaced in turn.
        let Answer::Done(Ok(w)) = h.jump(&Request::new(EDIT), 2, &PopOrigin::User) else {
            panic!("jumped");
        };
        assert_eq!(w.head, 2);
        assert_eq!(value(&values), Value::Float(2.0));
        let line = h.line(&EDIT).unwrap();
        assert_eq!(line.undo.iter().map(|s| s.id).collect::<Vec<_>>(), [1, 2]);
        let replaced = h.replaced(&EDIT);
        assert_eq!(replaced.len(), 1);
        assert_eq!((replaced[0].step.id, replaced[0].after), (3, 1));
        // An agent may not walk the user's steps there.
        let agent = PopOrigin::Agent("retouch".into());
        let Answer::Done(Err(r)) = h.jump(&Request::new(EDIT), 3, &agent) else {
            panic!("refused");
        };
        assert_eq!(r.kind, RefusalKind::Owner);
        // A step that never was refuses as before.
        let Answer::Done(Err(r)) = h.jump(&Request::new(EDIT), 9, &PopOrigin::User) else {
            panic!("refused");
        };
        assert_eq!(r.kind, RefusalKind::Nothing);
    }

    #[test]
    fn a_snapshot_is_pending_and_its_restore_is_one_step() {
        let values = store();
        set(&values, 1.0);
        let mut h = HeliosHistory::new(values.clone());
        assert!(h.features(&EDIT).snapshots);
        let empty = h.snapshot(&Request::new(EDIT), "  ", &PopOrigin::User);
        assert_eq!(empty.unwrap_err().kind, RefusalKind::Owner);
        let snap = h
            .snapshot(&Request::new(EDIT), "Bright", &PopOrigin::User)
            .unwrap();
        assert_eq!(
            (snap.step, snap.name.as_str()),
            (1, "Bright (not saved yet)")
        );
        set(&values, 2.0);
        let Answer::Done(Ok(w)) = h.restore(&Request::new(EDIT), snap.id, &PopOrigin::User) else {
            panic!("restored");
        };
        assert_eq!(w.steps[0].label.text, "Restore “Bright”");
        assert_eq!(value(&values), Value::Float(1.0));
        assert_eq!(h.line(&EDIT).unwrap().undo.len(), 3);
        // Already there: nothing to do, no step.
        let Answer::Done(Ok(w)) = h.restore(&Request::new(EDIT), snap.id, &PopOrigin::User) else {
            panic!("nothing");
        };
        assert!(w.steps.is_empty());
        let Answer::Done(Ok(_)) = h.undo(&Request::new(EDIT), &PopOrigin::User) else {
            panic!("undone");
        };
        assert_eq!(value(&values), Value::Float(2.0));
        h.forget(&Request::new(EDIT), snap.id).unwrap();
        assert!(h.snapshots(&EDIT).is_empty());
        let Answer::Done(Err(r)) = h.restore(&Request::new(EDIT), snap.id, &PopOrigin::User) else {
            panic!("refused");
        };
        assert_eq!(r.kind, RefusalKind::Nothing);
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
                    EV,
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
