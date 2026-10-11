//! Evaluates a graph, never computing the same content twice.
//!
//! Evaluation runs in two passes. **Planning** walks the graph from the
//! requested node: each module prepares its plan and says which input region
//! it needs, and every step gets a content key (hash of the module, its
//! parameters and context, the region, and the keys of its inputs). Nothing
//! is computed yet. **Execution** then looks each key up: content already
//! computed is shared by reference; content being computed by another thread
//! is waited for (single flight); only missing content is computed.
//!
//! The engine counts every request and every computation, so tests can
//! assert budgets ("rendering twice computes nothing the second time").

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use crate::bus::{Bus, Event};
use crate::describe::Describe;
use crate::error::Error;
use crate::graph::{Graph, Kind, NodeId};
use crate::image::{ContentHash, Image, Region};
use crate::manifest::{Family, ModuleKind, Role};
use crate::module::ModuleRef;
use crate::status::{Load, NodeState, NodeStatus, NodeType};

/// The source name the engine publishes events under.
const SOURCE: &str = "engine";

type Outputs = Result<Arc<Vec<Image>>, Error>;

/// A render was asked for. Events of that render carry its sequence number as cause.
#[derive(Serialize)]
pub struct RenderRequested {
    pub node: NodeId,
    pub port: String,
    pub region: Region,
}

impl Event for RenderRequested {
    const KIND: &'static str = "engine.render_requested";
}

/// A step was computed (not found in the cache).
#[derive(Serialize)]
pub struct StepComputed {
    pub node: NodeId,
    pub name: String,
    pub key: String,
    pub region: Region,
    pub ok: bool,
}

impl Event for StepComputed {
    const KIND: &'static str = "engine.step_computed";
}

/// What the engine did, since it was created.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counters {
    /// Steps asked for (computed or found).
    pub requests: u64,
    /// Steps computed.
    pub computed: u64,
    /// Source regions read.
    pub source_reads: u64,
}

impl Counters {
    /// Steps served from the cache, or from a computation already in flight.
    pub fn shared(&self) -> u64 {
        self.requests - self.computed
    }

    /// What happened between `earlier` and `self`.
    pub fn since(&self, earlier: &Counters) -> Counters {
        Counters {
            requests: self.requests - earlier.requests,
            computed: self.computed - earlier.computed,
            source_reads: self.source_reads - earlier.source_reads,
        }
    }
}

/// A step of an evaluation, planned but not run.
struct Step {
    key: ContentHash,
    graph: u64,
    node: NodeId,
    name: String,
    region: Region,
    work: Work,
}

enum Work {
    Source(Image),
    Module {
        module: ModuleRef,
        plan: crate::module::Plan,
        inputs: Vec<(Step, usize)>,
    },
}

pub struct Engine {
    bus: Arc<Bus>,
    cache: Mutex<HashMap<ContentHash, Arc<OnceLock<Outputs>>>>,
    requests: AtomicU64,
    computed: AtomicU64,
    source_reads: AtomicU64,
    /// What each node did, per graph and region.
    tracks: Mutex<HashMap<TrackKey, Track>>,
}

type TrackKey = (u64, NodeId, Region);

#[derive(Default)]
struct Track {
    /// The content key last asked of this node over this region.
    last_key: Option<ContentHash>,
    requests: u64,
    computed: u64,
    running: u64,
}

impl Engine {
    pub fn new(bus: Arc<Bus>) -> Self {
        Self {
            bus,
            cache: Mutex::new(HashMap::new()),
            requests: AtomicU64::new(0),
            computed: AtomicU64::new(0),
            source_reads: AtomicU64::new(0),
            tracks: Mutex::new(HashMap::new()),
        }
    }

    fn track<R>(&self, step: &Step, f: impl FnOnce(&mut Track) -> R) -> R {
        let mut tracks = self.tracks.lock().expect("engine tracks lock");
        f(tracks
            .entry((step.graph, step.node, step.region))
            .or_default())
    }

    /// The normalized status of a node and of every node it reads from, for
    /// a render of `region` (inputs first, the node itself last). Nothing is
    /// computed: the network is only planned and compared with what the
    /// engine holds.
    pub fn status(
        &self,
        graph: &Graph,
        node: NodeId,
        region: Region,
    ) -> Result<Vec<NodeStatus>, Error> {
        let step = self.plan(graph, node, region)?;
        let mut out = Vec::new();
        self.collect_status(graph, &step, &mut out)?;
        Ok(out)
    }

    fn collect_status(
        &self,
        graph: &Graph,
        step: &Step,
        out: &mut Vec<NodeStatus>,
    ) -> Result<(), Error> {
        if let Work::Module { inputs, .. } = &step.work {
            for (input, _) in inputs {
                self.collect_status(graph, input, out)?;
            }
        }
        if out
            .iter()
            .any(|s| s.node == step.node && s.region == step.region)
        {
            return Ok(());
        }
        let cell = self
            .cache
            .lock()
            .expect("engine cache lock")
            .get(&step.key)
            .cloned();
        let (last_key, load) = self.track(step, |t| {
            (
                t.last_key,
                Load {
                    requests: t.requests,
                    computed: t.computed,
                    shared: t.requests - t.computed,
                    running: t.running,
                },
            )
        });
        let state = match cell.as_deref().map(OnceLock::get) {
            Some(Some(Ok(_))) => NodeState::Done,
            Some(Some(Err(_))) => NodeState::Failed,
            Some(None) => NodeState::Running,
            None if last_key.is_some_and(|k| k != step.key) => NodeState::Stale,
            None => NodeState::Idle,
        };
        let (node_type, family, role) = match &graph.node(step.node)?.kind {
            Kind::Source { .. } => (NodeType::Source, Family::Io, Role::General),
            Kind::Module { module, .. } => {
                let m = module.manifest();
                let t = match m.kind {
                    ModuleKind::Process => NodeType::Process,
                    ModuleKind::Decision => NodeType::Decision,
                    ModuleKind::Sink => NodeType::Sink,
                };
                (t, m.family, m.role)
            }
        };
        out.push(NodeStatus {
            node: step.node,
            name: step.name.clone(),
            node_type,
            family,
            role,
            state,
            region: step.region,
            key: step.key.short(),
            load,
            runs_on: "cpu",
        });
        Ok(())
    }

