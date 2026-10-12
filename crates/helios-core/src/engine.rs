//! Evaluates a graph, never computing the same content twice.
//!
//! Evaluation runs in two passes. **Planning** walks the graph from the
//! requested node, once per node however many paths lead to it. It first
//! collects, consumers before producers, the regions each consumer reads of
//! each node (each module prepares its plan and says which input region it
//! needs), then gives each node one step over the union of those regions:
//! the bounding box, or a few boxes when they lie far apart (see
//! [`unions`]). Each consumer reads its region as a crop of that step's
//! output, a view into the same buffer. Every step gets a content key: the
//! hash of its *lineage* (the module, its parameters and context, the scale,
//! and the lineages of its inputs, but no region) and of its region.
//! Nothing is computed yet. **Execution** then looks each key up: content
//! already computed is shared by reference; content being computed by
//! another thread is waited for (single flight); content of the same
//! lineage cached over a wider region is cropped; only missing content is
//! computed.
//!
//! The engine counts every request and every computation, so tests can
//! assert budgets ("rendering twice computes nothing the second time").
//!
//! With an [`Accelerator`], a step it runs keeps its result in the
//! accelerator's memory ([`DeviceImage`]): the next accelerated step reads
//! it there. Content moves only where the other side needs it: a host image
//! is uploaded when an accelerated step reads it, a device image is read
//! back (once, the copy kept with it) when a CPU step or the render's caller
//! reads it. A chain of accelerated steps thus uploads its source once and
//! reads back its result once. Device content is cached like host content,
//! under the same key and within the same budget (its bytes count).

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
use crate::module::{Accelerator, DeviceImage};
use crate::status::{Load, NodeState, NodeStatus, NodeType};

/// The source name the engine publishes events under.
const SOURCE: &str = "engine";

type Outputs = Result<Arc<Content>, Error>;

/// A step's outputs, where they live.
enum Content {
    /// In host memory, one image per output port.
    Host(Vec<Image>),
    /// An accelerated step's only output, in the accelerator's memory, and
    /// its copy in host memory once something on the CPU read it.
    Device {
        image: DeviceImage,
        host: OnceLock<Image>,
    },
}

impl Content {
    /// Bytes held, in host and device memory.
    fn bytes(&self) -> usize {
        match self {
            Content::Host(images) => images.iter().map(Image::bytes).sum(),
            Content::Device { image, host } => image.bytes() + host.get().map_or(0, Image::bytes),
        }
    }
}

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

/// The cache after a render: what it holds and what it evicted.
#[derive(Serialize)]
pub struct CacheSettled {
    /// Bytes held by ready entries.
    pub bytes: usize,
    /// The budget, `None` without a limit.
    pub budget: Option<usize>,
    pub entries: usize,
    /// Entries evicted during the render.
    pub evicted: u64,
}

impl Event for CacheSettled {
    const KIND: &'static str = "engine.cache_settled";
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
    /// Host images copied into the accelerator's memory.
    pub uploads: u64,
    /// Accelerator images read back into host memory.
    pub readbacks: u64,
    /// Content keys computed while planning: one per step (a node over
    /// the union of what its consumers read) of each render, status or key
    /// asked for.
    pub keys: u64,
    /// Steps served by cropping content of the same lineage cached over a
    /// wider region.
    pub crops: u64,
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
            uploads: self.uploads - earlier.uploads,
            readbacks: self.readbacks - earlier.readbacks,
            keys: self.keys - earlier.keys,
            crops: self.crops - earlier.crops,
        }
    }
}

/// A step of an evaluation, planned but not run.
struct Step {
    key: ContentHash,
    /// The content's identity without its region: content of the same
    /// lineage over a wider region holds this step's pixels.
    lineage: ContentHash,
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
        /// Steps read, each over a region holding `input_region`.
        inputs: Vec<(Arc<Step>, usize)>,
        /// The region of every input the module reads.
        input_region: Region,
        /// Optional inputs left unconnected: their index among the
        /// module's inputs and their channels.
        absent: Vec<(usize, u32)>,
        /// Computed by the engine's accelerator.
        accelerated: bool,
    },
    /// A decision node with no recorded answer: its inputs are planned (a
    /// surface shows what it will look at), nothing is computed.
    Pending { inputs: Vec<(Arc<Step>, usize)> },
    /// A module that reads full-resolution input, in a render at a scale
    /// below 1: its full-resolution step, reduced.
    Reduce { input: Arc<Step>, scale: f64 },
}

