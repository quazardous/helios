//! The history beyond undo, redo and a walk along its line: one step
//! undone out of order.

use crate::{Origin, Refused, Store};

impl Store {
    /// Whether the step `id` can be undone alone, now, by `origin`, even
    /// under later steps (VS Code's rule): its place in
    /// [`Store::undo_steps`], `None` when it is not on the undo list.
    /// Refused when a later step changed one of its values (with those
    /// steps' labels), or when a gesture of another origin holds one.
    pub fn check_undo_step(&self, id: u64, origin: &Origin) -> Result<Option<usize>, Refused> {
        let Some(i) = self.undo.iter().position(|s| s.id == id) else {
            return Ok(None);
        };
        self.check_alone(&[i], None, origin)?;
        Ok(Some(i))
    }

    /// Undoes the step `id` alone, when [`Store::check_undo_step`] allows
    /// it, as [`Store::undo_group`] undoes a group: it goes on the redo
    /// list, so a redo makes it again. Its label, `None` when it is not on
    /// the undo list.
    pub fn undo_step(&mut self, id: u64, origin: &Origin) -> Result<Option<String>, Refused> {
        let Some(i) = self.check_undo_step(id, origin)? else {
            return Ok(None);
        };
        let step = self.undo.remove(i);
        if let Err(e) = self.replay(&step, origin, true) {
            self.undo.insert(i, step);
            return Err(e);
        }
        let label = step.label.clone();
        self.commit(format!("undo {label}"), origin);
        self.redo.push(step);
        Ok(Some(label))
    }
}
