//! Macros made, opened, folded, dissolved and upgraded on a workspace, as
//! pure operations: each takes the workspace's state and the open image's
//! values, and gives the next state, the values to move and what was done
//! as data. Nothing is written and nothing renders: the desktop turns a
//! step into one undo group, the MCP tools into one edit (see
//! `docs/design/macros.md`).
//!
//! A macro is a composite node pinned by id and version. Making one
//! writes a composite file into the workspace's bundle and puts one node
//! in place of the nodes it packages; its inner steps keep their content
//! keys, so nothing renders again.

use std::collections::{BTreeMap, BTreeSet};

use helios_core::{Manifest, ModuleRef, ParamKind, PortKind, PortSpec, Value};
use serde_json::{Value as Json, json};

use crate::build::SOURCE_PORT;
use crate::catalogue::Catalogue;
use crate::format::{
    CompositeFile, CompositeRef, Connection, Endpoint, Exit, Exposed, FORMAT, InnerDesc,
    NetworkDesc, NodeDesc, PORTS, Port, SourceKind,
};
use crate::network::{feeds, fresh_name, names};
use crate::workspace::State;

/// The open image's values by path: `node/<node>/<param>` and
/// `bypass/<node>`.
pub type Values = BTreeMap<String, Value>;

/// What a step does to the open image's values: each path set to a value,
/// or taken out (`None`).
pub type Moves = BTreeMap<String, Option<Value>>;

/// A macro step: the next state, the values to move and the answer.
#[derive(Clone, Debug)]
pub struct Step {
    pub state: State,
    pub values: Moves,
    pub answer: Json,
}

/// The id prefix of a user's macros; `helios.` is the shipped ones'.
pub const USER_PREFIX: &str = "user.";

pub(crate) fn node_path(node: &str, param: &str) -> String {
    format!("node/{node}/{param}")
}

fn bypass_path(node: &str) -> String {
    format!("bypass/{node}")
}

/// The values held on `node`: `(param, value)`.
pub(crate) fn values_of<'a>(values: &'a Values, node: &str) -> Vec<(&'a str, &'a Value)> {
    let prefix = format!("node/{node}/");
    values
        .iter()
        .filter_map(|(k, v)| {
            let param = k.strip_prefix(&prefix)?;
            (!param.contains('/')).then_some((param, v))
        })
        .collect()
}

/// The paths of values held on the inner nodes of an opened macro
/// `node` (`node/<node>.<inner>/<param>`).
/// What exposed entry `x` writes into its inner parameters at `value`:
/// `(inner node, param, value)`. A semantic slider's curves are evaluated
/// by `built`, the file built as a macro; nothing for one when it is not
/// given or the value is not a number.
fn inner_values(
    built: Option<&helios_core::Macro>,
    x: &Exposed,
    value: &Value,
) -> Vec<(String, String, Value)> {
    if !x.is_driven() {
        return vec![(x.node.clone(), x.param.clone(), value.clone())];
    }
    #[allow(clippy::cast_precision_loss)]
    let at = match value {
        Value::Float(v) => *v,
        Value::Int(v) => *v as f64,
        _ => return Vec::new(),
    };
    built.map(|m| m.driven(&x.name, at)).unwrap_or_default()
}

/// The file built as a macro, when it has a semantic slider (its curves
/// are evaluated by it); `None` otherwise or when it does not build.
fn built_if_driven(file: &CompositeFile, cat: &Catalogue) -> Option<helios_core::Macro> {
    if file.exposed.iter().any(Exposed::is_driven) {
        crate::catalogue::build(file, cat, 0).ok()
    } else {
        None
    }
}

fn inner_paths(values: &Values, node: &str) -> Vec<String> {
    let prefix = format!("node/{node}.");
    values
        .keys()
        .filter(|k| k.starts_with(&prefix))
        .cloned()
        .collect()
}

fn bypassed(desc: &NodeDesc, values: &Values) -> bool {
    values.get(&bypass_path(&desc.name)) == Some(&Value::Bool(true))
        || (desc.bypassed && values.get(&bypass_path(&desc.name)).is_none())
}

/// A node's module or composite, built.
fn module_of(catalogue: &Catalogue, n: &NodeDesc) -> Result<ModuleRef, String> {
    catalogue
        .resolve(n.module.as_deref(), n.composite.as_ref(), 0)
        .map_err(|e| format!("`{}`: {e}", n.name))
}

/// A lowercase slug of words joined by `-`: `Warm skin` → `warm-skin`.
fn slug(name: &str) -> String {
    let words: Vec<String> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if words.is_empty() {
        "macro".into()
    } else {
        words.join("-")
    }
}

/// `user.<slug of name>`, made unique among the catalogue's composites.
pub(crate) fn free_id(catalogue: &Catalogue, name: &str) -> String {
    let taken: BTreeSet<String> = catalogue.composite_files().map(|f| f.id.clone()).collect();
    let base = format!("{USER_PREFIX}{}", slug(name));
    if !taken.contains(&base) {
        return base;
    }
    // Among `taken.len() + 1` ids, one is free.
    (2..=taken.len() + 2)
        .map(|i| format!("{base}-{i}"))
        .find(|id| !taken.contains(id))
        .expect("a free id")
}

/// `base`, else `base_2`, `base_3`... among `taken`.
fn free_among(taken: &BTreeSet<String>, base: &str) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    (2..=taken.len() + 2)
        .map(|i| format!("{base}_{i}"))
        .find(|n| !taken.contains(n))
        .expect("a free name")
}

/// A port of the composite file from a port spec, when its channels are
/// fixed.
fn port_of(spec: &PortSpec, name: &str) -> Option<Port> {
    let PortKind::Image { channels } = spec.kind else {
        return None;
    };
    Some(Port {
        name: name.into(),
        channels,
        layout: spec.layout.clone(),
        optional: spec.optional,
    })
}

/// The output `port` of `from` (a source or a node) as a spec.
fn output_spec(
    net: &NetworkDesc,
    catalogue: &Catalogue,
    from: &str,
    port: &str,
) -> Option<PortSpec> {
    if net.sources.iter().any(|s| s == from) {
        let kind = net.source_kind(from);
        let mut spec = PortSpec::image(SOURCE_PORT, kind.channels());
        if kind != SourceKind::Raw {
            spec.layout = Some("rgba".into());
        }
        return Some(spec);
    }
    let n = net.nodes.iter().find(|n| n.name == from)?;
    let m = module_of(catalogue, n).ok()?;
    m.manifest()
        .outputs
        .iter()
        .find(|o| o.name == port)
        .cloned()
}

/// Checks a node may be packaged into a macro.
fn check_member<'a>(
    net: &'a NetworkDesc,
    node: &str,
    values: &Values,
) -> Result<&'a NodeDesc, String> {
    if net.sources.iter().any(|s| s == node) {
        return Err(format!("`{node}` is a source: a macro packages modules"));
    }
    let Some(desc) = net.nodes.iter().find(|n| n.name == node) else {
        if let Some((head, _)) = node.split_once('.')
            && net.nodes.iter().any(|n| n.name == head && n.open)
        {
            return Err(format!(
                "`{node}` is inside the opened macro `{head}`: take `{head}` whole"
            ));
        }
        return Err(format!("no node `{node}`"));
    };
    if bypassed(desc, values) {
        return Err(format!(
            "`{node}` is bypassed: a macro's inner nodes are never bypassed, turn it on or leave it out"
        ));
    }
    if desc.open && !inner_paths(values, node).is_empty() {
        return Err(format!(
            "the opened macro `{node}` has changes of its own: fold it first"
        ));
    }
    Ok(desc)
}

