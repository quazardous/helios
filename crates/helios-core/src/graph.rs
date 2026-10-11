//! A network of nodes: sources and modules connected port to port.
//!
//! The graph is only a description. Evaluating it is the [`Engine`]'s job.
//!
//! [`Engine`]: crate::engine::Engine

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::describe::Describe;
use crate::error::Error;
use crate::image::{ContentHash, Image};
use crate::manifest::ModuleKind;
use crate::module::{Context, ModuleRef};
use crate::params::{Params, Value};

/// The only output port of a source node.
pub const SOURCE_PORT: &str = "image";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct NodeId(pub usize);

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

pub(crate) struct Node {
    pub(crate) name: String,
    pub(crate) kind: Kind,
    /// For each input port: the node and output port feeding it.
    pub(crate) inputs: Vec<Option<(NodeId, usize)>>,
}

pub struct Graph {
    /// Unique in the process: what the engine remembers about a node is kept per graph.
    id: u64,
    nodes: Vec<Node>,
}

impl Default for Graph {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            nodes: Vec::new(),
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

    /// Changes one parameter of a module node. A decision node is not
    /// edited: it is answered ([`Graph::answer`]), so that every change to
    /// it is a recorded decision.
    pub fn set_param(&mut self, id: NodeId, name: &str, value: Value) -> Result<(), Error> {
        let node = self.node_mut(id)?;
        match &mut node.kind {
            Kind::Module { module, .. } if module.manifest().kind == ModuleKind::Decision => {
                Err(Error::Param {
                    module: node.name.clone(),
                    param: name.into(),
                    reason: "a decision node is answered, not edited".into(),
                })
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
    /// completed like any parameters. Renders use it from now on.
    pub fn answer(&mut self, id: NodeId, values: &Params) -> Result<(), Error> {
        let node = self.node_mut(id)?;
        match &mut node.kind {
            Kind::Module {
                module,
                params,
                answered,
                ..
            } if module.manifest().kind == ModuleKind::Decision => {
                *params = module.manifest().resolve(values)?;
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
            Kind::Module { module, .. } if module.manifest().kind == ModuleKind::Decision => {
                module.manifest().resolve(values).map(|_| ())
            }
            _ => Err(Error::Contract {
                module: node.name.clone(),
                reason: "only a decision node is answered".into(),
            }),
        }
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

    /// The node feeding each input of a node (`None` when unconnected).
    pub fn input_nodes(&self, id: NodeId) -> Result<Vec<Option<NodeId>>, Error> {
        Ok(self
            .node(id)?
            .inputs
            .iter()
            .map(|i| i.map(|(n, _)| n))
            .collect())
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

    /// Whether `upstream` feeds `node`, directly or not.
    fn feeds(&self, upstream: NodeId, node: NodeId) -> Result<bool, Error> {
        for (src, _) in self.node(node)?.inputs.iter().flatten() {
            if *src == upstream || self.feeds(upstream, *src)? {
                return Ok(true);
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
        let n = self.nodes.len();
        let mut consumers: Vec<Vec<NodeId>> = vec![Vec::new(); n];
        for (i, node) in self.nodes.iter().enumerate() {
            for (src, _) in node.inputs.iter().flatten() {
                consumers[src.0].push(NodeId(i));
            }
        }
        let mut rank = vec![None; n];
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
                        node
                    }
                }
            })
            .collect();
        serde_json::json!({ "nodes": nodes, "layout": self.layout() })
    }
}
