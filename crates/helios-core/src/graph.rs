//! A network of nodes: sources and modules connected port to port.
//!
//! The graph is only a description. Evaluating it is the [`Engine`]'s job.
//!
//! [`Engine`]: crate::engine::Engine

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::describe::Describe;
use crate::error::Error;
use crate::image::{ContentHash, Image, Region};
use crate::manifest::{Manifest, ModuleKind, ParamHint};
use crate::module::{Context, ModuleRef};
use crate::params::{Params, Value};

/// The only output port of a source node.
pub const SOURCE_PORT: &str = "image";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct NodeId(pub usize);

#[derive(Clone)]
pub(crate) enum Kind {
    /// Image data handed to the graph by the framework (its loaders own IO).
    Source { image: Image, hash: ContentHash },
    Module {
        module: ModuleRef,
        params: Params,
        context: Context,
        /// False for a decision node until an answer is recorded: its
        /// parameters are the answer, not something to edit.
        answered: bool,
    },
}

#[derive(Clone)]
pub(crate) struct Node {
    pub(crate) name: String,
    pub(crate) kind: Kind,
    /// For each input port: the node and output port feeding it.
    pub(crate) inputs: Vec<Option<(NodeId, usize)>>,
    /// Rendered as if absent: each output is the input it passes through
    /// (see [`Graph::set_bypass`]).
    pub(crate) bypassed: bool,
}

pub struct Graph {
    /// Unique in the process: what the engine remembers about a node is kept per graph.
    id: u64,
    nodes: Vec<Node>,
}

static NEXT_GRAPH: AtomicU64 = AtomicU64::new(1);

impl Default for Graph {
    fn default() -> Self {
        Self {
            id: NEXT_GRAPH.fetch_add(1, Ordering::Relaxed),
            nodes: Vec::new(),
        }
    }
}

/// A copy to render from elsewhere (another thread), cheap: images and
/// modules are shared. It is another graph to the engine, which still
/// finds the same content by its keys.
impl Clone for Graph {
    fn clone(&self) -> Self {
        Self {
            id: NEXT_GRAPH.fetch_add(1, Ordering::Relaxed),
            nodes: self.nodes.clone(),
        }
    }
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn add_source(&mut self, name: &str, image: Image) -> NodeId {
        let hash = image.content_hash();
        self.push(Node {
            name: name.into(),
            kind: Kind::Source { image, hash },
            inputs: Vec::new(),
            bypassed: false,
        })
    }

    /// Adds a module node; `params` are checked and completed with defaults.
    pub fn add_module(
        &mut self,
        name: &str,
        module: ModuleRef,
        params: &Params,
    ) -> Result<NodeId, Error> {
        let params = module.manifest().resolve(params)?;
        let inputs = vec![None; module.manifest().inputs.len()];
        let answered = module.manifest().kind != ModuleKind::Decision;
        Ok(self.push(Node {
            name: name.into(),
            kind: Kind::Module {
                module,
                params,
                context: Context::new(),
                answered,
            },
            inputs,
            bypassed: false,
        }))
    }

    fn push(&mut self, node: Node) -> NodeId {
        self.nodes.push(node);
        NodeId(self.nodes.len() - 1)
    }

    pub(crate) fn node(&self, id: NodeId) -> Result<&Node, Error> {
        self.nodes.get(id.0).ok_or(Error::Node(id.0))
    }

    fn node_mut(&mut self, id: NodeId) -> Result<&mut Node, Error> {
        self.nodes.get_mut(id.0).ok_or(Error::Node(id.0))
    }

    /// Gives a source node another image (another file loaded by the
    /// framework): the network stays, its results are computed again for the
    /// new content, found by their keys like any other.
    pub fn set_source(&mut self, id: NodeId, image: Image) -> Result<(), Error> {
        let node = self.node_mut(id)?;
        match &mut node.kind {
            Kind::Source { image: i, hash } => {
                *hash = image.content_hash();
                *i = image;
                Ok(())
            }
            Kind::Module { .. } => Err(Error::Module {
                module: node.name.clone(),
                reason: "not a source: only a source takes an image".into(),
            }),
        }
    }

