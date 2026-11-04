//! The open image's history snapshots: named states of its edit.
//!
//! - **Taking one saves nothing.** [`crate::history::HeliosHistory`]
//!   records a name, the value store's head step and the values there
//!   (what a restore writes back) in the shared [`Book`].
//! - **The next save pins it.** Each frame, once the edit is saved
//!   (`crate::persist`), a snapshot taken at the step the saved edit
//!   holds is pinned to that version (to no version for an image never
//!   edited), and the image's pinned snapshots are kept in the vault
//!   (`helios_vault::snapshots`). A pinned snapshot survives a quit and a
//!   `kill -9`.
//! - **Pending until then.** A snapshot not pinned yet is shown "not
//!   saved yet"; if the app quits (or another image opens) first, it is
//!   lost. One whose step the history left before a save stays pending
//!   for the session.
//! - **Restoring** writes its values back as one new step of the value
//!   store, which an undo takes back; a toast says so.
//!
//! When an image opens, its pinned snapshots are read back from the
//! vault, each with the values of the version it is pinned to.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use helios_core::Value;
use helios_shell::commands::Origin;
use helios_values::{BYPASS, Store};
use helios_vault::{ImageId, VersionId, edit};
use serde_json::Value as Json;

use crate::mask_tree::{label_path, lock_path};

/// The book, shared by the history owner and the vault's link.
pub type Shared = Arc<Mutex<Book>>;

/// The book behind `shared`, a poisoned lock taken over.
pub fn lock(shared: &Shared) -> MutexGuard<'_, Book> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where a snapshot's state is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pin {
    /// A version of the image's edit.
    Version(VersionId),
    /// The image with no edit (it had none when the snapshot was pinned).
    Unedited,
}

/// One snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct Taken {
    /// Unique among the image's snapshots; never 0.
    pub id: u64,
    pub name: String,
    /// The value store's head step when it was taken (0: the start).
    pub step: u64,
    pub origin: Origin,
    /// When it was taken, in seconds since the epoch.
    pub at: f64,
    /// Where its state is kept; none: pending, until the next save.
    pub pin: Option<Pin>,
    /// What a restore writes back: paths of the value store and values.
    pub values: Vec<(String, Value)>,
}

/// The snapshots of the image open.
#[derive(Debug, Default)]
pub struct Book {
    image: Option<ImageId>,
    list: Vec<Taken>,
    next: u64,
    /// The nodes and parameters an edit saves (`crate::persist`): only
    /// these are taken, so a snapshot holds what its version holds.
    saved: Option<BTreeMap<String, BTreeSet<String>>>,
    /// The pinned snapshots changed since the vault last kept them.
    dirty: bool,
    /// The snapshots restored since last asked, by name (the toast).
    restored: Vec<String>,
    /// How a pending snapshot's name is shown, `{name}` standing for it,
    /// in the interface's language (the shell keeps it so).
    pending: Option<String>,
}

impl Book {
    /// The image whose snapshots these are (none: no vault behind).
    pub fn image(&self) -> Option<ImageId> {
        self.image
    }

    /// Shows a pending snapshot's name through `pattern`, `{name}`
    /// standing for it ("{name} (not saved yet)").
    pub fn set_pending_pattern(&mut self, pattern: &str) {
        self.pending = Some(pattern.to_string());
    }

    /// A snapshot's name as the History panel shows it: a pending one
    /// says it is not saved yet.
    pub fn shown_name(&self, t: &Taken) -> String {
        if t.pin.is_some() {
            return t.name.clone();
        }
        match &self.pending {
            Some(p) => p.replace("{name}", &t.name),
            None => format!("{} (not saved yet)", t.name),
        }
    }

    /// Every snapshot, oldest first.
    pub fn list(&self) -> &[Taken] {
        &self.list
    }

    pub fn get(&self, id: u64) -> Option<&Taken> {
        self.list.iter().find(|t| t.id == id)
    }

    /// Another image is shown: the snapshots go, the pending ones lost.
    pub fn clear(&mut self) {
        *self = Book {
            saved: self.saved.take(),
            pending: self.pending.take(),
            ..Book::default()
        };
    }

    /// The snapshots of `image`, as the vault keeps them.
    pub fn load(&mut self, image: ImageId, list: Vec<Taken>) {
        self.next = list.iter().map(|t| t.id).max().unwrap_or(0);
        self.image = Some(image);
        self.list = list;
        self.dirty = false;
    }

    /// The nodes and their parameters an edit saves, by node name.
    pub fn set_saved(&mut self, saved: BTreeMap<String, BTreeSet<String>>) {
        self.saved = Some(saved);
    }

