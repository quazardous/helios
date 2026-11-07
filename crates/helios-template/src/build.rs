//! The headless build: a template's network as a `helios_core::Graph`,
//! its layout with the composites' panel hints applied, no window.

use std::collections::BTreeMap;

use egui_pop_model::ids::PanelId;
use egui_pop_model::layout::Layout;
use helios_core::{Context, Expansion, Graph, Image, ModuleRef, NodeId, Params, Region, Wire};

use crate::catalogue::Catalogue;
use crate::format::{CompositeRef, NodeDesc, PanelHint, RAW_FRONT, SourceKind, Template};

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

/// A small grey mosaic, for building a raw source without a raw at hand.
pub fn raw_placeholder() -> Image {
    Image::from_fn(Region::sized(8, 8), 1, |_, _, _| 0.18)
}

/// What a template's sources are given (see [`SourceKind`]): a raw
/// source takes a raw, a picture source a picture, an image source
/// either.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    /// A picture in the working space, four channels.
    Picture(&'a Image),
    /// A raw file's sensor mosaic, one channel, and its camera's
    /// description as context (`helios_photo::CameraInfo::to_context`):
    /// every node reading the source, directly or not, is given it.
    Raw {
        mosaic: &'a Image,
        camera: &'a Context,
    },
}

impl Source<'_> {
    pub fn kind(&self) -> SourceKind {
        match self {
            Source::Picture(_) => SourceKind::Picture,
            Source::Raw { .. } => SourceKind::Raw,
        }
    }
}

