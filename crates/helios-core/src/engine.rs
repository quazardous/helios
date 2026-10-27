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
//! A bypassed node ([`Graph::set_bypass`]) has no step: what reads it
//! reads the input it passes through, under that input's content key, so a
//! network with a node bypassed shares its content with the same network
//! without the node.
//!
//! The engine counts every request and every computation, so tests can
//! assert budgets ("rendering twice computes nothing the second time").
//!
//! The cache stays within a memory budget (see [`Engine::with_budget`]),
//! evicting what was used least recently, but never what the current
//! render still needs: a render pins the content of its steps until every
//! step that reads it has run. After each render the cache's size and
//! evictions go on the bus ([`CacheSettled`]).
//!
//! With an [`Accelerator`], a step it runs keeps its result in the
//! accelerator's memory ([`DeviceImage`]): the next accelerated step reads
//! it there. Content moves only where the other side needs it: a host image
//! is uploaded when an accelerated step reads it, a device image is read
//! back (once, the copy kept with it) when a CPU step or the render's caller
//! reads it. A chain of accelerated steps thus uploads its source once and
//! reads back its result once. Device content is cached like host content,
//! under the same key and within the same budget (its bytes count).
//!
//! The accelerator may fail; a render never does because of it. A step
//! whose kernel fails (it does not compile, does not run, or its memory
//! budget is spent once the engine evicted the device content it could) is
//! computed on the CPU instead, and the engine says so on the bus
//! ([`GpuFellBack`]). A lost or hung device ([`Failure::is_device`]) holds
//! nothing readable: the engine drops its device content, asks the
//! accelerator to rebuild its device and replays the render; when that
//! fails, it goes on without the accelerator for the rest of its life and
//! says so ([`GpuLost`]).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use crate::bus::{Bus, Event};
use crate::describe::Describe;
use crate::error::Error;
use crate::graph::{Graph, Kind, NodeId};
use crate::image::{ContentHash, Image, Region};
use crate::manifest::{Family, ModuleKind, Role};
use crate::message::Message;
use crate::module::{Accelerator, AcceleratorError, DeviceImage, Extent, Failure};
use crate::module::{Context, ModuleRef};
use crate::network::{Expansion, Network, Wire};
use crate::params::Params;
use crate::status::{Load, NodeState, NodeStatus, NodeType};

/// The source name the engine publishes events under.
const SOURCE: &str = "engine";

/// The share of physical memory the render cache may use by default, in
/// percent.
pub const DEFAULT_CACHE_SHARE: f64 = 25.0;

/// The cache budget where physical memory is not known: 2 GiB.
const UNKNOWN_MEMORY_BUDGET: usize = 2 << 30;

/// This machine's physical memory in bytes, where it is known (Linux:
/// `MemTotal` of `/proc/meminfo`).
pub fn physical_memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// A cache budget of `share` percent of `memory` bytes (2 GiB when memory
/// is not known); `None`, no limit, for a share of 0.
pub fn memory_share(share: f64, memory: Option<u64>) -> Option<usize> {
    if share <= 0.0 {
        return None;
    }
    Some(memory.map_or(UNKNOWN_MEMORY_BUDGET, |m| {
        (m as f64 * share.min(100.0) / 100.0) as usize
    }))
}

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

/// A step's GPU variant failed: the step was computed on the CPU instead.
#[derive(Serialize)]
pub struct GpuFellBack {
    pub node: NodeId,
    pub name: String,
    pub module: String,
    pub failure: Failure,
    pub reason: String,
    /// What a user should read.
    pub message: Message,
}

impl GpuFellBack {
    /// The event, with what a user should read.
    pub fn new(node: NodeId, name: &str, module: &str, failure: Failure, reason: &str) -> Self {
        GpuFellBack {
            node,
            name: name.into(),
            module: module.into(),
            failure,
            reason: reason.into(),
            message: Message::new(
                "engine-gpu-fell-back",
                "{ $module } ran on the CPU: its GPU version failed ({ $failure }: { $reason }).",
            )
            .with("module", module)
            .with("failure", failure.message())
            .with("reason", reason),
        }
    }
}

impl Event for GpuFellBack {
    const KIND: &'static str = "engine.gpu_fell_back";
}

/// The accelerator's device was lost or hung during a render. Either it
/// was rebuilt and the render replayed on it, or the engine computes
/// everything on the CPU from now on.
#[derive(Serialize)]
pub struct GpuLost {
    pub failure: Failure,
    pub reason: String,
    /// Rebuilt: the render is replayed on the new device.
    pub recovered: bool,
    /// What a user should read.
    pub message: Message,
}

