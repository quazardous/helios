//! The library's history: the scope [`LIBRARY`] on egui-pop's history
//! contract, beside the image's edit (`crate::history`).
//!
//! - **One step per import**: each import that changed the library,
//!   finished or cancelled, leaves one step, "Import 124 photos", holding
//!   what it changed ([`helios_vault::Changes`]). Undo takes its entries
//!   out of the Library ([`helios_vault::Vault::revert`]), redo puts them
//!   back ([`helios_vault::Vault::replay`]). No file is touched: copies
//!   stay in the library folder, and a later import finds them there by
//!   their hash.
//! - **One step per album change** (`docs/design/library.md`, L6): a
//!   new album, a rename, a deletion, photos added or removed; its
//!   changes are the album's document before and after.
//! - **One step per "Remove from Library"** (L8): the photos' flags
//!   before and after, as the node's `library.images.remove` answers
//!   them.
//! - **One step per rotation** (L9): the photos' `rotation` field
//!   before and after, as the node's `library.images.rotate` answers
//!   them.
//! - **One step per keyword branch renamed or merged** (L12): every
//!   photo's tags before and after, as the node's
//!   `library.keywords.rename` and `.merge` answer them.
//! - **One step per group edit of metadata** (L13): the photos' fields
//!   before and after, as the node's `library.meta.set` answers them.
//! - **One step per manual stack change** (L11): a group, an unstack, a
//!   new top; the stacks' documents before and after, as the node's
//!   `library.stacks.group`, `.ungroup` and `.top` answer them.
//! - **One step per pairing** (RAW+JPEG files paired or unpaired): the
//!   photos' rows and the albums and stacks they were in, before and
//!   after, as the node's `library.pairs.pair` and `.unpair` answer
//!   them.
//! - **For the session only** (docs/design/undo.md, decided 2026-10-14):
//!   the steps live in memory; a restart starts with none.
//! - **Active with the library's panels** (the Library, the Filmstrip,
//!   the Duplicates panel): Ctrl+Z there undoes the newest step among
//!   the library's and the edit's, by time.
//! - **An agent's lane**, as for the edit: an agent walks only its own
//!   steps; an import's step is the user's.
//! - **A batch is one step** ([`Group`]): the library changes of the
//!   window commands a `command.batch` ran are one step, "Agent: 3
//!   commands", by the batch's origin (an agent's, the CLI's), each set
//!   of changes undone in reverse order and redone in order, all or
//!   nothing. A batch that changed nothing undoable leaves no step; the
//!   user's own actions between two batches stay steps of their own.
//! - **The user's walks run off the frame** ([`LibraryHistory::undo_later`]):
//!   the line moves at once, the steps undone marked in progress, and the
//!   node is asked on a thread of the history's; a second Ctrl+Z while one
//!   is in flight queues behind it, the walks applied in the order asked.
//!   A walk the node refuses is put back, with those queued behind it,
//!   and its reason waits for the shell to show it
//!   ([`LibraryHistory::take_refusals`]). Others' walks (an agent's, a
//!   script's) wait for those in flight, then for their own answer.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use egui_pop::api::history::{
    Answer, Label, Line, Refusal, RefusalKind, Request, ScopeId, ScopeInfo, StepInfo, Walked,
    plan_jump,
};
use egui_pop::model::commands::Origin as PopOrigin;
use helios_core::Message;
use helios_vault::{Changes, Replayed};

/// The library's scope.
pub const LIBRARY: ScopeId = ScopeId::from_static("library");

/// The panels that make the library's scope active.
pub const FOCUS: &[&str] = &[
    "helios.vault",
    "helios.filters",
    "helios.filmstrip",
    "helios.duplicates",
];

/// Step ids start here, apart from the value store's (egui-pop wants
/// them unique within the owner).
const FIRST_ID: u64 = 1 << 48;

/// Takes a step's changes back (`false`) or puts them again (`true`):
/// the vault's revert or replay, then the Library filled again.
pub type Apply = Arc<dyn Fn(&Changes, bool) -> Result<Replayed, String> + Send + Sync>;

/// A `command.batch`'s group: its library changes are one step, made
/// by `origin` (`crate::window_commands`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// The batch's, unique in the process.
    pub id: u64,
    pub origin: PopOrigin,
}

thread_local! {
    /// The batch whose command runs now on this thread (the interface's).
    static BATCH: RefCell<Option<Group>> = const { RefCell::new(None) };
}

/// The batch whose command runs now: what it asks of the library is
/// stamped with it ([`within_batch`]).
pub fn current_batch() -> Option<Group> {
    BATCH.with(|b| b.borrow().clone())
}

/// Runs `f` as a command of `batch` (none: the user's or alone), then
/// puts back the batch that ran before.
pub fn within_batch<T>(batch: Option<Group>, f: impl FnOnce() -> T) -> T {
    let before = BATCH.with(|b| b.replace(batch));
    let out = f();
    BATCH.with(|b| *b.borrow_mut() = before);
    out
}

