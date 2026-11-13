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
//! - **For the session only** (docs/design/undo.md, decided 2026-10-14):
//!   the steps live in memory; a restart starts with none.
//! - **Active with the library's panels** (the Library, the Filmstrip,
//!   the Duplicates panel): Ctrl+Z there undoes the newest step among
//!   the library's and the edit's, by time.
//! - **An agent's lane**, as for the edit: an agent walks only its own
//!   steps; an import's step is the user's.

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

struct Entry {
    info: StepInfo,
    changes: Changes,
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

/// An album change's label: "New album “Trip”", "Add 3 photos to “Trip”"
/// (`name`: the album's name before the change; `count`: the photos
/// added or removed).
pub fn album_label(edit: &crate::library::AlbumEdit, name: &str, count: usize) -> Label {
    use crate::library::AlbumEdit as E;
    let m = match edit {
        E::Create { .. } => Message::new("gui-history-album-new", "New album “{ $name }”"),
        E::Rename { .. } => Message::new("gui-history-album-rename", "Rename album “{ $name }”"),
        E::Delete { .. } => Message::new("gui-history-album-delete", "Delete album “{ $name }”"),
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
    /// labelled `label`. Nothing changed, no step. Its id.
    pub fn record(&self, label: Label, changes: Changes, at: f64) -> Option<u64> {
        if changes.is_empty() {
            return None;
        }
        let mut inner = self.lock();
        inner.next += 1;
        let id = FIRST_ID + inner.next;
        let mut info = StepInfo::new(id, LIBRARY, label, PopOrigin::User);
        info.at = at;
        inner.undo.push(Entry { info, changes });
        inner.redo.clear();
        inner.rev += 1;
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
                if let Err(e) = apply(&entry.changes, redo) {
                    // Put back what was walked: all or nothing.
                    let mut walked = vec![entry];
                    for _ in 0..done {
                        let back = if redo {
                            inner.undo.pop()
                        } else {
                            inner.redo.pop()
                        }
                        .expect("walked");
                        let _ = apply(&back.changes, !redo);
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
}