impl Event for GpuLost {
    const KIND: &'static str = "engine.gpu_lost";
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
    /// Steps whose GPU variant failed, computed on the CPU instead.
    pub fallbacks: u64,
    /// Renders that lost the accelerator's device (lost or hung).
    pub device_losses: u64,
    /// Steps outside their module's extent, filled with its constant
    /// instead of running it (counted in `computed` too): see
    /// [`crate::Extent`].
    pub filled: u64,
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
            fallbacks: self.fallbacks - earlier.fallbacks,
            device_losses: self.device_losses - earlier.device_losses,
            filled: self.filled - earlier.filled,
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

impl Step {
    /// The steps it reads.
    fn inputs(&self) -> Vec<&Arc<Step>> {
        match &self.work {
            Work::Module { inputs, .. } | Work::Pending { inputs } => {
                inputs.iter().map(|(s, _)| s).collect()
            }
            Work::Reduce { input, .. } => vec![input],
            Work::Source(..) => Vec::new(),
        }
    }
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
        /// Outside the module's extent: every output holds this value, and
        /// nothing is read (`inputs` is empty) or run.
        fill: Option<f32>,
    },
    /// A decision node with no recorded answer: its inputs are planned (a
    /// surface shows what it will look at), nothing is computed.
    Pending { inputs: Vec<(Arc<Step>, usize)> },
    /// A module that reads full-resolution input, in a render at a scale
    /// below 1: its full-resolution step, reduced.
    Reduce { input: Arc<Step>, scale: f64 },
}

/// A node at a scale (its bits): what planning plans once per render. An
/// inner node of a composite is a unit of its own, `inner` numbering it
/// within the plan (0 for a node of the graph).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Unit {
    node: NodeId,
    inner: u32,
    scale: u64,
}

impl Unit {
    fn of(node: NodeId, scale: u64) -> Self {
        Self {
            node,
            inner: 0,
            scale,
        }
    }
}

/// An inner node of a composite, as planning knows it.
struct InnerUnit {
    /// The composite's unit.
    parent: Unit,
    network: Arc<Network>,
    index: usize,
    /// The path of names from the graph's node, `/`-separated.
    name: String,
    context: Context,
}

/// What a unit stands for, before it is shaped.
enum Desc {
    Source(Image, ContentHash),
    Module {
        name: String,
        module: ModuleRef,
        params: Params,
        context: Context,
        answered: bool,
        /// For each input port: the unit and output port feeding it.
        inputs: Vec<Option<(Unit, usize)>>,
    },
}

/// Inner slots past this one are a composite's reduced outputs (a
/// composite needing full resolution, in a render at a scale below 1).
const REDUCED_SLOT: usize = usize::MAX / 2;

/// One render's planning: each unit shaped once.
struct Planning<'g> {
    graph: &'g Graph,
    shapes: HashMap<Unit, Shape>,
    /// Units with steps, producers first (composites have none).
    order: Vec<Unit>,
    /// Inner units by composite unit and slot.
    inner_ids: HashMap<(Unit, usize), u32>,
    inners: HashMap<u32, InnerUnit>,
}

impl Planning<'_> {
    /// The unit of slot `slot` of the composite `parent`.
    fn intern(&mut self, parent: Unit, slot: usize) -> Unit {
        let next = self.inner_ids.len() as u32 + 1;
        let inner = *self.inner_ids.entry((parent, slot)).or_insert(next);
        Unit {
            node: parent.node,
            inner,
            scale: parent.scale,
        }
    }

    /// The unit and port that really produce output `port` of `unit`
    /// (shaped): a composite's output is one of its inner nodes', a
    /// bypassed node's the input it passes through.
    fn resolve(&self, unit: Unit, port: usize) -> Result<(Unit, usize), Error> {
        let shape = &self.shapes[&unit];
        match &shape.kind {
            ShapeKind::Composite { exits, .. } => Ok(exits[port]),
            ShapeKind::Bypass { exits } => exits[port].ok_or_else(|| Error::Module {
                module: shape.name.clone(),
                reason: format!("bypassed: no connected input passes its output {port} through"),
            }),
            _ => Ok((unit, port)),
        }
    }
}

/// A unit as planning knows it before regions: its lineage and what it reads.
struct Shape {
    lineage: ContentHash,
    name: String,
    kind: ShapeKind,
}

