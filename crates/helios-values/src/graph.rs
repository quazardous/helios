//! A graph's module parameters as values: `node/<node>/<param>`, and
//! whether a module node is bypassed: `bypass/<node>`. The store is where
//! they are edited (an undo step like any value); the graph keeps
//! rendering from its own copy, which [`sync`] brings up to date.

use helios_core::{Graph, ModuleKind, NodeId, Value};

use crate::{Descriptor, Store};

pub fn node_path(node: &str, param: &str) -> String {
    format!("node/{node}/{param}")
}

/// Where a module node's bypass is kept: true while it is bypassed.
pub fn bypass_path(node: &str) -> String {
    format!("{BYPASS}{node}")
}

/// The prefix of the bypass paths.
pub const BYPASS: &str = "bypass/";

/// Declares the parameters of a module node at their current values, with
/// descriptors from its manifest and hints. A decision node is answered,
/// not edited: it declares its question only (what it is asked, see
/// `helios_core::Module::question`), whose change makes it pending again.
/// Declares its bypass too (`bypass/<node>`), when it has an input to
/// pass through. Returns how many parameters were declared.
pub fn bind_node(store: &mut Store, graph: &Graph, id: NodeId) -> Result<usize, String> {
    let e = |e: helios_core::Error| e.to_string();
    let Some(manifest) = graph.manifest(id).map_err(e)? else {
        return Ok(0);
    };
    // Its bypass, when it has an input to pass through.
    if graph.can_bypass(id).map_err(e)? {
        let name = graph.name(id).map_err(e)?;
        store.declare_with(
            &bypass_path(name),
            Descriptor::bool(&format!("bypass {name}"), false),
            Value::Bool(graph.is_bypassed(id).map_err(e)?),
        );
    }
    let question = if manifest.kind == ModuleKind::Decision {
        let q = graph.question(id).map_err(e)?;
        if q.iter().next().is_none() {
            return Ok(0);
        }
        Some(q)
    } else {
        None
    };
    let name = graph.name(id).map_err(e)?.to_string();
    let params = graph.params(id).map_err(e)?.unwrap_or_default();
    let hints = graph.param_hints(id).map_err(e)?;
    let mut declared = 0;
    for spec in &manifest.params {
        if question
            .as_ref()
            .is_some_and(|q| q.get(&spec.name).is_none())
        {
            continue;
        }
        let hint = hints.iter().find(|h| h.name == spec.name);
        let value = params
            .get(&spec.name)
            .cloned()
            .unwrap_or_else(|| spec.kind.default_value());
        let mut descriptor = Descriptor::from_spec(spec, hint);
        descriptor.module = Some(manifest.name.clone());
        store.declare_with(&node_path(&name, &spec.name), descriptor, value);
        declared += 1;
    }
    Ok(declared)
}

/// Brings the graph's parameters and bypasses up to the store's; returns
/// how many were set. A path of a node the graph does not have, or of a
/// parameter its node no longer has, is left: it stays in the store for
/// an undo that brings the node back (see [`rebind`]).
pub fn sync(store: &mut Store, graph: &mut Graph) -> Result<usize, String> {
    let mut n = 0;
    for path in store.take_dirty() {
        if let Some(node) = path.strip_prefix(BYPASS) {
            let Some(id) = graph.find(node) else {
                continue;
            };
            let on = store.get(&path).and_then(|e| e.value.as_bool()) == Some(true);
            graph
                .set_bypass(id, on)
                .map_err(|e| format!("`{path}`: {e}"))?;
            n += 1;
            continue;
        }
        let Some((node, param)) = path
            .strip_prefix("node/")
            .and_then(|rest| rest.rsplit_once('/'))
        else {
            continue;
        };
        let Some(id) = graph.find(node) else {
            continue;
        };
        if !has_param(graph, id, param) {
            continue;
        }
        let value = store
            .get(&path)
            .expect("a dirty path is declared")
            .value
            .clone();
        graph
            .set_param(id, param, value)
            .map_err(|e| format!("`{path}`: {e}"))?;
        n += 1;
    }
    Ok(n)
}

fn has_param(graph: &Graph, id: NodeId, param: &str) -> bool {
    graph
        .manifest(id)
        .ok()
        .flatten()
        .is_some_and(|m| m.param(param).is_some())
}

/// Which values of the store a graph put in place of another takes (see
/// [`rebind`]).
#[derive(Clone, Debug, Default)]
pub struct Keep {
    /// Every value the store holds, for every node: the graph came back
    /// through an undo or a redo, whose values are the store's.
    pub all: bool,
    /// The paths a step set for nodes it brought (a macro's exposed
    /// values): the store's, though their nodes are new.
    pub paths: std::collections::BTreeSet<String>,
}

/// What [`rebind`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rebound {
    /// The nodes bound again: new to the graph, or running another module
    /// or version.
    pub bound: Vec<String>,
    /// The values the graph refused (`<path>: <why>`): it keeps its own.
    pub refused: Vec<String>,
}

/// Puts `graph` in place of `before` under the same image, the store's
/// history kept: an edit of the network (a node added, removed, a macro
/// made) is not another image.
///
/// - A node `before` had, running the same module at the same version,
///   takes the store's values: the image keeps its look. It is not
///   declared again.
/// - A node new to the graph, or running another module or version, takes
///   the network's values (a node just added starts from them), except
///   for the paths `keep` names, or all of them after an undo or a redo.
///   It is declared again at its values.
/// - The paths of nodes the graph no longer has stay in the store, so an
///   undo that brings a node back finds its values; [`sync`] leaves them.
pub fn rebind(
    store: &mut Store,
    before: &Graph,
    graph: &mut Graph,
    keep: &Keep,
) -> Result<Rebound, String> {
    let e = |e: helios_core::Error| e.to_string();
    let mut rebound = Rebound::default();
    let ids: Vec<NodeId> = graph.ids().collect();
    for id in ids {
        let Some(manifest) = graph.manifest(id).map_err(e)? else {
            continue;
        };
        let name = graph.name(id).map_err(e)?.to_string();
        let same = before.find(&name).is_some_and(|b| {
            before
                .manifest(b)
                .ok()
                .flatten()
                .is_some_and(|m| m.name == manifest.name && m.version == manifest.version)
        });
        let takes = |path: &str| same || keep.all || keep.paths.contains(path);
        for spec in &manifest.params {
            let path = node_path(&name, &spec.name);
            if !takes(&path) {
                continue;
            }
            let Some(entry) = store.get(&path) else {
                continue;
            };
            let value = entry.value.clone();
            let current = graph
                .params(id)
                .map_err(e)?
                .and_then(|p| p.get(&spec.name).cloned());
            if current.as_ref() == Some(&value) {
                continue;
            }
            if let Err(err) = graph.set_param(id, &spec.name, value) {
                rebound.refused.push(format!("{path}: {err}"));
            }
        }
        let bypass = bypass_path(&name);
        if takes(&bypass)
            && graph.can_bypass(id).map_err(e)?
            && let Some(on) = store.get(&bypass).and_then(|e| e.value.as_bool())
            && let Err(err) = graph.set_bypass(id, on)
        {
            rebound.refused.push(format!("{bypass}: {err}"));
        }
        if !same {
            bind_node(store, graph, id)?;
            rebound.bound.push(name);
        }
    }
    Ok(rebound)
}
