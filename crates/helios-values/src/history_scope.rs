//! The history scopes of the store's steps: the image's, the
//! workspace's and the plugins', each its own line.
//!
//! One store holds the open image's values (`node/…`, `bypass/…`, the
//! masks' marks) and the workspace's state (`workspace/…`: its network,
//! macros, sub-workspaces, workshop design, name, keys and tools, edited
//! in the workshops' diagrams and the workspace editor). They do not
//! belong to the same thing, so their steps are two lines: each with its
//! own undo, redo and replaced lists, undone and redone apart. A step
//! goes on the workspace's line when it changed a workspace path (a graph
//! edit, even one that moved image values with it), else on the image's.
//!
//! A plugin's own values (`plugin/<id>/<key>`, its declared controls) are
//! not the image's either: a camera's ISO stays when another photo
//! opens. Their steps are a third line, the plugins'.
//!
//! The store's own lists hold the image's line;
//! [`Store::in_history`] walks another's with the same calls (undo,
//! redo, jump, one step alone, a group), the image's parked meanwhile.
//!
//! [`Store::leave_image`] is what a photo switch does: the image's line
//! goes; the workspace's stays, each step without the image values it
//! moved (a graph edit that carried values into a new node), which an
//! undo would otherwise write into the next image. [`Store::park_image`]
//! keeps the image's line aside first, found again when the image opens
//! again ([`Store::resume_image`], in `image_lines`).

use crate::points::Branch;
use crate::{Step, Store};

/// The paths of the workspace's history scope.
pub const WORKSPACE_PATHS: &str = "workspace/";

/// The paths of the plugins' history scope: each plugin's own values.
pub const PLUGIN_PATHS: &str = "plugin/";

/// The history scope a path's steps belong to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HistoryScope {
    /// The open image's edit: left when another image opens.
    #[default]
    Image,
    /// The workspace (its network, tools, layout): kept across images.
    Workspace,
    /// The plugins' own values (`plugin/<id>/<key>`): kept across images.
    Plugin,
}

impl HistoryScope {
    /// Every scope.
    pub const ALL: [Self; 3] = [Self::Image, Self::Workspace, Self::Plugin];