/// A node at a scale (its bits): what planning plans once per render.
type Unit = (NodeId, u64);

/// A unit as planning knows it before regions: its lineage and what it reads.
struct Shape {
    lineage: ContentHash,
    name: String,
    kind: ShapeKind,
}

enum ShapeKind {
    Source(Image, f64),
    Pending {
        inputs: Vec<(Unit, usize)>,
    },
    /// The same node at full resolution, reduced to the unit's scale.
    Reduce {
        input: Unit,
        scale: f64,
    },
    Module {
        module: ModuleRef,
        plan: crate::module::Plan,
        inputs: Vec<(Unit, usize)>,
        absent: Vec<(usize, u32)>,
        accelerated: bool,
    },
}

/// The regions a node is computed over, from the regions its consumers
/// read: two regions are merged into their bounding box when the box has
/// no more pixels than the two apart (so a merge never computes more than
/// it saves), until no two can be. Overlapping reads (the same image read
/// with different halos) become one region; reads far apart stay apart.
pub fn unions(mut regions: Vec<Region>) -> Vec<Region> {
    regions.sort_by_key(|r| (r.y, r.x, r.height, r.width));
    regions.dedup();
    loop {
        let mut merged = None;
        'pairs: for i in 0..regions.len() {
            for j in i + 1..regions.len() {
                let (a, b) = (regions[i], regions[j]);
                let bounds = a.bounds(&b);
                if bounds.area() <= a.area() + b.area() {
                    merged = Some((i, j, bounds));
                    break 'pairs;
                }
            }
        }
        let Some((i, j, bounds)) = merged else {
            return regions;
        };
        regions[i] = bounds;
        regions.remove(j);
    }
}

/// What a lookup in the cache finds for a step.
enum Found {
    /// The step's own entry, ready or not.
    Cell(Arc<OnceLock<Outputs>>),
    /// Ready content of the same lineage over a wider region.
    Wider(Arc<Content>),
}

/// Computed content, by key, within an optional memory budget.
#[derive(Default)]
struct Cache {
    entries: HashMap<ContentHash, Entry>,
    /// Keys of the entries of each lineage, to find a wider region.
    lineages: HashMap<ContentHash, Vec<ContentHash>>,
    /// Bytes held by ready entries.
    bytes: usize,
    /// Bumped on every use, to find the least recently used entry.
    tick: u64,
    evictions: u64,
}

struct Entry {
    cell: Arc<OnceLock<Outputs>>,
    lineage: ContentHash,
    region: Region,
    /// Bytes of its outputs, counted once they are ready.
    size: usize,
    last_used: u64,
}

impl Cache {
    /// The step's own cell if it has one; else ready content of its
    /// lineage over a wider region; else a new cell. What is found is
    /// marked as just used.
    fn find(&mut self, step: &Step) -> Found {
        self.tick += 1;
        let tick = self.tick;
        if let Some(entry) = self.entries.get_mut(&step.key) {
            entry.last_used = tick;
            return Found::Cell(entry.cell.clone());
        }
        if let Some((key, content)) = self.wider(step) {
            self.entries.get_mut(&key).expect("listed").last_used = tick;
            return Found::Wider(content);
        }
        let cell: Arc<OnceLock<Outputs>> = Arc::default();
        self.entries.insert(
            step.key,
            Entry {
                cell: cell.clone(),
                lineage: step.lineage,
                region: step.region,
                size: 0,
                last_used: tick,
            },
        );
        self.lineages
            .entry(step.lineage)
            .or_default()
            .push(step.key);
        Found::Cell(cell)
    }