/// The selected nodes in an order where each reads only nodes before it:
/// the network's order, moved where a wire needs.
fn ordered(net: &NetworkDesc, selected: &BTreeSet<&str>) -> Vec<String> {
    let mut left: Vec<&str> = net
        .nodes
        .iter()
        .map(|n| n.name.as_str())
        .filter(|n| selected.contains(n))
        .collect();
    let mut out: Vec<String> = Vec::with_capacity(left.len());
    while !left.is_empty() {
        let ready = left
            .iter()
            .position(|n| {
                net.connections.iter().all(|c| {
                    c.to != *n || !selected.contains(c.from.as_str()) || out.contains(&c.from)
                })
            })
            // A cycle cannot be built; keep the order rather than loop.
            .unwrap_or(0);
        out.push(left.remove(ready).to_string());
    }
    out
}

/// Fits `value` to `kind`: the same value, a number clamped into the
/// range (`true`), or `None` when it cannot fit (another kind, an option
/// no longer offered).
fn fit(kind: &ParamKind, value: &Value) -> Option<(Value, bool)> {
    match (kind, value) {
        (ParamKind::Float { min, max, .. }, Value::Float(v)) => {
            let c = v.clamp(*min, *max);
            Some((Value::Float(c), c != *v))
        }
        (ParamKind::Int { min, max, .. }, Value::Int(v)) => {
            let c = (*v).clamp(*min, *max);
            Some((Value::Int(c), c != *v))
        }
        (ParamKind::Floats { min, max, .. }, Value::List(v)) => {
            let c: Vec<f64> = v.iter().map(|x| x.clamp(*min, *max)).collect();
            let changed = c != *v;
            Some((Value::List(c), changed))
        }
        (ParamKind::Ints { min, max, .. }, Value::Ints(v)) => {
            let c: Vec<i64> = v.iter().map(|x| (*x).clamp(*min, *max)).collect();
            let changed = c != *v;
            Some((Value::Ints(c), changed))
        }
        (ParamKind::Choice { options, .. }, Value::Choice(v)) => {
            options.contains(v).then(|| (value.clone(), false))
        }
        (ParamKind::Bool { .. }, Value::Bool(_)) | (ParamKind::Text { .. }, Value::Text(_)) => {
            Some((value.clone(), false))
        }
        _ => None,
    }
}

impl State {
    /// The next state checked: refused with its problems when it does not
    /// validate against `catalogue` (the bundle's composites added).
    pub(crate) fn checked(next: State, catalogue: &Catalogue) -> Result<State, String> {
        let problems = next.bundle.validate(catalogue).problems;
        if problems.is_empty() {
            Ok(next)
        } else {
            Err(problems.join("; "))
        }
    }

    /// A composite node of the network, by name.
    pub(crate) fn macro_node(
        &self,
        node: &str,
    ) -> Result<(usize, &NodeDesc, CompositeRef), String> {
        let net = &self.bundle.template.network;
        let (i, desc) = net
            .nodes
            .iter()
            .enumerate()
            .find(|(_, n)| n.name == node)
            .ok_or_else(|| format!("no node `{node}`"))?;
        let r = desc
            .composite
            .clone()
            .ok_or_else(|| format!("`{node}` is a module, not a macro"))?;
        Ok((i, desc, r))
    }

