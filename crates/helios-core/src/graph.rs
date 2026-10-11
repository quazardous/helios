//! A network of nodes: sources and modules connected port to port.
//!
//! The graph is only a description. Evaluating it is the [`Engine`]'s job.
//!
//! [`Engine`]: crate::engine::Engine

use serde::Serialize;

use crate::describe::Describe;
use crate::error::Error;
use crate::image::{ContentHash, Image};
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
    },
}

pub(crate) struct Node {
    pub(crate) name: String,
    pub(crate) kind: Kind,
    /// For each input port: the node and output port feeding it.
    pub(crate) inputs: Vec<Option<(NodeId, usize)>>,
}

#[derive(Default)]
pub struct Graph {
    nodes: Vec<Node>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
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
        Ok(self.push(Node {
            name: name.into(),
            kind: Kind::Module {
                module,
                params,
                context: Context::new(),
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

    /// Changes one parameter of a module node.
    pub fn set_param(&mut self, id: NodeId, name: &str, value: Value) -> Result<(), Error> {
        let node = self.node_mut(id)?;
        match &mut node.kind {
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
                Ok((i, module.manifest().outputs[i].channels()))
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
        let expected = module.manifest().inputs[in_index].channels();
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
                    Kind::Module { module, params, .. } => serde_json::json!({
                        "id": i, "name": n.name, "kind": "module",
                        "module": module.manifest().name, "version": module.manifest().version,
                        "params": params, "inputs": inputs,
                    }),
                }
            })
            .collect();
        serde_json::json!({ "nodes": nodes })
    }
}
