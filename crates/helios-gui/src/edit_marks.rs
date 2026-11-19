//! What the open photo's edit changed from its defaults, and the resets.
//!
//! - **The marks.** [`Marks`] holds the modules whose parameters differ
//!   from their defaults and the modules bypassed. A module with a
//!   changed parameter carries a mark in the Modules panel and on its
//!   header in the Parameters panel; a photo with any of them is
//!   "edited", said by the darkroom's status bar (`helios.status.edit`).
//!   A parameter's own mark (a dot that resets) is the controls layer's
//!   (`crate::params`).
//! - **Said when they change.** The value store is the edit's one
//!   writer; each time it is written (a drag's frames, a step, an undo, a
//!   redo) or its history moves (another photo opened), the shell reads
//!   the marks again and, when they changed,
//!   passes them through the hook point [`EditChanged`] (`edit.changed`)
//!   on the application's bus, its listeners in the order [`ORDER`]
//!   declares, then says `edit.changed` on the journal with the marks and
//!   the listeners that ran. A frame with no write reads nothing.
//!   The listeners need the desktop's own state, which is not
//!   `Send + Sync` as a bus listener is: each marks its place, and the
//!   one passing the point does its part right after (the pattern of
//!   `crate::library_changed`; local listeners on the bus would let them
//!   do it themselves).
//! - **The resets.** [`reset_node`] (`node.reset`) puts every parameter
//!   of a module back to its default, its bypass kept apart;
//!   [`reset_all`] (`edit.resetAll`) puts every module's parameters back
//!   and brings back the modules bypassed: the photo as it was before it
//!   was edited. Each is one undo step, and does nothing when there is
//!   nothing to reset.

use std::collections::BTreeSet;

use egui_pop::api::bus::bus_contract::{
    self, Bus, HookError, ListenOpts, Point, PointDecl, Refused,
};
use helios_shell::commands::Origin;
use helios_values::{BYPASS, Change, Set, Store};
use serde_json::{Value as Json, json};

/// The hook point a change of the edit's marks passes through, and the
/// journal's event once it did.
pub const CHANGED: &str = "helios.edit.changed";
/// The owner of the desktop's listeners.
pub const BUS_OWNER: &str = "helios.edit";
/// The marks shown: the Modules panel's rows, the Parameters panel's
/// headers, the darkroom's status bar.
pub const MARKS: &str = "helios.edit.marks";

/// The listeners of [`EditChanged`], each with the one it runs after.
pub const ORDER: &[(&str, Option<&str>)] = &[(MARKS, None)];

/// Resets a module's parameters to their defaults.
pub const RESET_NODE: &str = "node.reset";
/// Resets the whole edit: the photo back to its original.
pub const RESET_ALL: &str = "edit.resetAll";

/// What the edit changed from its defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    /// The modules with a parameter that differs from its default.
    pub nodes: BTreeSet<String>,
    /// The modules bypassed.
    pub bypassed: BTreeSet<String>,
}

impl Marks {
    /// The marks of the values `store` holds: its modules' parameters
    /// (`node/<node>/<param>`, the transient ones left out) against their
    /// defaults, and its bypasses.
    pub fn read(store: &Store) -> Marks {
        let mut marks = Marks::default();
        for (path, entry) in store.list("node/") {
            if entry.descriptor.transient || entry.value == entry.descriptor.default {
                continue;
            }
            if let Some((node, _)) = path
                .strip_prefix("node/")
                .and_then(|rest| rest.rsplit_once('/'))
            {
                marks.nodes.insert(node.to_string());
            }
        }
        for (path, entry) in store.list(BYPASS) {
            if entry.value.as_bool() == Some(true)
                && let Some(node) = path.strip_prefix(BYPASS)
            {
                marks.bypassed.insert(node.to_string());
            }
        }
        marks
    }

    /// Whether a parameter of `node` differs from its default.
    pub fn changed(&self, node: &str) -> bool {
        self.nodes.contains(node)
    }

    /// Whether the photo is edited: a parameter changed or a module
    /// bypassed.
    pub fn edited(&self) -> bool {
        !self.nodes.is_empty() || !self.bypassed.is_empty()
    }

    /// As the journal says it.
    pub fn to_json(&self) -> Json {
        json!({
            "edited": self.edited(),
            "nodes": self.nodes,
            "bypassed": self.bypassed,
        })
    }
}

/// What the point's listeners did, in order.
#[derive(Clone, Debug, Default)]
pub struct Reactions {
    /// The listeners that ran, in their order.
    pub ran: Vec<String>,
}

