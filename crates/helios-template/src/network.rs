//! A workspace's network edited node by node (`network.*`): connect,
//! disconnect, move, add, remove and paste copied nodes. The desktop's
//! Graph panel and the node's MCP tools (on the desktop holding the
//! workspace, else on its files) run the same edits, each one step,
//! checked like any edit of the workspace.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use helios_core::{PortKind, PortSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::build::SOURCE_PORT;
use crate::catalogue::Catalogue;
use crate::format::{CompositeRef, Connection, NetworkDesc, NodeDesc};
use crate::workspace::State;

/// A port type that takes any image.
pub const ANY: &str = "any";

/// The network's commands, with their titles.
pub const COMMANDS: &[(&str, &str)] = &[
    (
        "network.connect",
        "Connect an output to an input (`from`, `port`, `to`, `input`, `replaces`)",
    ),
    (
        "network.disconnect",
        "Remove wires of the network (`connections`: each `from`, `port`, `to`, `input`)",
    ),
    (
        "network.move",
        "Move nodes in the graph view (`moves`: each `node`, `to`: [x, y])",
    ),
    (
        "network.add",
        "Add nodes to the network (`nodes`: each `module`, `composite` or `copy_of`, `at`: [x, y])",
    ),
    (
        "network.remove",
        "Remove nodes from the network, with their wires (`nodes`)",
    ),
    (
        "network.paste",
        "Paste copied nodes into the network (`clip`: a `helios.nodes` payload; `parts`), renamed on a clash",
    ),
];

/// Whether `id` is one of [`COMMANDS`].
pub fn is_command(id: &str) -> bool {
    COMMANDS.iter().any(|(c, _)| *c == id)
}

// ---------------------------------------------------------------------
// Ports
// ---------------------------------------------------------------------

/// A port of a node of the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortInfo {
    pub name: String,
    /// `image` (four channels), `mask` (one), `<n> channels`, `buffer`,
    /// or [`ANY`].
    pub ty: String,
}

impl PortInfo {
    fn of(spec: &PortSpec) -> PortInfo {
        let ty = match &spec.kind {
            PortKind::Image { channels: 4 } => "image".to_string(),
            PortKind::Image { channels: 1 } => "mask".to_string(),
            PortKind::Image { channels } => format!("{channels} channels"),
            PortKind::AnyImage | PortKind::SameAs { .. } => ANY.to_string(),
            PortKind::Buffer => "buffer".to_string(),
        };
        PortInfo {
            name: spec.name.clone(),
            ty,
        }
    }
}

/// Whether an output of type `from` may feed an input of type `to`.
pub fn types_fit(from: &str, to: &str) -> bool {
    from == ANY || to == ANY || from == to
}

/// Every node's name: the sources, then the nodes, in order.
pub fn names(net: &NetworkDesc) -> Vec<String> {
    net.sources
        .iter()
        .cloned()
        .chain(net.nodes.iter().map(|n| n.name.clone()))
        .collect()
}

/// A node's inputs and outputs, from its module's manifest or its
/// composite's file; a source has one output, its image. `None` for a
/// node the network does not have, or whose module is unknown.
pub fn ports_of(
    net: &NetworkDesc,
    catalogue: &Catalogue,
    node: &str,
) -> Option<(Vec<PortInfo>, Vec<PortInfo>)> {
    if net.sources.iter().any(|s| s == node) {
        let image = PortInfo {
            name: SOURCE_PORT.into(),
            ty: ANY.into(),
        };
        return Some((Vec::new(), vec![image]));
    }
    let n = net.nodes.iter().find(|n| n.name == node)?;
    if let Some(m) = &n.module {
        let manifest = catalogue.get_module(m)?.manifest();
        return Some((
            manifest.inputs.iter().map(PortInfo::of).collect(),
            manifest.outputs.iter().map(PortInfo::of).collect(),
        ));
    }
    let file = catalogue.get_composite(n.composite.as_ref()?)?;
    let of =
        |ports: &[crate::format::Port]| ports.iter().map(|p| PortInfo::of(&p.spec())).collect();
    Some((of(&file.inputs), of(&file.outputs)))
}

/// Whether `upstream` feeds `node`, directly or not.
pub fn feeds(net: &NetworkDesc, upstream: &str, node: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut todo = vec![node.to_string()];
    while let Some(at) = todo.pop() {
        for c in net.connections.iter().filter(|c| c.to == at) {
            if c.from == upstream {
                return true;
            }
            if seen.insert(c.from.clone()) {
                todo.push(c.from.clone());
            }
        }
    }
    false
}

