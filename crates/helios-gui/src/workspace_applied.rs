//! A workspace's state applied, said once on the bus.
//!
//! Whatever puts a workspace's state in place (it opened; an edit, a
//! reset or an agent's step; an undo or a redo of one; its files changed
//! outside and taken in) passes once through a hook point:
//! [`Applied`] (`workspace.applied`), with what changed ([`Changed`]).
//! The shell has applied it first (its tools, library, layout and name
//! in place, its files written). Its listeners then run in the order
//! [`ORDER`] declares, each only when a part it reads changed:
//!
//! 1. `workspace.keys` (tools): egui-pop's workspace keymap layer is
//!    remembered as the workspace's keys loaded it, so the shortcuts
//!    editor's follower takes no step for keys an undo put back.
//! 2. `contexts.subworkspaces` (sub-workspaces): the list of places the
//!    workspace offers, read again; a sub-workspace shown that is gone
//!    is left, with a toast.
//! 3. `contexts.scope` (sub-workspaces): the value store's scope follows
//!    the sub-workspace shown as the list now has it. After the list.
//! 4. `graph.template` (network, sub-workspaces, composites): the graph
//!    editor's network, positions, groups and Add menu.
//! 5. `workshops.composites` (network, composites): the Workshops
//!    panel's workshops and the nodes that run them.
//! 6. `library.heads` (nodes): the Compare panel's renders, made through
//!    the old network, made again.
//! 7. `darkroom.network` (nodes): the host's; the image shown goes
//!    through the new network in place, or is opened again.
//!
//! The pass is then said on the journal under the point's name with
//! what changed (`changed`) and the listeners that ran (`ran`). The
//! listeners hold the interface's state (`Rc`): they are local
//! listeners, bound to the interface's thread, where the shell passes
//! the point.

use std::rc::Rc;
use std::sync::Arc;

use egui_pop::api::bus::bus_contract::{
    self, Bus, HookError, ListenOpts, Point, PointDecl, Refused,
};
use helios_template::workspace::State;
use serde_json::Value as Json;

/// The hook point a workspace's state applied passes through, and the
/// journal's event once it did.
pub const APPLIED: &str = "workspace.applied";

/// egui-pop's workspace keymap layer, as the workspace's keys loaded it.
pub const KEYS: &str = "workspace.keys";
/// The workspace's sub-workspaces, the places it offers.
pub const SUBWORKSPACES: &str = "contexts.subworkspaces";
/// The value store's scope, on the sub-workspace shown.
pub const SCOPE: &str = "contexts.scope";
/// The graph editor's network, groups and Add menu.
pub const GRAPH: &str = "graph.template";
/// The Workshops panel's workshops and their nodes.
pub const WORKSHOPS: &str = "workshops.composites";
/// The library's head renders (the Compare panel's), made again.
pub const HEADS: &str = "library.heads";
/// The image shown, through the new network: the host's listener
/// ([`listen_network`]).
pub const NETWORK: &str = "darkroom.network";

/// Whether a listener reads a part that changed.
type Wants = fn(&Changed) -> bool;

/// The listeners: each with its owner, the one it runs after and the
/// parts it reads; the order they run in.
pub const ORDER: &[(&str, &str, Option<&str>, Wants)] = &[
    (KEYS, "helios.workspace", None, |c| c.tools),
    (SUBWORKSPACES, "helios.contexts", Some(KEYS), |c| {
        c.subworkspaces
    }),
    (SCOPE, "helios.contexts", Some(SUBWORKSPACES), |c| {
        c.subworkspaces
    }),
    (GRAPH, "helios.graph", Some(SCOPE), |c| {
        c.network || c.subworkspaces || c.composites
    }),
    (WORKSHOPS, "helios.workshops", Some(GRAPH), |c| {
        c.network || c.composites
    }),
    (HEADS, "helios.library", Some(WORKSHOPS), |c| c.nodes),
    (NETWORK, "helios.darkroom", Some(HEADS), |c| c.nodes),
];

