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
use crate::module::Accelerator;
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

/// Where an injected fault strikes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultKind {
    /// The next computation of a module node fails.
    Compute,
    /// The next read of a source node fails.
    SourceRead,
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
    /// Cache entries evicted to stay within the memory budget.
    pub evictions: u64,
    /// Steps computed by the accelerator (the GPU).
    pub accelerated: u64,
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
            evictions: self.evictions - earlier.evictions,
            accelerated: self.accelerated - earlier.accelerated,
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
    /// A source read at a scale.
    Source(Image, f64),
    Module {
        module: ModuleRef,
        plan: crate::module::Plan,
        inputs: Vec<(Step, usize)>,
        /// Computed by the engine's accelerator.
        accelerated: bool,
    },
    /// A decision node with no recorded answer: its inputs are planned (a
    /// surface shows what it will look at), nothing is computed.
    Pending { inputs: Vec<(Step, usize)> },
}

/// Computed content, by key, within an optional memory budget.
#[derive(Default)]
struct Cache {
    entries: HashMap<ContentHash, Entry>,
    /// Bytes held by ready entries.
    bytes: usize,
    /// Bumped on every use, to find the least recently used entry.
    tick: u64,
    evictions: u64,
}

struct Entry {
    cell: Arc<OnceLock<Outputs>>,
    /// Bytes of its outputs, counted once they are ready.
    size: usize,
    last_used: u64,
}

impl Cache {
    /// The cell for a key, created if missing, marked as just used.
    fn cell(&mut self, key: ContentHash) -> Arc<OnceLock<Outputs>> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.entries.entry(key).or_insert_with(|| Entry {
            cell: Arc::default(),
            size: 0,
            last_used: tick,
        });
        entry.last_used = tick;
        entry.cell.clone()
    }

    /// Counts a newly ready entry, then evicts the least recently used
    /// ready entries (never `keep`, never content still being computed)
    /// until the cache fits its budget.
    fn settle(&mut self, keep: ContentHash, budget: Option<usize>) {
        if let Some(entry) = self.entries.get_mut(&keep)
            && entry.size == 0
            && let Some(Ok(outputs)) = entry.cell.get()
        {
            entry.size = outputs
                .iter()
                .map(|i| std::mem::size_of_val(i.data()))
                .sum();
            self.bytes += entry.size;
        }
        let Some(budget) = budget else { return };
        while self.bytes > budget {
            let victim = self
                .entries
                .iter()
                .filter(|(k, e)| **k != keep && e.size > 0 && e.cell.get().is_some())
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| *k);
            let Some(key) = victim else { break };
            let entry = self.entries.remove(&key).expect("victim exists");
            self.bytes -= entry.size;
            self.evictions += 1;
        }
    }
}

pub struct Engine {
    bus: Arc<Bus>,
    cache: Mutex<Cache>,
    /// Bytes the cache may hold; `None` for no limit.
    budget: Option<usize>,
    requests: AtomicU64,
    computed: AtomicU64,
    source_reads: AtomicU64,
    /// What each node did, per graph and region.
    tracks: Mutex<HashMap<TrackKey, Track>>,
    /// Faults injected and not yet hit: (graph, node, kind).
    faults: Mutex<Vec<(u64, NodeId, FaultKind)>>,
    /// Runs GPU variants; `None`: everything on the CPU.
    accelerator: Option<Arc<dyn Accelerator>>,
    accelerated: AtomicU64,
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
    /// An engine whose cache has no memory limit.
    pub fn new(bus: Arc<Bus>) -> Self {
        Self::with_budget(bus, None)
    }

    /// An engine whose cache stays within `budget` bytes (`None`: no limit).
    /// Content evicted is computed again when asked for.
    pub fn with_budget(bus: Arc<Bus>, budget: Option<usize>) -> Self {
        Self {
            bus,
            cache: Mutex::new(Cache::default()),
            budget,
            requests: AtomicU64::new(0),
            computed: AtomicU64::new(0),
            source_reads: AtomicU64::new(0),
            tracks: Mutex::new(HashMap::new()),
            faults: Mutex::new(Vec::new()),
            accelerator: None,
            accelerated: AtomicU64::new(0),
        }
    }

    /// Computes the steps whose module has a GPU variant on `accelerator`;
    /// the others stay on the CPU. Their content is keyed apart from CPU
    /// content, as the two agree within a tolerance only.
    pub fn with_accelerator(mut self, accelerator: Arc<dyn Accelerator>) -> Self {
        self.accelerator = Some(accelerator);
        self
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
        self.status_at(graph, node, region, 1.0)
    }