    /// Packages `nodes` as one macro node named after `name` (`macro.make`).
    ///
    /// - A composite file at version 1 is added to the workspace's bundle,
    ///   its id `user.<slug of name>`, unique in the catalogue; its inner
    ///   nodes are `nodes` in an order where each reads only nodes before
    ///   it, with the workspace's values and the image's values of what is
    ///   not exposed.
    /// - The wires reaching the selection from outside become its inputs
    ///   (one per output read, named after the first inner input it
    ///   feeds); the outputs read outside or shown become its outputs.
    /// - `expose` names the parameters exposed; without it, those the
    ///   image changed from the workspace's values, named after the
    ///   parameter (`<node>_<param>` when two would clash). Their image
    ///   values move to `node/<macro>/<exposed>`.
    /// - The node takes the place of the first of `nodes`, wired as they
    ///   were, in their sub-workspace and at their position.
    ///
    /// Refused: no node, a source, a node unknown or inside an opened
    /// macro, a bypassed node, a node outside reading the selection and
    /// feeding it (a cycle once folded), nodes crossing a sub-workspace's
    /// edge, a port whose channels are not fixed, nothing read outside.
    pub fn macro_made(
        &self,
        catalogue: &Catalogue,
        nodes: &[String],
        name: &str,
        expose: Option<&[Exposed]>,
        values: &Values,
    ) -> Result<Step, String> {
        let cat = self.bundle.catalogue(catalogue);
        let template = &self.bundle.template;
        let net = &template.network;
        let name = name.trim();
        if name.is_empty() {
            return Err("name: empty".into());
        }
        if nodes.is_empty() {
            return Err("nodes: none given".into());
        }
        let mut selected: BTreeSet<&str> = BTreeSet::new();
        for n in nodes {
            if !selected.insert(n.as_str()) {
                return Err(format!("nodes: `{n}` given twice"));
            }
            check_member(net, n, values).map_err(|e| format!("nodes: {e}"))?;
        }
        // No path leaves the selection and comes back.
        for outside in net
            .nodes
            .iter()
            .filter(|n| !selected.contains(n.name.as_str()))
        {
            let read = selected.iter().any(|s| feeds(net, s, &outside.name));
            let fed = selected.iter().any(|s| feeds(net, &outside.name, s));
            if read && fed {
                return Err(format!(
                    "nodes: `{}` reads the selection and feeds it: folded, that would be a cycle; take it too",
                    outside.name
                ));
            }
        }
        // Sub-workspaces: all of the selection in one, or none of it.
        for s in &template.subworkspaces {
            let inside = selected
                .iter()
                .filter(|n| s.nodes.iter().any(|m| m == *n))
                .count();
            if inside > 0 && inside < selected.len() {
                return Err(format!(
                    "nodes: they cross the sub-workspace `{}` ({}): some are in it, some are not",
                    s.id, s.name
                ));
            }
        }
        let order = ordered(net, &selected);
        let desc = |n: &str| net.nodes.iter().find(|d| d.name == n).expect("checked");
        let mut manifests: BTreeMap<&str, Manifest> = BTreeMap::new();
        for n in &order {
            let m = module_of(&cat, desc(n))?;
            manifests.insert(n.as_str(), m.manifest().clone());
        }
        // Inputs: one per outside output read, in the order of the inner
        // inputs it feeds.
        let mut inputs: Vec<Port> = Vec::new();
        let mut input_of: BTreeMap<(String, String), String> = BTreeMap::new();
        let mut file_connections: Vec<Connection> = Vec::new();
        let mut port_names: BTreeSet<String> = BTreeSet::new();
        for n in &order {
            for spec in &manifests[n.as_str()].inputs {
                for c in net
                    .connections
                    .iter()
                    .filter(|c| c.to == *n && c.input == spec.name)
                {
                    if selected.contains(c.from.as_str()) {
                        file_connections.push(c.clone());
                        continue;
                    }
                    let key = (c.from.clone(), c.port.clone());
                    let port = if let Some(p) = input_of.get(&key) {
                        p.clone()
                    } else {
                        let p = free_among(&port_names, &spec.name);
                        let port = port_of(spec, &p)
                            .or_else(|| {
                                output_spec(net, &cat, &c.from, &c.port)
                                    .and_then(|o| port_of(&o, &p))
                            })
                            .ok_or_else(|| {
                                format!(
                                    "nodes: the channels of input `{}` of `{n}` are not fixed",
                                    spec.name
                                )
                            })?;
                        port_names.insert(p.clone());
                        input_of.insert(key, p.clone());
                        inputs.push(port);
                        p
                    };
                    file_connections.push(Connection {
                        from: PORTS.into(),
                        port,
                        to: n.clone(),
                        input: spec.name.clone(),
                    });
                }
            }
        }
        // Outputs: those read outside or shown.
        let mut outputs: Vec<Port> = Vec::new();
        let mut exits: Vec<Exit> = Vec::new();
        let mut output_of: BTreeMap<(String, String), String> = BTreeMap::new();
        let mut out_names: BTreeSet<String> = BTreeSet::new();
        for n in &order {
            for spec in &manifests[n.as_str()].outputs {
                let read = net.connections.iter().any(|c| {
                    c.from == *n && c.port == spec.name && !selected.contains(c.to.as_str())
                }) || (net.view.node == *n && net.view.port == spec.name);
                if !read {
                    continue;
                }
                let p = free_among(&out_names, &spec.name);
                let port = port_of(spec, &p).ok_or_else(|| {
                    format!(
                        "nodes: the channels of output `{}` of `{n}` are not fixed",
                        spec.name
                    )
                })?;
                out_names.insert(p.clone());
                output_of.insert((n.clone(), spec.name.clone()), p.clone());
                outputs.push(port);
                exits.push(Exit {
                    from: n.clone(),
                    port: spec.name.clone(),
                    output: p,
                });
            }
        }
        if outputs.is_empty() {
            return Err("nodes: nothing outside reads them and the view does not show them".into());
        }
        // What is exposed.
        let exposed: Vec<Exposed> = if let Some(list) = expose {
            for (i, x) in list.iter().enumerate() {
                if !selected.contains(x.node.as_str()) {
                    return Err(format!("expose[{i}]: `{}` is not among the nodes", x.node));
                }
            }
            list.to_vec()
        } else {
            let mut changed: Vec<(String, String)> = Vec::new();
            for n in &order {
                let d = desc(n);
                for (param, v) in values_of(values, n) {
                    let Some(spec) = manifests[n.as_str()].param(param) else {
                        continue;
                    };
                    let saved = d
                        .params
                        .get(param)
                        .cloned()
                        .unwrap_or_else(|| spec.kind.default_value());
                    if *v != saved {
                        changed.push((n.clone(), param.to_string()));
                    }
                }
            }
            let clashes = |p: &str| changed.iter().filter(|(_, q)| q == p).count() > 1;
            changed
                .iter()
                .map(|(n, p)| Exposed {
                    name: if clashes(p) {
                        format!("{n}_{p}")
                    } else {
                        p.clone()
                    },
                    node: n.clone(),
                    param: p.clone(),
                    ..Exposed::default()
                })
                .collect()
        };
        let is_exposed = |n: &str, p: &str| exposed.iter().find(|x| x.node == n && x.param == p);
        // The inner nodes: the workspace's values, and the image's for
        // what is not exposed.
        let mut inner = Vec::new();
        for n in &order {
            let d = desc(n);
            let mut params = d.params.clone();
            for (param, v) in values_of(values, n) {
                if is_exposed(n, param).is_none() {
                    params.set(param, v.clone());
                }
            }
            inner.push(InnerDesc {
                name: n.clone(),
                module: d.module.clone(),
                composite: d.composite.clone(),
                params,
            });
        }
        let id = free_id(&cat, name);
        let file = CompositeFile {
            format: FORMAT,
            id: id.clone(),
            version: 1,
            name: name.into(),
            description: String::new(),
            inputs,
            outputs,
            nodes: inner,
            connections: file_connections,
            exits,
            exposed: exposed.clone(),
            panel: None,
            workshop: None,
        };
        let r = file.reference();
        // The network: one node in the selection's place.
        let node = fresh_name(net, name);
        let mut next = self.clone();
        let t = &mut next.bundle.template;
        let first = t
            .network
            .nodes
            .iter()
            .position(|n| selected.contains(n.name.as_str()))
            .expect("a node selected");
        t.network
            .nodes
            .retain(|n| !selected.contains(n.name.as_str()));
        t.network.nodes.insert(
            first,
            NodeDesc {
                name: node.clone(),
                composite: Some(r.clone()),
                ..NodeDesc::default()
            },
        );
        let mut connections: Vec<Connection> = Vec::new();
        for c in &net.connections {
            let from_in = selected.contains(c.from.as_str());
            let to_in = selected.contains(c.to.as_str());
            let mapped = match (from_in, to_in) {
                (true, true) => continue,
                (false, true) => Connection {
                    from: c.from.clone(),
                    port: c.port.clone(),
                    to: node.clone(),
                    input: input_of[&(c.from.clone(), c.port.clone())].clone(),
                },
                (true, false) => Connection {
                    from: node.clone(),
                    port: output_of[&(c.from.clone(), c.port.clone())].clone(),
                    to: c.to.clone(),
                    input: c.input.clone(),
                },
                (false, false) => c.clone(),
            };
            if !connections.contains(&mapped) {
                connections.push(mapped);
            }
        }
        t.network.connections = connections;
        if selected.contains(net.view.node.as_str()) {
            t.network.view = Endpoint {
                node: node.clone(),
                port: output_of[&(net.view.node.clone(), net.view.port.clone())].clone(),
            };
        }
        // Presets: their exposed values move to the macro, the others go.
        let mut preset_dropped = Vec::new();
        for p in &mut t.network.presets {
            let mut moved = helios_core::Params::new();
            for n in &order {
                let Some(held) = p.values.remove(n) else {
                    continue;
                };
                for (param, v) in held.iter() {
                    match is_exposed(n, param) {
                        Some(x) => moved.set(&x.name, v.clone()),
                        None => preset_dropped.push(format!("{}: {n}.{param}", p.id)),
                    }
                }
            }
            if moved.iter().next().is_some() {
                p.values.insert(node.clone(), moved);
            }
        }
        // Positions: the macro where the first node stood.
        let at = template.positions.get(&order[0]).copied().or_else(|| {
            nodes
                .iter()
                .find_map(|n| template.positions.get(n.as_str()).copied())
        });
        t.positions.retain(|k, _| {
            let head = k.split('.').next().unwrap_or(k);
            !selected.contains(head)
        });
        if let Some(at) = at {
            t.positions.insert(node.clone(), at);
        }
        // Sub-workspaces: the macro in the place of its nodes.
        for s in &mut t.subworkspaces {
            if let Some(i) = s.nodes.iter().position(|n| selected.contains(n.as_str())) {
                s.nodes.retain(|n| !selected.contains(n.as_str()));
                s.nodes.insert(i.min(s.nodes.len()), node.clone());
            }
        }
        next.bundle.composites.push(file);
        let next = State::checked(next, catalogue)?;
        // The image's values: the exposed ones under the macro, the
        // others are the macro's now.
        let mut moves = Moves::new();
        for n in &order {
            for (param, v) in values_of(values, n) {
                moves.insert(node_path(n, param), None);
                if let Some(x) = is_exposed(n, param) {
                    moves.insert(node_path(&node, &x.name), Some(v.clone()));
                }
            }
            if values.contains_key(&bypass_path(n)) {
                moves.insert(bypass_path(n), None);
            }
        }
        let answer = json!({
            "node": node,
            "composite": { "id": id, "version": 1 },
            "nodes": order,
            "inputs": next_ports(&next, &r, true),
            "outputs": next_ports(&next, &r, false),
            "exposed": exposed.iter().map(|x| json!({
                "name": x.name, "node": x.node, "param": x.param,
            })).collect::<Vec<_>>(),
            "values_moved": moves.iter().filter(|(_, v)| v.is_some()).count(),
            "preset_values_dropped": preset_dropped,
        });
        Ok(Step {
            state: next,
            values: moves,
            answer,
        })
    }