/// The hook point a change of the edit's marks passes through.
pub struct EditChanged;

impl Point for EditChanged {
    const ID: &'static str = CHANGED;
    type Input = Marks;
    type Value = Reactions;
}

/// The edit's point, this crate's contract with the application's bus.
pub const POINTS: &[PointDecl] = &[PointDecl::of::<EditChanged>(
    "The edit's marks changed: what shows them follows",
)];

/// Declares [`POINTS`] on `bus` (once).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "edit: {refused}");
        }
    }
}

/// Registers the listener `id` of [`EditChanged`] (one of [`ORDER`]'s)
/// on `bus`, in its declared place: it marks its place; its part is
/// done by the one passing the point. One there already under `id` is
/// replaced.
///
/// # Errors
/// `id` is not in [`ORDER`], or the bus refused it.
pub fn listen(bus: &dyn Bus, id: &'static str) -> Result<(), String> {
    let Some((_, after)) = ORDER.iter().find(|(listener, _)| *listener == id) else {
        return Err(format!("`{id}` is not a listener of {CHANGED}"));
    };
    declare(bus);
    bus.unlisten(CHANGED, id);
    let opts = ListenOpts::new().owner(BUS_OWNER);
    let opts = match after {
        Some(after) => opts.after(*after),
        None => opts,
    };
    bus.listen::<EditChanged>(id, opts, move |_, reactions| {
        reactions.ran.push(id.to_owned());
        Ok::<(), HookError>(())
    })
    .map_err(|e| e.to_string())
}

/// Passes `marks` through [`EditChanged`] on `bus`: the listeners that
/// ran, in order.
pub fn pass(bus: &dyn Bus, marks: Marks) -> Vec<String> {
    let altered = bus.alter::<EditChanged>(marks, Reactions::default());
    for failure in &altered.failures {
        log::warn!(target: "helios", "{CHANGED}: {failure}");
    }
    altered.value.ran
}

/// Puts the paths `paths` back to their defaults as one undo step
/// labelled `label`; returns how many changed.
fn reset_paths(
    store: &mut Store,
    paths: &[String],
    label: &str,
    origin: &Origin,
) -> Result<usize, String> {
    if paths.is_empty() {
        return Ok(0);
    }
    let group = store.begin_group(origin.clone(), label);
    let mut refused = None;
    for path in paths {
        if let Err(e) = store.set(Set::new(path, Change::Reset, origin.clone()).gesture(group)) {
            refused = Some(e.to_string());
            break;
        }
    }
    if refused.is_some() {
        // Nothing half done: what was reset is put back.
        store.cancel(group);
    } else {
        store.end(group);
    }
    refused.map_or(Ok(paths.len()), Err)
}

/// The paths under `prefix` whose value differs from its default, in
/// the store's scope.
fn changed_paths(
    store: &Store,
    prefix: &str,
    node_of: impl Fn(&str) -> Option<&str>,
) -> Vec<String> {
    store
        .list(prefix)
        .into_iter()
        .filter(|(_, e)| !e.descriptor.transient && e.value != e.descriptor.default)
        .filter(|(p, _)| node_of(p).is_some_and(|n| store.in_scope(n)))
        .map(|(p, _)| p.to_string())
        .collect()
}

fn param_node(path: &str) -> Option<&str> {
    path.strip_prefix("node/")
        .and_then(|rest| rest.rsplit_once('/'))
        .map(|(node, _)| node)
}

/// Resets every parameter of `node` (the selected module by default) to
/// its default, its bypass kept: one undo step, "reset \<node\>".
/// Returns the node and how many parameters changed.
///
/// # Errors
/// No module is selected, `node` has no parameter, or the store refused
/// a write (nothing is then changed).
pub fn reset_node(
    store: &mut Store,
    node: Option<&str>,
    origin: &Origin,
) -> Result<(String, usize), String> {
    let node = match node {
        Some(n) => n.to_string(),
        None => crate::modules::selected(store).ok_or("no module is selected")?,
    };
    let prefix = format!("node/{node}/");
    if store.list(&prefix).is_empty() {
        return Err(format!("`{node}` is not a module with parameters"));
    }
    let paths: Vec<String> = changed_paths(store, &prefix, param_node)
        .into_iter()
        .filter(|p| param_node(p) == Some(node.as_str()))
        .collect();
    let n = reset_paths(store, &paths, &format!("reset {node}"), origin)?;
    Ok((node, n))
}

