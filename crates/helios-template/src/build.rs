//! The headless build: a template's network as a `helios_core::Graph`,
//! its layout with the composites' panel hints applied, no window.

use std::collections::BTreeMap;

use egui_pop_model::ids::PanelId;
use egui_pop_model::layout::Layout;
use helios_core::{Context, Expansion, Graph, Image, ModuleRef, NodeId, Params, Region, Wire};

use crate::catalogue::Catalogue;
use crate::format::{NodeDesc, PanelHint, Template};

/// The output port of a source node.
pub const SOURCE_PORT: &str = "image";

/// A template built: its graph, its nodes by name, its view and its
/// layout.
pub struct Built {
    pub graph: Graph,
    /// Every node of the graph by name: sources, modules, folded
    /// composites and the inner nodes of opened ones (`<name>.<inner>`).
    pub nodes: BTreeMap<String, NodeId>,
    /// The source nodes, by name.
    pub sources: BTreeMap<String, NodeId>,
    /// The node and port the viewport shows.
    pub view: (NodeId, String),
    /// The layout, the composites' panel hints applied.
    pub layout: Layout,
    /// What the build changed (a panel docked for a hint).
    pub notes: Vec<String>,
}

/// A small grey image, for building without an image at hand.
pub fn placeholder() -> Image {
    Image::from_fn(
        Region::sized(8, 8),
        4,
        |_, _, c| {
            if c == 3 { 1.0 } else { 0.18 }
        },
    )
}

/// Where a node's ports are in the graph.
#[derive(Default)]
struct Placed {
    /// Output port → the graph node and port giving it.
    outputs: BTreeMap<String, (NodeId, String)>,
    /// Input port → the graph nodes and ports reading it.
    inputs: BTreeMap<String, Vec<(NodeId, String)>>,
}

impl Placed {
    fn module(id: NodeId, module: &ModuleRef) -> Self {
        let m = module.manifest();
        Placed {
            outputs: m
                .outputs
                .iter()
                .map(|p| (p.name.clone(), (id, p.name.clone())))
                .collect(),
            inputs: m
                .inputs
                .iter()
                .map(|p| (p.name.clone(), vec![(id, p.name.clone())]))
                .collect(),
        }
    }
}

/// Builds `template`'s network, each source given `source`, with
/// `preset`'s values over the nodes' own, and `layout` with the panel
/// hints of its composites.
pub fn build(
    template: &Template,
    layout: &Layout,
    catalogue: &Catalogue,
    source: &Image,
    preset: Option<&str>,
) -> Result<Built, String> {
    let net = &template.network;
    let preset = match preset {
        Some(id) => Some(
            net.presets
                .iter()
                .find(|p| p.id == id)
                .ok_or_else(|| format!("no preset `{id}`"))?,
        ),
        None => None,
    };
    let mut graph = Graph::new();
    let mut nodes = BTreeMap::new();
    let mut sources = BTreeMap::new();
    let mut placed: BTreeMap<String, Placed> = BTreeMap::new();
    for name in &net.sources {
        let id = graph.add_source(name, source.clone());
        nodes.insert(name.clone(), id);
        sources.insert(name.clone(), id);
        let mut p = Placed::default();
        p.outputs
            .insert(SOURCE_PORT.into(), (id, SOURCE_PORT.into()));
        placed.insert(name.clone(), p);
    }
    let mut hints: Vec<PanelHint> = Vec::new();
    for (i, n) in net.nodes.iter().enumerate() {
        let at = |e: String| format!("network.nodes[{i}] ({}): {e}", n.name);
        if placed.contains_key(&n.name) {
            return Err(at("a second node of that name".into()));
        }
        let mut params = n.params.clone();
        if let Some(values) = preset.and_then(|p| p.values.get(&n.name)) {
            for (k, v) in values.iter() {
                params.set(k, v.clone());
            }
        }
        let module = catalogue
            .resolve(n.module.as_deref(), n.composite.as_ref(), 0)
            .map_err(at)?;
        if let Some(r) = &n.composite {
            let file = catalogue
                .get_composite(r)
                .ok_or_else(|| at(format!("unknown composite `{r}`")))?;
            if let Some(h) = n.panel.as_ref().or(file.panel.as_ref()) {
                hints.push(h.clone());
            }
        }
        let p = if n.open {
            open(&mut graph, &mut nodes, n, &module, &params).map_err(at)?
        } else {
            let id = graph
                .add_module(&n.name, module.clone(), &params)
                .map_err(|e| at(e.to_string()))?;
            nodes.insert(n.name.clone(), id);
            Placed::module(id, &module)
        };
        placed.insert(n.name.clone(), p);
    }
    for (i, c) in net.connections.iter().enumerate() {
        let at = |e: String| format!("network.connections[{i}]: {e}");
        let (from, port) = placed
            .get(&c.from)
            .ok_or_else(|| at(format!("no node `{}`", c.from)))?
            .outputs
            .get(&c.port)
            .cloned()
            .ok_or_else(|| at(format!("`{}` has no output `{}`", c.from, c.port)))?;
        let targets = placed
            .get(&c.to)
            .ok_or_else(|| at(format!("no node `{}`", c.to)))?
            .inputs
            .get(&c.input)
            .ok_or_else(|| at(format!("`{}` has no input `{}`", c.to, c.input)))?;
        for (to, input) in targets {
            graph
                .connect(from, &port, *to, input)
                .map_err(|e| at(e.to_string()))?;
        }
    }
    unconnected(&graph)?;
    let view = placed
        .get(&net.view.node)
        .and_then(|p| p.outputs.get(&net.view.port))
        .cloned()
        .ok_or_else(|| {
            format!(
                "network.view: no output `{}` of `{}`",
                net.view.port, net.view.node
            )
        })?;
    let (layout, notes) = hinted(layout, &hints)?;
    Ok(Built {
        graph,
        nodes,
        sources,
        view,
        layout,
        notes,
    })
}