    /// Opens the macro `node` in place (`macro.open`): its inner nodes
    /// join the graph as `<node>.<inner>`, wired as inside. Its exposed
    /// values keep their paths. Refused for a module, a macro already
    /// open, a bypassed one (bypass its inner nodes once opened), or one
    /// passing an input straight to an output.
    pub fn macro_opened(
        &self,
        catalogue: &Catalogue,
        node: &str,
        values: &Values,
    ) -> Result<Step, String> {
        let (i, desc, r) = self.macro_node(node)?;
        if desc.open {
            return Err(format!("`{node}` is already open"));
        }
        if bypassed(desc, values) {
            return Err(format!(
                "`{node}` is bypassed: an opened macro is bypassed node by node, turn it on first"
            ));
        }
        let file = self
            .bundle
            .catalogue(catalogue)
            .get_composite(&r)
            .ok_or_else(|| format!("unknown composite `{r}`"))?
            .clone();
        let mut next = self.clone();
        next.bundle.template.network.nodes[i].open = true;
        next.bundle.template.network.nodes[i].bypassed = false;
        let next = State::checked(next, catalogue)?;
        let mut moves = Moves::new();
        if values.contains_key(&bypass_path(node)) {
            moves.insert(bypass_path(node), None);
        }
        // The image's exposed values onto the inner parameters they set,
        // where the opened macro's nodes show them (a semantic slider's
        // through its curves).
        let built = built_if_driven(&file, &self.bundle.catalogue(catalogue));
        for (name, v) in values_of(values, node) {
            for x in file.exposed.iter().filter(|x| x.name == name) {
                for (inner, param, v) in inner_values(built.as_ref(), x, v) {
                    moves.insert(node_path(&format!("{node}.{inner}"), &param), Some(v));
                }
            }
        }
        Ok(Step {
            state: next,
            values: moves,
            answer: json!({ "node": node, "composite": r, "open": true }),
        })
    }

    /// Checks `node` is an opened macro.
    pub fn macro_node_open(&self, node: &str) -> Result<(), String> {
        let (_, desc, _) = self.macro_node(node)?;
        if desc.open {
            Ok(())
        } else {
            Err(format!("`{node}` is not open"))
        }
    }

    /// What the image changed inside the opened macro `node` that its
    /// file does not hold: values of inner parameters it does not expose
    /// (`node/<node>.<inner>/<param>`) and inner nodes bypassed
    /// (`bypass/<node>.<inner>`), by path. Folding asks what to do with
    /// them (`Changes`); empty for a folded macro.
    pub fn macro_changes(
        &self,
        catalogue: &Catalogue,
        node: &str,
        values: &Values,
    ) -> Result<Vec<String>, String> {
        let (_, desc, r) = self.macro_node(node)?;
        if !desc.open {
            return Ok(Vec::new());
        }
        let cat = self.bundle.catalogue(catalogue);
        let file = cat
            .get_composite(&r)
            .ok_or_else(|| format!("unknown composite `{r}`"))?;
        Ok(changed_inside(file, node, values))
    }

