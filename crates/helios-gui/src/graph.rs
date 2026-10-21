//! The Graph panel (`helios.graph`): the workspace's network drawn by
//! egui-pop's node graph editor (`egui_pop::viewport::graph`), through a
//! [`GraphModel`] adapter over the workspace's state, which stays the
//! one source of truth (see `docs/gui.md`, "Graph view").
//!
//! - **Nodes, ports and wires** are the network's: its sources, its
//!   modules and composites with their manifests' ports, its
//!   connections. A node stands where the workspace's `positions` put
//!   it, else at its rank (a layered layout).
//! - **Headers** carry the engine's normalized status (`Engine::status`)
//!   for the image shown: its state and where it runs.
//! - **Stacks**: every node that is not principal (`Graph::layout`'s
//!   rule) is grouped under the principal node it leads to, so the
//!   overview folds it into that node's stack; a top sub-workspace is a
//!   group of its own, entered by a double click on its title or stack.
//! - **Edits** go through egui-pop's `graph.*` steps, then become
//!   Helios's `network.*` commands, each one undoable workspace step that
//!   agents see; a parameter's slider writes the value store as one
//!   gesture, as the Parameters panel does.
//! - **Selection** is the value store's `view/graph/selection`, shared
//!   with the Parameters, Masks and Modules panels: both ways.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use egui::Ui;
use egui_pop::api::AppHandle;
use egui_pop::viewport::graph::{
    Edge, Edit, GraphEditor, GraphEvent, GraphModel, GraphState, Group, Kind, Level, Node, Param,
    Port, PortRef, PortType, Refusal, Request, WireStyle,
};
use helios_core::{Engine, Graph, Message, ModuleKind, NodeState, PortKind, PortSpec, Region};
use helios_shell::commands::Origin;
use helios_template::Catalogue;
use helios_template::format::{Connection, NetworkDesc, NodeDesc};
use helios_template::{SOURCE_PORT, Template};
use helios_values::{Change, Descriptor, GestureId, Set, Store, Type};
use serde_json::{Value as Json, json};

/// The panel.
pub const PANEL: &str = "helios.graph";

/// The editor's name to egui-pop's `graph.*` steps and commands
/// (`graph.read`, `graph.edit`, `graph.select`, `graph.view`).
pub const GRAPH: &str = "helios.graph";

/// How the graph's wires are drawn: `curves` (the default),
/// `orthogonal` (routed around the nodes) or `straight`.
pub const WIRES: &str = "view/graph/wires";

/// The place a delegated group enters, once Helios declares it.
pub const SUBWORKSPACE_PLACE: &str = "helios.sub-workspace";

/// A port type that takes any other (an input whose channels are
/// decided when connected, a source whose image may be anything).
const ANY: &str = "any";

/// The layered layout's steps, in graph units.
const RANK_STEP: f32 = 260.0;
const ROW_STEP: f32 = 160.0;

/// The editor's node width (egui-pop's default) and the room above a
/// group's nodes taken by its frame and title, for the double click on
/// a sub-workspace.
const NODE_WIDTH: f32 = 180.0;
const GROUP_MARGIN: f32 = 14.0;
const GROUP_TITLE: f32 = 24.0;
const NODE_HEIGHT: f32 = 64.0;

fn lock(m: &Mutex<Store>) -> std::sync::MutexGuard<'_, Store> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Declares the panel's values: the wires' style.
pub fn declare(store: &mut Store) {
    store.declare(
        WIRES,
        Descriptor::choice("wires", &["curves", "orthogonal", "straight"], "curves").transient(),
    );
}

/// The wire style a value names (curves for anything else).
pub fn wire_style(name: &str) -> WireStyle {
    match name {
        "orthogonal" => WireStyle::Orthogonal,
        "straight" => WireStyle::Straight,
        _ => WireStyle::Curves,
    }
}

// ---------------------------------------------------------------------
// The network, read
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
    let of = |ports: &[helios_template::format::Port]| {
        ports.iter().map(|p| PortInfo::of(&p.spec())).collect()
    };
    Some((of(&file.inputs), of(&file.outputs)))
}