enum ShapeKind {
    Source(Image, f64),
    /// A node standing for a network: no step of its own, its outputs are
    /// its inner nodes' (resolved) and its inputs feed them.
    Composite {
        exits: Vec<(Unit, usize)>,
        inputs: Vec<Option<(Unit, usize)>>,
    },
    Pending {
        inputs: Vec<(Unit, usize)>,
    },
    /// A bypassed node: no step of its own, each output the input it
    /// passes through (resolved), `None` for an output it cannot pass.
    Bypass {
        exits: Vec<Option<(Unit, usize)>>,
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
        /// Where its outputs depend on anything (see [`crate::Extent`]).
        extent: Option<Extent>,
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
    /// Keys the renders running still need, with how many renders pin each.
    pins: HashMap<ContentHash, usize>,
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

    /// One render less needs `key`.
    fn unpin(&mut self, key: ContentHash) {
        if let Some(n) = self.pins.get_mut(&key) {
            *n -= 1;
            if *n == 0 {
                self.pins.remove(&key);
            }
        }
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
    /// (never `keep`, never content still being computed, never content a
    /// render pinned) until the cache fits its budget.
    fn settle(&mut self, keep: Option<ContentHash>, budget: Option<usize>) {
        if let Some(keep) = keep
            && let Some(entry) = self.entries.get_mut(&keep)
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
                .filter(|(k, e)| {
                    Some(**k) != keep
                        && e.size > 0
                        && e.cell.get().is_some()
                        && !self.pins.contains_key(*k)
                })
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
    /// Runs GPU variants; unset: everything on the CPU. Set once, at
    /// construction or later ([`Engine::attach_accelerator`]).
    accelerator: OnceLock<Arc<dyn Accelerator>>,
    accelerated: AtomicU64,
    uploads: AtomicU64,
    readbacks: AtomicU64,
    keys: AtomicU64,
    crops: AtomicU64,
    fallbacks: AtomicU64,
    device_losses: AtomicU64,
    filled: AtomicU64,
    /// Why the accelerator is no longer used, once its device was lost for
    /// good: everything runs on the CPU from then on.
    gpu_off: Mutex<Option<String>>,
    /// The last reason a step fell back to the CPU.
    last_fallback: Mutex<Option<String>>,
}

/// A step's graph, node, name (an inner node's path) and region.
type TrackKey = (u64, NodeId, String, Region);

/// A planned render: the step asked for (it reads every other).
struct Planned {
    root: Arc<Step>,
    /// The root's output port asked for (a composite's output is one of
    /// its inner nodes').
    port: usize,
    /// Every step, producers first.
    steps: Vec<Arc<Step>>,
}

/// What a render pins in the cache, and when each pin goes: a step's
/// content is needed until every step reading it has run.
struct Pins {
    /// Steps still to run that read each pinned key.
    readers: Mutex<HashMap<ContentHash, usize>>,
    /// Steps whose inputs were released (a shared step runs once).
    released: Mutex<HashSet<ContentHash>>,
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
            accelerator: OnceLock::new(),
            accelerated: AtomicU64::new(0),
            uploads: AtomicU64::new(0),
            readbacks: AtomicU64::new(0),
            keys: AtomicU64::new(0),
            crops: AtomicU64::new(0),
            fallbacks: AtomicU64::new(0),
            device_losses: AtomicU64::new(0),
            filled: AtomicU64::new(0),
            gpu_off: Mutex::new(None),
            last_fallback: Mutex::new(None),
        }
    }

    /// Computes the steps whose module has a GPU variant on `accelerator`;
    /// the others stay on the CPU. Their content is keyed apart from CPU
    /// content, as the two agree within a tolerance only.
    pub fn with_accelerator(self, accelerator: Arc<dyn Accelerator>) -> Self {
        self.attach_accelerator(accelerator);
        self
    }

    /// [`Engine::with_accelerator`] on an engine already shared: the
    /// desktop's, made before its window opened the GPU device. Renders
    /// planned from then on accelerate; one under way finishes as planned.
    /// An engine takes one accelerator in its life: false, and nothing
    /// changes, when it has one already.
    pub fn attach_accelerator(&self, accelerator: Arc<dyn Accelerator>) -> bool {
        self.accelerator.set(accelerator).is_ok()
    }

    /// The accelerator, unless its device was lost for good.
    fn active(&self) -> Option<&Arc<dyn Accelerator>> {
        if self.gpu_off.lock().expect("engine gpu lock").is_some() {
            return None;
        }
        self.accelerator.get()
    }

    /// Why the accelerator is no longer used (its device was lost and could
    /// not be rebuilt), if it is not.
    pub fn gpu_off(&self) -> Option<String> {
        self.gpu_off.lock().expect("engine gpu lock").clone()
    }

    /// Why a step last fell back to the CPU, or why the accelerator is off:
    /// what a caller reports when [`Counters::fallbacks`] grew.
    pub fn fallback_reason(&self) -> Option<String> {
        self.gpu_off()
            .or_else(|| self.last_fallback.lock().expect("engine gpu lock").clone())
    }

    fn track<R>(&self, step: &Step, f: impl FnOnce(&mut Track) -> R) -> R {
        let mut tracks = self.tracks.lock().expect("engine tracks lock");
        f(tracks
            .entry((step.graph, step.node, step.name.clone(), step.region))
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
        let step = self.plan(graph, node, 0, region, 1.0)?.root;
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
            .any(|s| s.node == step.node && s.name == step.name && s.region == step.region)
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
            fallbacks: self.fallbacks.load(Ordering::SeqCst),
            device_losses: self.device_losses.load(Ordering::SeqCst),
            filled: self.filled.load(Ordering::SeqCst),
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
        self.render_content(graph, node, port, region, scale, |step, content, index| {
            Ok(self.host(step, content, index)?.sample(region))
        })
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
        if self.active().is_none() {
            return Err(Error::NoAccelerator);
        }
        self.render_content(graph, node, port, region, scale, |step, content, index| {
            // The device was lost for good during the render: the caller
            // reads the result on the host instead.
            let accelerator = self.active().ok_or(Error::NoAccelerator)?;
            self.device(accelerator.as_ref(), step, step, content, index, region)
        })
    }

    /// Plans and executes a render, then hands the requested step, its
    /// content and the index of the port asked for to `finish` (which reads
    /// it where the caller wants it). A lost or hung device on the way
    /// replays the whole render: on the rebuilt device, else on the CPU.
    fn render_content<T>(
        &self,
        graph: &Graph,
        node: NodeId,
        port: &str,
        region: Region,
        scale: f64,
        finish: impl Fn(&Arc<Step>, &Arc<Content>, usize) -> Result<T, Error>,
    ) -> Result<T, Error> {
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
        let mut rebuilt = false;
        // A render is replayed at most twice: on the rebuilt device, then
        // on the CPU.
        let mut replays = 0;
        loop {
            let planned = self.plan(graph, node, index, region, scale)?;
            let pins = self.pin(&planned);
            let evictions = self.cache.lock().expect("engine cache lock").evictions;
            let result = self
                .execute(&planned.root, cause, &pins)
                .and_then(|content| finish(&planned.root, &content, planned.port));
            // Whatever happened, the render's pins go, and the cache settles
            // within its budget.
            let settled = {
                let mut cache = self.cache.lock().expect("engine cache lock");
                for key in pins.readers.lock().expect("pins lock").keys() {
                    cache.unpin(*key);
                }
                cache.settle(None, self.budget);
                CacheSettled {
                    bytes: cache.bytes,
                    budget: self.budget,
                    entries: cache.entries.len(),
                    evicted: cache.evictions - evictions,
                }
            };
            self.bus.publish(SOURCE, Some(cause), &settled);
            match result {
                Err(Error::Accelerator { failure, reason }) if replays < 2 => {
                    replays += 1;
                    // Off already (another render lost it): replayed on
                    // the CPU.
                    if self.active().is_some() {
                        self.device_lost(failure, &reason, &mut rebuilt, cause);
                    }
                }
                result => return result,
            }
        }
    }

    /// The accelerator's device was lost or hung: its content goes, and the
    /// device is rebuilt once a render; failing that (or lost again on the
    /// rebuilt device), the accelerator is off for good.
    fn device_lost(&self, failure: Failure, reason: &str, rebuilt: &mut bool, cause: u64) {
        self.device_losses.fetch_add(1, Ordering::SeqCst);
        self.purge_device();
        let accelerator = self.active().expect("checked by the caller").clone();
        let recovered = if *rebuilt {
            Err("lost again on the rebuilt device".to_string())
        } else {
            accelerator.recover()
        };
        *rebuilt = true;
        let message = match &recovered {
            Ok(()) => Message::new(
                "engine-gpu-lost-recovered",
                "The GPU ({ $gpu }) failed ({ $failure }: { $reason }); it was restarted and the render replayed.",
            ),
            Err(why) => Message::new(
                "engine-gpu-lost",
                "The GPU ({ $gpu }) failed ({ $failure }: { $reason }) and could not be restarted ({ $why }); the rest of this session renders on the CPU.",
            )
            .with("why", why),
        }
        .with("gpu", accelerator.id())
        .with("failure", failure.message())
        .with("reason", reason);
        if recovered.is_err() {
            *self.gpu_off.lock().expect("engine gpu lock") = Some(message.english.clone());
        }
        self.bus.publish(
            SOURCE,
            Some(cause),
            &GpuLost {
                failure,
                reason: reason.into(),
                recovered: recovered.is_ok(),
                message,
            },
        );
    }

    /// Drops every content held in the accelerator's memory: after a loss,
    /// none of it can be read.
    fn purge_device(&self) {
        let mut cache = self.cache.lock().expect("engine cache lock");
        let device: Vec<ContentHash> = cache
            .entries
            .iter()
            .filter(|(_, e)| matches!(e.cell.get(), Some(Ok(c)) if matches!(**c, Content::Device { .. })))
            .map(|(k, _)| *k)
            .collect();
        for key in device {
            if let Some(entry) = cache.remove(key) {
                cache.bytes -= entry.size;
            }
        }
    }

    /// Evicts the least recently used device content no render pins, to
    /// free accelerator memory; `false` when there is none.
    fn evict_device(&self) -> bool {
        let mut cache = self.cache.lock().expect("engine cache lock");
        let victim = cache
            .entries
            .iter()
            .filter(|(k, e)| {
                matches!(e.cell.get(), Some(Ok(c)) if matches!(**c, Content::Device { .. }))
                    && !cache.pins.contains_key(*k)
            })
            .min_by_key(|(_, e)| e.last_used)
            .map(|(k, _)| *k);
        let Some(key) = victim else { return false };
        let entry = cache.remove(key).expect("victim exists");
        cache.bytes -= entry.size;
        cache.evictions += 1;
        true
    }

    /// Pins every step of a render, counting the steps that read each.
    fn pin(&self, planned: &Planned) -> Pins {
        let mut readers: HashMap<ContentHash, usize> = HashMap::new();
        for step in &planned.steps {
            readers.entry(step.key).or_default();
            for input in step.inputs() {
                *readers.entry(input.key).or_default() += 1;
            }
        }
        let mut cache = self.cache.lock().expect("engine cache lock");
        for key in readers.keys() {
            *cache.pins.entry(*key).or_default() += 1;
        }
        Pins {
            readers: Mutex::new(readers),
            released: Mutex::default(),
        }
    }

    /// `step` has run (or was found): what it reads is one reader short,
    /// and unpinned when no reader is left. Once per step.
    fn release(&self, step: &Step, pins: &Pins) {
        if !pins.released.lock().expect("pins lock").insert(step.key) {
            return;
        }
        let mut unpinned = Vec::new();
        {
            let mut readers = pins.readers.lock().expect("pins lock");
            for input in step.inputs() {
                if let Some(n) = readers.get_mut(&input.key) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        readers.remove(&input.key);
                        unpinned.push(input.key);
                    }
                }
            }
        }
        if !unpinned.is_empty() {
            let mut cache = self.cache.lock().expect("engine cache lock");
            for key in unpinned {
                cache.unpin(key);
            }
            cache.settle(None, self.budget);
        }
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
            .get()
            .expect("device content comes from the accelerator");
        self.readbacks.fetch_add(1, Ordering::SeqCst);
        let copy = accelerator
            .download(image)
            .map_err(|e| transfer_error(step, e))?;
        let copy = host.get_or_init(|| copy);
        self.cache
            .lock()
            .expect("engine cache lock")
            .settle(Some(step.key), self.budget);
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
        self.on_device(accelerator, step, content, port, region)?
            .map_err(|e| transfer_error(consumer, e))
    }