    /// [`Engine::status`] for a render at a scale (`region` in pixels at
    /// that scale): what a viewport showing a proxy asks.
    pub fn status_at(
        &self,
        graph: &Graph,
        node: NodeId,
        region: Region,
        scale: f64,
    ) -> Result<Vec<NodeStatus>, Error> {
        let step = self.plan(graph, node, region, scale)?;
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
        if let Work::Module { inputs, .. } | Work::Pending { inputs } = &step.work {
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
            .entries
            .get(&step.key)
            .map(|e| e.cell.clone());
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
            _ if matches!(step.work, Work::Pending { .. }) => NodeState::Pending,
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
            evictions: self.cache.lock().expect("engine cache lock").evictions,
            accelerated: self.accelerated.load(Ordering::SeqCst),
        }
    }

    /// Renders one output port of a node over a region, at full resolution.
    pub fn render(
        &self,
        graph: &Graph,
        node: NodeId,
        port: &str,
        region: Region,
    ) -> Result<Image, Error> {
        self.render_at(graph, node, port, region, 1.0)
    }

    /// Renders at a scale: 1 is full resolution, 0.5 half of it, and so on
    /// (above 0, at most 1). `region` is in pixels at that scale. Sources are
    /// reduced by the framework (each pixel the mean of the full-resolution
    /// pixels it covers); modules get the scale in `prepare_at`. A render at
    /// a scale is cached apart from every other scale.
    pub fn render_at(
        &self,
        graph: &Graph,
        node: NodeId,
        port: &str,
        region: Region,
        scale: f64,
    ) -> Result<Image, Error> {
        if !(scale > 0.0 && scale <= 1.0) {
            return Err(Error::Scale(scale.to_string()));
        }
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
        let step = self.plan(graph, node, region, scale)?;
        let outputs = self.execute(&step, cause)?;
        Ok(outputs[index].clone())
    }

    /// The content key of a node's outputs over a region, without computing them.
    pub fn key(&self, graph: &Graph, node: NodeId, region: Region) -> Result<ContentHash, Error> {
        self.key_at(graph, node, region, 1.0)
    }

    /// The content key of a render at a scale.
    pub fn key_at(
        &self,
        graph: &Graph,
        node: NodeId,
        region: Region,
        scale: f64,
    ) -> Result<ContentHash, Error> {
        Ok(self.plan(graph, node, region, scale)?.key)
    }