/// Whether `upstream` feeds `node`, directly or not.
fn feeds(net: &NetworkDesc, upstream: &str, node: &str) -> bool {
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

/// A node's rank in the layered layout.
fn rank_of(i: usize, inputs: &[Vec<usize>], memo: &mut [Option<usize>], depth: usize) -> usize {
    if let Some(r) = memo[i] {
        return r;
    }
    // A cycle (a hand-written network) never recurses forever.
    if depth > inputs.len() {
        return 0;
    }
    let r = inputs[i]
        .iter()
        .map(|&f| rank_of(f, inputs, memo, depth + 1) + 1)
        .max()
        .unwrap_or(0);
    memo[i] = Some(r);
    r
}
/// The principal node a node is stacked under.
fn stack_of(
    i: usize,
    principal: &[bool],
    consumers: &[Vec<usize>],
    memo: &mut [Option<usize>],
    depth: usize,
) -> usize {
    if let Some(g) = memo[i] {
        return g;
    }
    let g = match consumers[i].iter().min() {
        Some(&first) if !principal[i] && depth <= principal.len() => {
            stack_of(first, principal, consumers, memo, depth + 1)
        }
        _ => i,
    };
    memo[i] = Some(g);
    g
}

/// Where a node goes when nothing placed it: its rank, whether it is
/// principal, and the principal node it is stacked under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub node: String,
    pub rank: usize,
    pub principal: bool,
    /// The principal node it leads to (itself when principal).
    pub stack: String,
}

/// The network's layout, as `Graph::layout` gives it for a built graph:
/// a rank per node (0 without inputs, else one more than the deepest
/// input), principal nodes (sources, merges, decisions and sinks, the
/// network's outputs and the node the viewport shows), every other node
/// stacked under its first consumer's principal node.
pub fn placements(net: &NetworkDesc, catalogue: &Catalogue) -> Vec<Placement> {
    let order = names(net);
    let index: BTreeMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let n = order.len();
    let mut inputs: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut consumers: Vec<Vec<usize>> = vec![Vec::new(); n];
    for c in &net.connections {
        if let (Some(&f), Some(&t)) = (index.get(c.from.as_str()), index.get(c.to.as_str())) {
            inputs[t].push(f);
            consumers[f].push(t);
        }
    }
    let declared = |i: usize| -> bool {
        if i < net.sources.len() {
            return true;
        }
        let node = &net.nodes[i - net.sources.len()];
        node.module
            .as_deref()
            .and_then(|m| catalogue.get_module(m))
            .is_some_and(|m| m.manifest().kind != ModuleKind::Process)
    };
    let principal: Vec<bool> = (0..n)
        .map(|i| {
            declared(i)
                || inputs[i].len() >= 2
                || consumers[i].is_empty()
                || order[i] == net.view.node
        })
        .collect();
    let mut ranks = vec![None; n];
    let mut stacks = vec![None; n];
    (0..n)
        .map(|i| Placement {
            node: order[i].clone(),
            rank: rank_of(i, &inputs, &mut ranks, 0),
            principal: principal[i],
            stack: order[stack_of(i, &principal, &consumers, &mut stacks, 0)].clone(),
        })
        .collect()
}

/// The group id of a principal node's stack.
pub fn stack_group(principal: &str) -> String {
    format!("stack:{principal}")
}

/// The group id of a sub-workspace.
pub fn subworkspace_group(id: &str) -> String {
    format!("subworkspace:{id}")
}

// ---------------------------------------------------------------------
// The adapter
// ---------------------------------------------------------------------

/// The workspace's network as egui-pop's editor reads it: built from the
/// workspace's template (its network, positions and sub-workspaces) and
/// the module catalogue, its parameters from the value store.
#[derive(Clone, Debug, Default)]
pub struct NetworkModel {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    groups: Vec<Group>,
    kinds: Vec<Kind>,
    params: BTreeMap<String, Vec<Param>>,
    /// The top sub-workspaces' ids by group id.
    subworkspaces: BTreeMap<String, String>,
}

