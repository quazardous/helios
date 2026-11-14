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
//! - **One step per manual stack change** (L11): a group, an unstack, a
//!   new top; the stacks' documents before and after, as the node's
//!   `library.stacks.group`, `.ungroup` and `.top` answer them.
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

use std::cell::RefCell;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

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
pub const FOCUS: &[&str] = &["helios.vault", "helios.filmstrip", "helios.duplicates"];

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

struct Entry {
    info: StepInfo,
    /// The sets of changes, in the order made: undone in reverse.
    changes: Vec<Changes>,
    /// The batch it holds the changes of.
    group: Option<u64>,
}

#[derive(Default)]
struct Inner {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    rev: u64,
    next: u64,
    apply: Option<Apply>,
}

/// The library's steps, shared by the history owner and whoever records
/// an import (the import's thread).
#[derive(Clone, Default)]
pub struct LibraryHistory {
    inner: Arc<Mutex<Inner>>,
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

impl LibraryHistory {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
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

    /// Undoes (`redo` false) or redoes `count` steps, all or nothing.
    fn walk(
        &self,
        request: &Request,
        origin: &PopOrigin,
        redo: bool,
        count: usize,
    ) -> Result<Walked, Refusal> {
        let mut inner = self.lock();
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
        if !request.dry_run && count > 0 {
            let apply = inner
                .apply
                .clone()
                .ok_or_else(|| Refusal::new(RefusalKind::Owner, "no library to undo in"))?;
            for done in 0..count {
                let entry = if redo {
                    inner.redo.pop()
                } else {
                    inner.undo.pop()
                }
                .expect("counted");
                if let Err(e) = apply_entry(&apply, &entry, redo) {
                    // Put back what was walked: all or nothing.
                    let mut walked = vec![entry];
                    for _ in 0..done {
                        let back = if redo {
                            inner.undo.pop()
                        } else {
                            inner.redo.pop()
                        }
                        .expect("walked");
                        let _ = apply_entry(&apply, &back, !redo);
                        walked.push(back);
                    }
                    for w in walked {
                        if redo {
                            inner.redo.push(w);
                        } else {
                            inner.undo.push(w);
                        }
                    }
                    return Err(Refusal::new(RefusalKind::Owner, e));
                }
                if redo {
                    inner.undo.push(entry);
                } else {
                    inner.redo.push(entry);
                }
            }
            inner.rev += 1;
        }
        Ok(Walked {
            scope: LIBRARY,
            steps,
            rev: inner.rev,
            head: inner.undo.last().map_or(0, |e| e.info.id),
            dry_run: request.dry_run,
        })
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_vault::Change;
    use std::sync::atomic::{AtomicU64, Ordering};

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
}