    /// Folds the opened macro `node` back into one node (`macro.fold`).
    /// The image's values of exposed inner parameters go back to the
    /// macro's (`node/<node>/<exposed>`).
    ///
    /// What the image changed inside that the file does not hold
    /// (`macro_changes`) is a change to the macro itself, settled by
    /// `changes`; without it, the fold is refused with the list:
    /// - [`Changes::Version`]: the file's next version holds the changed
    ///   values as its inner ones, this node pinned to it;
    /// - [`Changes::Expose`]: the next version exposes the changed
    ///   parameters (named after the parameter, `<inner>_<param>` when
    ///   the name is taken), their values kept on this image under the
    ///   macro;
    /// - [`Changes::Discard`]: the changes are taken out, the macro back to
    ///   its file's inside.
    ///
    /// A file is never rewritten, and the macro's other nodes keep their
    /// version (offered the new one as an update). A bypassed inner node
    /// cannot be saved (a macro's inner nodes are never bypassed): only
    /// discarded.
    pub fn macro_folded(
        &self,
        catalogue: &Catalogue,
        node: &str,
        changes: Option<Changes>,
        values: &Values,
    ) -> Result<Step, String> {
        let cat = self.bundle.catalogue(catalogue);
        let (i, desc, r) = self.macro_node(node)?;
        if !desc.open {
            return Err(format!("`{node}` is not open"));
        }
        if changes == Some(Changes::Keep) {
            return Err(format!("`{node}` is kept open"));
        }
        let file = cat
            .get_composite(&r)
            .ok_or_else(|| format!("unknown composite `{r}`"))?
            .clone();
        let prefix = format!("node/{node}.");
        let changed = changed_inside(&file, node, values);
        let mut moves = Moves::new();
        // Inner nodes' bypasses mean nothing once folded.
        let inner_bypass = format!("{BYPASS_PREFIX}{node}.");
        for path in values.keys().filter(|k| k.starts_with(&inner_bypass)) {
            moves.insert(path.clone(), None);
        }
        // Exposed values set on their inner nodes go back to the macro. A
        // semantic slider's inner values are its curves': the slider kept
        // its own value, so they are dropped.
        for path in inner_paths(values, node) {
            if changed.contains(&path) {
                continue;
            }
            let exposed = path[prefix.len()..]
                .split_once('/')
                .and_then(|(inner, param)| {
                    file.exposed
                        .iter()
                        .find(|x| x.inner().contains(&(inner, param)))
                });
            match exposed {
                Some(x) if x.is_driven() => {
                    moves.insert(path.clone(), None);
                }
                Some(x) => {
                    moves.insert(node_path(node, &x.name), values.get(&path).cloned());
                }
                None => {}
            }
        }
        // A semantic slider keeps its value: the image's if it still holds
        // it, else read back through its first strictly monotonic curve
        // from the inner value it wrote.
        for x in file.exposed.iter().filter(|x| x.is_driven()) {
            if let Some(v) = values.get(&node_path(node, &x.name)) {
                moves.insert(node_path(node, &x.name), Some(v.clone()));
                continue;
            }
            let back = x.drives.iter().find_map(|d| {
                let y = values
                    .get(&node_path(&format!("{node}.{}", d.node), &d.param))?
                    .as_f64()?;
                d.curve().ok()?.invert(y)
            });
            if let Some(at) = back {
                moves.insert(node_path(node, &x.name), Some(Value::Float(at)));
            }
        }
        let mut next = self.clone();
        next.bundle.template.network.nodes[i].open = false;
        let mut answer = json!({ "node": node, "composite": r, "open": false });
        if !changed.is_empty() {
            let Some(changes) = changes else {
                return Err(format!(
                    "`{node}` changed while open ({}): give `changes`, `version`, `expose` or `discard`, or keep it open",
                    changed.join(", ")
                ));
            };
            let bypassed: Vec<&String> = changed
                .iter()
                .filter(|p| p.starts_with(BYPASS_PREFIX))
                .collect();
            if changes != Changes::Discard
                && let Some(b) = bypassed.first()
            {
                let inner = &b[BYPASS_PREFIX.len()..];
                return Err(format!(
                    "`{inner}` is bypassed: a macro's inner nodes are never bypassed, turn it on or discard the changes"
                ));
            }
            // Each changed inner parameter: `(path, inner, param, value)`.
            let params: Vec<(&String, &str, &str, &Value)> = changed
                .iter()
                .filter_map(|p| {
                    let (inner, param) = p.strip_prefix(&prefix)?.split_once('/')?;
                    Some((p, inner, param, values.get(p)?))
                })
                .collect();
            for p in &changed {
                moves.insert(p.clone(), None);
            }
            if changes == Changes::Discard {
                answer["discarded"] = json!(changed);
            } else {
                let to = next_ref(&cat, &r, &file.name);
                let mut next_file = file.clone();
                next_file.id.clone_from(&to.id);
                next_file.version = to.version;
                let mut exposed_now = Vec::new();
                for (_, inner, param, v) in &params {
                    if changes == Changes::Version {
                        let n = next_file
                            .nodes
                            .iter_mut()
                            .find(|n| n.name == *inner)
                            .ok_or_else(|| format!("`{r}` has no inner node `{inner}`"))?;
                        n.params.set(param, (*v).clone());
                        continue;
                    }
                    let taken: BTreeSet<String> =
                        next_file.exposed.iter().map(|x| x.name.clone()).collect();
                    let name = if taken.contains(*param) {
                        free_among(&taken, &format!("{inner}_{param}"))
                    } else {
                        (*param).to_string()
                    };
                    next_file.exposed.push(Exposed {
                        name: name.clone(),
                        node: (*inner).to_string(),
                        param: (*param).to_string(),
                        ..Exposed::default()
                    });
                    moves.insert(node_path(node, &name), Some((*v).clone()));
                    exposed_now.push(name);
                }
                crate::catalogue::build(&next_file, &cat, 0)?;
                next.bundle.template.network.nodes[i].composite = Some(to.clone());
                next.bundle.composites.push(next_file);
                answer["composite"] = json!(to);
                answer["from"] = json!(r);
                if changes == Changes::Version {
                    answer["saved"] = json!(changed);
                } else {
                    answer["exposed"] = json!(exposed_now);
                }
            }
        }
        let next = State::checked(next, catalogue)?;
        Ok(Step {
            state: next,
            values: moves,
            answer,
        })
    }