#[derive(Clone)]
struct Entry {
    info: StepInfo,
    /// The sets of changes, in the order made: undone in reverse.
    changes: Vec<Changes>,
    /// The batch it holds the changes of.
    group: Option<u64>,
}

/// A walk sent off the frame: its steps moved in the line already, the
/// node not answered yet.
struct Flight {
    /// A redo (else an undo).
    redo: bool,
    /// The steps walked, in the order walked.
    entries: Vec<Entry>,
    /// Where they were in the list they left, to be put back there.
    at: usize,
    /// The redo list's clears when it left ([`Inner::cleared`]): a redo
    /// refused after a new step cleared the list is not put back.
    cleared: u64,
}

#[derive(Default)]
struct Inner {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    rev: u64,
    next: u64,
    apply: Option<Apply>,
    /// The walks in flight, the first being applied.
    flights: VecDeque<Flight>,
    /// The thread applying them, while there are some.
    worker: Option<std::thread::JoinHandle<()>>,
    /// Whether the worker still takes flights.
    working: bool,
    /// How many times a new step cleared the redo list.
    cleared: u64,
    /// Why walks were refused, for the shell to show.
    refusals: Vec<String>,
    /// Wakes the window when a walk lands.
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<Inner>,
    /// Signalled when the last flight landed.
    idle: Condvar,
}

/// The library's steps, shared by the history owner and whoever records
/// an import (the import's thread).
#[derive(Clone, Default)]
pub struct LibraryHistory {
    inner: Arc<Shared>,
}

impl std::fmt::Debug for LibraryHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryHistory").finish_non_exhaustive()
    }
}

/// An import's label: "Import 124 photos".
pub fn import_label(photos: u64) -> Label {
    let m = Message::new("gui-history-import", "Import { $count } photos").with("count", photos);
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: [("count".to_string(), photos.to_string())].into(),
    }
}

/// A batch's label: "Agent: 3 commands" (by an agent, a script, the
/// CLI), "3 commands" (by the user); `count`: the commands that changed
/// the library.
pub fn batch_label(origin: &PopOrigin, count: usize) -> Label {
    let m = if *origin == PopOrigin::User {
        Message::new("gui-history-user-batch", "{ $count } commands")
    } else {
        Message::new("gui-history-agent-batch", "Agent: { $count } commands")
    }
    .with("count", count);
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: [("count".to_string(), count.to_string())].into(),
    }
}

/// Walks `entries` (in the order walked) through `apply`, all or
/// nothing: on a failure, the steps already walked are walked back.
fn apply_walk(apply: &Apply, entries: &[Entry], forward: bool) -> Result<(), String> {
    for (done, entry) in entries.iter().enumerate() {
        if let Err(e) = apply_entry(apply, entry, forward) {
            for back in entries[..done].iter().rev() {
                let _ = apply_entry(apply, back, !forward);
            }
            return Err(e);
        }
    }
    Ok(())
}

/// Takes `entry`'s sets of changes back (`forward` false: the last
/// first) or puts them again (in order), all or nothing: on a failure,
/// those already walked are walked back.
fn apply_entry(apply: &Apply, entry: &Entry, forward: bool) -> Result<(), String> {
    let sets: Vec<&Changes> = if forward {
        entry.changes.iter().collect()
    } else {
        entry.changes.iter().rev().collect()
    };
    for (done, changes) in sets.iter().enumerate() {
        if let Err(e) = apply(changes, forward) {
            for back in sets[..done].iter().rev() {
                let _ = apply(back, !forward);
            }
            return Err(e);
        }
    }
    Ok(())
}

/// "Remove from Library"'s label: "Remove 3 photos from the Library".
pub fn remove_label(photos: usize) -> Label {
    let m = Message::new(
        "gui-history-library-remove",
        "Remove { $count } photos from the Library",
    )
    .with("count", photos);
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: [("count".to_string(), photos.to_string())].into(),
    }
}

/// A manual stack's step: "Group 3 photos into a stack", "Unstack",
/// "Move to top of stack".
pub fn stack_label(ask: &crate::library::StackAsk) -> Label {
    use crate::library::StackAsk;
    let (m, args) = match ask {
        StackAsk::Group { images, .. } => (
            Message::new(
                "gui-history-stack-group",
                "Group { $count } photos into a stack",
            )
            .with("count", images.len()),
            [("count".to_string(), images.len().to_string())].into(),
        ),
        StackAsk::Ungroup { .. } => (
            Message::new("gui-history-stack-ungroup", "Unstack"),
            std::collections::BTreeMap::default(),
        ),
        StackAsk::Top { .. } => (
            Message::new("gui-history-stack-top", "Move to top of stack"),
            std::collections::BTreeMap::default(),
        ),
    };
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args,
    }
}