/// An opened composite: its network (exposed values applied) put in the
/// graph as ordinary nodes named `<name>.<inner>`, wired as inside.
fn open(
    graph: &mut Graph,
    nodes: &mut BTreeMap<String, NodeId>,
    n: &NodeDesc,
    module: &ModuleRef,
    params: &Params,
) -> Result<Placed, String> {
    let Some(r) = &n.composite else {
        return Err("only a composite opens".into());
    };
    let params = module
        .manifest()
        .resolve(params)
        .map_err(|e| e.to_string())?;
    // As the engine expands it for a render at full resolution.
    let network = module
        .expand(&params, &Context::new(), &Expansion::new(1.0, None))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("`{r}` is not a composite"))?;
    let mut ids = Vec::with_capacity(network.nodes().len());
    let mut placed = Placed::default();
    for inner in network.nodes() {
        let name = format!("{}.{}", n.name, inner.name);
        let id = graph
            .add_module(&name, inner.module.clone(), &inner.params)
            .map_err(|e| e.to_string())?;
        nodes.insert(name, id);
        ids.push(id);
    }
    for (i, inner) in network.nodes().iter().enumerate() {
        let m = inner.module.manifest();
        for (k, wire) in inner.inputs.iter().enumerate() {
            let input = m.inputs[k].name.clone();
            match wire {
                Some(Wire::Node(from, port)) => {
                    let out = network.nodes()[from.0].module.manifest().outputs[*port]
                        .name
                        .clone();
                    graph
                        .connect(ids[from.0], &out, ids[i], &input)
                        .map_err(|e| e.to_string())?;
                }
                Some(Wire::Input(j)) => placed
                    .inputs
                    .entry(network.inputs()[*j].name.clone())
                    .or_default()
                    .push((ids[i], input)),
                None => {}
            }
        }
    }
    for port in network.inputs() {
        placed.inputs.entry(port.name.clone()).or_default();
    }
    for (k, exit) in network.exits().iter().enumerate() {
        let output = network.outputs()[k].name.clone();
        match exit {
            Some(Wire::Node(from, port)) => {
                let out = network.nodes()[from.0].module.manifest().outputs[*port]
                    .name
                    .clone();
                placed.outputs.insert(output, (ids[from.0], out));
            }
            Some(Wire::Input(_)) => {
                return Err(format!(
                    "`{r}` passes an input straight to output `{output}`: it cannot be opened"
                ));
            }
            None => return Err(format!("`{r}` has no exit for output `{output}`")),
        }
    }
    Ok(placed)
}

/// Every required input of every module node is connected.
fn unconnected(graph: &Graph) -> Result<(), String> {
    for id in graph.ids() {
        let Some(m) = graph.manifest(id).map_err(|e| e.to_string())? else {
            continue;
        };
        let feeds = graph.input_ports(id).map_err(|e| e.to_string())?;
        for (spec, feed) in m.inputs.iter().zip(feeds) {
            if feed.is_none() && !spec.optional {
                let name = graph.name(id).map_err(|e| e.to_string())?;
                return Err(format!(
                    "network: input `{}` of `{name}` is not connected",
                    spec.name
                ));
            }
        }
    }
    Ok(())
}

/// `layout` with each hinted panel docked in its area when the layout
/// does not show it.
fn hinted(layout: &Layout, hints: &[PanelHint]) -> Result<(Layout, Vec<String>), String> {
    let mut layout = layout.clone();
    let mut notes = Vec::new();
    for h in hints {
        let panel = PanelId::new(h.panel.as_str()).map_err(|e| format!("panel hint: {e}"))?;
        if !layout.shows(&panel) {
            layout.place(&panel, h.area);
            notes.push(format!(
                "`{}` docked in {}, as its composite asks",
                h.panel,
                h.area.name()
            ));
        }
    }
    Ok((layout, notes))
}