    /// [`Engine::device`], an upload's failure apart from the engine's
    /// errors (a read back that failed): the caller decides what it means.
    fn on_device(
        &self,
        accelerator: &dyn Accelerator,
        step: &Step,
        content: &Content,
        port: usize,
        region: Region,
    ) -> Result<Result<DeviceImage, AcceleratorError>, Error> {
        if let Content::Device { image, .. } = content
            && image.region() == region
        {
            return Ok(Ok(image.clone()));
        }
        let host = self.host(step, content, port)?.sample(region);
        self.uploads.fetch_add(1, Ordering::SeqCst);
        Ok(accelerator.upload(&host))
    }

    /// Runs an accelerated step: its input on the device, then its kernel.
    /// A module without inputs (a generator) reads a one-pixel placeholder
    /// instead (see [`crate::GpuKernel`]). An allocation over the
    /// accelerator's budget evicts device content the render does not
    /// need, and tries again while there is some.
    fn accelerate(
        &self,
        accelerator: &dyn Accelerator,
        step: &Step,
        module: &ModuleRef,
        plan: &crate::module::Plan,
        input: Option<(&Step, &Content, usize)>,
        input_region: Region,
    ) -> Result<Result<DeviceImage, AcceleratorError>, Error> {
        loop {
            let on_device = if let Some((s, content, port)) = input {
                self.on_device(accelerator, s, content, port, input_region)?
            } else {
                self.uploads.fetch_add(1, Ordering::SeqCst);
                accelerator.upload(&ModuleRef::placeholder(step.region))
            };
            let attempt = match on_device {
                Ok(on_device) => module.accelerate(accelerator, plan, &on_device, step.region),
                Err(e) => Err(e),
            };
            match attempt {
                Err(e) if e.failure == Failure::OutOfMemory && self.evict_device() => {}
                attempt => return Ok(attempt),
            }
        }
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
        Ok(self.plan(graph, node, 0, region, scale)?.root.key)
    }

