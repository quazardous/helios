//! The `controls.*` commands (`egui_hud::commands`) declared in the
//! shell's command registry, on `helios_values::Store`, and what the
//! `controls` collector reads ([`instances`]).

use std::sync::{Arc, Mutex, PoisonError};

use helios_shell::commands::{CommandSpec, Registry as Commands};
use helios_values::Store;
use serde_json::Value as Json;

use egui_hud::commands::{self as hc, Shared};
pub use egui_hud::commands::{COMMANDS, CommandInfo, Contexts, GROUP, SharedContexts};

use crate::card::Card;
use crate::flow::SharedSteps;
use crate::registry::Registry;
use crate::resolve::{CardPlacement, Values};
use crate::values::{StoreMut, StoreRef};

/// The store path a control edits on a node: the parameter its patch
/// names, when the node is of the patch's module.
pub fn path_of(
    store: &Store,
    registry: &Registry,
    node: &str,
    control: &str,
) -> Result<String, String> {
    hc::path_of(&StoreRef(store), registry, node, control)
}

/// A node's card: one item per parameter (keyed by its name) with the
/// control patched to it or its implicit one, the registry with those
/// implicit controls added, and the values.
pub fn node_card(
    store: &Store,
    registry: &Registry,
    node: &str,
) -> Result<(Card, Registry, Values), String> {
    hc::node_card(&StoreRef(store), registry, node)
}

/// Resolves a node's card in the current context, and keeps the result
/// as the node's previous one.
pub fn resolve_node(
    store: &Store,
    registry: &Registry,
    contexts: &mut Contexts,
    node: &str,
) -> Result<CardPlacement, String> {
    hc::resolve_node(&StoreRef(store), registry, contexts, node)
}

/// Every node parameter and the control that edits it: `control` (its
/// id), `implicit` when built from the descriptor, the value and how the
/// control shows it.
pub fn instances(store: &Store, registry: &Registry) -> Json {
    hc::instances(&StoreRef(store), registry)
}

/// Declares the `controls.*` commands, with a context of their own.
pub fn register(
    commands: &mut Commands,
    store: Arc<Mutex<Store>>,
    registry: Arc<Registry>,
    steps: SharedSteps,
) -> Result<(), String> {
    register_with(commands, store, registry, steps, SharedContexts::default())
}

/// Declares the `controls.*` commands over a context the host shares
/// (it sets the box, the hand and the task as they change).
pub fn register_with(
    commands: &mut Commands,
    store: Arc<Mutex<Store>>,
    registry: Arc<Registry>,
    steps: SharedSteps,
    contexts: SharedContexts,
) -> Result<(), String> {
    let shared = Shared {
        registry,
        steps,
        contexts,
    };
    for info in COMMANDS {
        let mut spec = CommandSpec::new(info.name, info.help, GROUP);
        if info.read_only {
            spec = spec.read_only();
        }
        let (store, shared) = (store.clone(), shared.clone());
        commands.register(spec, move |args, origin| {
            let mut s = store.lock().unwrap_or_else(PoisonError::into_inner);
            hc::run(info.name, args, origin, &mut StoreMut(&mut s), &shared)
        })?;
    }
    Ok(())
}