/// A keyword branch's step: "Rename keyword Animals to Fauna", "Merge
/// keyword Animals|Dog into Pets|Dog".
pub fn keyword_label(ask: &crate::library::KeywordAsk) -> Label {
    use crate::library::KeywordAsk;
    let (from, to) = ask.paths();
    let m = match ask {
        KeywordAsk::Rename { .. } => Message::new(
            "gui-history-keyword-rename",
            "Rename keyword { $from } to { $to }",
        ),
        KeywordAsk::Merge { .. } => Message::new(
            "gui-history-keyword-merge",
            "Merge keyword { $from } into { $to }",
        ),
    }
    .with("from", from)
    .with("to", to);
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: [
            ("from".to_string(), from.to_string()),
            ("to".to_string(), to.to_string()),
        ]
        .into(),
    }
}

/// A group edit's label: "Edit the metadata of 3 photos".
pub fn meta_label(photos: usize) -> Label {
    let m = Message::new(
        "gui-history-meta-set",
        "Edit the metadata of { $count } photos",
    )
    .with("count", photos);
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: [("count".to_string(), photos.to_string())].into(),
    }
}

/// A pairing's step: "Pair 2 RAW+JPEG files", "Unpair files".
pub fn pair_label(ask: &crate::library::PairAsk, count: usize) -> Label {
    use crate::library::PairAsk;
    let (m, args) = match ask {
        PairAsk::Pair { .. } => (
            Message::new("gui-history-pair-files", "Pair { $count } RAW+JPEG files")
                .with("count", count),
            [("count".to_string(), count.to_string())].into(),
        ),
        PairAsk::Unpair { .. } => (
            Message::new("gui-history-unpair-files", "Unpair files"),
            std::collections::BTreeMap::default(),
        ),
    };
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args,
    }
}

/// A step of a photo's versions: "Switch version", "Remove version".
pub fn version_label(removed: bool) -> Label {
    let m = if removed {
        Message::new("gui-history-version-remove", "Remove version")
    } else {
        Message::new("gui-history-version-switch", "Switch version")
    };
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: std::collections::BTreeMap::default(),
    }
}

/// A rotation's label: "Rotate 3 photos left", "Rotate 3 photos
/// right", "Reset the rotation of 3 photos".
pub fn rotate_label(turn: crate::library::Turn, photos: usize) -> Label {
    use crate::library::Turn;
    let m = match turn {
        Turn::Left => Message::new("gui-history-rotate-left", "Rotate { $count } photos left"),
        Turn::Right => Message::new("gui-history-rotate-right", "Rotate { $count } photos right"),
        Turn::Reset => Message::new(
            "gui-history-rotate-reset",
            "Reset the rotation of { $count } photos",
        ),
    }
    .with("count", photos);
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: [("count".to_string(), photos.to_string())].into(),
    }
}

/// An album change's label: "New album “Trip”", "Add 3 photos to “Trip”"
/// (`name`: the album's name before the change; `count`: the photos
/// added or removed; `added`: whether B put photos in the quick
/// collection, else took them out).
pub fn album_label(
    edit: &crate::library::AlbumEdit,
    name: &str,
    count: usize,
    added: bool,
) -> Label {
    use crate::library::AlbumEdit as E;
    let m = match edit {
        E::Create { .. } => Message::new("gui-history-album-new", "New album “{ $name }”"),
        E::Rename { .. } => Message::new("gui-history-album-rename", "Rename album “{ $name }”"),
        E::Delete { .. } => Message::new("gui-history-album-delete", "Delete album “{ $name }”"),
        E::CreateSet { .. } => Message::new("gui-history-set-new", "New album set “{ $name }”"),
        E::Move { set: Some(_), .. } => {
            Message::new("gui-history-album-move", "Move “{ $name }” into a set")
        }
        E::Move { set: None, .. } => Message::new(
            "gui-history-album-move-out",
            "Move “{ $name }” out of its set",
        ),
        E::Add { .. } => Message::new(
            "gui-history-album-add",
            "Add { $count } photos to “{ $name }”",
        ),
        E::Remove { .. } => Message::new(
            "gui-history-album-remove",
            "Remove { $count } photos from “{ $name }”",
        ),
        E::CreateSmart { .. } => {
            Message::new("gui-history-album-new-smart", "New smart album “{ $name }”")
        }
        E::Rules { .. } => Message::new("gui-history-album-rules", "Edit the rules of “{ $name }”"),
        E::QuickToggle { .. } if added => Message::new(
            "gui-history-quick-add",
            "Add { $count } photos to the quick collection",
        ),
        E::QuickToggle { .. } => Message::new(
            "gui-history-quick-remove",
            "Remove { $count } photos from the quick collection",
        ),
        E::QuickClear => Message::new("gui-history-quick-clear", "Clear the quick collection"),
        E::QuickSave { .. } => Message::new(
            "gui-history-quick-save",
            "Save the quick collection as “{ $name }”",
        ),
    }
    .with("name", name)
    .with("count", count);
    Label {
        text: m.english.clone(),
        key: Some(m.key),
        args: [
            ("name".to_string(), name.to_string()),
            ("count".to_string(), count.to_string()),
        ]
        .into(),
    }
}

/// Joins the thread walking a [`LibraryHistory`] off the frame when
/// dropped: its shell holds it, so no walk outlives the shell.
pub struct Walks(LibraryHistory);