impl NetworkModel {
    /// The model of a template's network; `moved` overrides positions
    /// (moves handed to the workspace, not applied yet).
    pub fn new(
        t: &Template,
        catalogue: &Catalogue,
        moved: &BTreeMap<String, [f32; 2]>,
    ) -> NetworkModel {
        let net = &t.network;
        let placements = placements(net, catalogue);
        // The top sub-workspaces, and the node each holds.
        let mut delegated: BTreeMap<String, String> = BTreeMap::new();
        let mut subworkspaces = BTreeMap::new();
        let mut groups = Vec::new();
        for s in t.subworkspaces.iter().filter(|s| s.parent.is_none()) {
            let group = subworkspace_group(&s.id);
            for n in &s.nodes {
                let top = n.split('.').next().unwrap_or(n);
                delegated.insert(top.to_string(), group.clone());
            }
            subworkspaces.insert(group.clone(), s.id.clone());
            groups.push(Group::new(group, s.name.clone()));
        }
        let mut rows: BTreeMap<usize, usize> = BTreeMap::new();
        let mut stacks: BTreeSet<String> = BTreeSet::new();
        let mut nodes = Vec::new();
        for p in &placements {
            let (inputs, outputs) = ports_of(net, catalogue, &p.node).unwrap_or_default();
            let kind = net
                .nodes
                .iter()
                .find(|n| n.name == p.node)
                .map_or_else(|| "source".to_string(), kind_of);
            let row = rows.entry(p.rank).or_default();
            let auto = [p.rank as f32 * RANK_STEP, *row as f32 * ROW_STEP];
            *row += 1;
            let pos = moved
                .get(&p.node)
                .or_else(|| t.positions.get(&p.node))
                .copied()
                .unwrap_or(auto);
            let mut node = Node::new(&p.node, kind, &p.node, pos);
            let port = |i: &PortInfo| Port::new(&i.name, &i.name, PortType::new(&i.ty));
            node.inputs = inputs.iter().map(port).collect();
            node.outputs = outputs.iter().map(port).collect();
            if let Some(g) = delegated.get(&p.node) {
                node.group = Some(g.clone());
            } else if !p.principal {
                let g = stack_group(&p.stack);
                stacks.insert(p.stack.clone());
                node.group = Some(g);
            }
            nodes.push(node);
        }
        groups.extend(
            stacks
                .into_iter()
                .map(|principal| Group::new(stack_group(&principal), principal)),
        );
        let edges = net.connections.iter().map(edge_of).collect();
        let kinds = catalogue.module_names().map(|m| Kind::new(m, m)).collect();
        NetworkModel {
            nodes,
            edges,
            groups,
            kinds,
            params: BTreeMap::new(),
            subworkspaces,
        }
    }

    /// The nodes' number parameters, from the value store (an image's
    /// values, bound while it is open), shown in the detail view.
    pub fn with_params(mut self, ctx: &egui::Context, store: &Store) -> NetworkModel {
        for node in &self.nodes {
            let prefix = format!("node/{}/", node.id);
            let params: Vec<Param> = store
                .list(&prefix)
                .into_iter()
                .filter_map(|(path, e)| {
                    let d = &e.descriptor;
                    let value = match (&d.ty, &e.value) {
                        (Type::Float, helios_core::Value::Float(v)) => *v,
                        (Type::Int, helios_core::Value::Int(v)) => *v as f64,
                        _ => return None,
                    };
                    let (lo, hi) = d.soft.or(d.hard)?;
                    let name = path.strip_prefix(&prefix)?;
                    let mut p =
                        Param::new(name, crate::params::label(ctx, path, d), value, lo..=hi)
                            .step(d.step);
                    if let Some(u) = &d.unit {
                        p = p.unit(u);
                    }
                    match d.default {
                        helios_core::Value::Float(v) => p = p.default(v),
                        helios_core::Value::Int(v) => p = p.default(v as f64),
                        _ => {}
                    }
                    Some(p)
                })
                .collect();
            if !params.is_empty() {
                self.params.insert(node.id.clone(), params);
            }
        }
        self
    }

    /// The sub-workspace a group stands for.
    pub fn subworkspace_of_group(&self, group: &str) -> Option<&str> {
        self.subworkspaces.get(group).map(String::as_str)
    }

    /// Whether the network has a node.
    pub fn has(&self, node: &str) -> bool {
        self.nodes.iter().any(|n| n.id == node)
    }

    /// The sub-workspace whose title (or, in the overview, whose stack)
    /// holds a point of the graph.
    fn subworkspace_at(&self, at: egui::Pos2, level: Level) -> Option<String> {
        for (group, id) in &self.subworkspaces {
            let members: Vec<&Node> = self
                .nodes
                .iter()
                .filter(|n| n.group.as_deref() == Some(group.as_str()))
                .collect();
            let Some(first) = members.first() else {
                continue;
            };
            let mut min = egui::pos2(first.pos[0], first.pos[1]);
            let mut max = min;
            for n in &members {
                min = min.min(egui::pos2(n.pos[0], n.pos[1]));
                max = max.max(egui::pos2(n.pos[0] + NODE_WIDTH, n.pos[1] + NODE_HEIGHT));
            }
            let frame = egui::Rect::from_min_max(min, max).expand(GROUP_MARGIN);
            let hit = match level {
                Level::Overview => frame.expand2(egui::vec2(0.0, GROUP_TITLE)).contains(at),
                _ => egui::Rect::from_min_max(
                    egui::pos2(frame.min.x, frame.min.y - GROUP_TITLE),
                    egui::pos2(frame.max.x, frame.min.y),
                )
                .contains(at),
            };
            if hit {
                return Some(id.clone());
            }
        }
        None
    }
}