/// The composites a node may be added as: the newest version of each,
/// as `(id@version, name)`.
pub fn addable_composites(catalogue: &Catalogue) -> Vec<(String, String)> {
    catalogue
        .composite_files()
        .filter(|f| {
            let r = CompositeRef {
                id: f.id.clone(),
                version: f.version,
            };
            catalogue.newer(&r).is_none()
        })
        .map(|f| (format!("{}@{}", f.id, f.version), f.name.clone()))
        .collect()
}

/// A composite named `id@version`.
fn composite_ref(kind: &str) -> Option<CompositeRef> {
    let (id, version) = kind.rsplit_once('@')?;
    Some(CompositeRef {
        id: id.to_string(),
        version: version.parse().ok()?,
    })
}

// ---------------------------------------------------------------------
// The edits
// ---------------------------------------------------------------------

/// A connection's four fields; the other keys of a command's arguments
/// (`replaces`) are not the connection's, which refuses unknown fields.
fn conn(v: &Json) -> Result<Connection, String> {
    let only = json!({
        "from": v["from"],
        "port": v["port"],
        "to": v["to"],
        "input": v["input"],
    });
    serde_json::from_value(only).map_err(|e| format!("a connection: {e}"))
}

fn conns(v: &Json) -> Result<Vec<Connection>, String> {
    match v {
        Json::Null => Ok(Vec::new()),
        Json::Array(a) => a.iter().map(conn).collect(),
        _ => Err("connections are a list".into()),
    }
}

fn names_arg(v: &Json, key: &str) -> Result<Vec<String>, String> {
    serde_json::from_value(v[key].clone()).map_err(|e| format!("`{key}`: {e}"))
}

fn pair(v: &Json) -> Option<[f32; 2]> {
    Some([v.get(0)?.as_f64()? as f32, v.get(1)?.as_f64()? as f32])
}

/// Connects an output to an input (`from`, `port`, `to`, `input`): the
/// input's wire, and the wires in `replaces`, removed first. Refused
/// when a port is not there, the types differ or the wire would close a
/// loop.
pub fn connect(
    net: &mut NetworkDesc,
    catalogue: &Catalogue,
    args: &Json,
) -> Result<Connection, String> {
    let c = conn(args)?;
    let replaces = conns(&args["replaces"])?;
    let (_, outputs) =
        ports_of(net, catalogue, &c.from).ok_or_else(|| format!("no node `{}`", c.from))?;
    let (inputs, _) =
        ports_of(net, catalogue, &c.to).ok_or_else(|| format!("no node `{}`", c.to))?;
    let out = outputs
        .iter()
        .find(|p| p.name == c.port)
        .ok_or_else(|| format!("`{}` has no output `{}`", c.from, c.port))?;
    let input = inputs
        .iter()
        .find(|p| p.name == c.input)
        .ok_or_else(|| format!("`{}` has no input `{}`", c.to, c.input))?;
    if c.from == c.to {
        return Err(format!("`{}` cannot feed itself", c.from));
    }
    if !types_fit(&out.ty, &input.ty) {
        return Err(format!(
            "`{}.{}` gives {}, `{}.{}` takes {}",
            c.from, c.port, out.ty, c.to, c.input, input.ty
        ));
    }
    let mut next = net.clone();
    next.connections
        .retain(|k| !(replaces.contains(k) || k.to == c.to && k.input == c.input));
    if feeds(&next, &c.to, &c.from) {
        return Err(format!("`{}` already feeds `{}`", c.to, c.from));
    }
    next.connections.push(c.clone());
    *net = next;
    Ok(c)
}

/// Removes wires (`connections`: each `{from, port, to, input}`); every
/// one must be there.
pub fn disconnect(net: &mut NetworkDesc, args: &Json) -> Result<usize, String> {
    let gone = conns(&args["connections"])?;
    if gone.is_empty() {
        return Err("`connections` is missing".into());
    }
    for c in &gone {
        if !net.connections.contains(c) {
            return Err(format!(
                "no wire from `{}.{}` to `{}.{}`",
                c.from, c.port, c.to, c.input
            ));
        }
    }
    net.connections.retain(|c| !gone.contains(c));
    Ok(gone.len())
}