impl Drop for Walks {
    fn drop(&mut self) {
        self.0.join();
    }
}

impl LibraryHistory {
    /// What joins this history's thread when dropped (the shell's).
    pub fn walks(&self) -> Walks {
        Walks(self.clone())
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The window to wake when a walk sent off the frame lands.
    pub fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.lock().wake = Some(wake);
    }

    /// Whether a window is woken when a walk lands.
    pub fn wakes(&self) -> bool {
        self.lock().wake.is_some()
    }

    /// Why walks sent off the frame were refused since the last call
    /// (their steps put back), for the shell to say.
    pub fn take_refusals(&self) -> Vec<String> {
        std::mem::take(&mut self.lock().refusals)
    }

    /// The steps of the walks in flight, the first being applied: each
    /// with whether it is a redo.
    pub fn in_flight(&self) -> Vec<(bool, Label)> {
        self.lock()
            .flights
            .iter()
            .flat_map(|f| f.entries.iter().map(|e| (f.redo, e.info.label.clone())))
            .collect()
    }

    /// Waits until no walk is in flight, `within` at most; whether none
    /// is.
    pub fn wait_idle(&self, within: Duration) -> bool {
        let end = Instant::now() + within;
        let mut inner = self.lock();
        while inner.working {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            inner = self
                .inner
                .idle
                .wait_timeout(inner, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Waits for the walks in flight to land and their thread to end:
    /// none outlives its shell.
    pub fn join(&self) {
        let worker = {
            let mut inner = self.lock();
            while inner.working {
                inner = self
                    .inner
                    .idle
                    .wait(inner)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            inner.worker.take()
        };
        if let Some(w) = worker {
            let _ = w.join();
        }
    }

    /// Undo and redo go through `apply`.
    pub fn set_apply(&self, apply: Apply) {
        self.lock().apply = Some(apply);
    }

    /// Records a step: `changes` made at `at` (seconds since the epoch),
    /// labelled `label`, by the user. Nothing changed, no step. Its id.
    pub fn record(&self, label: Label, changes: Changes, at: f64) -> Option<u64> {
        self.record_in(None, label, changes, at)
    }

    /// Records `changes` as [`LibraryHistory::record`] does, or, made by
    /// batch `group`, into the batch's step: the newest step when it is
    /// the batch's, else a new one by the batch's origin, labelled
    /// [`batch_label`]. Nothing changed, nothing recorded. The step's id.
    pub fn record_in(
        &self,
        group: Option<&Group>,
        label: Label,
        changes: Changes,
        at: f64,
    ) -> Option<u64> {
        if changes.is_empty() {
            return None;
        }
        let mut inner = self.lock();
        inner.redo.clear();
        inner.cleared += 1;
        inner.rev += 1;
        if let Some(g) = group
            && let Some(top) = inner.undo.last_mut()
            && top.group == Some(g.id)
        {
            top.changes.push(changes);
            top.info.label = batch_label(&g.origin, top.changes.len());
            top.info.at = at;
            return Some(top.info.id);
        }
        inner.next += 1;
        let id = FIRST_ID + inner.next;
        let (label, origin) = match group {
            Some(g) => (batch_label(&g.origin, 1), g.origin.clone()),
            None => (label, PopOrigin::User),
        };
        let mut info = StepInfo::new(id, LIBRARY, label, origin);
        info.at = at;
        inner.undo.push(Entry {
            info,
            changes: vec![changes],
            group: group.map(|g| g.id),
        });
        Some(id)
    }

    /// The scope, as egui-pop lists it.
    pub fn scope() -> ScopeInfo {
        let mut info = ScopeInfo::new(LIBRARY, "Library");
        info.focus = FOCUS.iter().map(|s| (*s).to_string()).collect();
        info
    }

    pub fn rev(&self) -> u64 {
        self.lock().rev
    }

    pub fn line(&self) -> Line {
        let inner = self.lock();
        Line {
            undo: inner.undo.iter().map(|e| e.info.clone()).collect(),
            redo: inner.redo.iter().map(|e| e.info.clone()).collect(),
            rev: inner.rev,
        }
    }

    /// Checks a walk of `count` steps (a redo, else an undo) by
    /// `origin`: the steps it walks, the newest first.
    fn plan(
        inner: &Inner,
        request: &Request,
        origin: &PopOrigin,
        redo: bool,
        count: usize,
    ) -> Result<Vec<StepInfo>, Refusal> {
        request.check(inner.rev)?;
        let list = if redo { &inner.redo } else { &inner.undo };
        if count > list.len() {
            return Err(Refusal::new(RefusalKind::Nothing, "no such step"));
        }
        let steps: Vec<StepInfo> = list
            .iter()
            .rev()
            .take(count)
            .map(|e| e.info.clone())
            .collect();
        if matches!(origin, PopOrigin::Agent(_))
            && let Some(s) = steps.iter().find(|s| s.origin != *origin)
        {
            return Err(Refusal::new(
                RefusalKind::Owner,
                format!("“{}” is not this agent's step: the user undoes it", s.label),
            ));
        }
        Ok(steps)
    }

    fn walked(inner: &Inner, steps: Vec<StepInfo>, dry_run: bool) -> Walked {
        Walked {
            scope: LIBRARY,
            steps,
            rev: inner.rev,
            head: inner.undo.last().map_or(0, |e| e.info.id),
            dry_run,
        }
    }

    /// Undoes (`redo` false) or redoes `count` steps, all or nothing,
    /// once the walks in flight have landed.
    fn walk(
        &self,
        request: &Request,
        origin: &PopOrigin,
        redo: bool,
        count: usize,
    ) -> Result<Walked, Refusal> {
        let mut inner = self.lock();
        while inner.working && !request.dry_run {
            inner = self
                .inner
                .idle
                .wait(inner)
                .unwrap_or_else(PoisonError::into_inner);
        }
        let steps = Self::plan(&inner, request, origin, redo, count)?;
        if !request.dry_run && count > 0 {
            let apply = inner
                .apply
                .clone()
                .ok_or_else(|| Refusal::new(RefusalKind::Owner, "no library to undo in"))?;
            let entries = Self::take(&mut inner, redo, count);
            if let Err(e) = apply_walk(&apply, &entries, redo) {
                // All or nothing: the steps stay where they were.
                for e in entries.into_iter().rev() {
                    if redo {
                        inner.redo.push(e);
                    } else {
                        inner.undo.push(e);
                    }
                }
                return Err(Refusal::new(RefusalKind::Owner, e));
            }
            Self::land(&mut inner, redo, entries);
            inner.rev += 1;
        }
        Ok(Self::walked(&inner, steps, request.dry_run))
    }

    /// Takes the `count` steps a walk walks off their list, in the order
    /// walked.
    fn take(inner: &mut Inner, redo: bool, count: usize) -> Vec<Entry> {
        let list = if redo {
            &mut inner.redo
        } else {
            &mut inner.undo
        };
        let at = list.len() - count;
        list.drain(at..).rev().collect()
    }

    /// Puts walked steps on the other list, in the order walked.
    fn land(inner: &mut Inner, redo: bool, entries: Vec<Entry>) {
        let to = if redo {
            &mut inner.undo
        } else {
            &mut inner.redo
        };
        to.extend(entries);
    }

    /// Undoes (`redo` false) or redoes `count` steps off the frame: the
    /// line moves now, the node is asked on the history's thread, after
    /// the walks already in flight.
    fn walk_later(
        &self,
        request: &Request,
        origin: &PopOrigin,
        redo: bool,
        count: usize,
    ) -> Result<Walked, Refusal> {
        let mut inner = self.lock();
        let steps = Self::plan(&inner, request, origin, redo, count)?;
        if request.dry_run || count == 0 {
            return Ok(Self::walked(&inner, steps, request.dry_run));
        }
        if inner.apply.is_none() {
            return Err(Refusal::new(RefusalKind::Owner, "no library to undo in"));
        }
        let entries = Self::take(&mut inner, redo, count);
        let flight = Flight {
            redo,
            at: if redo {
                inner.redo.len()
            } else {
                inner.undo.len()
            },
            cleared: inner.cleared,
            entries: entries.clone(),
        };
        let marked = entries.into_iter().map(|mut e| {
            e.info.in_progress = true;
            e
        });
        let marked: Vec<Entry> = marked.collect();
        Self::land(&mut inner, redo, marked);
        inner.flights.push_back(flight);
        inner.rev += 1;
        if !inner.working {
            if let Some(old) = inner.worker.take() {
                // Ended: it took its last flight.
                let _ = old.join();
            }
            let this = self.clone();
            match std::thread::Builder::new()
                .name("helios-library-undo".into())
                .spawn(move || this.drain())
            {
                Ok(handle) => {
                    inner.working = true;
                    inner.worker = Some(handle);
                }
                Err(e) => {
                    let flight = inner.flights.pop_back().expect("pushed");
                    Self::put_back(&mut inner, flight);
                    return Err(Refusal::new(RefusalKind::Owner, e.to_string()));
                }
            }
        }
        Ok(Self::walked(&inner, steps, false))
    }

    /// Puts a walk the node did not make back where it was.
    fn put_back(inner: &mut Inner, flight: Flight) {
        let ids: Vec<u64> = flight.entries.iter().map(|e| e.info.id).collect();
        let (from, to) = if flight.redo {
            (&mut inner.redo, &mut inner.undo)
        } else {
            (&mut inner.undo, &mut inner.redo)
        };
        to.retain(|e| !ids.contains(&e.info.id));
        if flight.redo && flight.cleared != inner.cleared {
            // A new step cleared the redo list since: nothing to redo.
            return;
        }
        let at = flight.at.min(from.len());
        for (i, e) in flight.entries.into_iter().rev().enumerate() {
            from.insert(at + i, e);
        }
    }

    /// The history's thread: applies the walks in flight in order; one
    /// refused is put back with those queued behind it.
    fn drain(&self) {
        loop {
            let (apply, entries, redo) = {
                let mut inner = self.lock();
                let Some(f) = inner.flights.front() else {
                    inner.working = false;
                    self.inner.idle.notify_all();
                    return;
                };
                (inner.apply.clone(), f.entries.clone(), f.redo)
            };
            // A walk that panics is refused like one that fails: the
            // thread goes on to its end, so `working` is cleared and
            // `join` (the shell's drop) never waits on a dead thread.
            let result = apply.map_or_else(
                || Err("no library to undo in".to_string()),
                |a| {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        apply_walk(&a, &entries, redo)
                    }))
                    .unwrap_or_else(|_| Err("the walk failed unexpectedly".to_string()))
                },
            );
            let mut inner = self.lock();
            let flight = inner.flights.pop_front().expect("in flight");
            match result {
                Ok(()) => {
                    let ids: Vec<u64> = flight.entries.iter().map(|e| e.info.id).collect();
                    let to = if redo {
                        &mut inner.undo
                    } else {
                        &mut inner.redo
                    };
                    for e in to.iter_mut().filter(|e| ids.contains(&e.info.id)) {
                        e.info.in_progress = false;
                    }
                }
                Err(e) => {
                    // Those queued behind it were asked of a line that
                    // is no more: put back too, the newest first.
                    let mut back: Vec<Flight> = inner.flights.drain(..).collect();
                    back.insert(0, flight);
                    for f in back.into_iter().rev() {
                        Self::put_back(&mut inner, f);
                    }
                    inner.refusals.push(e);
                }
            }
            inner.rev += 1;
            let wake = inner.wake.clone();
            drop(inner);
            if let Some(wake) = wake {
                wake();
            }
        }
    }

    pub fn undo(&self, request: &Request, origin: &PopOrigin) -> Answer {
        let count = usize::from(!self.lock().undo.is_empty());
        Answer::Done(self.walk(request, origin, false, count))
    }

    pub fn redo(&self, request: &Request, origin: &PopOrigin) -> Answer {
        let count = usize::from(!self.lock().redo.is_empty());
        Answer::Done(self.walk(request, origin, true, count))
    }

    pub fn jump(&self, request: &Request, to: u64, origin: &PopOrigin) -> Answer {
        let line = self.line();
        Answer::Done(plan_jump(&line, to).and_then(|j| self.walk(request, origin, j.redo, j.count)))
    }

    /// [`LibraryHistory::undo`] off the frame for the user (the menu,
    /// Ctrl+Z): the step moves at once and is applied when the node
    /// answers. Others' walks wait for their answer.
    pub fn undo_later(&self, request: &Request, origin: &PopOrigin) -> Answer {
        if *origin != PopOrigin::User {
            return self.undo(request, origin);
        }
        let count = usize::from(!self.lock().undo.is_empty());
        Answer::Done(self.walk_later(request, origin, false, count))
    }

    /// [`LibraryHistory::redo`] off the frame for the user.
    pub fn redo_later(&self, request: &Request, origin: &PopOrigin) -> Answer {
        if *origin != PopOrigin::User {
            return self.redo(request, origin);
        }
        let count = usize::from(!self.lock().redo.is_empty());
        Answer::Done(self.walk_later(request, origin, true, count))
    }

    /// [`LibraryHistory::jump`] off the frame for the user.
    pub fn jump_later(&self, request: &Request, to: u64, origin: &PopOrigin) -> Answer {
        if *origin != PopOrigin::User {
            return self.jump(request, to, origin);
        }
        let line = self.line();
        Answer::Done(
            plan_jump(&line, to).and_then(|j| self.walk_later(request, origin, j.redo, j.count)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_vault::Change;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    fn changes() -> Changes {
        Changes {
            images: vec![Change {
                image: helios_vault::ImageId(helios_core::ContentHash([1; 32])),
                before: None,
                after: None,
            }],
            ..Changes::default()
        }
    }

    #[test]
    fn an_import_is_one_step_undone_and_redone_through_the_vault() {
        let h = LibraryHistory::default();
        let (undone, redone) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let (u, r) = (undone.clone(), redone.clone());
        h.set_apply(Arc::new(move |_, forward| {
            if forward { &r } else { &u }.fetch_add(1, Ordering::SeqCst);
            Ok(Replayed::default())
        }));
        assert_eq!(h.record(import_label(3), Changes::default(), 1.0), None);
        let id = h.record(import_label(124), changes(), 2.0).unwrap();
        let line = h.line();
        assert_eq!(line.head_id(), id);
        assert_eq!(line.undo[0].label.text, "Import 124 photos");
        assert_eq!(line.undo[0].at, 2.0);
        let rev = h.rev();
        let Answer::Done(Ok(w)) = h.undo(&Request::new(LIBRARY), &PopOrigin::User) else {
            panic!("undone");
        };
        assert_eq!((w.head, undone.load(Ordering::SeqCst)), (0, 1));
        assert!(h.rev() > rev);
        // An agent does not walk the user's import.
        let agent = PopOrigin::Agent("mcp".into());
        assert!(matches!(
            h.redo(&Request::new(LIBRARY), &agent),
            Answer::Done(Err(_))
        ));
        let Answer::Done(Ok(w)) = h.redo(&Request::new(LIBRARY), &PopOrigin::User) else {
            panic!("redone");
        };
        assert_eq!((w.head, redone.load(Ordering::SeqCst)), (id, 1));
        let Answer::Done(Ok(_)) = h.jump(&Request::new(LIBRARY), 0, &PopOrigin::User) else {
            panic!("jumped");
        };
        assert_eq!(undone.load(Ordering::SeqCst), 2);
    }

    fn changes_of(n: u8) -> Changes {
        Changes {
            images: vec![Change {
                image: helios_vault::ImageId(helios_core::ContentHash([n; 32])),
                before: None,
                after: None,
            }],
            ..Changes::default()
        }
    }

    #[test]
    fn a_batch_s_changes_are_one_step_undone_in_reverse_all_or_nothing() {
        let h = LibraryHistory::default();
        let walked = Arc::new(Mutex::new(Vec::new()));
        let fail_on = Arc::new(AtomicU64::new(0));
        let (w, f) = (walked.clone(), fail_on.clone());
        h.set_apply(Arc::new(move |changes, forward| {
            let n = changes.images[0].image.0.0[0];
            if u64::from(n) == f.load(Ordering::SeqCst) {
                return Err(format!("{n} refused"));
            }
            w.lock().unwrap().push((n, forward));
            Ok(Replayed::default())
        }));
        let agent = PopOrigin::Agent("mcp".into());
        let batch = Group {
            id: 7,
            origin: agent.clone(),
        };
        for n in 1..=3 {
            h.record_in(Some(&batch), import_label(1), changes_of(n), f64::from(n));
        }
        // The user's step after it is its own.
        h.record(import_label(1), changes_of(9), 4.0);
        let line = h.line();
        assert_eq!(line.undo.len(), 2);
        assert_eq!(line.undo[0].label.text, "Agent: 3 commands");
        assert_eq!(line.undo[0].origin, agent);
        assert_eq!(line.undo[1].origin, PopOrigin::User);
        let Answer::Done(Ok(_)) = h.undo(&Request::new(LIBRARY), &PopOrigin::User) else {
            panic!("the user's step undone");
        };
        walked.lock().unwrap().clear();
        // A set refused midway: those taken back are put again.
        fail_on.store(1, Ordering::SeqCst);
        assert!(matches!(
            h.undo(&Request::new(LIBRARY), &agent),
            Answer::Done(Err(_))
        ));
        assert_eq!(
            *walked.lock().unwrap(),
            [(3, false), (2, false), (2, true), (3, true)]
        );
        assert_eq!(h.line().undo.len(), 1, "still there");
        walked.lock().unwrap().clear();
        fail_on.store(0, Ordering::SeqCst);
        let Answer::Done(Ok(_)) = h.undo(&Request::new(LIBRARY), &agent) else {
            panic!("the batch undone");
        };
        let Answer::Done(Ok(_)) = h.redo(&Request::new(LIBRARY), &agent) else {
            panic!("the batch redone");
        };
        assert_eq!(
            *walked.lock().unwrap(),
            [
                (3, false),
                (2, false),
                (1, false),
                (1, true),
                (2, true),
                (3, true)
            ]
        );
        // A user's batch is labelled without "Agent".
        assert_eq!(batch_label(&PopOrigin::User, 2).text, "2 commands");
    }

    /// An apply that waits on `held` (another writer of the vault),
    /// noting each set walked, refusing set `fail_on`.
    fn held_apply(
        held: &Arc<Mutex<()>>,
        walked: &Arc<Mutex<Vec<(u8, bool)>>>,
        fail_on: &Arc<AtomicU64>,
    ) -> Apply {
        let (held, walked, fail_on) = (held.clone(), walked.clone(), fail_on.clone());
        Arc::new(move |changes, forward| {
            let _writer = held.lock().unwrap();
            let n = changes.images[0].image.0.0[0];
            if u64::from(n) == fail_on.load(Ordering::SeqCst) {
                return Err(format!("{n} refused"));
            }
            walked.lock().unwrap().push((n, forward));
            Ok(Replayed::default())
        })
    }

    #[test]
    fn the_user_s_walks_run_off_the_frame_queued_in_order() {
        let h = LibraryHistory::default();
        let (held, walked, fail_on) = (
            Arc::new(Mutex::new(())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(AtomicU64::new(0)),
        );
        h.set_apply(held_apply(&held, &walked, &fail_on));
        let _walks = h.walks();
        for n in 1..=3 {
            h.record(import_label(1), changes_of(n), f64::from(n));
        }
        let writer = held.lock().unwrap();
        // Two undos, then a redo: each answers at once, the line moved.
        let start = std::time::Instant::now();
        for _ in 0..2 {
            let Answer::Done(Ok(_)) = h.undo_later(&Request::new(LIBRARY), &PopOrigin::User) else {
                panic!("undone");
            };
        }
        let Answer::Done(Ok(_)) = h.redo_later(&Request::new(LIBRARY), &PopOrigin::User) else {
            panic!("redone");
        };
        assert!(start.elapsed() < Duration::from_millis(500));
        let line = h.line();
        assert_eq!((line.undo.len(), line.redo.len()), (2, 1));
        assert!(line.redo[0].in_progress, "{line:?}");
        assert_eq!(h.in_flight().len(), 3);
        assert!(walked.lock().unwrap().is_empty());
        // A dry run does not queue.
        let mut dry = Request::new(LIBRARY);
        dry.dry_run = true;
        assert!(matches!(
            h.undo_later(&dry, &PopOrigin::User),
            Answer::Done(Ok(_))
        ));
        assert_eq!(h.in_flight().len(), 3);
        drop(writer);
        assert!(h.wait_idle(Duration::from_secs(10)));
        assert_eq!(*walked.lock().unwrap(), [(3, false), (2, false), (2, true)]);
        let line = h.line();
        assert!(line.undo.iter().chain(&line.redo).all(|s| !s.in_progress));
        assert!(h.take_refusals().is_empty());
        // An agent's walk waits for its answer.
        let agent = PopOrigin::Agent("mcp".into());
        assert!(matches!(
            h.undo_later(&Request::new(LIBRARY), &agent),
            Answer::Done(Err(_))
        ));
    }

    /// A walk whose apply panics is refused and put back: the thread
    /// ends, so waiting for it and joining it return.
    #[test]
    fn a_walk_that_panics_is_refused_and_its_thread_ends() {
        let h = LibraryHistory::default();
        h.set_apply(Arc::new(|_, _| panic!("the writer's lock is poisoned")));
        h.record(import_label(1), changes_of(1), 1.0);
        let Answer::Done(Ok(_)) = h.undo_later(&Request::new(LIBRARY), &PopOrigin::User) else {
            panic!("undone");
        };
        assert!(h.wait_idle(Duration::from_secs(10)), "the thread ended");
        let line = h.line();
        assert_eq!((line.undo.len(), line.redo.len()), (1, 0), "{line:?}");
        assert!(!line.undo[0].in_progress);
        assert_eq!(h.take_refusals(), ["the walk failed unexpectedly"]);
        h.join();
    }

    #[test]
    fn a_refused_walk_is_put_back_with_those_queued_behind_it() {
        let h = LibraryHistory::default();
        let (held, walked, fail_on) = (
            Arc::new(Mutex::new(())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(AtomicU64::new(3)),
        );
        h.set_apply(held_apply(&held, &walked, &fail_on));
        let _walks = h.walks();
        // A batch of two sets, then a step of the user's.
        let batch = Group {
            id: 4,
            origin: PopOrigin::User,
        };
        h.record_in(Some(&batch), import_label(1), changes_of(1), 1.0);
        h.record_in(Some(&batch), import_label(1), changes_of(2), 2.0);
        h.record(import_label(1), changes_of(3), 3.0);
        let before = h.line();
        let writer = held.lock().unwrap();
        for _ in 0..2 {
            let Answer::Done(Ok(_)) = h.undo_later(&Request::new(LIBRARY), &PopOrigin::User) else {
                panic!("undone");
            };
        }
        assert_eq!(h.line().undo.len(), 0);
        drop(writer);
        assert!(h.wait_idle(Duration::from_secs(10)));
        // The first refused: both back, in their places; nothing walked.
        let line = h.line();
        assert_eq!(
            line.undo.iter().map(|s| s.id).collect::<Vec<_>>(),
            before.undo.iter().map(|s| s.id).collect::<Vec<_>>()
        );
        assert!(line.redo.is_empty());
        assert!(line.undo.iter().all(|s| !s.in_progress));
        assert!(walked.lock().unwrap().is_empty());
        assert_eq!(h.take_refusals(), ["3 refused"]);
        // The batch refused midway: its sets walked back, the step back.
        fail_on.store(1, Ordering::SeqCst);
        for _ in 0..2 {
            h.undo_later(&Request::new(LIBRARY), &PopOrigin::User);
            assert!(h.wait_idle(Duration::from_secs(10)));
        }
        assert_eq!(*walked.lock().unwrap(), [(3, false), (2, false), (2, true)]);
        let line = h.line();
        assert_eq!((line.undo.len(), line.redo.len()), (1, 1), "{line:?}");
        assert_eq!(h.take_refusals(), ["1 refused"]);
        // A redo refused after a new step cleared the redo list: gone.
        fail_on.store(3, Ordering::SeqCst);
        let writer = held.lock().unwrap();
        h.redo_later(&Request::new(LIBRARY), &PopOrigin::User);
        h.record(import_label(1), changes_of(7), 5.0);
        drop(writer);
        assert!(h.wait_idle(Duration::from_secs(10)));
        let line = h.line();
        assert!(line.redo.is_empty(), "{line:?}");
        assert_eq!(line.undo.len(), 2, "{line:?}");
        assert_eq!(h.take_refusals(), ["3 refused"]);
    }
}