    pub fn bus(&self) -> &Arc<Bus> {
        &self.bus
    }

    pub fn counters(&self) -> Counters {
        Counters {
            requests: self.requests.load(Ordering::SeqCst),
            computed: self.computed.load(Ordering::SeqCst),
            source_reads: self.source_reads.load(Ordering::SeqCst),
        }
    }

    /// Renders one output port of a node over a region.
    pub fn render(
        &self,
        graph: &Graph,
        node: NodeId,
        port: &str,
        region: Region,
    ) -> Result<Image, Error> {
        let (index, _) = graph.output_port(node, port)?;
        let cause = self.bus.publish(
            SOURCE,
            None,
            &RenderRequested {
                node,
                port: port.into(),
                region,
            },
        );
        let step = self.plan(graph, node, region)?;
        let outputs = self.execute(&step, cause)?;
        Ok(outputs[index].clone())
    }

    /// The content key of a node's outputs over a region, without computing them.
    pub fn key(&self, graph: &Graph, node: NodeId, region: Region) -> Result<ContentHash, Error> {
        Ok(self.plan(graph, node, region)?.key)
    }

    fn plan(&self, graph: &Graph, id: NodeId, region: Region) -> Result<Step, Error> {
        let node = graph.node(id)?;
        let mut h = blake3::Hasher::new();
        h.update(&region.x.to_le_bytes());
        h.update(&region.y.to_le_bytes());
        h.update(&region.width.to_le_bytes());
        h.update(&region.height.to_le_bytes());
        let work = match &node.kind {
            Kind::Source { image, hash } => {
                h.update(b"helios.step.source.v1");
                h.update(&hash.0);
                Work::Source(image.clone())
            }
            Kind::Module {
                module,
                params,
                context,
            } => {
                let manifest = module.manifest();
                h.update(b"helios.step.module.v1");
                h.update(manifest.name.as_bytes());
                h.update(&manifest.version.to_le_bytes());
                h.update(&params.canonical_bytes());
                let plan = module.prepare(params, context)?;
                h.update(&context.restricted_to(&manifest.context).canonical_bytes());
                let input_region = module.input_region(&plan, region);
                let mut inputs = Vec::with_capacity(node.inputs.len());
                for (i, connection) in node.inputs.iter().enumerate() {
                    let (src, port) = connection.ok_or_else(|| Error::Unconnected {
                        node: node.name.clone(),
                        port: manifest.inputs[i].name.clone(),
                    })?;
                    let step = self.plan(graph, src, input_region)?;
                    h.update(&step.key.0);
                    h.update(&(port as u64).to_le_bytes());
                    inputs.push((step, port));
                }
                Work::Module {
                    module: module.clone(),
                    plan,
                    inputs,
                }
            }
        };
        Ok(Step {
            key: ContentHash(*h.finalize().as_bytes()),
            graph: graph.id(),
            node: id,
            name: node.name.clone(),
            region,
            work,
        })
    }

    fn execute(&self, step: &Step, cause: u64) -> Outputs {
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.track(step, |t| {
            t.requests += 1;
            t.last_key = Some(step.key);
        });
        let cell = {
            let mut cache = self.cache.lock().expect("engine cache lock");
            cache.entry(step.key).or_default().clone()
        };
        // Only one thread runs the initializer; others asking for the same key wait for it.
        cell.get_or_init(|| {
            self.computed.fetch_add(1, Ordering::SeqCst);
            self.track(step, |t| {
                t.computed += 1;
                t.running += 1;
            });
            let result = self.compute(step, cause);
            self.track(step, |t| t.running -= 1);
            let event = StepComputed {
                node: step.node,
                name: step.name.clone(),
                key: step.key.short(),
                region: step.region,
                ok: result.is_ok(),
            };
            self.bus.publish(SOURCE, Some(cause), &event);
            result
        })
        .clone()
    }

    fn compute(&self, step: &Step, cause: u64) -> Outputs {
        match &step.work {
            Work::Source(image) => {
                self.source_reads.fetch_add(1, Ordering::SeqCst);
                Ok(Arc::new(vec![image.sample(step.region)]))
            }
            Work::Module {
                module,
                plan,
                inputs,
            } => {
                let mut held = Vec::with_capacity(inputs.len());
                for (input, port) in inputs {
                    held.push((self.execute(input, cause)?, *port));
                }
                let images: Vec<&Image> =
                    held.iter().map(|(outputs, port)| &outputs[*port]).collect();
                Ok(Arc::new(module.process(plan, &images, step.region)?))
            }
        }
    }
}

impl Describe for Engine {
    fn describe(&self) -> serde_json::Value {
        let cache = self.cache.lock().expect("engine cache lock");
        let ready = cache.values().filter(|c| c.get().is_some()).count();
        serde_json::json!({
            "cache": { "entries": cache.len(), "ready": ready },
            "counters": self.counters(),
        })
    }
}
