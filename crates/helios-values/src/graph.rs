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
/// how many were set.
pub fn sync(store: &mut Store, graph: &mut Graph) -> Result<usize, String> {
    let mut n = 0;
    for path in store.take_dirty() {
        if let Some(node) = path.strip_prefix(BYPASS) {
            let id = graph
                .find(node)
                .ok_or_else(|| format!("`{path}`: no node `{node}` in the graph"))?;
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
        let id = graph
            .find(node)
            .ok_or_else(|| format!("`{path}`: no node `{node}` in the graph"))?;
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
