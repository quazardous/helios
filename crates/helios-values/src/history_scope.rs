//! The history scopes of the store's steps: the image's and the
//! workspace's.
//!
//! One store holds the open image's values (`node/…`, `bypass/…`, the
//! masks' marks) and the workspace's state (`workspace/…`: its network,
//! tools and layout, edited in the workshops' diagrams). Their steps share
//! one line, in the order they were made, but they do not belong to the
//! same thing: when another image opens, its values take the paths of the
//! one before, so that one's steps must not be undone into them, while a
//! graph edit is the workspace's and stays undoable.
//!
//! [`Store::leave_image`] is what a photo switch does: the image's steps
//! go; the workspace's stay, each without the image values it moved (a
//! graph edit that carried values into a new node), which an undo would
//! otherwise write into the next image.

use crate::points::Branch;
use crate::{Step, Store};

/// The paths of the workspace's history scope.
pub const WORKSPACE_PATHS: &str = "workspace/";

/// The history scope a path's steps belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryScope {
    /// The open image's edit: left when another image opens.
    Image,
    /// The workspace (its network, tools, layout): kept across images.
    Workspace,
}

impl HistoryScope {
    /// The scope of `path`: the workspace's for `workspace/…`, else the
    /// image's.
    pub fn of(path: &str) -> Self {
        if path.starts_with(WORKSPACE_PATHS) {
            Self::Workspace
        } else {
            Self::Image
        }
    }
}

impl Step {
    /// The scope it is in: the workspace's when it changed a workspace
    /// path (a graph edit, even one that moved image values with it), else
    /// the image's.
    pub fn history_scope(&self) -> HistoryScope {
        if self
            .changes
            .iter()
            .any(|(p, _, _)| HistoryScope::of(p) == HistoryScope::Workspace)
        {
            HistoryScope::Workspace
        } else {
            HistoryScope::Image
        }
    }

    /// Keeps only its changes to the workspace's paths; whether any is
    /// left.
    fn keep_workspace(&mut self) -> bool {
        self.changes
            .retain(|(p, _, _)| HistoryScope::of(p) == HistoryScope::Workspace);
        !self.changes.is_empty()
    }
}

impl Store {
    /// The image's history leaves: another image opens, its values sharing
    /// the paths of the one before, which an undo must not reach. The
    /// image's steps go (undo, redo and replaced lines); the workspace's
    /// stay in their order, each without the image values it moved. A
    /// replaced line whose starting step went goes too. Values, gestures
    /// and the committed count stay; the preview ends.
    pub fn leave_image(&mut self) {
        let keep = |steps: &mut Vec<Step>| steps.retain_mut(Step::keep_workspace);
        keep(&mut self.undo);
        keep(&mut self.redo);
        for line in &mut self.replaced {
            keep(&mut line.steps);
        }
        let mut ids: std::collections::BTreeSet<u64> = self
            .undo
            .iter()
            .chain(&self.redo)
            .chain(self.replaced.iter().flat_map(|l| &l.steps))
            .map(|s| s.id)
            .collect();
        ids.insert(0);
        // Lines left from a step that is still there; a line that leaves
        // from another kept line is checked against the ids before it.
        self.replaced
            .retain(|Branch { after, steps }| !steps.is_empty() && ids.contains(after));
        let groups: std::collections::BTreeSet<u64> = self
            .undo
            .iter()
            .chain(&self.redo)
            .filter_map(|s| s.group)
            .collect();
        self.named.retain(|_, g| groups.contains(g));
        self.preview.clear();
        self.history_rev += 1;
        self.history_moved(None);
        self.announce();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Change, Descriptor, Set};
    use helios_core::Value;
    use helios_shell::commands::Origin;

    const STATE: &str = "workspace/state";
    const EV: &str = "node/exposure/exposure";

    fn store() -> Store {
        let mut s = Store::new();
        s.declare(EV, Descriptor::float("Exposure", -3.0, 3.0, 0.0));
        s.declare(STATE, Descriptor::text("workspace", "a"));
        s
    }

    fn set(s: &mut Store, path: &str, v: Value) {
        s.set(Set::new(path, Change::To(v), Origin::User)).unwrap();
    }

    #[test]
    fn a_switch_keeps_the_workspace_s_steps_without_the_image_s_values() {
        let mut s = store();
        set(&mut s, EV, Value::Float(1.0));
        // A graph edit moving an image value with it: one step.
        let g = s.begin_group(Origin::User, "add a node");
        s.set(Set::new(STATE, Change::To(Value::Text("b".into())), Origin::User).gesture(g))
            .unwrap();
        s.set(Set::new(EV, Change::To(Value::Float(2.0)), Origin::User).gesture(g))
            .unwrap();
        s.end(g);
        set(&mut s, EV, Value::Float(2.5));
        assert_eq!(s.undo_steps().len(), 3);
        assert_eq!(s.undo_steps()[1].history_scope(), HistoryScope::Workspace);
        assert_eq!(s.undo_steps()[0].history_scope(), HistoryScope::Image);
        let rev = s.history_rev();
        s.leave_image();
        assert!(s.history_rev() > rev);
        let left = s.undo_steps();
        assert_eq!(left.len(), 1, "only the graph edit");
        assert_eq!(left[0].label, "add a node");
        assert_eq!(left[0].paths(), [STATE], "without the image's value");
        // Undone on the next image: the state back, the image's value
        // untouched.
        s.undo(&Origin::User).unwrap();
        assert_eq!(s.get(STATE).unwrap().value, Value::Text("a".into()));
        assert_eq!(s.get(EV).unwrap().value, Value::Float(2.5));
        assert_eq!(s.undo(&Origin::User).unwrap(), None);
    }

    #[test]
    fn redo_and_replaced_lines_keep_only_the_workspace_s_steps() {
        let mut s = store();
        set(&mut s, STATE, Value::Text("b".into()));
        set(&mut s, EV, Value::Float(1.0));
        s.undo(&Origin::User).unwrap();
        s.undo(&Origin::User).unwrap();
        // A new step replaces the redo list (the state, then the value).
        set(&mut s, EV, Value::Float(0.5));
        set(&mut s, STATE, Value::Text("c".into()));
        s.undo(&Origin::User).unwrap();
        assert_eq!(s.redo_steps().len(), 1);
        s.leave_image();
        assert!(s.undo_steps().is_empty());
        assert_eq!(s.redo_steps().len(), 1, "the state's redo kept");
        // The replaced line left from the start: kept, its state step only.
        let replaced = s.replaced_steps();
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].1.paths(), [STATE]);
    }
}