    /// Changes one parameter of a module node. A decision node is not
    /// edited: it is answered ([`Graph::answer`]), so that every change to
    /// it is a recorded decision. Only its question (see
    /// [`Module::question`]) is set here, and a question set to another
    /// value withdraws the answer: it is pending again.
    ///
    /// [`Module::question`]: crate::Module::question
    pub fn set_param(&mut self, id: NodeId, name: &str, value: Value) -> Result<(), Error> {
        let node = self.node_mut(id)?;
        match &mut node.kind {
            Kind::Module {
                module,
                params,
                answered,
                ..
            } if module.manifest().kind == ModuleKind::Decision => {
                if !module.question().iter().any(|q| q == name) {
                    return Err(Error::Param {
                        module: node.name.clone(),
                        param: name.into(),
                        reason: "a decision node is answered, not edited".into(),
                    });
                }
                let mut changed = params.clone();
                changed.set(name, value);
                let changed = module.manifest().resolve(&changed)?;
                if changed != *params {
                    *params = changed;
                    *answered = false;
                }
                Ok(())
            }
            Kind::Module { module, params, .. } => {
                let mut changed = params.clone();
                changed.set(name, value);
                *params = module.manifest().resolve(&changed)?;
                Ok(())
            }
            Kind::Source { .. } => Err(Error::Param {
                module: node.name.clone(),
                param: name.into(),
                reason: "a source has no parameters".into(),
            }),
        }
    }

    /// Records the answer of a decision node: its parameters, checked and
    /// completed like any parameters. Renders use it from now on. The
    /// question's values the answer does not name are kept (see
    /// [`Module::question`]).
    ///
    /// [`Module::question`]: crate::Module::question
    pub fn answer(&mut self, id: NodeId, values: &Params) -> Result<(), Error> {
        let node = self.node_mut(id)?;
        match &mut node.kind {
            Kind::Module {
                module,
                params,
                answered,
                ..
            } if module.manifest().kind == ModuleKind::Decision => {
                let values = with_question(module, params, values);
                *params = module.manifest().resolve(&values)?;
                *answered = true;
                Ok(())
            }
            _ => Err(Error::Contract {
                module: node.name.clone(),
                reason: "only a decision node is answered".into(),
            }),
        }
    }

    /// Checks an answer for a decision node without recording it.
    pub fn check_answer(&self, id: NodeId, values: &Params) -> Result<(), Error> {
        let node = self.node(id)?;
        match &node.kind {
            Kind::Module { module, params, .. }
                if module.manifest().kind == ModuleKind::Decision =>
            {
                let values = with_question(module, params, values);
                module.manifest().resolve(&values).map(|_| ())
            }
            _ => Err(Error::Contract {
                module: node.name.clone(),
                reason: "only a decision node is answered".into(),
            }),
        }
    }

    /// A decision node's question as it stands: the values of the
    /// parameters its module names as its question (none for another
    /// node).
    pub fn question(&self, id: NodeId) -> Result<Params, Error> {
        Ok(match &self.node(id)?.kind {
            Kind::Module { module, params, .. } => {
                module
                    .question()
                    .iter()
                    .fold(Params::new(), |q, name| match params.get(name) {
                        Some(v) => q.with(name, v.clone()),
                        None => q,
                    })
            }
            Kind::Source { .. } => Params::new(),
        })
    }

    /// Whether a node renders: false for a decision waiting for its
    /// answer, true for any other node.
    pub fn is_answered(&self, id: NodeId) -> Result<bool, Error> {
        Ok(match &self.node(id)?.kind {
            Kind::Module { answered, .. } => *answered,
            Kind::Source { .. } => true,
        })
    }

    /// Takes a decision node's answer back: it is pending again.
    pub fn withdraw(&mut self, id: NodeId) -> Result<(), Error> {
        let node = self.node_mut(id)?;
        match &mut node.kind {
            Kind::Module {
                module, answered, ..
            } if module.manifest().kind == ModuleKind::Decision => {
                *answered = false;
                Ok(())
            }
            _ => Err(Error::Contract {
                module: node.name.clone(),
                reason: "only a decision node is answered".into(),
            }),
        }
    }