/// What changed in the state applied: everything when it opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Changed {
    /// Its tools: keys, toolboxes.
    pub tools: bool,
    /// Its library settings: import, sort, views.
    pub library: bool,
    /// Its network, the nodes' positions, its parameters.
    pub network: bool,
    /// What an image goes through: its network's nodes and links, or its
    /// composites (not the nodes' positions nor its parameters).
    pub nodes: bool,
    /// Its sub-workspaces.
    pub subworkspaces: bool,
    /// Its own composites (its macros and workshops).
    pub composites: bool,
    /// Its layout.
    pub layout: bool,
    /// Its name.
    pub name: bool,
}

impl Changed {
    /// Everything: a workspace opened.
    pub const ALL: Changed = Changed {
        tools: true,
        library: true,
        network: true,
        nodes: true,
        subworkspaces: true,
        composites: true,
        layout: true,
        name: true,
    };

    /// What differs from `before` to `next`.
    pub fn between(before: &State, next: &State) -> Changed {
        let (b, n) = (&before.bundle.template, &next.bundle.template);
        Changed {
            tools: b.tools != n.tools,
            library: b.library != n.library,
            network: b.network != n.network
                || b.positions != n.positions
                || b.parameters != n.parameters,
            nodes: b.network != n.network || before.bundle.composites != next.bundle.composites,
            subworkspaces: b.subworkspaces != n.subworkspaces
                || b.retired_subworkspaces != n.retired_subworkspaces,
            composites: before.bundle.composites != next.bundle.composites,
            layout: before.bundle.layout != next.bundle.layout,
            name: before.meta.name != next.meta.name,
        }
    }

    /// The parts that changed, by name, as the journal says them.
    pub fn names(self) -> Vec<&'static str> {
        [
            (self.tools, "tools"),
            (self.library, "library"),
            (self.network, "network"),
            (self.nodes, "nodes"),
            (self.subworkspaces, "subworkspaces"),
            (self.composites, "composites"),
            (self.layout, "layout"),
            (self.name, "name"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect()
    }
}

/// The state applied, as the listeners read it.
#[derive(Clone, Debug, Default)]
pub struct Workspace {
    /// What changed.
    pub changed: Changed,
    /// The state as its JSON (`State::to_json`): `template`,
    /// `composites`, ….
    pub json: Arc<Json>,
}

/// What the point's listeners did, in order.
#[derive(Clone, Debug, Default)]
pub struct Reactions {
    /// The listeners that ran, in their order.
    pub ran: Vec<String>,
    /// What failed.
    pub failed: Vec<String>,
}

/// The hook point a workspace's state applied passes through.
pub struct Applied;

impl Point for Applied {
    const ID: &'static str = APPLIED;
    type Input = Workspace;
    type Value = Reactions;
}

/// The workspace point, this crate's contract with the application's
/// bus: declared by [`declare`].
pub const POINTS: &[PointDecl] = &[PointDecl::of::<Applied>(
    "A workspace's state applied (opened, edited, undone, taken in from its files): who reads it follows",
)
.budget(crate::INTERFACE_PASS_BUDGET)];

/// Declares [`POINTS`] on `bus` (once; a second desktop on the same bus
/// finds them there).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "workspace: {refused}");
        }
    }
}

/// Registers `run` as the listener `id` (one of [`ORDER`]'s) on `bus`,
/// in its declared place: a local listener bound to the calling thread
/// (the interface's), run only when a part it reads changed. One there
/// already under `id` is replaced.
///
/// # Errors
/// `id` is not in [`ORDER`], or the bus refused it.
pub fn listen_local(
    bus: &dyn Bus,
    id: &'static str,
    run: impl Fn(&Workspace, &mut Reactions) + 'static,
) -> Result<(), String> {
    let Some(&(_, owner, after, wants)) = ORDER.iter().find(|(listener, ..)| *listener == id)
    else {
        return Err(format!("`{id}` is not a listener of {APPLIED}"));
    };
    declare(bus);
    let opts = ListenOpts::new().owner(owner);
    let opts = match after {
        Some(after) => opts.after(after),
        None => opts,
    };
    let run = Rc::new(run);
    bus.unlisten(APPLIED, id);
    bus.listen_local::<Applied>(id, opts, move |workspace, reactions| {
        if wants(&workspace.changed) {
            run(workspace, reactions);
            reactions.ran.push(id.to_owned());
        }
        Ok::<(), HookError>(())
    })
    .map_err(|e| e.to_string())
}