/// Why a source of `kind` refuses what it was given.
fn mismatch(name: &str, kind: SourceKind) -> String {
    match kind {
        SourceKind::Raw => {
            format!("network.sources: `{name}` takes a raw file's sensor data, not a picture")
        }
        SourceKind::Picture => {
            format!("network.sources: `{name}` takes a picture, not a raw file's sensor data")
        }
        SourceKind::Image => format!("network.sources: `{name}` takes a picture or a raw"),
    }
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

/// Builds `template`'s network, each source given `source` (without
/// one, a placeholder of the kind it takes), with `preset`'s values over
/// the nodes' own, and `layout` with the panel hints of its composites.
/// A source given what it does not take is refused.
///
/// An image source given a raw is its mosaic followed by the raw front
/// ([`RAW_FRONT`]), opened: its nodes go in the graph under their own
/// names (`rawprepare`, `whitebalance`, `highlights`, `demosaic`,
/// `colorin`, the names a darktable edit's items take), or as
/// `<source>.<inner>` where the network already has that name, and what
/// reads the source reads the raw front's output.
pub fn build(
    template: &Template,
    layout: &Layout,
    catalogue: &Catalogue,
    source: Option<Source<'_>>,
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
    let mut camera = None;
    let mut raws = Vec::new();
    // The names the network gives: a raw front's node takes another.
    let mut taken: std::collections::BTreeSet<String> = net
        .sources
        .iter()
        .chain(net.nodes.iter().map(|n| &n.name))
        .cloned()
        .collect();
    for name in &net.sources {
        let kind = net.source_kind(name);
        let (image, raw) = match (kind, source) {
            (SourceKind::Picture | SourceKind::Image, None) => (placeholder(), false),
            (SourceKind::Raw, None) => (raw_placeholder(), true),
            (SourceKind::Picture | SourceKind::Image, Some(Source::Picture(image))) => {
                (image.clone(), false)
            }
            (SourceKind::Raw | SourceKind::Image, Some(Source::Raw { mosaic, camera: c })) => {
                camera = Some(c);
                (mosaic.clone(), true)
            }
            (kind, Some(_)) => return Err(mismatch(name, kind)),
        };
        let id = graph.add_source(name, image);
        if raw {
            raws.push(id);
        }
        nodes.insert(name.clone(), id);
        sources.insert(name.clone(), id);
        let mut p = Placed::default();
        p.outputs
            .insert(SOURCE_PORT.into(), (id, SOURCE_PORT.into()));
        if raw && kind == SourceKind::Image {
            p = raw_front(&mut graph, &mut nodes, &mut taken, catalogue, name, id)
                .map_err(|e| format!("network.sources: `{name}`: {e}"))?;
        }
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
            if n.bypassed {
                return Err(at(
                    "an opened composite is not bypassed whole: bypass its inner nodes".into(),
                ));
            }
            open(&mut graph, &mut nodes, n, &module, &params).map_err(at)?
        } else {
            let id = graph
                .add_module(&n.name, module.clone(), &params)
                .map_err(|e| at(e.to_string()))?;
            if n.bypassed {
                graph.set_bypass(id, true).map_err(|e| at(e.to_string()))?;
            }
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
    unconnected(&graph, view.0)?;
    if let Some(camera) = camera {
        give_camera(&mut graph, &raws, camera)?;
    }
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
    place_network(graph, nodes, &r.to_string(), &network, |inner| {
        format!("{}.{inner}", n.name)
    })
}

/// The raw front after the source `source` (`id`), given a raw: its
/// nodes opened in the graph under their own names where `taken` does
/// not hold them, the source wired to its input. Returns where what
/// reads the source reads.
fn raw_front(
    graph: &mut Graph,
    nodes: &mut BTreeMap<String, NodeId>,
    taken: &mut std::collections::BTreeSet<String>,
    catalogue: &Catalogue,
    source: &str,
    id: NodeId,
) -> Result<Placed, String> {
    let r = CompositeRef {
        id: RAW_FRONT.0.into(),
        version: RAW_FRONT.1,
    };
    let module = catalogue
        .resolve(None, Some(&r), 0)
        .map_err(|e| format!("no raw front to develop a raw with: {e}"))?;
    let params = module
        .manifest()
        .resolve(&Params::new())
        .map_err(|e| e.to_string())?;
    let network = module
        .expand(&params, &Context::new(), &Expansion::new(1.0, None))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("`{r}` is not a composite"))?;
    let names: BTreeMap<String, String> = network
        .nodes()
        .iter()
        .map(|inner| {
            let name = if taken.contains(&inner.name) {
                format!("{source}.{}", inner.name)
            } else {
                inner.name.clone()
            };
            taken.insert(name.clone());
            (inner.name.clone(), name)
        })
        .collect();
    let placed = place_network(graph, nodes, &r.to_string(), &network, |inner| {
        names[inner].clone()
    })?;
    for (port, targets) in &placed.inputs {
        for (to, input) in targets {
            graph
                .connect(id, SOURCE_PORT, *to, input)
                .map_err(|e| format!("`{r}`'s input `{port}`: {e}"))?;
        }
    }
    let output = placed
        .outputs
        .values()
        .next()
        .cloned()
        .ok_or_else(|| format!("`{r}` has no output"))?;
    let mut p = Placed::default();
    p.outputs.insert(SOURCE_PORT.into(), output);
    Ok(p)
}

/// A composite's network (`r`'s) put in the graph as ordinary nodes,
/// each named by `name_of` from its inner name, wired as inside.
fn place_network(
    graph: &mut Graph,
    nodes: &mut BTreeMap<String, NodeId>,
    r: &str,
    network: &helios_core::Network,
    name_of: impl Fn(&str) -> String,
) -> Result<Placed, String> {
    let mut ids = Vec::with_capacity(network.nodes().len());
    let mut placed = Placed::default();
    for inner in network.nodes() {
        let name = name_of(&inner.name);
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

/// Gives every node reading one of the raw sources `raws`, directly or
/// not, the camera's description as context.
fn give_camera(graph: &mut Graph, raws: &[NodeId], camera: &Context) -> Result<(), String> {
    let ids: Vec<NodeId> = graph.ids().collect();
    for id in ids {
        let mut seen = std::collections::BTreeSet::new();
        let mut todo = vec![id];
        while let Some(n) = todo.pop() {
            for from in graph
                .input_nodes(n)
                .map_err(|e| e.to_string())?
                .into_iter()
                .flatten()
            {
                if seen.insert(from) {
                    todo.push(from);
                }
            }
        }
        if raws.iter().any(|r| seen.contains(r)) {
            graph
                .set_context(id, camera.clone())
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Every required input is connected on the network's render paths: the
/// nodes the view reads from, and those every sink (an export, a
/// writer) reads from. A node off those paths may wait for its wires (a
/// node just added in the graph view, wired by the next edit): nothing
/// renders it, and asking for its output says which input is missing.
fn unconnected(graph: &Graph, view: NodeId) -> Result<(), String> {
    let mut todo = vec![view];
    for id in graph.ids() {
        if graph
            .manifest(id)
            .map_err(|e| e.to_string())?
            .is_some_and(|m| m.kind == helios_core::ModuleKind::Sink)
        {
            todo.push(id);
        }
    }
    let mut needed = std::collections::BTreeSet::new();
    while let Some(id) = todo.pop() {
        if !needed.insert(id) {
            continue;
        }
        for (from, _) in graph
            .input_ports(id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .flatten()
        {
            todo.push(from);
        }
    }
    for id in graph.ids().filter(|id| needed.contains(id)) {
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