    /// Takes a snapshot of `store` at its head (`dry_run`: the one it
    /// would take, nothing kept).
    pub fn take(
        &mut self,
        store: &Store,
        name: &str,
        origin: Origin,
        at: f64,
        dry_run: bool,
    ) -> Taken {
        let taken = Taken {
            id: self.next + 1,
            name: name.to_string(),
            step: head_step(store),
            origin,
            at,
            pin: None,
            values: self.values_of(store),
        };
        if !dry_run {
            self.next += 1;
            self.list.push(taken.clone());
        }
        taken
    }

    /// Deletes a snapshot; whether it was there.
    pub fn forget(&mut self, id: u64) -> bool {
        let Some(i) = self.list.iter().position(|t| t.id == id) else {
            return false;
        };
        let gone = self.list.remove(i);
        self.dirty |= gone.pin.is_some();
        true
    }

    /// Pins the pending snapshots taken at `step` (the head the saved
    /// edit holds) to `pin`; how many.
    pub fn pin_at(&mut self, step: u64, pin: Pin) -> usize {
        let mut n = 0;
        for t in &mut self.list {
            if t.pin.is_none() && t.step == step {
                t.pin = Some(pin);
                n += 1;
            }
        }
        self.dirty |= n > 0;
        n
    }

    /// Whether the vault must keep the pinned snapshots again; cleared.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Asks for the pinned snapshots to be kept again (a write failed).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// A restore was made: the toast's to say.
    pub fn restored(&mut self, name: &str) {
        self.restored.push(name.to_string());
    }

    /// The snapshots restored since last asked, by name.
    pub fn take_restored(&mut self) -> Vec<String> {
        std::mem::take(&mut self.restored)
    }

    /// The pinned snapshots, as the vault keeps them.
    pub fn pinned(&self) -> Vec<helios_vault::snapshots::Snapshot> {
        self.list
            .iter()
            .filter_map(|t| {
                Some(helios_vault::snapshots::Snapshot {
                    id: t.id,
                    name: t.name.clone(),
                    origin: crate::persist::who(&t.origin),
                    at: t.at.max(0.0) as u64,
                    version: match t.pin? {
                        Pin::Version(v) => Some(v),
                        Pin::Unedited => None,
                    },
                })
            })
            .collect()
    }

    /// What a restore of the state of `store` writes back: the saved
    /// nodes' parameters, the bypasses, the labels and locks.
    fn values_of(&self, store: &Store) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        for (path, e) in store.list("node/") {
            let keep = match (&self.saved, path_param(path)) {
                (_, None) => false,
                (None, Some(_)) => true,
                (Some(saved), Some((node, param))) => {
                    saved.get(node).is_some_and(|p| p.contains(param))
                }
            };
            if keep {
                out.push((path.to_string(), e.value.clone()));
            }
        }
        for prefix in [BYPASS, "label/", "lock/"] {
            for (path, e) in store.list(prefix) {
                out.push((path.to_string(), e.value.clone()));
            }
        }
        out
    }
}

/// The value store's head step (0: the start).
pub fn head_step(store: &Store) -> u64 {
    store.undo_steps().last().map_or(0, |s| s.id)
}

/// A parameter's node and name, from its path (`node/<node>/<param>`).
fn path_param(path: &str) -> Option<(&str, &str)> {
    path.strip_prefix("node/")?.rsplit_once('/')
}

/// What a restore of a saved edit writes back: its nodes' parameters,
/// whether each is bypassed, and its marks (labels, locks).
pub fn values_of_edit(edit: &Json) -> Result<Vec<(String, Value)>, String> {
    let bypassed = edit::bypassed(edit);
    let mut out = Vec::new();
    for (node, (_, params)) in edit::modules(edit)? {
        for (param, value) in params.iter() {
            out.push((format!("node/{node}/{param}"), value.clone()));
        }
        out.push((
            format!("{BYPASS}{node}"),
            Value::Bool(bypassed.contains(&node)),
        ));
    }
    for (node, m) in edit["marks"].as_object().into_iter().flatten() {
        if let Some(label) = m.get("label").and_then(Json::as_str) {
            out.push((label_path(node), Value::Text(label.into())));
        }
        if m.get("locked").and_then(Json::as_bool) == Some(true) {
            out.push((lock_path(node), Value::Bool(true)));
        }
    }
    Ok(out)
}

/// Who took a snapshot, from the vault's name for it (`user`,
/// `agent:<id>`…).
pub fn origin_of(who: &str) -> Origin {
    match who.split_once(':') {
        Some(("agent", id)) => Origin::Agent(id.into()),
        Some(("script", id)) => Origin::Script(id.into()),
        Some(("remote", id)) => Origin::Remote(id.into()),
        _ if who == "midi" => Origin::Midi,
        _ => Origin::User,
    }
}

