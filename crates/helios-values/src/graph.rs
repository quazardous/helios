//! A graph's module parameters as values: `node/<node>/<param>`. The store
//! is where they are edited; the graph keeps rendering from its own copy,
//! which [`sync`] brings up to date.

use helios_core::{Graph, ModuleKind, NodeId};

use crate::{Descriptor, Store};

pub fn node_path(node: &str, param: &str) -> String {
    format!("node/{node}/{param}")
}

/// Declares the parameters of a module node at their current values, with
/// descriptors from its manifest and hints. A decision node is answered,
/// not edited: it declares nothing. Returns how many paths were declared.
pub fn bind_node(store: &mut Store, graph: &Graph, id: NodeId) -> Result<usize, String> {
    let e = |e: helios_core::Error| e.to_string();
    let Some(manifest) = graph.manifest(id).map_err(e)? else {
        return Ok(0);
    };
    if manifest.kind == ModuleKind::Decision {
        return Ok(0);
    }
    let name = graph.name(id).map_err(e)?.to_string();
    let params = graph.params(id).map_err(e)?.unwrap_or_default();
    let hints = graph.param_hints(id).map_err(e)?;
    for spec in &manifest.params {
        let hint = hints.iter().find(|h| h.name == spec.name);
        let value = params
            .get(&spec.name)
            .cloned()
            .unwrap_or_else(|| spec.kind.default_value());
        let mut descriptor = Descriptor::from_spec(spec, hint);
        descriptor.module = Some(manifest.name.clone());
        store.declare_with(&node_path(&name, &spec.name), descriptor, value);
    }
    Ok(manifest.params.len())
}

/// Brings the graph's parameters up to the store's; returns how many were
/// set.
pub fn sync(store: &mut Store, graph: &mut Graph) -> Result<usize, String> {
    let mut n = 0;
    for path in store.take_dirty() {
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