    /// Bypasses a module node, or brings it back. A bypassed node keeps
    /// its parameters and its wiring, and renders as if it were absent:
    /// each of its outputs is the input it passes through (see
    /// [`passed_input`]), so what reads it gets the same content, under
    /// the same keys, as without it. Its other inputs are not read. A
    /// source is never bypassed, nor a module with no input carrying what
    /// its first output gives.
    pub fn set_bypass(&mut self, id: NodeId, on: bool) -> Result<(), Error> {
        let node = self.node(id)?;
        match &node.kind {
            Kind::Source { .. } if on => {
                return Err(Error::Module {
                    module: node.name.clone(),
                    reason: "a source cannot be bypassed".into(),
                });
            }
            Kind::Module { module, .. } if on && passed_input(module.manifest(), 0).is_none() => {
                return Err(Error::Module {
                    module: node.name.clone(),
                    reason: "nothing to pass through: no input carries what its output gives"
                        .into(),
                });
            }
            _ => {}
        }
        self.node_mut(id)?.bypassed = on;
        Ok(())
    }

    /// Whether a node is bypassed (see [`Graph::set_bypass`]).
    pub fn is_bypassed(&self, id: NodeId) -> Result<bool, Error> {
        Ok(self.node(id)?.bypassed)
    }

    /// Whether a node can be bypassed: a module with an input carrying
    /// what its first output gives.
    pub fn can_bypass(&self, id: NodeId) -> Result<bool, Error> {
        Ok(match &self.node(id)?.kind {
            Kind::Source { .. } => false,
            Kind::Module { module, .. } => passed_input(module.manifest(), 0).is_some(),
        })
    }

    /// The extent of what a node outputs: its source's image region,
    /// followed up the first connected inputs (modules keep the geometry
    /// they are given). `None` when no source feeds it.
    pub fn extent(&self, id: NodeId) -> Result<Option<Region>, Error> {
        let mut at = id;
        loop {
            let node = self.node(at)?;
            match &node.kind {
                Kind::Source { image, .. } => return Ok(Some(image.region())),
                Kind::Module { .. } => match node.inputs.iter().flatten().next().copied() {
                    Some((from, _)) => at = from,
                    None => return Ok(None),
                },
            }
        }
    }

    /// The node feeding each input of a node (`None` when unconnected).
    pub fn input_nodes(&self, id: NodeId) -> Result<Vec<Option<NodeId>>, Error> {
        Ok(self
            .node(id)?
            .inputs
            .iter()
            .map(|i| i.map(|(n, _)| n))
            .collect())
    }

    /// The node and output port feeding each input of a node, the port by
    /// name, as `Engine::render` takes it (`None` when unconnected).
    pub fn input_ports(&self, id: NodeId) -> Result<Vec<Option<(NodeId, String)>>, Error> {
        let node = self.node(id)?;
        node.inputs
            .iter()
            .map(|i| {
                i.map(|(src, port)| {
                    let name = match &self.node(src)?.kind {
                        Kind::Source { .. } => SOURCE_PORT.to_string(),
                        Kind::Module { module, .. } => module.manifest().outputs[port].name.clone(),
                    };
                    Ok((src, name))
                })
                .transpose()
            })
            .collect()
    }

    /// The manifest of a module node; `None` for a source.
    pub fn manifest(&self, id: NodeId) -> Result<Option<Manifest>, Error> {
        Ok(match &self.node(id)?.kind {
            Kind::Source { .. } => None,
            Kind::Module { module, .. } => Some(module.manifest().clone()),
        })
    }

    /// The parameters of a module node, completed with defaults; `None`
    /// for a source.
    pub fn params(&self, id: NodeId) -> Result<Option<Params>, Error> {
        Ok(match &self.node(id)?.kind {
            Kind::Source { .. } => None,
            Kind::Module { params, .. } => Some(params.clone()),
        })
    }

    /// The parameter hints of a module node (see [`Module::param_hints`]).
    ///
    /// [`Module::param_hints`]: crate::Module::param_hints
    pub fn param_hints(&self, id: NodeId) -> Result<Vec<ParamHint>, Error> {
        Ok(match &self.node(id)?.kind {
            Kind::Source { .. } => Vec::new(),
            Kind::Module { module, .. } => module.param_hints(),
        })
    }

    /// The name of a node.
    pub fn name(&self, id: NodeId) -> Result<&str, Error> {
        Ok(&self.node(id)?.name)
    }