    /// Gives the macro `node` way to its inner nodes, as ordinary nodes
    /// (`macro.dissolve`): named as inside when the name is free, else
    /// `<node>_<inner>`, wired as inside and to what the macro was wired
    /// to, its exposed values written to each inner parameter, in the
    /// workspace and in the image. The composite file stays in the
    /// workspace. Refused for a bypassed macro.
    pub fn macro_dissolved(
        &self,
        catalogue: &Catalogue,
        node: &str,
        values: &Values,
    ) -> Result<Step, String> {
        let cat = self.bundle.catalogue(catalogue);
        let (at, desc, r) = self.macro_node(node)?;
        if bypassed(desc, values) {
            return Err(format!(
                "`{node}` is bypassed: its inner nodes cannot all be, turn it on first"
            ));
        }
        let file = cat
            .get_composite(&r)
            .ok_or_else(|| format!("unknown composite `{r}`"))?
            .clone();
        let template = &self.bundle.template;
        let net = &template.network;
        // New names.
        let mut taken: BTreeSet<String> = names(net).into_iter().filter(|n| n != node).collect();
        let mut renamed: BTreeMap<String, String> = BTreeMap::new();
        for n in &file.nodes {
            let name = if taken.contains(&n.name) {
                free_among(&taken, &format!("{node}_{}", n.name))
            } else {
                n.name.clone()
            };
            taken.insert(name.clone());
            renamed.insert(n.name.clone(), name);
        }
        // The exposed values: the workspace's, then the image's.
        let mut nodes: Vec<NodeDesc> = file
            .nodes
            .iter()
            .map(|n| NodeDesc {
                name: renamed[&n.name].clone(),
                module: n.module.clone(),
                composite: n.composite.clone(),
                params: n.params.clone(),
                ..NodeDesc::default()
            })
            .collect();
        let target = |nodes: &[NodeDesc], inner: &str| -> usize {
            let name = &renamed[inner];
            nodes.iter().position(|n| n.name == *name).expect("renamed")
        };
        let built = built_if_driven(&file, &cat);
        for x in &file.exposed {
            if let Some(v) = desc.params.get(&x.name) {
                for (inner, param, v) in inner_values(built.as_ref(), x, v) {
                    let i = target(&nodes, &inner);
                    nodes[i].params.set(&param, v);
                }
            }
        }
        let mut moves = Moves::new();
        for (name, v) in values_of(values, node) {
            moves.insert(node_path(node, name), None);
            for x in file.exposed.iter().filter(|x| x.name == name) {
                for (inner, param, v) in inner_values(built.as_ref(), x, v) {
                    moves.insert(node_path(&renamed[&inner], &param), Some(v));
                }
            }
        }
        for path in inner_paths(values, node) {
            let rest = &path[format!("node/{node}.").len()..];
            moves.insert(path.clone(), None);
            if let Some((inner, param)) = rest.split_once('/')
                && let Some(new) = renamed.get(inner)
            {
                moves.insert(node_path(new, param), values.get(&path).cloned());
            }
        }
        if values.contains_key(&bypass_path(node)) {
            moves.insert(bypass_path(node), None);
        }
        // Where the macro's inputs come from, and its outputs.
        let feeder = |input: &str| -> Vec<&Connection> {
            net.connections
                .iter()
                .filter(|c| c.to == node && c.input == input)
                .collect()
        };
        let exit = |output: &str| file.exits.iter().find(|x| x.output == output);
        let mut connections: Vec<Connection> = Vec::new();
        let mut push = |c: Connection| {
            if !connections.contains(&c) {
                connections.push(c);
            }
        };
        for c in &net.connections {
            if c.to == node {
                // Rewired below, from the file's inner wiring.
                continue;
            }
            if c.from == node {
                let Some(x) = exit(&c.port) else { continue };
                if x.from == PORTS {
                    for f in feeder(&x.port) {
                        push(Connection {
                            from: f.from.clone(),
                            port: f.port.clone(),
                            to: c.to.clone(),
                            input: c.input.clone(),
                        });
                    }
                } else {
                    push(Connection {
                        from: renamed[&x.from].clone(),
                        port: x.port.clone(),
                        to: c.to.clone(),
                        input: c.input.clone(),
                    });
                }
                continue;
            }
            push(c.clone());
        }
        for c in &file.connections {
            if c.from == PORTS {
                for f in feeder(&c.port) {
                    push(Connection {
                        from: f.from.clone(),
                        port: f.port.clone(),
                        to: renamed[&c.to].clone(),
                        input: c.input.clone(),
                    });
                }
            } else {
                push(Connection {
                    from: renamed[&c.from].clone(),
                    port: c.port.clone(),
                    to: renamed[&c.to].clone(),
                    input: c.input.clone(),
                });
            }
        }
        let mut next = self.clone();
        let t = &mut next.bundle.template;
        t.network.nodes.remove(at);
        for (k, n) in nodes.into_iter().enumerate() {
            t.network.nodes.insert(at + k, n);
        }
        t.network.connections = connections;
        if net.view.node == node {
            let x = exit(&net.view.port)
                .ok_or_else(|| format!("`{r}` has no output `{}`", net.view.port))?;
            t.network.view = if x.from == PORTS {
                let f = feeder(&x.port);
                let f = f.first().ok_or_else(|| {
                    format!(
                        "the view shows `{node}`, whose input `{}` is not connected",
                        x.port
                    )
                })?;
                Endpoint {
                    node: f.from.clone(),
                    port: f.port.clone(),
                }
            } else {
                Endpoint {
                    node: renamed[&x.from].clone(),
                    port: x.port.clone(),
                }
            };
        }
        for p in &mut t.network.presets {
            let Some(held) = p.values.remove(node) else {
                continue;
            };
            for (name, v) in held.iter() {
                for x in file.exposed.iter().filter(|x| x.name == name) {
                    for (inner, param, v) in inner_values(built.as_ref(), x, v) {
                        p.values
                            .entry(renamed[&inner].clone())
                            .or_default()
                            .set(&param, v);
                    }
                }
            }
        }
        let origin = template.positions.get(node).copied();
        let opened: Vec<(String, [f32; 2])> = template
            .positions
            .iter()
            .filter_map(|(k, v)| {
                let inner = k.strip_prefix(&format!("{node}."))?;
                Some((renamed.get(inner)?.clone(), *v))
            })
            .collect();
        t.positions
            .retain(|k, _| k != node && !k.starts_with(&format!("{node}.")));
        if opened.is_empty() {
            if let Some([x, y]) = origin {
                for (k, n) in file.nodes.iter().enumerate() {
                    #[allow(clippy::cast_precision_loss)]
                    let dx = 180.0 * k as f32;
                    t.positions.insert(renamed[&n.name].clone(), [x + dx, y]);
                }
            }
        } else {
            t.positions.extend(opened);
        }
        let all: Vec<String> = file
            .nodes
            .iter()
            .map(|n| renamed[&n.name].clone())
            .collect();
        for s in &mut t.subworkspaces {
            let inner_prefix = format!("{node}.");
            let i = s
                .nodes
                .iter()
                .position(|n| n == node || n.starts_with(&inner_prefix));
            if let Some(i) = i {
                s.nodes
                    .retain(|n| n != node && !n.starts_with(&inner_prefix));
                for (k, n) in all.iter().enumerate() {
                    s.nodes.insert((i + k).min(s.nodes.len()), n.clone());
                }
            }
        }
        let next = State::checked(next, catalogue)?;
        let answer = json!({
            "node": node,
            "composite": r,
            "nodes": all,
            "renamed": renamed.iter().filter(|(a, b)| a != b).collect::<BTreeMap<_, _>>(),
            "values_moved": moves.iter().filter(|(_, v)| v.is_some()).count(),
        });
        Ok(Step {
            state: next,
            values: moves,
            answer,
        })
    }

    /// Moves the macro `node` to `version` of its composite, the newest
    /// the catalogue has without one (`macro.upgrade`). Its values, in
    /// the workspace and in the image, are carried by name: clamped into
    /// a narrower range, dropped when the version no longer exposes them
    /// or they no longer fit; the answer says which. Presets' values for
    /// it are carried the same way. Refused while the
    /// macro is open, and for a version the catalogue lacks.
    pub fn macro_upgraded(
        &self,
        catalogue: &Catalogue,
        node: &str,
        version: Option<u32>,
        values: &Values,
    ) -> Result<Step, String> {
        let cat = self.bundle.catalogue(catalogue);
        let (i, desc, r) = self.macro_node(node)?;
        if desc.open {
            return Err(format!("`{node}` is open: fold it first"));
        }
        let version = match version {
            Some(v) => v,
            None => cat
                .newer(&r)
                .ok_or_else(|| format!("`{node}` is at the newest version of `{}`", r.id))?,
        };
        if version == r.version {
            return Err(format!("`{node}` is already at version {version}"));
        }
        let to = CompositeRef {
            id: r.id.clone(),
            version,
        };
        let file = cat
            .get_composite(&to)
            .ok_or_else(|| format!("unknown composite `{to}`"))?
            .clone();
        let built = cat.build_composite(&to, 0)?;
        let manifest = helios_core::Composite::manifest(&built);
        let mut dropped: Vec<String> = Vec::new();
        let mut clamped: Vec<String> = Vec::new();
        let mut carry = |name: &str, v: &Value| -> Option<Value> {
            let Some(spec) = manifest.param(name) else {
                dropped.push(name.to_string());
                return None;
            };
            if let Some((v, c)) = fit(&spec.kind, v) {
                if c {
                    clamped.push(name.to_string());
                }
                Some(v)
            } else {
                dropped.push(name.to_string());
                None
            }
        };
        let mut params = helios_core::Params::new();
        for (name, v) in desc.params.iter() {
            if let Some(v) = carry(name, v) {
                params.set(name, v);
            }
        }
        let mut moves = Moves::new();
        // Every value carried is set again, unchanged ones too: the node
        // on another version starts from the network's values.
        for (name, v) in values_of(values, node) {
            moves.insert(node_path(node, name), carry(name, v));
        }
        dropped.sort();
        dropped.dedup();
        clamped.sort();
        clamped.dedup();
        let mut next = self.clone();
        // Presets carry their values the same way.
        let mut preset_dropped = Vec::new();
        for p in &mut next.bundle.template.network.presets {
            let Some(held) = p.values.get_mut(node) else {
                continue;
            };
            let mut kept = helios_core::Params::new();
            for (name, v) in held.iter() {
                match manifest.param(name).and_then(|s| fit(&s.kind, v)) {
                    Some((v, _)) => kept.set(name, v),
                    None => preset_dropped.push(format!("{}: {name}", p.id)),
                }
            }
            *held = kept;
        }
        let n = &mut next.bundle.template.network.nodes[i];
        n.composite = Some(to.clone());
        n.params = params;
        if !next.bundle.composites.iter().any(|f| f.reference() == to) {
            next.bundle.composites.push(file);
        }
        let next = State::checked(next, catalogue)?;
        Ok(Step {
            state: next,
            values: moves,
            answer: json!({
                "node": node,
                "from": r,
                "to": to,
                "dropped": dropped,
                "clamped": clamped,
                "preset_values_dropped": preset_dropped,
            }),
        })
    }

