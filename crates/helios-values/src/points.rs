//! The history beyond undo, redo and a walk along its line: one step
//! undone out of order, and the redo lists new steps dropped (replaced
//! lines, reachable by a jump).

use crate::{Origin, Refused, Step, Store};

/// A redo list a new step dropped.
#[derive(Clone, Debug)]
pub(crate) struct Branch {
    /// The step it came after (0: the start).
    pub(crate) after: u64,
    /// Its steps, in the order they were made.
    pub(crate) steps: Vec<Step>,
}

impl Store {
    /// A new step drops the redo list: kept as a replaced line leaving
    /// from the head.
    pub(crate) fn drop_redo(&mut self) {
        if self.redo.is_empty() {
            return;
        }
        let after = self.undo.last().map_or(0, |s| s.id);
        let steps: Vec<Step> = self.redo.drain(..).rev().collect();
        self.replaced.push(Branch { after, steps });
    }

    /// The steps new steps dropped from the redo list, each with the step
    /// its line left after (0: the start), oldest line first, each line
    /// in the order its steps were made.
    pub fn replaced_steps(&self) -> Vec<(u64, &Step)> {
        self.replaced
            .iter()
            .flat_map(|b| b.steps.iter().map(move |s| (b.after, s)))
            .collect()
    }

    /// How to bring the line to step `to` (0: the start): redos or undos,
    /// and how many; `None` when `to` is not on the line.
    fn plan_walk(&self, to: u64) -> Option<(bool, usize)> {
        if to == 0 {
            return Some((false, self.undo.len()));
        }
        if let Some(i) = self.undo.iter().position(|s| s.id == to) {
            return Some((false, self.undo.len() - 1 - i));
        }
        self.redo
            .iter()
            .position(|s| s.id == to)
            .map(|i| (true, self.redo.len() - i))
    }

    /// Brings the history to a replaced step `to`: back (or forward) to
    /// the step its line left after, then along that line; the redo list
    /// there becomes a replaced line in its place. All or nothing: a
    /// value held by another origin's gesture refuses it before anything
    /// moves. The steps walked, in order; `None` when `to` is not a
    /// replaced step, or when its line left from a step that is itself
    /// replaced (go to that line first). With `dry_run`, checked and
    /// answered, nothing changed.
    pub fn jump_replaced(
        &mut self,
        to: u64,
        origin: &Origin,
        dry_run: bool,
    ) -> Result<Option<Vec<Step>>, Refused> {
        let Some((b, pos)) = self.replaced.iter().enumerate().find_map(|(b, line)| {
            line.steps
                .iter()
                .position(|s| s.id == to)
                .map(|pos| (b, pos))
        }) else {
            return Ok(None);
        };
        let after = self.replaced[b].after;
        let Some((redo, count)) = self.plan_walk(after) else {
            return Ok(None);
        };
        let back: Vec<Step> = {
            let list = if redo { &self.redo } else { &self.undo };
            list.iter().rev().take(count).cloned().collect()
        };
        let along: Vec<Step> = self.replaced[b].steps[..=pos].to_vec();
        for s in back.iter().chain(&along) {
            for (path, _, _) in &s.changes {
                let current = self
                    .entries
                    .get(path)
                    .map(|e| e.value.clone())
                    .ok_or_else(|| Refused::Unknown { path: path.clone() })?;
                if let Some(by) = self.holder(path, origin, None) {
                    return Err(Refused::Held {
                        current,
                        by: by.clone(),
                    });
                }
            }
        }
        let walked: Vec<Step> = back.into_iter().chain(along).collect();
        if dry_run {
            return Ok(Some(walked));
        }
        for _ in 0..count {
            if redo {
                self.redo(origin)?;
            } else {
                self.undo(origin)?;
            }
        }
        let line = self.replaced.remove(b);
        let old: Vec<Step> = self.redo.drain(..).rev().collect();
        if !old.is_empty() {
            self.replaced.push(Branch { after, steps: old });
        }
        self.redo = line.steps.into_iter().rev().collect();
        self.history_rev += 1;
        for _ in 0..=pos {
            self.redo(origin)?;
        }
        Ok(Some(walked))
    }

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