    /// Every node, in the order they were added.
    pub fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        (0..self.nodes.len()).map(NodeId)
    }

    /// A node by name.
    pub fn find(&self, name: &str) -> Option<NodeId> {
        self.nodes.iter().position(|n| n.name == name).map(NodeId)
    }

    /// Sets the context (image metadata) a module node is evaluated with.
    pub fn set_context(&mut self, id: NodeId, context: Context) -> Result<(), Error> {
        if let Kind::Module { context: c, .. } = &mut self.node_mut(id)?.kind {
            *c = context;
        }
        Ok(())
    }

    /// Index and channel count of an output port.
    pub(crate) fn output_port(&self, id: NodeId, port: &str) -> Result<(usize, u32), Error> {
        let node = self.node(id)?;
        match &node.kind {
            Kind::Source { image, .. } if port == SOURCE_PORT => Ok((0, image.channels())),
            Kind::Source { .. } => Err(Error::Port {
                module: node.name.clone(),
                port: port.into(),
            }),
            Kind::Module { module, .. } => {
                let i = module.manifest().output_index(port)?;
                let spec = &module.manifest().outputs[i];
                let channels = spec.channels().ok_or_else(|| Error::Contract {
                    module: module.manifest().name.clone(),
                    reason: format!("port `{}` is not an image the engine runs yet", spec.name),
                })?;
                Ok((i, channels))
            }
        }
    }

    /// Connects an output port to an input port.
    pub fn connect(
        &mut self,
        from: NodeId,
        out_port: &str,
        to: NodeId,
        in_port: &str,
    ) -> Result<(), Error> {
        let (out_index, channels) = self.output_port(from, out_port)?;
        let target = self.node(to)?;
        let Kind::Module { module, .. } = &target.kind else {
            return Err(Error::Port {
                module: target.name.clone(),
                port: in_port.into(),
            });
        };
        let in_index = module.manifest().input_index(in_port)?;
        let spec = &module.manifest().inputs[in_index];
        let expected = spec.channels().ok_or_else(|| Error::Contract {
            module: module.manifest().name.clone(),
            reason: format!("port `{}` is not an image the engine runs yet", spec.name),
        })?;
        if channels != expected {
            return Err(Error::Contract {
                module: module.manifest().name.clone(),
                reason: format!(
                    "input `{in_port}` takes {expected} channels, `{out_port}` gives {channels}"
                ),
            });
        }
        if from == to || self.feeds(to, from)? {
            return Err(Error::Cycle {
                from: self.node(from)?.name.clone(),
                to: target.name.clone(),
            });
        }
        self.node_mut(to)?.inputs[in_index] = Some((from, out_index));
        Ok(())
    }

    /// Leaves an input port unconnected; returns the node and output port
    /// that fed it, if one did. A required input left so makes rendering
    /// the node fail with `Unconnected`; an optional one gets an empty
    /// image.
    pub fn disconnect(
        &mut self,
        to: NodeId,
        in_port: &str,
    ) -> Result<Option<(NodeId, String)>, Error> {
        let target = self.node(to)?;
        let Kind::Module { module, .. } = &target.kind else {
            return Err(Error::Port {
                module: target.name.clone(),
                port: in_port.into(),
            });
        };
        let in_index = module.manifest().input_index(in_port)?;
        let was = self.input_ports(to)?[in_index].clone();
        self.node_mut(to)?.inputs[in_index] = None;
        Ok(was)
    }

    /// Whether `upstream` feeds `node`, directly or not. Each node is
    /// visited once: a node read by several consumers is reached by many
    /// paths, as many as 2^n through n diamonds.
    fn feeds(&self, upstream: NodeId, node: NodeId) -> Result<bool, Error> {
        let mut seen = std::collections::HashSet::new();
        let mut todo = vec![node];
        while let Some(at) = todo.pop() {
            for (src, _) in self.node(at)?.inputs.iter().flatten() {
                if *src == upstream {
                    return Ok(true);
                }
                if seen.insert(*src) {
                    todo.push(*src);
                }
            }
        }
        Ok(false)
    }
}

/// Where a node goes when a surface draws the graph: the same answer for
/// every surface (GUI, CLI dump, web).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct LayoutHint {
    pub node: NodeId,
    /// Column of a layered layout: 0 for nodes without inputs, else one more
    /// than the deepest input.
    pub rank: usize,
    /// Shown in the overview. Proposed automatically: sources, merges (two
    /// inputs or more), decision nodes, sinks and the network's outputs.
    pub principal: bool,
    /// The principal node this node is stacked under in the overview: itself
    /// when principal, else the group of its first consumer.
    pub group: NodeId,
}