    /// The scope of `path`: the workspace's for `workspace/…`, the
    /// plugins' for `plugin/…`, else the image's.
    pub fn of(path: &str) -> Self {
        if path.starts_with(WORKSPACE_PATHS) {
            Self::Workspace
        } else if path.starts_with(PLUGIN_PATHS) {
            Self::Plugin
        } else {
            Self::Image
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// A scope's line while the store's lists hold another's.
#[derive(Debug, Default)]
pub(crate) struct Parked {
    pub(crate) undo: Vec<Step>,
    pub(crate) redo: Vec<Step>,
    pub(crate) replaced: Vec<Branch>,
}

/// The lines not held, by scope (the one held is left empty).
pub(crate) type Lines = [Parked; 3];

impl Step {
    /// The scope it is in: the workspace's when it changed a workspace
    /// path (a graph edit, even one that moved image values with it), the
    /// plugins' when it changed only plugin values, else the image's.
    pub fn history_scope(&self) -> HistoryScope {
        let of = |s| {
            self.changes
                .iter()
                .any(|(p, _, _)| HistoryScope::of(p) == s)
        };
        if of(HistoryScope::Workspace) {
            HistoryScope::Workspace
        } else if !self.changes.is_empty()
            && self
                .changes
                .iter()
                .all(|(p, _, _)| HistoryScope::of(p) == HistoryScope::Plugin)
        {
            HistoryScope::Plugin
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
    /// Parks the line held and holds `scope`'s.
    fn hold(&mut self, scope: HistoryScope) {
        let held = Parked {
            undo: std::mem::take(&mut self.undo),
            redo: std::mem::take(&mut self.redo),
            replaced: std::mem::take(&mut self.replaced),
        };
        let line = std::mem::take(&mut self.parked[scope.index()]);
        self.parked[self.active.index()] = held;
        self.undo = line.undo;
        self.redo = line.redo;
        self.replaced = line.replaced;
        self.active = scope;
    }

    /// Runs `f` on `scope`'s line: inside it, every history call of the
    /// store ([`Store::undo`], [`Store::redo`], [`Store::undo_steps`],
    /// [`Store::jump_replaced`], [`Store::undo_step`],
    /// [`Store::undo_group`], [`Store::values_at`]…) is that line's. The
    /// image's line is the one held outside it. A step made inside goes
    /// on its own scope's line all the same.
    pub fn in_history<R>(&mut self, scope: HistoryScope, f: impl FnOnce(&mut Self) -> R) -> R {
        let before = self.active;
        if scope == before {
            return f(self);
        }
        self.hold(scope);
        let r = f(self);
        self.hold(before);
        r
    }

    fn line_of(&self, scope: HistoryScope) -> (&[Step], &[Step]) {
        if scope == self.active {
            (&self.undo, &self.redo)
        } else {
            let p = &self.parked[scope.index()];
            (&p.undo, &p.redo)
        }
    }

    /// `scope`'s undo steps, oldest first.
    pub fn undo_steps_of(&self, scope: HistoryScope) -> &[Step] {
        self.line_of(scope).0
    }

    /// `scope`'s redo steps, oldest first: the next redo is the last.
    pub fn redo_steps_of(&self, scope: HistoryScope) -> &[Step] {
        self.line_of(scope).1
    }

    /// The undo steps of every scope, in the order they were made.
    pub fn all_undo_steps(&self) -> Vec<&Step> {
        let mut all: Vec<&Step> = self
            .undo
            .iter()
            .chain(self.parked.iter().flat_map(|p| &p.undo))
            .collect();
        all.sort_by_key(|s| s.id);
        all
    }

    /// Undoes the newest step of any scope (the one made last, by id),
    /// as the one line of all did: what a host with no view to route
    /// Ctrl+Z by (a test, a tool) walks. Its label, `None` when all are
    /// empty.
    pub fn undo_newest(
        &mut self,
        origin: &crate::Origin,
    ) -> Result<Option<String>, crate::Refused> {
        let head = |h| self.undo_steps_of(h).last().map(|s: &Step| s.id);
        let scope = HistoryScope::ALL
            .into_iter()
            .max_by_key(|h| head(*h))
            .unwrap_or_default();
        self.in_history(scope, |s| s.undo(origin))
    }

    /// The image's history leaves: another image opens, its values sharing
    /// the paths of the one before, which an undo must not reach. The
    /// image's line goes (undo, redo and replaced); the workspace's stays,
    /// each step without the image values it moved, and the plugins'
    /// stays whole. Values, gestures and
    /// the committed count stay; the preview ends.
    pub fn leave_image(&mut self) {
        self.in_history(HistoryScope::Image, |s| {
            s.undo.clear();
            s.redo.clear();
            s.replaced.clear();
        });
        self.in_history(HistoryScope::Workspace, |s| {
            let keep = |steps: &mut Vec<Step>| steps.retain_mut(Step::keep_workspace);
            keep(&mut s.undo);
            keep(&mut s.redo);
            for line in &mut s.replaced {
                keep(&mut line.steps);
            }
            let mut ids: std::collections::BTreeSet<u64> = s
                .undo
                .iter()
                .chain(&s.redo)
                .chain(s.replaced.iter().flat_map(|l| &l.steps))
                .map(|st| st.id)
                .collect();
            ids.insert(0);
            // Lines left from a step that is still there.
            s.replaced
                .retain(|Branch { after, steps }| !steps.is_empty() && ids.contains(after));
        });
        let mut groups: std::collections::BTreeSet<u64> = self
            .undo
            .iter()
            .chain(&self.redo)
            .chain(
                self.parked
                    .iter()
                    .flat_map(|p| p.undo.iter().chain(&p.redo)),
            )
            .filter_map(|s| s.group)
            .collect();
        groups.extend(self.images.groups());
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
    fn a_graph_edit_is_on_the_workspace_s_line_and_undone_apart() {
        let mut s = store();
        set(&mut s, STATE, Value::Text("b".into()));
        set(&mut s, EV, Value::Float(1.0));
        assert_eq!(s.undo_steps().len(), 1, "the image's line: the exposure");
        assert_eq!(s.undo_steps_of(HistoryScope::Workspace).len(), 1);
        assert_eq!(s.all_undo_steps().len(), 2);
        // The workspace's undo takes the graph edit back, under the
        // exposure made after it, which stays.
        s.in_history(HistoryScope::Workspace, |s| s.undo(&Origin::User))
            .unwrap();
        assert_eq!(s.get(STATE).unwrap().value, Value::Text("a".into()));
        assert_eq!(s.get(EV).unwrap().value, Value::Float(1.0));
        // A new exposure keeps the workspace's redo.
        set(&mut s, EV, Value::Float(2.0));
        assert_eq!(s.redo_steps_of(HistoryScope::Workspace).len(), 1);
        // The image's undo takes the exposure back, the state untouched.
        s.undo(&Origin::User).unwrap();
        assert_eq!(s.get(EV).unwrap().value, Value::Float(1.0));
        assert_eq!(s.get(STATE).unwrap().value, Value::Text("a".into()));
        s.in_history(HistoryScope::Workspace, |s| s.redo(&Origin::User))
            .unwrap();
        assert_eq!(s.get(STATE).unwrap().value, Value::Text("b".into()));
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
        assert_eq!(s.undo_steps().len(), 2, "the image's two exposures");
        let ws = s.undo_steps_of(HistoryScope::Workspace);
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].history_scope(), HistoryScope::Workspace);
        assert_eq!(s.undo_steps()[0].history_scope(), HistoryScope::Image);
        let rev = s.history_rev();
        s.leave_image();
        assert!(s.history_rev() > rev);
        assert!(s.undo_steps().is_empty(), "the image's line went");
        let left = s.undo_steps_of(HistoryScope::Workspace);
        assert_eq!(left.len(), 1, "only the graph edit");
        assert_eq!(left[0].label, "add a node");
        assert_eq!(left[0].paths(), [STATE], "without the image's value");
        // Undone on the next image: the state back, the image's value
        // untouched.
        s.in_history(HistoryScope::Workspace, |s| s.undo(&Origin::User))
            .unwrap();
        assert_eq!(s.get(STATE).unwrap().value, Value::Text("a".into()));
        assert_eq!(s.get(EV).unwrap().value, Value::Float(2.5));
        assert_eq!(
            s.in_history(HistoryScope::Workspace, |s| s.undo(&Origin::User))
                .unwrap(),
            None
        );
    }

    #[test]
    fn redo_and_replaced_lines_keep_only_the_workspace_s_steps() {
        let mut s = store();
        set(&mut s, STATE, Value::Text("b".into()));
        set(&mut s, EV, Value::Float(1.0));
        s.undo(&Origin::User).unwrap();
        s.in_history(HistoryScope::Workspace, |s| s.undo(&Origin::User))
            .unwrap();
        // New steps replace each line's redo list.
        set(&mut s, EV, Value::Float(0.5));
        set(&mut s, STATE, Value::Text("c".into()));
        s.in_history(HistoryScope::Workspace, |s| s.undo(&Origin::User))
            .unwrap();
        assert_eq!(s.redo_steps_of(HistoryScope::Workspace).len(), 1);
        s.leave_image();
        assert!(s.undo_steps().is_empty());
        assert!(
            s.replaced_steps().is_empty(),
            "the image's replaced line went"
        );
        s.in_history(HistoryScope::Workspace, |s| {
            assert!(s.undo_steps().is_empty());
            assert_eq!(s.redo_steps().len(), 1, "the state's redo kept");
            // The replaced line left from the start: kept.
            let replaced = s.replaced_steps();
            assert_eq!(replaced.len(), 1);
            assert_eq!(replaced[0].1.paths(), [STATE]);
        });
    }

    #[test]
    fn a_plugin_s_values_are_a_line_of_their_own_kept_across_images() {
        const ISO: &str = "plugin/org.example.camera/iso";
        let mut s = store();
        s.declare(ISO, Descriptor::float("ISO", 100.0, 6400.0, 400.0));
        set(&mut s, EV, Value::Float(1.0));
        set(&mut s, ISO, Value::Float(800.0));
        set(&mut s, STATE, Value::Text("b".into()));
        assert_eq!(s.undo_steps_of(HistoryScope::Plugin).len(), 1);
        assert_eq!(s.undo_steps().len(), 1, "the image's line: the exposure");
        assert_eq!(s.all_undo_steps().len(), 3);
        s.leave_image();
        assert!(s.undo_steps().is_empty());
        assert_eq!(s.undo_steps_of(HistoryScope::Plugin).len(), 1, "kept");
        // The newest is the workspace's, then the plugin's.
        s.undo_newest(&Origin::User).unwrap();
        assert_eq!(s.get(STATE).unwrap().value, Value::Text("a".into()));
        s.undo_newest(&Origin::User).unwrap();
        assert_eq!(s.get(ISO).unwrap().value, Value::Float(400.0));
        assert_eq!(s.redo_steps_of(HistoryScope::Plugin).len(), 1);
        s.in_history(HistoryScope::Plugin, |s| s.redo(&Origin::User))
            .unwrap();
        assert_eq!(s.get(ISO).unwrap().value, Value::Float(800.0));
    }
}