    /// Ready content of `step`'s lineage over a region holding its own,
    /// the smallest such: its key and content.
    fn wider(&self, step: &Step) -> Option<(ContentHash, Arc<Content>)> {
        self.lineages
            .get(&step.lineage)?
            .iter()
            .filter_map(|key| {
                let entry = &self.entries[key];
                match entry.cell.get() {
                    Some(Ok(content)) if entry.region.contains(&step.region) => {
                        Some((entry.region.area(), *key, content.clone()))
                    }
                    _ => None,
                }
            })
            .min_by_key(|(area, ..)| *area)
            .map(|(_, key, content)| (key, content))
    }

    fn remove(&mut self, key: ContentHash) -> Option<Entry> {
        let entry = self.entries.remove(&key)?;
        if let Some(keys) = self.lineages.get_mut(&entry.lineage) {
            keys.retain(|k| *k != key);
            if keys.is_empty() {
                self.lineages.remove(&entry.lineage);
            }
        }
        Some(entry)
    }

    /// Counts what a ready entry holds (again: a device entry grows when it
    /// is read back), then evicts the least recently used ready entries
    /// (never `keep`, never content still being computed) until the cache
    /// fits its budget.
    fn settle(&mut self, keep: ContentHash, budget: Option<usize>) {
        if let Some(entry) = self.entries.get_mut(&keep)
            && let Some(Ok(content)) = entry.cell.get()
        {
            let size = content.bytes();
            self.bytes = self.bytes - entry.size + size;
            entry.size = size;
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
            let entry = self.remove(key).expect("victim exists");
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
    uploads: AtomicU64,
    readbacks: AtomicU64,
    keys: AtomicU64,
    crops: AtomicU64,
}

type TrackKey = (u64, NodeId, Region);

/// A planned render: the step asked for (it reads every other).
struct Planned {
    root: Arc<Step>,
}

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
            uploads: AtomicU64::new(0),
            readbacks: AtomicU64::new(0),
            keys: AtomicU64::new(0),
            crops: AtomicU64::new(0),
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
        let step = self.plan(graph, node, region, 1.0)?.root;
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
        // A step already listed has its inputs listed before it: shared
        // steps are walked once.
        if out
            .iter()
            .any(|s| s.node == step.node && s.region == step.region)
        {
            return Ok(());
        }
        match &step.work {
            Work::Module { inputs, .. } | Work::Pending { inputs } => {
                for (input, _) in inputs {
                    self.collect_status(graph, input, out)?;
                }
            }
            Work::Reduce { input, .. } => self.collect_status(graph, input, out)?,
            Work::Source(..) => {}
        }
        let (cell, wider) = {
            let cache = self.cache.lock().expect("engine cache lock");
            let cell = cache.entries.get(&step.key).map(|e| e.cell.clone());
            let wider = cell.is_none() && cache.wider(step).is_some();
            (cell, wider)
        };
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
            None if wider => NodeState::Done,
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
            uploads: self.uploads.load(Ordering::SeqCst),
            readbacks: self.readbacks.load(Ordering::SeqCst),
            keys: self.keys.load(Ordering::SeqCst),
            crops: self.crops.load(Ordering::SeqCst),
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
        let (step, content, index) = self.render_content(graph, node, port, region, scale)?;
        Ok(self.host(&step, &content, index)?.sample(region))
    }

    /// Renders at full resolution into the accelerator's memory, for a
    /// consumer on the same device (a viewport sharing the GUI's device):
    /// the result is not read back. An accelerated result is handed as is
    /// (shared with the cache: do not write to it); a result computed on
    /// the CPU is uploaded. [`Error::NoAccelerator`] without an accelerator.
    pub fn render_device(
        &self,
        graph: &Graph,
        node: NodeId,
        port: &str,
        region: Region,
    ) -> Result<DeviceImage, Error> {
        self.render_device_at(graph, node, port, region, 1.0)
    }

    /// [`Engine::render_device`] at a scale, as [`Engine::render_at`].
    pub fn render_device_at(
        &self,
        graph: &Graph,
        node: NodeId,
        port: &str,
        region: Region,
        scale: f64,
    ) -> Result<DeviceImage, Error> {
        let accelerator = self.accelerator.clone().ok_or(Error::NoAccelerator)?;
        let (step, content, index) = self.render_content(graph, node, port, region, scale)?;
        self.device(accelerator.as_ref(), &step, &step, &content, index, region)
    }