impl Graph {
    /// Layout hints for every node, in node order. Manual choices (pinning a
    /// node as principal, folding one into a stack) belong to the user's
    /// workspace and are applied on top by the surfaces.
    pub fn layout(&self) -> Vec<LayoutHint> {
        fn rank_of(g: &Graph, i: usize, memo: &mut [Option<usize>]) -> usize {
            if let Some(r) = memo[i] {
                return r;
            }
            let r = g.nodes[i]
                .inputs
                .iter()
                .flatten()
                .map(|(src, _)| rank_of(g, src.0, memo) + 1)
                .max()
                .unwrap_or(0);
            memo[i] = Some(r);
            r
        }
        fn group_of(
            i: usize,
            principal: &[bool],
            consumers: &[Vec<NodeId>],
            memo: &mut [Option<usize>],
        ) -> usize {
            if let Some(g) = memo[i] {
                return g;
            }
            let g = if principal[i] {
                i
            } else {
                // Not principal means it has a consumer; the graph is acyclic.
                let first = consumers[i]
                    .iter()
                    .min()
                    .expect("a non-principal node has a consumer");
                group_of(first.0, principal, consumers, memo)
            };
            memo[i] = Some(g);
            g
        }
        let n = self.nodes.len();
        let mut consumers: Vec<Vec<NodeId>> = vec![Vec::new(); n];
        for (i, node) in self.nodes.iter().enumerate() {
            for (src, _) in node.inputs.iter().flatten() {
                consumers[src.0].push(NodeId(i));
            }
        }
        let mut rank = vec![None; n];
        let principal: Vec<bool> = (0..n)
            .map(|i| {
                let node = &self.nodes[i];
                let declared = match &node.kind {
                    Kind::Source { .. } => true,
                    Kind::Module { module, .. } => module.manifest().kind != ModuleKind::Process,
                };
                declared || node.inputs.iter().flatten().count() >= 2 || consumers[i].is_empty()
            })
            .collect();
        let mut group = vec![None; n];
        (0..n)
            .map(|i| LayoutHint {
                node: NodeId(i),
                rank: rank_of(self, i, &mut rank),
                principal: principal[i],
                group: NodeId(group_of(i, &principal, &consumers, &mut group)),
            })
            .collect()
    }
}

impl Describe for Graph {
    fn describe(&self) -> serde_json::Value {
        let nodes: Vec<_> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let inputs: Vec<_> = n.inputs.iter().map(|c| c.map(|(src, port)| serde_json::json!([src.0, port]))).collect();
                match &n.kind {
                    Kind::Source { image, hash } => serde_json::json!({
                        "id": i, "name": n.name, "kind": "source",
                        "region": image.region(), "channels": image.channels(), "hash": hash.short(),
                    }),
                    Kind::Module { module, params, answered, .. } => {
                        let mut node = serde_json::json!({
                            "id": i, "name": n.name, "kind": "module",
                            "module": module.manifest().name, "version": module.manifest().version,
                            "params": params, "inputs": inputs,
                        });
                        if module.manifest().kind == ModuleKind::Decision {
                            node["answered"] = (*answered).into();
                        }
                        if n.bypassed {
                            node["bypassed"] = true.into();
                        }
                        node
                    }
                }
            })
            .collect();
        serde_json::json!({ "nodes": nodes, "layout": self.layout() })
    }
}

/// The input a bypassed module passes through as its output `output` (by
/// index): its first input carrying the same channels, `None` when it has
/// none (that output cannot be passed through).
pub fn passed_input(manifest: &Manifest, output: usize) -> Option<usize> {
    let channels = manifest.outputs.get(output)?.channels()?;
    manifest
        .inputs
        .iter()
        .position(|i| i.channels() == Some(channels))
}

/// An answer completed with the question it answers: the question's
/// values it does not name are the node's.
fn with_question(module: &ModuleRef, params: &Params, values: &Params) -> Params {
    let mut out = values.clone();
    for name in module.question() {
        if out.get(&name).is_none()
            && let Some(v) = params.get(&name)
        {
            out.set(&name, v.clone());
        }
    }
    out
}
