//! A point of the history shown without going there: the values at a
//! step ([`Store::values_at`]) and an overlay the graph renders from in
//! place of the store's own values ([`Store::set_preview`]). The overlay
//! is never a write: no step, no commit, no sequence number, no event,
//! so nothing is saved and every reader of the store (a widget, an
//! agent, the edit's save) keeps seeing the live values. Ending it marks
//! its paths for the next [`crate::sync`], which brings the graph back to
//! the live values.

use std::collections::BTreeMap;

use helios_core::Value;

use crate::{BYPASS, Step, Store};

/// Whether the graph mirrors a path: a node's parameter or bypass.
fn mirrored(path: &str) -> bool {
    path.starts_with("node/") || path.starts_with(BYPASS)
}

/// Each step's values before it, newest step first.
fn back<'a>(steps: impl Iterator<Item = &'a Step>, at: &mut BTreeMap<String, Value>) {
    for s in steps {
        for (path, old, _) in s.changes.iter().rev() {
            at.insert(path.clone(), old.clone());
        }
    }
}

/// Each step's values after it, oldest step first.
fn on<'a>(steps: impl Iterator<Item = &'a Step>, at: &mut BTreeMap<String, Value>) {
    for s in steps {
        for (path, _, new) in &s.changes {
            at.insert(path.clone(), new.clone());
        }
    }
}

impl Store {
    /// The values at step `to` (0: the start; a step undone or replaced
    /// included) where they differ from now, in the store's order:
    /// what a jump there would write, nothing walked. `None` when `to`
    /// is no step, or a replaced step whose line left from a step that
    /// is replaced too (a jump refuses it the same way).
    pub fn values_at(&self, to: u64) -> Option<Vec<(String, Value)>> {
        let mut at = BTreeMap::new();
        let walk = |at: &mut BTreeMap<String, Value>, redo: bool, count: usize| {
            if redo {
                on(self.redo.iter().rev().take(count), at);
            } else {
                back(self.undo.iter().rev().take(count), at);
            }
        };
        if let Some((redo, count)) = self.plan_walk(to) {
            walk(&mut at, redo, count);
        } else {
            let (line, pos) = self.replaced.iter().find_map(|line| {
                line.steps
                    .iter()
                    .position(|s| s.id == to)
                    .map(|pos| (line, pos))
            })?;
            let (redo, count) = self.plan_walk(line.after)?;
            walk(&mut at, redo, count);
            on(line.steps[..=pos].iter(), &mut at);
        }
        let mut out: Vec<(String, Value)> = at
            .into_iter()
            .filter(|(p, v)| self.entries.get(p).is_some_and(|e| e.value != *v))
            .collect();
        out.sort_by_key(|(p, _)| self.entries[p].order);
        Some(out)
    }

    /// Shows `values` in place of the store's own until the next call
    /// (empty: the live values again): the graph renders them at the next
    /// [`crate::sync`]. Paths the store does not declare are left out.
    /// Nothing is written; a write to a path shown this way ends its
    /// preview, so it shows through. Whether what is shown changed.
    pub fn set_preview(&mut self, values: Vec<(String, Value)>) -> bool {
        let values: BTreeMap<String, Value> = values
            .into_iter()
            .filter(|(p, _)| self.entries.contains_key(p))
            .collect();
        if values == self.preview {
            return false;
        }
        let before = std::mem::replace(&mut self.preview, values);
        for path in before.keys().chain(self.preview.keys()) {
            if mirrored(path) {
                self.dirty.insert(path.clone());
            }
        }
        true
    }

    /// The values shown in place of the store's own, by path.
    pub fn previewed(&self) -> &BTreeMap<String, Value> {
        &self.preview
    }

    /// The value the graph shows for `path`: the preview's, else the
    /// store's.
    pub fn shown(&self, path: &str) -> Option<&Value> {
        self.preview
            .get(path)
            .or_else(|| self.entries.get(path).map(|e| &e.value))
    }
}