/// Registers `run` as the host's listener of a network changed
/// ([`NETWORK`]): run once per pass whose nodes changed, after the
/// library's renders. One there already is replaced.
///
/// # Errors
/// The bus refused it.
pub fn listen_network(bus: &dyn Bus, run: impl Fn() + 'static) -> Result<(), String> {
    listen_local(bus, NETWORK, move |_, _| run())
}

/// The listener of egui-pop's `layout.changed` keeping the copy of the
/// layout with the workspace's state, after egui-pop's own save
/// (`layout.save`): egui-pop owns the layout's document and saves it,
/// Helios only keeps its copy beside the workspace's files.
pub const LAYOUT_COPY: &str = "helios.layout-copy";

/// Registers `run` as [`LAYOUT_COPY`] on `bus`, after egui-pop's save:
/// a local listener bound to the calling thread (the interface's). One
/// there already is replaced.
///
/// # Errors
/// The bus refused it.
pub fn listen_layout_copy(bus: &dyn Bus, run: impl Fn() + 'static) -> Result<(), String> {
    use egui_pop::api::bus::{LAYOUT_SAVE, LayoutChanged};
    bus.unlisten(LayoutChanged::ID, LAYOUT_COPY);
    bus.listen_local::<LayoutChanged>(
        LAYOUT_COPY,
        ListenOpts::new()
            .owner("helios.workspace")
            .after(LAYOUT_SAVE),
        move |_, ()| {
            run();
            Ok::<(), HookError>(())
        },
    )
    .map_err(|e| e.to_string())
}

/// The order `layout.changed`'s listeners run in on `bus`: egui-pop's
/// save, then the workspace's copy.
pub fn layout_order(bus: &dyn Bus) -> String {
    bus_contract::order_dump(bus, &[egui_pop::api::bus::LayoutChanged::ID])
}

/// Passes `workspace` through the point on `bus`: what the listeners
/// did, in order.
pub fn pass(bus: &dyn Bus, workspace: Workspace) -> Reactions {
    let altered = bus.alter::<Applied>(workspace, Reactions::default());
    let mut reactions = altered.value;
    for failure in &altered.failures {
        log::warn!(target: "helios", "{APPLIED}: {failure}");
        reactions.failed.push(failure.to_string());
    }
    reactions
}

/// The order the point's listeners run in on `bus`.
pub fn order(bus: &dyn Bus) -> String {
    bus_contract::order_dump(bus, &[APPLIED])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kernel() -> egui_pop::api::bus::egui_pop_hooks::Kernel {
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::new();
        for (id, ..) in ORDER.iter().rev() {
            listen_local(&bus, id, |_, _| {}).unwrap();
        }
        bus
    }

    fn ran(bus: &dyn Bus, changed: Changed) -> Vec<String> {
        pass(
            bus,
            Workspace {
                changed,
                json: Arc::default(),
            },
        )
        .ran
    }

    #[test]
    fn listeners_run_in_the_declared_order() {
        let bus = kernel();
        let ids: Vec<&str> = ORDER.iter().map(|(id, ..)| *id).collect();
        assert_eq!(ran(&bus, Changed::ALL), ids);
        assert!(listen_local(&bus, "cards.toast", |_, _| {}).is_err());
    }

    /// A listener runs only for the parts it reads.
    #[test]
    fn listeners_run_for_the_parts_they_read() {
        let bus = kernel();
        let only = |c: Changed| ran(&bus, c);
        assert!(
            only(Changed {
                name: true,
                layout: true,
                library: true,
                ..Changed::default()
            })
            .is_empty()
        );
        assert_eq!(
            only(Changed {
                tools: true,
                ..Changed::default()
            }),
            [KEYS]
        );
        assert_eq!(
            only(Changed {
                network: true,
                ..Changed::default()
            }),
            [GRAPH, WORKSHOPS]
        );
        assert_eq!(
            only(Changed {
                nodes: true,
                ..Changed::default()
            }),
            [HEADS, NETWORK]
        );
        assert_eq!(
            only(Changed {
                subworkspaces: true,
                ..Changed::default()
            }),
            [SUBWORKSPACES, SCOPE, GRAPH]
        );
        assert_eq!(Changed::ALL.names().len(), 8);
    }
}