/// Places nodes (`moves`: each `{node, to: [x, y]}`).
pub fn place(
    net: &NetworkDesc,
    positions: &mut BTreeMap<String, [f32; 2]>,
    args: &Json,
) -> Result<usize, String> {
    let moves = args["moves"].as_array().ok_or("`moves` is a list")?;
    let known = names(net);
    for m in moves {
        let node = m["node"].as_str().ok_or("a move names its `node`")?;
        if !known.iter().any(|n| n == node) {
            return Err(format!("no node `{node}`"));
        }
        let to = pair(&m["to"]).ok_or("a move's `to` is [x, y]")?;
        if !to.iter().all(|v| v.is_finite()) {
            return Err(format!("`{node}`: not a place"));
        }
        positions.insert(node.to_string(), to);
    }
    Ok(moves.len())
}

/// A name for a new node: `base`, else `base_2`, `base_3`...
pub fn fresh_name(net: &NetworkDesc, base: &str) -> String {
    let mut base: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if !base.starts_with(|c: char| c.is_ascii_lowercase()) {
        base.insert(0, 'n');
    }
    let taken = names(net);
    if !taken.contains(&base) {
        return base;
    }
    // Among `taken.len() + 1` names, one is free.
    (2..=taken.len() + 2)
        .map(|i| format!("{base}_{i}"))
        .find(|n| !taken.contains(n))
        .expect("a free name")
}

/// Adds nodes (`nodes`: each `{module, composite, at, copy_of}`): a
/// module by its name, a composite by `id@version` (the graph editor's
/// `kind` names either), or a copy of another node (its module or
/// composite and its parameters). Answers the names given.
pub fn add(
    net: &mut NetworkDesc,
    positions: &mut BTreeMap<String, [f32; 2]>,
    catalogue: &Catalogue,
    args: &Json,
) -> Result<Vec<String>, String> {
    let nodes = args["nodes"].as_array().ok_or("`nodes` is a list")?;
    let mut added = Vec::new();
    for n in nodes {
        let copy = n["copy_of"].as_str().filter(|s| !s.is_empty());
        let desc = if let Some(of) = copy {
            let orig = net
                .nodes
                .iter()
                .find(|d| d.name == of)
                .ok_or_else(|| format!("no node `{of}` to copy (a source is not copied)"))?;
            NodeDesc {
                name: fresh_name(net, &orig.name),
                ..orig.clone()
            }
        } else if let Some(c) = n["composite"].as_str().filter(|s| !s.is_empty()) {
            new_composite(net, catalogue, c)?
        } else {
            let kind = n["module"]
                .as_str()
                .or_else(|| n["kind"].as_str())
                .ok_or("a node to add names its `module` or `composite`")?;
            if catalogue.get_module(kind).is_some() {
                NodeDesc {
                    name: fresh_name(net, kind),
                    module: Some(kind.to_string()),
                    ..NodeDesc::default()
                }
            } else if composite_ref(kind).is_some() {
                new_composite(net, catalogue, kind)?
            } else {
                return Err(format!("unknown module `{kind}`"));
            }
        };
        if let Some(at) = pair(&n["at"]) {
            positions.insert(desc.name.clone(), at);
        }
        added.push(desc.name.clone());
        net.nodes.push(desc);
    }
    Ok(added)
}

/// A new node of a composite named `id@version`, named after the last
/// word of its id.
fn new_composite(net: &NetworkDesc, catalogue: &Catalogue, kind: &str) -> Result<NodeDesc, String> {
    let r = composite_ref(kind).ok_or_else(|| format!("`{kind}` is not `id@version`"))?;
    if catalogue.get_composite(&r).is_none() {
        return Err(format!("unknown composite `{kind}`"));
    }
    let base = r.id.rsplit('.').next().unwrap_or(&r.id).to_string();
    Ok(NodeDesc {
        name: fresh_name(net, &base),
        composite: Some(r),
        ..NodeDesc::default()
    })
}