    /// The files `macro.publish` copies into the user's library: the
    /// macro's file at the version `node` pins (its current version), or
    /// the composite `id` of the workspace (`id@version`, else its newest
    /// version), then the composites it nests that Helios does not ship.
    /// Refused for a module, a macro open (fold it first: what changed
    /// inside is not in its file yet) and a composite Helios ships.
    pub fn macro_publishable(
        &self,
        catalogue: &Catalogue,
        node: Option<&str>,
        id: Option<&str>,
    ) -> Result<Vec<CompositeFile>, String> {
        let cat = self.bundle.catalogue(catalogue);
        let r = match (node, id) {
            (Some(node), _) => {
                let (_, desc, r) = self.macro_node(node)?;
                if desc.open {
                    return Err(format!(
                        "`{node}` is open: fold it first, what changed inside is not in its file yet"
                    ));
                }
                r
            }
            (None, Some(id)) => {
                if let Some((id, v)) = id.rsplit_once('@') {
                    CompositeRef {
                        id: id.to_string(),
                        version: v
                            .parse()
                            .map_err(|_| format!("`{id}@{v}`: the version is a whole number"))?,
                    }
                } else {
                    let version = self
                        .bundle
                        .composites
                        .iter()
                        .filter(|f| f.id == id)
                        .map(|f| f.version)
                        .max()
                        .ok_or_else(|| format!("the workspace has no macro `{id}`"))?;
                    CompositeRef {
                        id: id.to_string(),
                        version,
                    }
                }
            }
            (None, None) => return Err("name the macro: `node` or `id`".into()),
        };
        let shipped: BTreeSet<CompositeRef> = crate::shipped::composites()
            .iter()
            .map(CompositeFile::reference)
            .collect();
        if shipped.contains(&r) {
            return Err(format!(
                "`{r}` is shipped with Helios: every workspace has it already"
            ));
        }
        let mut out: Vec<CompositeFile> = Vec::new();
        let mut wanted = vec![r];
        while let Some(r) = wanted.pop() {
            if shipped.contains(&r) || out.iter().any(|f| f.reference() == r) {
                continue;
            }
            let file = cat
                .get_composite(&r)
                .ok_or_else(|| format!("unknown composite `{r}`"))?
                .clone();
            wanted.extend(file.nodes.iter().filter_map(|n| n.composite.clone()));
            out.push(file);
        }
        Ok(out)
    }
}

/// One inner parameter of a macro, as its interface editor lists it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct InnerParam {
    /// The inner node.
    pub node: String,
    pub spec: helios_core::ParamSpec,
    /// Its module's hint, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<helios_core::ParamHint>,
    /// The inner node's value in the macro's file.
    pub value: Value,
}

/// What a macro's interface editor starts from (`State::macro_interface`).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Interface {
    pub node: String,
    /// The macro's file, as pinned.
    pub composite: CompositeRef,
    /// The macro's name, in the user's words.
    pub name: String,
    pub exposed: Vec<Exposed>,
    /// Every inner parameter, by inner node in the file's order.
    pub inner: Vec<InnerParam>,
    /// The file a new interface is saved as.
    pub next: CompositeRef,
}

/// The file a changed interface of `r` is saved as: the next version of
/// a user's macro; a copy of its own for a shipped one, whose id stays
/// the shipped files'.
pub(crate) fn next_ref(cat: &Catalogue, r: &CompositeRef, name: &str) -> CompositeRef {
    if r.id.starts_with(USER_PREFIX) {
        let newest = cat
            .composite_files()
            .filter(|f| f.id == r.id)
            .map(|f| f.version)
            .max()
            .unwrap_or(r.version);
        CompositeRef {
            id: r.id.clone(),
            version: newest.max(r.version) + 1,
        }
    } else {
        CompositeRef {
            id: free_id(cat, name),
            version: 1,
        }
    }
}

impl State {
    /// The interface of the macro `node` and every inner parameter it
    /// could expose: what the interface editor shows.
    pub fn macro_interface(&self, catalogue: &Catalogue, node: &str) -> Result<Interface, String> {
        let cat = self.bundle.catalogue(catalogue);
        let (_, _, r) = self.macro_node(node)?;
        let file = cat
            .get_composite(&r)
            .ok_or_else(|| format!("unknown composite `{r}`"))?;
        let mut inner = Vec::new();
        for n in &file.nodes {
            let m = cat
                .resolve(n.module.as_deref(), n.composite.as_ref(), 1)
                .map_err(|e| format!("`{}`: {e}", n.name))?;
            let hints = m.param_hints();
            for spec in &m.manifest().params {
                inner.push(InnerParam {
                    node: n.name.clone(),
                    spec: spec.clone(),
                    hint: hints.iter().find(|h| h.name == spec.name).cloned(),
                    value: n
                        .params
                        .get(&spec.name)
                        .cloned()
                        .unwrap_or_else(|| spec.kind.default_value()),
                });
            }
        }
        Ok(Interface {
            node: node.to_string(),
            composite: r.clone(),
            name: file.name.clone(),
            exposed: file.exposed.clone(),
            inner,
            next: next_ref(&cat, &r, &file.name),
        })
    }