    /// Plans a render of output `port` (by index) of `id` over `region` at
    /// `scale`: each node it reads once (a composite's inner nodes each
    /// once too), consumers before producers to gather the regions each
    /// node is read over, then the steps, producers first.
    fn plan(
        &self,
        graph: &Graph,
        id: NodeId,
        port: usize,
        region: Region,
        scale: f64,
    ) -> Result<Planned, Error> {
        let mut p = Planning {
            graph,
            shapes: HashMap::new(),
            order: Vec::new(),
            inner_ids: HashMap::new(),
            inners: HashMap::new(),
        };
        let top = Unit::of(id, scale.to_bits());
        self.shape(&mut p, top)?;
        let (root, port) = p.resolve(top, port)?;
        let Planning { shapes, order, .. } = p;

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
                    ShapeKind::Source(..)
                    | ShapeKind::Composite { .. }
                    | ShapeKind::Bypass { .. } => {}
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
                    // Filled: it reads nothing.
                    ShapeKind::Module {
                        extent: Some(e), ..
                    } if !e.region.overlaps(r) => {}
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
                    ShapeKind::Composite { .. } => unreachable!("a composite has no step"),
                    ShapeKind::Bypass { .. } => unreachable!("a bypassed node has no step"),
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
                        extent,
                    } => {
                        let input_region = module.input_region(plan, r);
                        let fill = extent.filter(|e| !e.region.overlaps(&r)).map(|e| e.fill);
                        Work::Module {
                            module: module.clone(),
                            plan: plan.clone(),
                            inputs: match fill {
                                Some(_) => Vec::new(),
                                None => inputs
                                    .iter()
                                    .map(|(u, p)| (reading(u, input_region), *p))
                                    .collect(),
                            },
                            input_region,
                            absent: absent.clone(),
                            accelerated: *accelerated,
                            fill,
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
                    node: unit.node,
                    name: shape.name.clone(),
                    region: r,
                    work,
                }));
            }
            steps.insert(*unit, planned);
        }
        let root = steps
            .get(&root)
            .and_then(|s| s.first().cloned())
            .expect("the unit asked for is planned");
        let steps = order
            .iter()
            .filter_map(|u| steps.remove(u))
            .flatten()
            .collect();
        Ok(Planned { root, port, steps })
    }

    /// What `unit` stands for: a node of the graph, or an inner node of a
    /// composite with its inputs wired to the composite's inputs and its
    /// siblings.
    fn describe(p: &mut Planning, unit: Unit) -> Result<Desc, Error> {
        if unit.inner == 0 {
            let node = p.graph.node(unit.node)?;
            return Ok(match &node.kind {
                Kind::Source { image, hash } => Desc::Source(image.clone(), *hash),
                Kind::Module {
                    module,
                    params,
                    context,
                    answered,
                } => Desc::Module {
                    name: node.name.clone(),
                    module: module.clone(),
                    params: params.clone(),
                    context: context.clone(),
                    answered: *answered,
                    inputs: node
                        .inputs
                        .iter()
                        .map(|c| c.map(|(src, port)| (Unit::of(src, unit.scale), port)))
                        .collect(),
                },
            });
        }
        let inner = &p.inners[&unit.inner];
        let (parent, network, index) = (inner.parent, inner.network.clone(), inner.index);
        let (name, context) = (inner.name.clone(), inner.context.clone());
        let ShapeKind::Composite {
            inputs: outer_inputs,
            ..
        } = &p.shapes[&parent].kind
        else {
            unreachable!("an inner unit's parent is a composite");
        };
        let outer_inputs = outer_inputs.clone();
        let node = &network.nodes()[index];
        let inputs = node
            .inputs
            .iter()
            .map(|wire| match wire {
                None => None,
                Some(Wire::Input(i)) => outer_inputs[*i],
                Some(Wire::Node(n, port)) => Some((p.intern(parent, n.0), *port)),
            })
            .collect();
        Ok(Desc::Module {
            name,
            module: node.module.clone(),
            params: node.params.clone(),
            context,
            answered: true,
            inputs,
        })
    }

    /// Shapes the unit feeding an input and gives the unit and port that
    /// really produce it.
    fn input(&self, p: &mut Planning, input: (Unit, usize)) -> Result<(Unit, usize), Error> {
        self.shape(p, input.0)?;
        p.resolve(input.0, input.1)
    }

    /// Plans `unit` and everything it reads, each once (`p.shapes`),
    /// adding those with steps to `p.order` producers first: their
    /// lineages and what they read. A composite is expanded into its inner
    /// nodes here, each a unit of its own.
    fn shape(&self, p: &mut Planning, unit: Unit) -> Result<(), Error> {
        if p.shapes.contains_key(&unit) {
            return Ok(());
        }
        if unit.inner == 0 && p.graph.node(unit.node)?.bypassed {
            return self.shape_bypass(p, unit);
        }
        let scale = f64::from_bits(unit.scale);
        let desc = Self::describe(p, unit)?;
        let mut h = blake3::Hasher::new();
        h.update(&scale.to_le_bytes());
        let (name, kind) = match desc {
            Desc::Source(image, hash) => {
                h.update(b"helios.step.source.v1");
                h.update(&hash.0);
                (
                    p.graph.name(unit.node)?.to_string(),
                    ShapeKind::Source(image, scale),
                )
            }
            Desc::Module {
                name,
                answered: false,
                inputs,
                ..
            } => {
                h.update(b"helios.step.pending.v1");
                let mut resolved = Vec::with_capacity(inputs.len());
                for input in inputs.into_iter().flatten() {
                    let input = self.input(p, input)?;
                    h.update(&p.shapes[&input.0].lineage.0);
                    resolved.push(input);
                }
                (name, ShapeKind::Pending { inputs: resolved })
            }
            Desc::Module { name, module, .. }
                if scale < 1.0 && module.manifest().needs.full_resolution =>
            {
                if unit.inner != 0 {
                    return Err(Error::Contract {
                        module: name,
                        reason: "an inner node needs full resolution: its composite must \
                                 declare it"
                            .into(),
                    });
                }
                // Computed at full resolution, then reduced.
                let full = Unit::of(unit.node, 1.0f64.to_bits());
                self.shape(p, full)?;
                if let ShapeKind::Composite { exits, .. } = &p.shapes[&full].kind {
                    let exits = exits.clone();
                    Self::reduce_composite(p, unit, name, exits);
                    return Ok(());
                }
                h.update(b"helios.step.reduce.v1");
                h.update(&p.shapes[&full].lineage.0);
                (name, ShapeKind::Reduce { input: full, scale })
            }
            Desc::Module {
                name,
                module,
                params,
                context,
                inputs,
                ..
            } if module.is_composite() => {
                return self.shape_composite(p, unit, name, &module, &params, &context, inputs);
            }
            Desc::Module {
                name,
                module,
                params,
                context,
                inputs,
                ..
            } => {
                let manifest = module.manifest();
                h.update(b"helios.step.module.v1");
                h.update(manifest.name.as_bytes());
                h.update(&manifest.version.to_le_bytes());
                h.update(&params.canonical_bytes());
                let plan = module.prepare(&params, &context, scale)?;
                h.update(&context.restricted_to(&manifest.context).canonical_bytes());
                let accelerated = match self.active() {
                    Some(a) if module.acceleratable() => {
                        h.update(b"helios.step.accelerated.v1");
                        h.update(a.id().as_bytes());
                        true
                    }
                    _ => false,
                };
                let mut resolved = Vec::with_capacity(inputs.len());
                let mut absent = Vec::new();
                for (i, connection) in inputs.into_iter().enumerate() {
                    let spec = &manifest.inputs[i];
                    let Some(input) = connection else {
                        if spec.optional {
                            h.update(b"helios.step.absent.v1");
                            h.update(&(i as u64).to_le_bytes());
                            absent.push((i, spec.channels().unwrap_or(1)));
                            continue;
                        }
                        return Err(Error::Unconnected {
                            node: name,
                            port: spec.name.clone(),
                        });
                    };
                    let (src, port) = self.input(p, input)?;
                    h.update(&p.shapes[&src].lineage.0);
                    h.update(&(port as u64).to_le_bytes());
                    resolved.push((src, port));
                }
                let extent = module.extent(&plan);
                (
                    name,
                    ShapeKind::Module {
                        module,
                        plan,
                        inputs: resolved,
                        absent,
                        accelerated,
                        extent,
                    },
                )
            }
        };
        p.shapes.insert(
            unit,
            Shape {
                lineage: ContentHash(*h.finalize().as_bytes()),
                name,
                kind,
            },
        );
        p.order.push(unit);
        Ok(())
    }

    /// A bypassed node: each output resolved to the input it passes
    /// through ([`crate::passed_input`]), shaped at the node's scale; the
    /// node itself has no step, and what reads it reads that input, under
    /// its content key. Inputs it does not pass are not read.
    fn shape_bypass(&self, p: &mut Planning, unit: Unit) -> Result<(), Error> {
        let node = p.graph.node(unit.node)?;
        let name = node.name.clone();
        let (manifest, inputs) = match &node.kind {
            Kind::Module { module, .. } => (module.manifest().clone(), node.inputs.clone()),
            Kind::Source { .. } => {
                return Err(Error::Module {
                    module: name,
                    reason: "a source cannot be bypassed".into(),
                });
            }
        };
        let mut h = blake3::Hasher::new();
        h.update(&f64::from_bits(unit.scale).to_le_bytes());
        h.update(b"helios.step.bypass.v1");
        let mut exits = Vec::with_capacity(manifest.outputs.len());
        for k in 0..manifest.outputs.len() {
            let wired = crate::graph::passed_input(&manifest, k)
                .and_then(|i| inputs[i])
                .map(|(src, port)| (Unit::of(src, unit.scale), port));
            let exit = match wired {
                Some(input) => {
                    let exit = self.input(p, input)?;
                    h.update(&p.shapes[&exit.0].lineage.0);
                    h.update(&(exit.1 as u64).to_le_bytes());
                    Some(exit)
                }
                None => None,
            };
            exits.push(exit);
        }
        p.shapes.insert(
            unit,
            Shape {
                lineage: ContentHash(*h.finalize().as_bytes()),
                name,
                kind: ShapeKind::Bypass { exits },
            },
        );
        Ok(())
    }

    /// A composite needing full resolution, in a render at a scale below
    /// 1: each output of its full-resolution expansion (`exits`), reduced
    /// on its own.
    fn reduce_composite(p: &mut Planning, unit: Unit, name: String, exits: Vec<(Unit, usize)>) {
        let scale = f64::from_bits(unit.scale);
        let mut h = blake3::Hasher::new();
        h.update(&scale.to_le_bytes());
        h.update(b"helios.step.composite.v1");
        let mut reduced = Vec::with_capacity(exits.len());
        for (k, (input, port)) in exits.into_iter().enumerate() {
            let r = p.intern(unit, REDUCED_SLOT + k);
            let mut hr = blake3::Hasher::new();
            hr.update(&scale.to_le_bytes());
            hr.update(b"helios.step.reduce.v1");
            hr.update(&p.shapes[&input].lineage.0);
            let lineage = ContentHash(*hr.finalize().as_bytes());
            h.update(&lineage.0);
            h.update(&(port as u64).to_le_bytes());
            p.shapes.insert(
                r,
                Shape {
                    lineage,
                    name: name.clone(),
                    kind: ShapeKind::Reduce { input, scale },
                },
            );
            p.order.push(r);
            reduced.push((r, port));
        }
        p.shapes.insert(
            unit,
            Shape {
                lineage: ContentHash(*h.finalize().as_bytes()),
                name,
                kind: ShapeKind::Composite {
                    exits: reduced,
                    inputs: Vec::new(),
                },
            },
        );
    }

    /// Expands a composite: its inputs shaped, its network asked for at
    /// the render's scale and its image's extent, then its outputs' inner
    /// nodes shaped (and through them, everything they read). Inner nodes
    /// no output reads are never planned.
    #[allow(clippy::too_many_arguments)]
    fn shape_composite(
        &self,
        p: &mut Planning,
        unit: Unit,
        name: String,
        module: &ModuleRef,
        params: &Params,
        context: &Context,
        inputs: Vec<Option<(Unit, usize)>>,
    ) -> Result<(), Error> {
        let manifest = module.manifest();
        let mut resolved = Vec::with_capacity(inputs.len());
        for (i, connection) in inputs.into_iter().enumerate() {
            match connection {
                Some(input) => resolved.push(Some(self.input(p, input)?)),
                None if manifest.inputs[i].optional => resolved.push(None),
                None => {
                    return Err(Error::Unconnected {
                        node: name,
                        port: manifest.inputs[i].name.clone(),
                    });
                }
            }
        }
        let scale = f64::from_bits(unit.scale);
        let at = Expansion::new(scale, p.graph.extent(unit.node)?);
        let network = Arc::new(
            module
                .expand(params, context, &at)?
                .expect("checked by the caller: a composite"),
        );
        // The context its inner nodes get: what it declares.
        let context = context.restricted_to(&manifest.context);
        p.shapes.insert(
            unit,
            Shape {
                lineage: ContentHash([0; 32]),
                name: name.clone(),
                kind: ShapeKind::Composite {
                    exits: Vec::new(),
                    inputs: resolved.clone(),
                },
            },
        );
        for (index, node) in network.nodes().iter().enumerate() {
            let inner = p.intern(unit, index);
            p.inners.insert(
                inner.inner,
                InnerUnit {
                    parent: unit,
                    network: network.clone(),
                    index,
                    name: format!("{name}/{}", node.name),
                    context: context.clone(),
                },
            );
        }
        let mut h = blake3::Hasher::new();
        h.update(&scale.to_le_bytes());
        h.update(b"helios.step.composite.v1");
        let mut exits = Vec::with_capacity(network.exits().len());
        for (k, exit) in network.exits().iter().enumerate() {
            let exit = match exit.expect("checked by expand") {
                Wire::Input(i) => resolved[i].ok_or_else(|| Error::Contract {
                    module: manifest.name.clone(),
                    reason: format!(
                        "output `{}` passes an unconnected input through",
                        manifest.outputs[k].name
                    ),
                })?,
                Wire::Node(n, port) => {
                    let inner = p.intern(unit, n.0);
                    self.input(p, (inner, port))?
                }
            };
            h.update(&p.shapes[&exit.0].lineage.0);
            h.update(&(exit.1 as u64).to_le_bytes());
            exits.push(exit);
        }
        let shape = p.shapes.get_mut(&unit).expect("inserted above");
        shape.lineage = ContentHash(*h.finalize().as_bytes());
        shape.kind = ShapeKind::Composite {
            exits,
            inputs: resolved,
        };
        Ok(())
    }

    fn execute(&self, step: &Step, cause: u64, pins: &Pins) -> Outputs {
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
                self.release(step, pins);
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
                let result = self.compute(step, cause, pins);
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
                .settle(Some(step.key), self.budget);
        }
        self.release(step, pins);
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

    /// A step's GPU variant failed: it is computed on the CPU, and the bus
    /// says why.
    fn fell_back(&self, step: &Step, module: &ModuleRef, e: AcceleratorError, cause: u64) {
        self.fallbacks.fetch_add(1, Ordering::SeqCst);
        let module = module.manifest().name.clone();
        *self.last_fallback.lock().expect("engine gpu lock") =
            Some(format!("{module}: {} ({})", e.reason, e.failure.as_str()));
        self.bus.publish(
            SOURCE,
            Some(cause),
            &GpuFellBack::new(step.node, &step.name, &module, e.failure, &e.reason),
        );
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

    fn compute(&self, step: &Step, cause: u64, pins: &Pins) -> Outputs {
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
                fill,
            } => {
                if let Some(fill) = *fill {
                    self.filled.fetch_add(1, Ordering::SeqCst);
                    let area = step.region.area();
                    return Ok(Arc::new(Content::Host(
                        module
                            .manifest()
                            .outputs
                            .iter()
                            .map(|port| {
                                let channels = port.channels().unwrap_or(1);
                                Image::new(
                                    step.region,
                                    channels,
                                    vec![fill; area * channels as usize],
                                )
                            })
                            .collect(),
                    )));
                }
                let mut held = Vec::with_capacity(inputs.len());
                for (input, port) in inputs {
                    held.push((self.execute(input, cause, pins)?, *port));
                }
                if self.take_fault(step, FaultKind::Compute) {
                    return Err(Error::Fault {
                        node: step.name.clone(),
                        what: "computation".into(),
                    });
                }
                if let (Some(a), true) = (self.active(), accelerated) {
                    let input = held
                        .first()
                        .map(|(content, port)| (inputs[0].0.as_ref(), content.as_ref(), *port));
                    match self.accelerate(a.as_ref(), step, module, plan, input, *input_region)? {
                        Ok(image) => {
                            self.accelerated.fetch_add(1, Ordering::SeqCst);
                            return Ok(Arc::new(Content::Device {
                                image,
                                host: OnceLock::new(),
                            }));
                        }
                        // Nothing on the device can be read: the render is
                        // replayed (see `render_content`).
                        Err(e) if e.failure.is_device() => {
                            return Err(Error::Accelerator {
                                failure: e.failure,
                                reason: e.reason,
                            });
                        }
                        Err(e) => self.fell_back(step, module, e, cause),
                    }
                }
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
            Work::Reduce { input, scale } => {
                let content = self.execute(input, cause, pins)?;
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

/// A copy to or from the accelerator failed, for `step`: a lost or hung
/// device is the render's to recover from, anything else is its module's
/// error.
fn transfer_error(step: &Step, e: AcceleratorError) -> Error {
    if e.failure.is_device() {
        return Error::Accelerator {
            failure: e.failure,
            reason: e.reason,
        };
    }
    let module = match &step.work {
        Work::Module { module, .. } => module.manifest().name.clone(),
        _ => step.name.clone(),
    };
    Error::Module {
        module,
        reason: format!("GPU: {}", e.reason),
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
                "pinned": cache.pins.len(),
            },
            "counters": counters,
            "gpu": {
                "accelerator": self.accelerator.get().map(|a| a.id().to_string()),
                "memory": self.accelerator.get().and_then(|a| a.memory()),
                "off": self.gpu_off(),
                "last_fallback": *self.last_fallback.lock().expect("engine gpu lock"),
            },
        })
    }
}