/// Removes nodes (`nodes`) with their wires and preset values. A source
/// is the workspace's image and stays; so does the node the viewport
/// shows.
pub fn remove(net: &mut NetworkDesc, args: &Json) -> Result<usize, String> {
    let nodes = names_arg(args, "nodes")?;
    for n in &nodes {
        if net.sources.contains(n) {
            return Err(format!("`{n}` is an image of the workspace: it stays"));
        }
        if net.view.node == *n {
            return Err(format!("`{n}` is what the viewport shows: it stays"));
        }
        if !net.nodes.iter().any(|d| d.name == *n) {
            return Err(format!("no node `{n}`"));
        }
    }
    net.nodes.retain(|d| !nodes.contains(&d.name));
    net.connections
        .retain(|c| !nodes.contains(&c.from) && !nodes.contains(&c.to));
    for p in &mut net.presets {
        p.values.retain(|k, _| !nodes.contains(k));
    }
    Ok(nodes.len())
}

// ---------------------------------------------------------------------
// Copied nodes
// ---------------------------------------------------------------------

/// How far a pasted node is placed from where it was copied.
pub const SHIFT: f32 = 40.0;

/// The part of paste special that wires the cut inputs again.
pub const INPUTS: &str = "inputs";

/// What a copy of nodes holds (the clipboard's `helios.nodes`): the
/// nodes, the wires between them, the wires coming in from nodes not
/// copied, and their places in the graph view.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub nodes: Vec<NodeDesc>,
    /// The wires between the nodes copied.
    #[serde(default)]
    pub connections: Vec<Connection>,
    /// The wires into them from nodes not copied.
    #[serde(default)]
    pub inputs: Vec<Connection>,
    /// Their places in the graph view.
    #[serde(default)]
    pub positions: BTreeMap<String, [f32; 2]>,
}

impl Clip {
    /// Its nodes' names.
    pub fn names(&self) -> Vec<String> {
        self.nodes.iter().map(|n| n.name.clone()).collect()
    }

    /// Its text fallback: the nodes' names.
    pub fn text(&self) -> String {
        self.names().join(", ")
    }

    /// What it is called ("a node", "3 nodes").
    pub fn title(&self) -> String {
        match self.nodes.len() {
            1 => "a node".into(),
            n => format!("{n} nodes"),
        }
    }

    pub fn to_json(&self) -> Json {
        serde_json::to_value(self).unwrap_or_default()
    }

    pub fn from_json(data: &Json) -> Result<Clip, String> {
        serde_json::from_value(data.clone()).map_err(|e| format!("not nodes: {e}"))
    }
}

/// What a paste did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Pasted {
    /// The names given, in the clip's order.
    pub added: Vec<String>,
    /// The names changed on a clash: copied → given.
    pub renamed: BTreeMap<String, String>,
    /// The cut wires wired again.
    pub reconnected: usize,
}

impl Pasted {
    /// In words, for paste special's preview.
    pub fn describe(&self) -> String {
        let mut s = format!("adds {}", self.added.join(", "));
        if !self.renamed.is_empty() {
            let r: Vec<String> = self
                .renamed
                .iter()
                .map(|(a, b)| format!("{a} as {b}"))
                .collect();
            let _ = write!(s, " ({})", r.join(", "));
        }
        if self.reconnected > 0 {
            let _ = write!(s, ", wires {} input(s) again", self.reconnected);
        }
        s
    }
}