/// What a node is: its module, or its composite (`id@version`).
fn kind_of(n: &NodeDesc) -> String {
    match (&n.module, &n.composite) {
        (Some(m), _) => m.clone(),
        (None, Some(c)) => c.to_string(),
        (None, None) => String::new(),
    }
}

/// A connection as the editor's wire.
pub fn edge_of(c: &Connection) -> Edge {
    Edge::new(
        PortRef::new(&c.from, &c.port),
        PortRef::new(&c.to, &c.input),
    )
}

/// The editor's wire as a connection.
pub fn connection_of(e: &Edge) -> Json {
    json!({ "from": e.from.node, "port": e.from.port, "to": e.to.node, "input": e.to.port })
}

impl GraphModel for NetworkModel {
    fn nodes(&self) -> Vec<Node> {
        self.nodes.clone()
    }

    fn edges(&self) -> Vec<Edge> {
        self.edges.clone()
    }

    fn groups(&self) -> Vec<Group> {
        self.groups.clone()
    }

    fn params(&self, node: &str) -> Vec<Param> {
        self.params.get(node).cloned().unwrap_or_default()
    }

    fn kinds(&self) -> Vec<Kind> {
        self.kinds.clone()
    }

    fn can_connect(&self, from: (&Node, &Port), to: (&Node, &Port)) -> Result<(), Refusal> {
        if from.0.id == to.0.id {
            return Err(Refusal::SameNode);
        }
        if !types_fit(&from.1.ty.name, &to.1.ty.name) {
            return Err(Refusal::Types {
                from: from.1.ty.name.clone(),
                to: to.1.ty.name.clone(),
            });
        }
        // A wire from a node fed by the input's node would close a loop.
        let mut todo = vec![from.0.id.clone()];
        let mut seen = BTreeSet::new();
        while let Some(at) = todo.pop() {
            for e in self.edges.iter().filter(|e| e.to.node == at) {
                if e.from.node == to.0.id {
                    return Err(Refusal::Other(format!(
                        "{} already feeds {}",
                        to.0.id, from.0.id
                    )));
                }
                if seen.insert(e.from.node.clone()) {
                    todo.push(e.from.node.clone());
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// The network, edited
// ---------------------------------------------------------------------

fn conn(v: &Json) -> Result<Connection, String> {
    serde_json::from_value(v.clone()).map_err(|e| format!("a connection: {e}"))
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
    net.connections
        .retain(|k| !(replaces.contains(k) || k.to == c.to && k.input == c.input));
    if feeds(net, &c.to, &c.from) {
        return Err(format!("`{}` already feeds `{}`", c.to, c.from));
    }
    net.connections.push(c.clone());
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
fn fresh_name(net: &NetworkDesc, base: &str) -> String {
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

/// Adds nodes (`nodes`: each `{module, at, copy_of}`): a module by its
/// name, or a copy of another node (its module or composite and its
/// parameters). Answers the names given.
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
        } else {
            let module = n["module"]
                .as_str()
                .or_else(|| n["kind"].as_str())
                .ok_or("a node to add names its `module`")?;
            if catalogue.get_module(module).is_none() {
                return Err(format!("unknown module `{module}`"));
            }
            NodeDesc {
                name: fresh_name(net, module),
                module: Some(module.to_string()),
                ..NodeDesc::default()
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

/// The command an editor's edit becomes (`None`: a parameter, written to
/// the value store instead).
pub fn command_of(edit: &Edit) -> Option<(&'static str, Json)> {
    Some(match edit {
        Edit::Connect { from, to, replaces } => (
            "network.connect",
            json!({
                "from": from.node,
                "port": from.port,
                "to": to.node,
                "input": to.port,
                "replaces": replaces.iter().map(connection_of).collect::<Vec<_>>(),
            }),
        ),
        Edit::Disconnect { edges } => (
            "network.disconnect",
            json!({ "connections": edges.iter().map(connection_of).collect::<Vec<_>>() }),
        ),
        Edit::Move { moves } => (
            "network.move",
            json!({
                "moves": moves
                    .iter()
                    .map(|m| json!({ "node": m.node, "to": [m.to[0], m.to[1]] }))
                    .collect::<Vec<_>>(),
            }),
        ),
        Edit::Delete { nodes, .. } => ("network.remove", json!({ "nodes": nodes })),
        Edit::Add { nodes } => (
            "network.add",
            json!({
                "nodes": nodes
                    .iter()
                    .map(|n| json!({ "module": n.kind, "at": [n.at[0], n.at[1]], "copy_of": n.copy_of }))
                    .collect::<Vec<_>>(),
            }),
        ),
        Edit::Param { .. } => return None,
    })
}

// ---------------------------------------------------------------------
// The engine's status
// ---------------------------------------------------------------------

/// What a node's header shows: the engine's state for the image shown,
/// where it runs, and more on hover.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub state: NodeState,
    pub runs_on: String,
    /// The content key and the load, or why it failed.
    pub detail: String,
}

/// Every node's header for a render of `region`: the status of each
/// node with what it reads (the last nodes first, so most come from one
/// planning); a node that cannot be planned is failed, with the reason.
pub fn statuses(engine: &Engine, graph: &Graph, region: Region) -> BTreeMap<String, Header> {
    let mut out = BTreeMap::new();
    let ids: Vec<_> = graph.ids().collect();
    for id in ids.into_iter().rev() {
        let Ok(name) = graph.name(id) else { continue };
        if out.contains_key(name) {
            continue;
        }
        match engine.status(graph, id, region) {
            Ok(list) => {
                for s in list {
                    out.entry(s.name.clone()).or_insert_with(|| Header {
                        state: s.state,
                        runs_on: s.runs_on.to_string(),
                        detail: format!(
                            "{} ({} requests, {} computed)",
                            s.key, s.load.requests, s.load.computed
                        ),
                    });
                }
            }
            Err(e) => {
                out.insert(
                    name.to_string(),
                    Header {
                        state: NodeState::Failed,
                        runs_on: String::new(),
                        detail: e.to_string(),
                    },
                );
            }
        }
    }
    out
}

/// A state in the interface's language.
fn state_text(ctx: &egui::Context, s: NodeState) -> String {
    let m = match s {
        NodeState::Idle => Message::new("gui-graph-state-idle", "idle"),
        NodeState::Stale => Message::new("gui-graph-state-stale", "stale"),
        NodeState::Running => Message::new("gui-graph-state-running", "running"),
        NodeState::Done => Message::new("gui-graph-state-done", "done"),
        NodeState::Failed => Message::new("gui-graph-state-failed", "failed"),
        NodeState::Pending => Message::new("gui-graph-state-pending", "waiting"),
    };
    crate::i18n::text(ctx, m)
}

/// A header's text: `done · CPU`.
pub fn header_text(ctx: &egui::Context, h: &Header) -> String {
    let state = state_text(ctx, h.state);
    if h.runs_on.is_empty() {
        state
    } else {
        format!("{state} · {}", h.runs_on.to_uppercase())
    }
}

fn header_ui(ui: &mut Ui, h: &Header) {
    let tokens = crate::theme::Tokens::of(ui.ctx());
    let extra = crate::theme::Extra::of(&tokens);
    let color = match h.state {
        NodeState::Done => extra.state_done,
        NodeState::Running => extra.state_running,
        NodeState::Stale => extra.state_stale,
        NodeState::Failed => extra.state_failed,
        NodeState::Pending => extra.state_pending,
        NodeState::Idle => tokens.text.muted,
    };
    let text = header_text(ui.ctx(), h);
    let r = ui.label(egui::RichText::new(text).small().color(color));
    if !h.detail.is_empty() {
        r.on_hover_text(&h.detail);
    }
}

// ---------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------

/// What the panel keeps between frames: the editor's view, never the
/// network.
pub struct GraphPanel {
    state: GraphState,
    app: Option<AppHandle>,
    catalogue: Catalogue,
    /// The workspace's state as last read (its sequence number), and its
    /// template.
    seen: Option<u64>,
    template: Option<Template>,
    /// Moves handed to the workspace, shown until its state changes.
    moved: BTreeMap<String, [f32; 2]>,
    /// The selection value as last followed.
    selection: Option<String>,
    /// A parameter's slider dragged: its path and gesture.
    gesture: Option<(String, GestureId)>,
    headers: BTreeMap<String, Header>,
    /// When the headers were read, for which graph.
    headers_at: Option<(u64, f64)>,
    /// Commands for the shell to run (`network.*`), in order.
    commands: Vec<(String, Json)>,
    /// A sub-workspace asked to be entered while its place is missing.
    pub notes: Vec<Message>,
}

impl Default for GraphPanel {
    fn default() -> Self {
        GraphPanel {
            state: GraphState::new(),
            app: None,
            catalogue: crate::templates::modules(),
            seen: None,
            template: None,
            moved: BTreeMap::new(),
            selection: None,
            gesture: None,
            headers: BTreeMap::new(),
            headers_at: None,
            commands: Vec::new(),
            notes: Vec::new(),
        }
    }
}

impl GraphPanel {
    /// The application, for the editor's steps and the context stack.
    pub fn set_app(&mut self, app: AppHandle) {
        self.app = Some(app);
    }

    /// The commands the editor's edits became, for the shell to run.
    pub fn take_commands(&mut self) -> Vec<(String, Json)> {
        std::mem::take(&mut self.commands)
    }

    /// The editor's state (its view, selection and folds).
    pub fn state(&self) -> &GraphState {
        &self.state
    }

    /// Reads the workspace's template again when its state changed.
    fn follow_workspace(&mut self, store: &Store) {
        let entry = store.get(crate::shell::WORKSPACE_STATE);
        let seq = entry.map(|e| e.seq);
        if seq == self.seen && (self.template.is_some() || entry.is_none()) {
            return;
        }
        self.seen = seq;
        self.moved.clear();
        self.template = entry.and_then(|e| match &e.value {
            helios_core::Value::Text(t) => serde_json::from_str::<Json>(t)
                .ok()
                .and_then(|j| serde_json::from_value(j["template"].clone()).ok()),
            _ => None,
        });
    }

    /// The headers, read again a few times a second.
    fn follow_engine(&mut self, now: f64, viewport: &crate::viewport::Viewport) {
        let Some(graph) = viewport.graph() else {
            self.headers.clear();
            return;
        };
        let fresh = self
            .headers_at
            .is_some_and(|(g, at)| g == graph.id() && now - at < 0.25);
        if fresh {
            return;
        }
        self.headers_at = Some((graph.id(), now));
        self.headers = match viewport.image_size() {
            Some([w, h]) => statuses(viewport.engine(), graph, Region::sized(w, h)),
            None => BTreeMap::new(),
        };
    }

    /// Draws the panel.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        values: &Arc<Mutex<Store>>,
        viewport: &std::cell::RefCell<crate::viewport::Viewport>,
    ) {
        let ctx = ui.ctx().clone();
        let (model, wires, selected) = {
            let store = lock(values);
            self.follow_workspace(&store);
            let Some(t) = &self.template else {
                drop(store);
                ui.centered_and_justified(|ui| {
                    ui.weak(crate::i18n::text(
                        &ctx,
                        Message::new(
                            "gui-graph-no-workspace",
                            "No workspace is open: its network shows here.",
                        ),
                    ));
                });
                return;
            };
            let model =
                NetworkModel::new(t, &self.catalogue, &self.moved).with_params(&ctx, &store);
            let wires = match store.get(WIRES).map(|e| &e.value) {
                Some(helios_core::Value::Choice(c)) => wire_style(c),
                _ => WireStyle::Curves,
            };
            let selected = match store.get(crate::params::SELECTION).map(|e| &e.value) {
                Some(helios_core::Value::Text(t)) => t.clone(),
                _ => String::new(),
            };
            (model, wires, selected)
        };
        if let Ok(v) = viewport.try_borrow() {
            self.follow_engine(ctx.input(|i| i.time), &v);
        }
        // The selection set elsewhere (the Parameters, Masks or Modules
        // panel, an agent): the editor follows.
        let mut requests = Vec::new();
        if self.selection.as_deref() != Some(selected.as_str()) {
            self.selection = Some(selected.clone());
            let want: Vec<String> = if model.has(&selected) {
                vec![selected.clone()]
            } else {
                Vec::new()
            };
            let now: Vec<String> = self.state.selection.iter().cloned().collect();
            if now != want {
                requests.push(Request::Select(want));
            }
        }
        Self::toolbar(ui, values, wires);
        let label = crate::i18n::text(
            &ctx,
            Message::new("gui-graph-label", "The workspace's network"),
        );
        let headers = self.headers.clone();
        let editor = GraphEditor::new(&model, &mut self.state)
            .label(label)
            .snap(Some(10.0))
            .wires(wires)
            .requests(requests)
            .header(move |ui, node| {
                if let Some(h) = headers.get(&node.id) {
                    header_ui(ui, h);
                }
            });
        let out = match &self.app {
            Some(app) => egui_pop::imaging::show_graph(app, GRAPH, editor, ui),
            None => editor.show(ui),
        };
        for e in &out.events {
            if let GraphEvent::Selection(nodes) = e {
                self.select(values, nodes);
            }
        }
        for edit in &out.edits {
            match edit {
                Edit::Param {
                    node,
                    param,
                    to,
                    done,
                    ..
                } => self.write_param(values, node, param, *to, *done),
                other => {
                    if let Edit::Move { moves } = other {
                        for m in moves {
                            self.moved.insert(m.node.clone(), m.to);
                        }
                    }
                    if let Some((id, args)) = command_of(other) {
                        self.commands.push((id.to_string(), args));
                    }
                }
            }
        }
        self.double_click(ui, &model, &out);
    }

    /// The wires' style, picked above the graph.
    fn toolbar(ui: &mut Ui, values: &Arc<Mutex<Store>>, wires: WireStyle) {
        let ctx = ui.ctx().clone();
        let name = |s: WireStyle| {
            crate::i18n::text(
                &ctx,
                match s {
                    WireStyle::Curves => Message::new("gui-graph-wires-curves", "Curves"),
                    WireStyle::Orthogonal => {
                        Message::new("gui-graph-wires-orthogonal", "Orthogonal")
                    }
                    WireStyle::Straight => Message::new("gui-graph-wires-straight", "Straight"),
                },
            )
        };
        ui.horizontal(|ui| {
            ui.label(crate::i18n::text(
                &ctx,
                Message::new("gui-graph-wires", "Wires"),
            ));
            let mut picked = wires;
            egui::ComboBox::from_id_salt("helios.graph.wires")
                .selected_text(name(wires))
                .show_ui(ui, |ui| {
                    for s in WireStyle::ALL {
                        ui.selectable_value(&mut picked, s, name(s));
                    }
                });
            if picked != wires {
                let _ = lock(values).set(Set::new(
                    WIRES,
                    Change::To(helios_core::Value::Choice(picked.name().into())),
                    Origin::User,
                ));
            }
        });
    }

    /// The editor's selection, written to the shared value: the node
    /// selected before when it still is, else the first.
    fn select(&mut self, values: &Arc<Mutex<Store>>, nodes: &[String]) {
        let keep = self
            .selection
            .as_ref()
            .filter(|s| nodes.contains(s))
            .cloned();
        let node = keep.or_else(|| nodes.first().cloned()).unwrap_or_default();
        if self.selection.as_deref() == Some(node.as_str()) {
            return;
        }
        let _ = lock(values).set(Set::new(
            crate::params::SELECTION,
            Change::To(helios_core::Value::Text(node.clone())),
            Origin::User,
        ));
        self.selection = Some(node);
    }

    /// A parameter's slider: one gesture from its first frame to its
    /// last, as the Parameters panel writes it.
    fn write_param(
        &mut self,
        values: &Arc<Mutex<Store>>,
        node: &str,
        param: &str,
        to: f64,
        done: bool,
    ) {
        let path = format!("node/{node}/{param}");
        let mut store = lock(values);
        let value = match store.get(&path).map(|e| e.descriptor.ty) {
            Some(Type::Float) => helios_core::Value::Float(to),
            Some(Type::Int) => helios_core::Value::Int(to.round() as i64),
            _ => return,
        };
        let gesture = match &self.gesture {
            Some((p, g)) if *p == path => *g,
            _ => {
                if let Some((_, g)) = self.gesture.take() {
                    store.end(g);
                }
                let g = store.begin_gesture(Origin::User, &format!("set {param} of {node}"));
                self.gesture = Some((path.clone(), g));
                g
            }
        };
        let _ = store.set(Set::new(&path, Change::To(value), Origin::User).gesture(gesture));
        if done {
            store.end(gesture);
            self.gesture = None;
        }
    }

    /// A double click on a sub-workspace's title (or its stack in the
    /// overview) enters it.
    fn double_click(
        &mut self,
        ui: &Ui,
        model: &NetworkModel,
        out: &egui_pop::viewport::graph::GraphOutput,
    ) {
        let rect = out.response.rect;
        let at = ui.input(|i| {
            i.pointer
                .button_double_clicked(egui::PointerButton::Primary)
                .then(|| i.pointer.interact_pos())
                .flatten()
        });
        let Some(at) = at.filter(|p| rect.contains(*p)) else {
            return;
        };
        let view = self.state.view;
        if !view.is_finite() || out.zoom <= 0.0 {
            return;
        }
        let graph_at = view.center() + (at - rect.center()) / out.zoom;
        if let Some(id) = model.subworkspace_at(graph_at, out.level) {
            self.enter_subworkspace(&id);
        }
    }

    /// Enters a sub-workspace: the `helios.sub-workspace` place, with
    /// the sub-workspace's id, once Helios declares that place; until
    /// then it says so.
    pub fn enter_subworkspace(&mut self, id: &str) {
        let Some(app) = &self.app else { return };
        let Ok(frame) = egui_pop::model::ids::FrameId::new(SUBWORKSPACE_PLACE) else {
            return;
        };
        let declared = app
            .service(&egui_pop::api::contexts::CONTEXTS)
            .is_some_and(|c| c.frames().get(&frame).is_some());
        if !declared {
            self.notes.push(
                Message::new(
                    "gui-graph-subworkspace-not-yet",
                    "Entering a sub-workspace ({ $id }) is not available yet",
                )
                .with("id", id),
            );
            return;
        }
        let _ = egui_pop::api::contexts::enter(app, frame, json!({ "subworkspace": id }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network() -> NetworkDesc {
        helios_template::workspace::blank_network()
    }

    #[test]
    fn the_blank_network_stacks_its_chains_under_the_blend() {
        let c = crate::templates::modules();
        let p = placements(&network(), &c);
        let of = |n: &str| p.iter().find(|x| x.node == n).unwrap().clone();
        assert!(of("source").principal);
        assert!(of("blend").principal, "a merge, and what the view shows");
        assert!(!of("exposure").principal);
        assert_eq!(of("exposure").stack, "blend");
        assert_eq!(of("source").rank, 0);
        assert_eq!(of("exposure").rank, 1);
        assert_eq!(of("blend").rank, 2);
    }

    #[test]
    fn edits_change_the_network_and_refuse_what_cannot_be() {
        let c = crate::templates::modules();
        let mut net = network();
        // A second wire into an input replaces the first.
        connect(
            &mut net,
            &c,
            &json!({ "from": "source", "port": "image", "to": "blend", "input": "processed" }),
        )
        .unwrap();
        let into: Vec<_> = net
            .connections
            .iter()
            .filter(|k| k.to == "blend" && k.input == "processed")
            .collect();
        assert_eq!(into.len(), 1);
        assert_eq!(into[0].from, "source");
        // A loop is refused.
        let looped = connect(
            &mut net,
            &c,
            &json!({ "from": "blend", "port": "output", "to": "exposure", "input": "input" }),
        );
        assert!(looped.is_err(), "{looped:?}");
        // A port that is not there.
        assert!(
            connect(
                &mut net,
                &c,
                &json!({ "from": "source", "port": "nothing", "to": "exposure", "input": "input" }),
            )
            .is_err()
        );
        let mut positions = BTreeMap::new();
        let added = add(
            &mut net,
            &mut positions,
            &c,
            &json!({ "nodes": [{ "module": "exposure", "at": [10.0, 20.0] }] }),
        )
        .unwrap();
        assert_eq!(added, ["exposure_2"]);
        assert_eq!(positions["exposure_2"], [10.0, 20.0]);
        assert!(remove(&mut net, &json!({ "nodes": ["source"] })).is_err());
        assert!(remove(&mut net, &json!({ "nodes": ["blend"] })).is_err());
        remove(&mut net, &json!({ "nodes": ["exposure"] })).unwrap();
        assert!(
            !net.connections
                .iter()
                .any(|k| k.from == "exposure" || k.to == "exposure")
        );
    }

    #[test]
    fn edits_become_network_commands() {
        let e = Edit::Connect {
            from: PortRef::new("a", "out"),
            to: PortRef::new("b", "in"),
            replaces: Vec::new(),
        };
        let (id, args) = command_of(&e).unwrap();
        assert_eq!(id, "network.connect");
        assert_eq!(args["to"], "b");
        assert_eq!(args["input"], "in");
        let p = Edit::Param {
            node: "a".into(),
            param: "x".into(),
            from: 0.0,
            to: 1.0,
            done: true,
        };
        assert!(command_of(&p).is_none());
    }
}