    /// Plans and executes a render: the requested step, its content and
    /// the index of the port asked for.
    fn render_content(
        &self,
        graph: &Graph,
        node: NodeId,
        port: &str,
        region: Region,
        scale: f64,
    ) -> Result<(Arc<Step>, Arc<Content>, usize), Error> {
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
        let step = self.plan(graph, node, region, scale)?.root;
        let content = self.execute(&step, cause)?;
        Ok((step, content, index))
    }

    /// Output `port` of `step`'s content in host memory. Device content is
    /// read back once; the copy stays with it, counted in the cache.
    fn host<'a>(&self, step: &Step, content: &'a Content, port: usize) -> Result<&'a Image, Error> {
        let (image, host) = match content {
            Content::Host(images) => return Ok(&images[port]),
            Content::Device { image, host } => (image, host),
        };
        if let Some(copy) = host.get() {
            return Ok(copy);
        }
        let accelerator = self
            .accelerator
            .as_ref()
            .expect("device content comes from the accelerator");
        self.readbacks.fetch_add(1, Ordering::SeqCst);
        let copy = accelerator
            .download(image)
            .map_err(|reason| transfer_error(step, reason))?;
        let copy = host.get_or_init(|| copy);
        self.cache
            .lock()
            .expect("engine cache lock")
            .settle(step.key, self.budget);
        Ok(copy)
    }

    /// Output `port` of `step`'s content over `region` in the
    /// accelerator's memory, for `consumer`, which reads it. Device content
    /// over that very region is handed as is; otherwise its crop is
    /// uploaded (device content over a wider region is read back first).
    fn device(
        &self,
        accelerator: &dyn Accelerator,
        step: &Step,
        consumer: &Step,
        content: &Content,
        port: usize,
        region: Region,
    ) -> Result<DeviceImage, Error> {
        if let Content::Device { image, .. } = content
            && image.region() == region
        {
            return Ok(image.clone());
        }
        let host = self.host(step, content, port)?.sample(region);
        self.uploads.fetch_add(1, Ordering::SeqCst);
        accelerator
            .upload(&host)
            .map_err(|reason| transfer_error(consumer, reason))
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
        Ok(self.plan(graph, node, region, scale)?.root.key)
    }

    /// Plans a render of `id` over `region` at `scale`: each node it reads
    /// once, consumers before producers to gather the regions each node is
    /// read over, then the steps, producers first.
    fn plan(
        &self,
        graph: &Graph,
        id: NodeId,
        region: Region,
        scale: f64,
    ) -> Result<Planned, Error> {
        let root: Unit = (id, scale.to_bits());
        let mut shapes = HashMap::new();
        let mut order = Vec::new();
        self.shape(graph, root, &mut shapes, &mut order)?;

        // The regions each unit is read over, then their unions.
        let mut asked: HashMap<Unit, Vec<Region>> = HashMap::from([(root, vec![region])]);
        let mut regions: HashMap<Unit, Vec<Region>> = HashMap::new();
        for unit in order.iter().rev() {
            let Some(reads) = asked.remove(unit) else {
                continue;
            };
            let merged = unions(reads);
            let shape: &Shape = &shapes[unit];
            for r in &merged {
                match &shape.kind {
                    ShapeKind::Source(..) => {}
                    ShapeKind::Reduce { input, scale } => {
                        asked
                            .entry(*input)
                            .or_default()
                            .push(r.full_resolution(*scale));
                    }
                    ShapeKind::Pending { inputs } => {
                        for (src, _) in inputs {
                            asked.entry(*src).or_default().push(*r);
                        }
                    }
                    ShapeKind::Module {
                        module,
                        plan,
                        inputs,
                        ..
                    } => {
                        let need = module.input_region(plan, *r);
                        for (src, _) in inputs {
                            asked.entry(*src).or_default().push(need);
                        }
                    }
                }
            }
            regions.insert(*unit, merged);
        }

        // The steps, producers first, each reading the union of its input
        // that holds what it needs.
        let mut steps: HashMap<Unit, Vec<Arc<Step>>> = HashMap::new();
        for unit in &order {
            let Some(merged) = regions.remove(unit) else {
                continue;
            };
            let shape = &shapes[unit];
            let reading = |src: &Unit, need: Region| -> Arc<Step> {
                steps[src]
                    .iter()
                    .filter(|s| s.region.contains(&need))
                    .min_by_key(|s| s.region.area())
                    .cloned()
                    .expect("a union holds every region read of it")
            };
            let mut planned = Vec::with_capacity(merged.len());
            for r in merged {
                let work = match &shape.kind {
                    ShapeKind::Source(image, scale) => Work::Source(image.clone(), *scale),
                    ShapeKind::Reduce { input, scale } => Work::Reduce {
                        input: reading(input, r.full_resolution(*scale)),
                        scale: *scale,
                    },
                    ShapeKind::Pending { inputs } => Work::Pending {
                        inputs: inputs.iter().map(|(u, p)| (reading(u, r), *p)).collect(),
                    },
                    ShapeKind::Module {
                        module,
                        plan,
                        inputs,
                        absent,
                        accelerated,
                    } => {
                        let input_region = module.input_region(plan, r);
                        Work::Module {
                            module: module.clone(),
                            plan: plan.clone(),
                            inputs: inputs
                                .iter()
                                .map(|(u, p)| (reading(u, input_region), *p))
                                .collect(),
                            input_region,
                            absent: absent.clone(),
                            accelerated: *accelerated,
                        }
                    }
                };
                self.keys.fetch_add(1, Ordering::SeqCst);
                let mut h = blake3::Hasher::new();
                h.update(b"helios.step.v2");
                h.update(&shape.lineage.0);
                h.update(&r.x.to_le_bytes());
                h.update(&r.y.to_le_bytes());
                h.update(&r.width.to_le_bytes());
                h.update(&r.height.to_le_bytes());
                planned.push(Arc::new(Step {
                    key: ContentHash(*h.finalize().as_bytes()),
                    lineage: shape.lineage,
                    graph: graph.id(),
                    node: unit.0,
                    name: shape.name.clone(),
                    region: r,
                    work,
                }));
            }
            steps.insert(*unit, planned);
        }
        let root = steps
            .remove(&root)
            .and_then(|s| s.into_iter().next())
            .expect("the unit asked for is planned");
        Ok(Planned { root })
    }

    /// Plans `unit` and everything it reads, each once (`shapes`), adding
    /// them to `order` producers first: their lineages and what they read.
    fn shape(
        &self,
        graph: &Graph,
        unit: Unit,
        shapes: &mut HashMap<Unit, Shape>,
        order: &mut Vec<Unit>,
    ) -> Result<(), Error> {
        if shapes.contains_key(&unit) {
            return Ok(());
        }
        let (id, bits) = unit;
        let scale = f64::from_bits(bits);
        let node = graph.node(id)?;
        let mut h = blake3::Hasher::new();
        h.update(&scale.to_le_bytes());
        let kind = match &node.kind {
            Kind::Source { image, hash } => {
                h.update(b"helios.step.source.v1");
                h.update(&hash.0);
                ShapeKind::Source(image.clone(), scale)
            }
            Kind::Module {
                answered: false, ..
            } => {
                h.update(b"helios.step.pending.v1");
                let mut inputs = Vec::with_capacity(node.inputs.len());
                for (src, port) in node.inputs.iter().flatten() {
                    let input = (*src, bits);
                    self.shape(graph, input, shapes, order)?;
                    h.update(&shapes[&input].lineage.0);
                    inputs.push((input, *port));
                }
                ShapeKind::Pending { inputs }
            }
            Kind::Module { module, .. }
                if scale < 1.0 && module.manifest().needs.full_resolution =>
            {
                // Computed at full resolution, then reduced.
                let full = (id, 1.0f64.to_bits());
                self.shape(graph, full, shapes, order)?;
                h.update(b"helios.step.reduce.v1");
                h.update(&shapes[&full].lineage.0);
                ShapeKind::Reduce { input: full, scale }
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
                let mut inputs = Vec::with_capacity(node.inputs.len());
                let mut absent = Vec::new();
                for (i, connection) in node.inputs.iter().enumerate() {
                    let spec = &manifest.inputs[i];
                    let Some((src, port)) = *connection else {
                        if spec.optional {
                            h.update(b"helios.step.absent.v1");
                            h.update(&(i as u64).to_le_bytes());
                            absent.push((i, spec.channels().unwrap_or(1)));
                            continue;
                        }
                        return Err(Error::Unconnected {
                            node: node.name.clone(),
                            port: spec.name.clone(),
                        });
                    };
                    let input = (src, bits);
                    self.shape(graph, input, shapes, order)?;
                    h.update(&shapes[&input].lineage.0);
                    h.update(&(port as u64).to_le_bytes());
                    inputs.push((input, port));
                }
                ShapeKind::Module {
                    module: module.clone(),
                    plan,
                    inputs,
                    absent,
                    accelerated,
                }
            }
        };
        shapes.insert(
            unit,
            Shape {
                lineage: ContentHash(*h.finalize().as_bytes()),
                name: node.name.clone(),
                kind,
            },
        );
        order.push(unit);
        Ok(())
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
        let found = self.cache.lock().expect("engine cache lock").find(step);
        let cell = match found {
            Found::Cell(cell) => cell,
            Found::Wider(content) => {
                self.crops.fetch_add(1, Ordering::SeqCst);
                return Ok(content);
            }
        };
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
                cache.remove(step.key);
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
                Ok(Arc::new(Content::Host(vec![
                    image.sample_at(step.region, *scale),
                ])))
            }
            Work::Module {
                module,
                plan,
                inputs,
                input_region,
                absent,
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
                if let (Some(a), true) = (&self.accelerator, accelerated) {
                    let (content, port) = &held[0];
                    let input = self.device(
                        a.as_ref(),
                        &inputs[0].0,
                        step,
                        content,
                        *port,
                        *input_region,
                    )?;
                    self.accelerated.fetch_add(1, Ordering::SeqCst);
                    let image = module.accelerate(a.as_ref(), plan, &input, step.region)?;
                    Ok(Arc::new(Content::Device {
                        image,
                        host: OnceLock::new(),
                    }))
                } else {
                    // Each input cropped to what the module reads: a view.
                    let mut images = held
                        .iter()
                        .zip(inputs)
                        .map(|((content, port), (input, _))| {
                            Ok(self.host(input, content, *port)?.sample(*input_region))
                        })
                        .collect::<Result<Vec<Image>, Error>>()?;
                    // In index order, so each lands at its port's place.
                    for &(i, channels) in absent {
                        images.insert(i, Image::absent(channels));
                    }
                    let images: Vec<&Image> = images.iter().collect();
                    Ok(Arc::new(Content::Host(module.process(
                        plan,
                        &images,
                        step.region,
                    )?)))
                }
            }
            Work::Reduce { input, scale } => {
                let content = self.execute(input, cause)?;
                let ports = match content.as_ref() {
                    Content::Host(images) => images.len(),
                    Content::Device { .. } => 1,
                };
                let images = (0..ports)
                    .map(|port| {
                        Ok(self
                            .host(input, &content, port)?
                            .sample_at(step.region, *scale))
                    })
                    .collect::<Result<Vec<Image>, Error>>()?;
                Ok(Arc::new(Content::Host(images)))
            }
            Work::Pending { .. } => unreachable!("execute answers pending steps itself"),
        }
    }
}

/// A copy to or from the accelerator failed, for `step`: its module's error.
fn transfer_error(step: &Step, reason: String) -> Error {
    let module = match &step.work {
        Work::Module { module, .. } => module.manifest().name.clone(),
        _ => step.name.clone(),
    };
    Error::Module {
        module,
        reason: format!("GPU: {reason}"),
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