/// Pastes nodes into a network: the nodes `parts` names (all when
/// `None`), renamed on a clash, their wires rewritten, the cut inputs
/// wired again when [`INPUTS`] is wanted and the node they came from is
/// there.
pub fn paste(
    net: &mut NetworkDesc,
    positions: &mut BTreeMap<String, [f32; 2]>,
    clip: &Clip,
    parts: Option<&[String]>,
) -> Result<Pasted, String> {
    let wants = |p: &str| parts.is_none_or(|ps| ps.iter().any(|x| x == p));
    let before = names(net);
    let mut out = Pasted::default();
    let mut given: BTreeMap<String, String> = BTreeMap::new();
    for n in clip.nodes.iter().filter(|n| wants(&n.name)) {
        let name = fresh_name(net, &n.name);
        if name != n.name {
            out.renamed.insert(n.name.clone(), name.clone());
        }
        if let Some(p) = clip.positions.get(&n.name) {
            positions.insert(name.clone(), [p[0] + SHIFT, p[1] + SHIFT]);
        }
        given.insert(n.name.clone(), name.clone());
        out.added.push(name.clone());
        net.nodes.push(NodeDesc { name, ..n.clone() });
    }
    if out.added.is_empty() {
        return Err("no node to paste".into());
    }
    for c in &clip.connections {
        if let (Some(from), Some(to)) = (given.get(&c.from), given.get(&c.to)) {
            net.connections.push(Connection {
                from: from.clone(),
                to: to.clone(),
                ..c.clone()
            });
        }
    }
    if wants(INPUTS) {
        for c in &clip.inputs {
            if let Some(to) = given.get(&c.to)
                && before.contains(&c.from)
            {
                net.connections.push(Connection {
                    to: to.clone(),
                    ..c.clone()
                });
                out.reconnected += 1;
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// One command, on a workspace's state
// ---------------------------------------------------------------------

/// The undo step's label of a command.
pub fn label(id: &str, args: &Json) -> Result<String, String> {
    Ok(match id {
        "network.connect" => format!(
            "connect {}.{} to {}.{}",
            args["from"].as_str().unwrap_or_default(),
            args["port"].as_str().unwrap_or_default(),
            args["to"].as_str().unwrap_or_default(),
            args["input"].as_str().unwrap_or_default(),
        ),
        "network.disconnect" => "disconnect".to_string(),
        "network.move" => "move nodes".to_string(),
        "network.add" => "add nodes".to_string(),
        "network.remove" => "remove nodes".to_string(),
        "network.paste" => "paste nodes".to_string(),
        other => return Err(format!("no network command `{other}`")),
    })
}

/// A workspace's state with one of [`COMMANDS`] run on its network, and
/// the answer (`added`: the names given, `renamed`: a paste's names
/// changed on a clash). Not checked against the catalogue: the caller
/// does, as for any edit of the workspace.
pub fn edited(
    now: &State,
    catalogue: &Catalogue,
    id: &str,
    args: &Json,
) -> Result<(State, Json), String> {
    let t = &now.bundle.template;
    let mut net = t.network.clone();
    let mut positions = t.positions.clone();
    let mut answer = json!({});
    match id {
        "network.connect" => {
            connect(&mut net, catalogue, args)?;
        }
        "network.disconnect" => {
            disconnect(&mut net, args)?;
        }
        "network.move" => {
            place(&net, &mut positions, args)?;
        }
        "network.add" => {
            answer["added"] = json!(add(&mut net, &mut positions, catalogue, args)?);
        }
        "network.remove" => {
            remove(&mut net, args)?;
        }
        "network.paste" => {
            let clip = Clip::from_json(&args["clip"])?;
            let parts: Option<Vec<String>> = match &args["parts"] {
                Json::Null => None,
                v => Some(serde_json::from_value(v.clone()).map_err(|e| format!("`parts`: {e}"))?),
            };
            let out = paste(&mut net, &mut positions, &clip, parts.as_deref())?;
            answer["added"] = json!(out.added);
            if !out.renamed.is_empty() {
                answer["renamed"] = json!(out.renamed);
            }
        }
        other => return Err(format!("no network command `{other}`")),
    }
    let next = now.edited(&json!({ "network": net, "positions": positions }))?;
    Ok((next, answer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogue() -> Catalogue {
        let mut c = Catalogue::new()
            .module(helios_core::ModuleRef::new(
                helios_module_exposure::Exposure,
            ))
            .module(helios_core::ModuleRef::new(helios_module_sigmoid::Sigmoid))
            .module(helios_core::ModuleRef::new(helios_module_masks::MaskBlend));
        for file in crate::shipped::composites() {
            c.add_composite(file);
        }
        c
    }

    #[test]
    fn a_composite_is_added_by_its_id_and_version() {
        let c = catalogue();
        let (kind, _) = addable_composites(&c)
            .into_iter()
            .next()
            .expect("a composite is shipped");
        let mut net = crate::workspace::blank_network();
        let mut positions = BTreeMap::new();
        let added = add(
            &mut net,
            &mut positions,
            &c,
            &json!({ "nodes": [{ "module": kind, "at": [0.0, 0.0] }] }),
        )
        .unwrap();
        let node = net.nodes.iter().find(|n| n.name == added[0]).unwrap();
        assert_eq!(node.composite.as_ref().map(ToString::to_string), Some(kind));
        assert!(ports_of(&net, &c, &added[0]).is_some());
        let unknown = add(
            &mut net,
            &mut positions,
            &c,
            &json!({ "nodes": [{ "composite": "nothing@1" }] }),
        );
        assert!(unknown.is_err(), "{unknown:?}");
    }
}