/// The writes that bring `store` to a snapshot's `values`: each value
/// that differs, the labels and locks it does not name put back to
/// their defaults (path, old, new). A path the store does not hold (a
/// node the network no longer has) is left out; a label or a lock is
/// declared first.
pub fn restore_writes(store: &Store, values: &[(String, Value)]) -> Vec<(String, Value)> {
    let named: BTreeSet<&str> = values.iter().map(|(p, _)| p.as_str()).collect();
    let mut out = Vec::new();
    for (path, value) in values {
        let current = store.get(path).map(|e| &e.value);
        let mark = path.starts_with("label/") || path.starts_with("lock/");
        match current {
            Some(v) if v == value => {}
            Some(_) => out.push((path.clone(), value.clone())),
            None if mark => out.push((path.clone(), value.clone())),
            None => {}
        }
    }
    for prefix in ["label/", "lock/"] {
        for (path, e) in store.list(prefix) {
            if !named.contains(path) && e.value != e.descriptor.default {
                out.push((path.to_string(), e.descriptor.default.clone()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use helios_values::{Change, Descriptor, Set};
    use serde_json::json;

    use super::*;

    #[test]
    fn an_edit_s_values_are_its_parameters_bypasses_and_marks() {
        let edit = json!({
            "modules": [
                { "name": "exp", "module": "exposure", "params": { "exposure": { "float": 1.5 } } },
                { "name": "spot", "module": "mask_spot", "params": {}, "bypassed": true },
            ],
            "marks": { "spot": { "label": "Face", "locked": true } },
        });
        let values = values_of_edit(&edit).unwrap();
        assert!(values.contains(&("node/exp/exposure".into(), Value::Float(1.5))));
        assert!(values.contains(&("bypass/exp".into(), Value::Bool(false))));
        assert!(values.contains(&("bypass/spot".into(), Value::Bool(true))));
        assert!(values.contains(&("label/spot".into(), Value::Text("Face".into()))));
        assert!(values.contains(&("lock/spot".into(), Value::Bool(true))));
    }

    #[test]
    fn a_snapshot_is_pending_until_pinned_at_its_step() {
        let mut store = Store::new();
        store.declare("node/exp/exposure", Descriptor::float("EV", -3.0, 3.0, 0.0));
        store.declare("node/gone/x", Descriptor::float("x", -3.0, 3.0, 0.0));
        let mut book = Book::default();
        book.set_saved([("exp".to_string(), ["exposure".to_string()].into())].into());
        let dry = book.take(&store, "Dry", Origin::User, 1.0, true);
        assert!(book.list().is_empty(), "{dry:?}");
        store
            .set(Set::new(
                "node/exp/exposure",
                Change::To(Value::Float(1.0)),
                Origin::User,
            ))
            .unwrap();
        let t = book.take(&store, "Before", Origin::User, 1.0, false);
        assert_eq!((t.id, t.step, t.pin), (1, 1, None));
        assert_eq!(
            t.values,
            [("node/exp/exposure".to_string(), Value::Float(1.0))],
            "only what an edit saves"
        );
        assert!(book.pinned().is_empty());
        assert_eq!(book.pin_at(0, Pin::Unedited), 0, "another step");
        assert_eq!(book.pin_at(1, Pin::Unedited), 1);
        assert!(book.take_dirty());
        assert_eq!(book.pinned().len(), 1);
        // Forgetting a pinned one asks the vault to keep the rest.
        assert!(book.forget(1));
        assert!(book.take_dirty());
        assert!(!book.forget(1));
    }

    #[test]
    fn a_restore_writes_what_differs_and_unnames_the_others() {
        let mut store = Store::new();
        store.declare("node/exp/exposure", Descriptor::float("EV", -3.0, 3.0, 0.0));
        store.declare_with(
            "label/a",
            Descriptor::text("label of a", ""),
            Value::Text("Sky".into()),
        );
        let writes = restore_writes(
            &store,
            &[
                ("node/exp/exposure".into(), Value::Float(0.0)),
                ("node/gone/x".into(), Value::Float(1.0)),
                ("label/b".into(), Value::Text("Face".into())),
            ],
        );
        assert_eq!(
            writes,
            [
                ("label/b".to_string(), Value::Text("Face".into())),
                ("label/a".to_string(), Value::Text(String::new())),
            ]
        );
    }

    #[test]
    fn origins_come_back_from_the_vault_s_names() {
        for o in [
            Origin::User,
            Origin::Agent("mcp".into()),
            Origin::Script("s".into()),
        ] {
            assert_eq!(origin_of(&crate::persist::who(&o)), o);
        }
    }
}