    fn plan(&self, graph: &Graph, id: NodeId, region: Region, scale: f64) -> Result<Step, Error> {
        let node = graph.node(id)?;
        let mut h = blake3::Hasher::new();
        h.update(&scale.to_le_bytes());
        h.update(&region.x.to_le_bytes());
        h.update(&region.y.to_le_bytes());
        h.update(&region.width.to_le_bytes());
        h.update(&region.height.to_le_bytes());
        let work = match &node.kind {
            Kind::Source { image, hash } => {
                h.update(b"helios.step.source.v1");
                h.update(&hash.0);
                Work::Source(image.clone(), scale)
            }
            Kind::Module {
                answered: false, ..
            } => {
                h.update(b"helios.step.pending.v1");
                let mut inputs = Vec::with_capacity(node.inputs.len());
                for (src, port) in node.inputs.iter().flatten() {
                    let step = self.plan(graph, *src, region, scale)?;
                    h.update(&step.key.0);
                    inputs.push((step, *port));
                }
                Work::Pending { inputs }
            }
            Kind::Module {
                module,
                params,
                context,
                ..
            } => {
                let manifest = module.manifest();
                h.update(b"helios.step.module.v1");
                h.update(manifest.name.as_bytes());
                h.update(&manifest.version.to_le_bytes());
                h.update(&params.canonical_bytes());
                let plan = module.prepare(params, context, scale)?;
                h.update(&context.restricted_to(&manifest.context).canonical_bytes());
                let accelerated = match &self.accelerator {
                    Some(a) if module.acceleratable() => {
                        h.update(b"helios.step.accelerated.v1");
                        h.update(a.id().as_bytes());
                        true
                    }
                    _ => false,
                };
                let input_region = module.input_region(&plan, region);
                let mut inputs = Vec::with_capacity(node.inputs.len());
                for (i, connection) in node.inputs.iter().enumerate() {
                    let (src, port) = connection.ok_or_else(|| Error::Unconnected {
                        node: node.name.clone(),
                        port: manifest.inputs[i].name.clone(),
                    })?;
                    let step = self.plan(graph, src, input_region, scale)?;
                    h.update(&step.key.0);
                    h.update(&(port as u64).to_le_bytes());
                    inputs.push((step, port));
                }
                Work::Module {
                    module: module.clone(),
                    plan,
                    inputs,
                    accelerated,
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
        if let Work::Pending { .. } = step.work {
            // Nothing to compute or cache: the answer is not there yet.
            return Err(Error::Pending {
                node: step.name.clone(),
            });
        }
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.track(step, |t| {
            t.requests += 1;
            t.last_key = Some(step.key);
        });
        let cell = self.cache.lock().expect("engine cache lock").cell(step.key);
        let mut computed_here = false;
        // Only one thread runs the initializer; others asking for the same key wait for it.
        let outputs = cell
            .get_or_init(|| {
                computed_here = true;
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
            .clone();
        if computed_here {
            self.cache
                .lock()
                .expect("engine cache lock")
                .settle(step.key, self.budget);
        }
        if outputs.as_ref().is_err_and(Error::is_transient) {
            // A transient failure is not an answer: forget it, so asking
            // again computes again.
            let mut cache = self.cache.lock().expect("engine cache lock");
            if cache
                .entries
                .get(&step.key)
                .is_some_and(|e| Arc::ptr_eq(&e.cell, &cell))
            {
                cache.entries.remove(&step.key);
            }
        }
        outputs
    }

    /// Makes the next computation (or source read) of a node fail once, to
    /// exercise error paths on demand. Consumed by the first step it hits.
    pub fn inject_fault(&self, graph: &Graph, node: NodeId, kind: FaultKind) {
        self.faults
            .lock()
            .expect("engine faults lock")
            .push((graph.id(), node, kind));
    }

    fn take_fault(&self, step: &Step, kind: FaultKind) -> bool {
        let mut faults = self.faults.lock().expect("engine faults lock");
        match faults
            .iter()
            .position(|f| *f == (step.graph, step.node, kind))
        {
            Some(i) => {
                faults.remove(i);
                true
            }
            None => false,
        }
    }

    fn compute(&self, step: &Step, cause: u64) -> Outputs {
        match &step.work {
            Work::Source(image, scale) => {
                self.source_reads.fetch_add(1, Ordering::SeqCst);
                if self.take_fault(step, FaultKind::SourceRead) {
                    return Err(Error::Fault {
                        node: step.name.clone(),
                        what: "source read".into(),
                    });
                }
                Ok(Arc::new(vec![image.sample_at(step.region, *scale)]))
            }
            Work::Module {
                module,
                plan,
                inputs,
                accelerated,
            } => {
                let mut held = Vec::with_capacity(inputs.len());
                for (input, port) in inputs {
                    held.push((self.execute(input, cause)?, *port));
                }
                if self.take_fault(step, FaultKind::Compute) {
                    return Err(Error::Fault {
                        node: step.name.clone(),
                        what: "computation".into(),
                    });
                }
                let images: Vec<&Image> =
                    held.iter().map(|(outputs, port)| &outputs[*port]).collect();
                match (&self.accelerator, accelerated) {
                    (Some(a), true) => {
                        self.accelerated.fetch_add(1, Ordering::SeqCst);
                        Ok(Arc::new(module.accelerate(
                            a.as_ref(),
                            plan,
                            images[0],
                            step.region,
                        )?))
                    }
                    _ => Ok(Arc::new(module.process(plan, &images, step.region)?)),
                }
            }
            Work::Pending { .. } => unreachable!("execute answers pending steps itself"),
        }
    }
}

impl Describe for Engine {
    fn describe(&self) -> serde_json::Value {
        // Read before taking the cache lock: `counters()` takes it too.
        let counters = self.counters();
        let cache = self.cache.lock().expect("engine cache lock");
        let ready = cache
            .entries
            .values()
            .filter(|e| e.cell.get().is_some())
            .count();
        serde_json::json!({
            "cache": {
                "entries": cache.entries.len(), "ready": ready,
                "bytes": cache.bytes, "budget": self.budget, "evictions": cache.evictions,
            },
            "counters": counters,
        })
    }
}