/// Resets the whole edit: every module's parameters to their defaults
/// and the modules bypassed brought back, one undo step, "reset all".
/// Inside a sub-workspace, its modules only. Returns how many values
/// changed.
///
/// # Errors
/// The store refused a write (nothing is then changed).
pub fn reset_all(store: &mut Store, origin: &Origin) -> Result<usize, String> {
    let mut paths = changed_paths(store, "node/", param_node);
    paths.extend(changed_paths(store, BYPASS, |p| p.strip_prefix(BYPASS)));
    reset_paths(store, &paths, "reset all", origin)
}

/// A boolean's value, for the tests.
#[cfg(test)]
fn bool_value(store: &Store, path: &str) -> Option<bool> {
    store.get(path).and_then(|e| e.value.as_bool())
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::Value;
    use helios_values::Descriptor;

    fn store() -> Store {
        let mut s = Store::new();
        s.declare(
            "node/exposure/exposure",
            Descriptor::float("Exposure", -3.0, 3.0, 0.0),
        );
        s.declare(
            "node/exposure/black",
            Descriptor::float("Black", -1.0, 1.0, 0.0),
        );
        s.declare(
            "node/tone/contrast",
            Descriptor::float("Contrast", 0.0, 2.0, 1.0),
        );
        s.declare("bypass/tone", Descriptor::bool("Bypass", false));
        s
    }

    fn set(s: &mut Store, path: &str, v: Value) {
        s.set(Set::new(path, Change::To(v), Origin::User)).unwrap();
    }

    #[test]
    fn the_marks_follow_the_values_against_their_defaults() {
        let mut s = store();
        assert_eq!(Marks::read(&s), Marks::default());
        assert!(!Marks::read(&s).edited());
        set(&mut s, "node/exposure/exposure", Value::Float(1.0));
        let m = Marks::read(&s);
        assert!(m.changed("exposure") && !m.changed("tone") && m.edited());
        set(&mut s, "bypass/tone", Value::Bool(true));
        let m = Marks::read(&s);
        assert!(!m.changed("tone"), "a bypass is not a changed parameter");
        assert!(m.bypassed.contains("tone"));
    }

    #[test]
    fn a_module_reset_is_one_step_and_keeps_its_bypass() {
        let mut s = store();
        set(&mut s, "node/exposure/exposure", Value::Float(1.0));
        set(&mut s, "node/exposure/black", Value::Float(0.5));
        set(&mut s, "node/tone/contrast", Value::Float(1.5));
        let steps = s.undo_steps().len();
        let (node, n) = reset_node(&mut s, Some("exposure"), &Origin::User).unwrap();
        assert_eq!((node.as_str(), n), ("exposure", 2));
        assert_eq!(s.undo_steps().len(), steps + 1);
        assert!(!Marks::read(&s).changed("exposure"));
        assert!(Marks::read(&s).changed("tone"));
        // Nothing more to reset: no step.
        assert_eq!(
            reset_node(&mut s, Some("exposure"), &Origin::User)
                .unwrap()
                .1,
            0
        );
        assert_eq!(s.undo_steps().len(), steps + 1);
        // One undo brings both back.
        s.undo(&Origin::User).unwrap();
        assert!(Marks::read(&s).changed("exposure"));
        assert!(reset_node(&mut s, Some("nobody"), &Origin::User).is_err());
    }

    #[test]
    fn reset_all_is_one_step_and_brings_back_the_modules_bypassed() {
        let mut s = store();
        set(&mut s, "node/exposure/exposure", Value::Float(1.0));
        set(&mut s, "node/tone/contrast", Value::Float(1.5));
        set(&mut s, "bypass/tone", Value::Bool(true));
        let steps = s.undo_steps().len();
        assert_eq!(reset_all(&mut s, &Origin::User).unwrap(), 3);
        assert_eq!(s.undo_steps().len(), steps + 1);
        assert!(!Marks::read(&s).edited());
        assert_eq!(bool_value(&s, "bypass/tone"), Some(false));
        s.undo(&Origin::User).unwrap();
        let m = Marks::read(&s);
        assert!(m.changed("exposure") && m.changed("tone") && m.bypassed.contains("tone"));
    }

    #[test]
    fn listeners_run_in_the_declared_order() {
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::new();
        listen(&bus, MARKS).unwrap();
        assert_eq!(pass(&bus, Marks::default()), [MARKS]);
        assert!(listen(&bus, "nobody.knows").is_err());
    }
}