    /// Gives the macro `node` the interface `exposed` (`macro.interface`):
    /// a file is never rewritten, so the interface is saved as a new
    /// version of the macro's file (a user's copy of a shipped one,
    /// `user.<slug>@1`), the node pinned to it; other nodes keep theirs.
    ///
    /// Values follow the inner parameters, not the names: a parameter
    /// renamed keeps its value under its new name (clamped into a
    /// narrower range), and one no longer exposed is written into the new
    /// file as its inner value (the image's, else the workspace's), so the
    /// image renders the same. Presets' values follow the same way. The
    /// answer says what was renamed, unexposed, clamped and dropped (a
    /// value that no longer fits: an option no longer kept). Refused while
    /// the macro is open, for an unchanged interface, and for one the
    /// inner parameters do not fit (the entry named, `exposed[0]: …`).
    pub fn macro_interfaced(
        &self,
        catalogue: &Catalogue,
        node: &str,
        exposed: &[Exposed],
        values: &Values,
    ) -> Result<Step, String> {
        let cat = self.bundle.catalogue(catalogue);
        let (i, desc, r) = self.macro_node(node)?;
        if desc.open {
            return Err(format!("`{node}` is open: fold it first"));
        }
        let file = cat
            .get_composite(&r)
            .ok_or_else(|| format!("unknown composite `{r}`"))?;
        if file.exposed == exposed {
            return Err(format!("the interface of `{node}` is unchanged"));
        }
        let to = next_ref(&cat, &r, &file.name);
        // Each name exposed before, followed to its inner parameters'
        // name now (the first that shares one of them).
        let mut follows: BTreeMap<&str, &str> = BTreeMap::new();
        for old in &file.exposed {
            if follows.contains_key(old.name.as_str()) {
                continue;
            }
            // A semantic slider follows its name, kept a semantic slider.
            if old.is_driven() {
                if exposed.iter().any(|x| x.is_driven() && x.name == old.name) {
                    follows.insert(old.name.as_str(), old.name.as_str());
                }
                continue;
            }
            let entries: Vec<(&str, &str)> = file
                .exposed
                .iter()
                .filter(|x| x.name == old.name)
                .map(|x| (x.node.as_str(), x.param.as_str()))
                .collect();
            if let Some(new) = exposed
                .iter()
                .find(|x| !x.is_driven() && entries.contains(&(x.node.as_str(), x.param.as_str())))
            {
                follows.insert(old.name.as_str(), new.name.as_str());
            }
        }
        let mut next_file = file.clone();
        next_file.id.clone_from(&to.id);
        next_file.version = to.version;
        next_file.exposed = exposed.to_vec();
        // What is no longer exposed keeps the value it renders with (a
        // semantic slider's inner values, its curves').
        let built = built_if_driven(file, &cat);
        for old in &file.exposed {
            if follows.contains_key(old.name.as_str()) {
                continue;
            }
            let held = values
                .get(&node_path(node, &old.name))
                .or_else(|| desc.params.get(&old.name));
            let Some(v) = held else { continue };
            for (inner, param, v) in inner_values(built.as_ref(), old, v) {
                if let Some(n) = next_file.nodes.iter_mut().find(|n| n.name == inner) {
                    n.params.set(&param, v);
                }
            }
        }
        let built = crate::catalogue::build(&next_file, &cat, 0)?;
        let manifest = helios_core::Composite::manifest(&built).clone();
        let mut dropped: Vec<String> = Vec::new();
        let mut clamped: Vec<String> = Vec::new();
        let mut carry = |name: &str, v: &Value| -> Option<(String, Value)> {
            // What is no longer exposed is in the file now.
            let new = follows.get(name)?;
            let spec = manifest.param(new)?;
            let Some((v, c)) = fit(&spec.kind, v) else {
                dropped.push(name.to_string());
                return None;
            };
            if c {
                clamped.push((*new).to_string());
            }
            Some(((*new).to_string(), v))
        };
        let mut params = helios_core::Params::new();
        for (name, v) in desc.params.iter() {
            if let Some((name, v)) = carry(name, v) {
                params.set(&name, v);
            }
        }
        let mut moves = Moves::new();
        // Every value carried is set again, unchanged ones too: the node
        // on another version starts from the network's values.
        for (name, v) in values_of(values, node) {
            let to = carry(name, v);
            moves.insert(node_path(node, name), None);
            if let Some((n, c)) = to {
                moves.insert(node_path(node, &n), Some(c));
            }
        }
        dropped.sort();
        dropped.dedup();
        clamped.sort();
        clamped.dedup();
        let mut next = self.clone();
        let mut preset_dropped = Vec::new();
        for p in &mut next.bundle.template.network.presets {
            let Some(held) = p.values.get_mut(node) else {
                continue;
            };
            let mut kept = helios_core::Params::new();
            for (name, v) in held.iter() {
                let fitted = follows.get(name).and_then(|new| {
                    let spec = manifest.param(new)?;
                    fit(&spec.kind, v).map(|(v, _)| (*new, v))
                });
                match fitted {
                    Some((new, v)) => kept.set(new, v),
                    None => preset_dropped.push(format!("{}: {name}", p.id)),
                }
            }
            *held = kept;
        }
        let n = &mut next.bundle.template.network.nodes[i];
        n.composite = Some(to.clone());
        n.params = params;
        next.bundle.composites.push(next_file);
        let next = State::checked(next, catalogue)?;
        let unexposed: Vec<&str> = names_of(&file.exposed)
            .into_iter()
            .filter(|n| !follows.contains_key(n))
            .collect();
        let renamed: BTreeMap<&str, &str> = follows
            .iter()
            .filter(|(o, n)| o != n)
            .map(|(o, n)| (*o, *n))
            .collect();
        Ok(Step {
            state: next,
            values: moves,
            answer: json!({
                "node": node,
                "from": r,
                "to": to,
                "exposed": names_of(exposed),
                "renamed": renamed,
                "unexposed": unexposed,
                "dropped": dropped,
                "clamped": clamped,
                "preset_values_dropped": preset_dropped,
            }),
        })
    }
}

/// The names exposed, each once, in the order shown.
fn names_of(exposed: &[Exposed]) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for x in exposed {
        if !names.contains(&x.name.as_str()) {
            names.push(&x.name);
        }
    }
    names
}

/// What folding an opened macro settles: the answer to "it changed while
/// open" (`macro.fold`'s `changes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Changes {
    /// Saved as the file's next version, this node pinned to it.
    Version,
    /// Exposed by the next version, their values kept on the image.
    Expose,
    /// Taken out: back to the file's inside.
    Discard,
    /// Not folded: the macro stays open (the dialog's fourth answer).
    Keep,
}

impl Changes {
    /// `version`, `expose`, `discard` or `keep`; `None` for null.
    pub fn from_arg(v: &Json) -> Result<Option<Changes>, String> {
        match v {
            Json::Null => Ok(None),
            Json::String(s) => match s.as_str() {
                "version" => Ok(Some(Changes::Version)),
                "expose" => Ok(Some(Changes::Expose)),
                "discard" => Ok(Some(Changes::Discard)),
                "keep" => Ok(Some(Changes::Keep)),
                other => Err(format!(
                    "changes: `{other}` is not `version`, `expose`, `discard` or `keep`"
                )),
            },
            _ => Err("changes: `version`, `expose`, `discard` or `keep`".into()),
        }
    }
}

/// The undo label of `macro.fold` with `changes`: "fold look", "fold
/// look as a new version", "fold look, its changes exposed", "fold look,
/// its changes discarded".
pub fn fold_label(node: &str, changes: Option<Changes>) -> String {
    match changes {
        Some(Changes::Version) => format!("fold {node} as a new version"),
        Some(Changes::Expose) => format!("fold {node}, its changes exposed"),
        Some(Changes::Discard) => format!("fold {node}, its changes discarded"),
        None | Some(Changes::Keep) => format!("fold {node}"),
    }
}

const BYPASS_PREFIX: &str = "bypass/";

/// What the image changed inside the opened macro `node` beyond its file:
/// values of inner parameters `file` does not expose, inner nodes
/// bypassed. Sorted paths.
fn changed_inside(file: &CompositeFile, node: &str, values: &Values) -> Vec<String> {
    let prefix = format!("node/{node}.");
    let mut out: Vec<String> = inner_paths(values, node)
        .into_iter()
        .filter(|path| {
            path[prefix.len()..]
                .split_once('/')
                .is_none_or(|(inner, param)| {
                    !file
                        .exposed
                        .iter()
                        .any(|x| x.inner().contains(&(inner, param)))
                })
        })
        .collect();
    let bypass = format!("{BYPASS_PREFIX}{node}.");
    out.extend(
        values
            .iter()
            .filter(|(k, v)| k.starts_with(&bypass) && **v == Value::Bool(true))
            .map(|(k, _)| k.clone()),
    );
    out.sort();
    out
}

/// The names of a bundled composite's inputs or outputs.
fn next_ports(state: &State, r: &CompositeRef, inputs: bool) -> Vec<String> {
    state
        .bundle
        .composites
        .iter()
        .find(|f| f.reference() == *r)
        .map(|f| {
            let ports = if inputs { &f.inputs } else { &f.outputs };
            ports.iter().map(|p| p.name.clone()).collect()
        })
        .unwrap_or_default()
}
