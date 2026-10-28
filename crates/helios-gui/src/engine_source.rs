//! The viewport's tile source over Helios's engine.
//!
//! egui-pop's viewport asks for tiles ([`TileSource::request`]) and must
//! never wait: [`EngineSource`] hands them to a render thread of its own
//! and returns at once. The thread
//!
//! - takes every job waiting, and answers the readout first (the pointer
//!   waits on it), with working-space values;
//! - skips a tile the viewport gave up on (its cancel token is set);
//! - renders the rest by quality (proxies first) and scale, one union of
//!   neighbouring tiles at a time (`helios_core::engine::unions`: the
//!   engine plans the union once, each tile is a view into it), at the
//!   tile's scale: proxies at powers of two, exact tiles at `min(zoom, 1)`;
//! - delivers linear RGBA floats in the working space, the display
//!   transform left to the viewport;
//! - on a source given the GPU the window draws with ([`GpuSlot`]), once
//!   the viewport's GPU path is installed on that very device, renders
//!   each union into the GPU's memory (`Engine::render_device_at`) and
//!   hands each tile as a view into that buffer (a device tile): nothing
//!   is read back, nothing uploaded again, the display transform runs in
//!   the viewport's shader. Each tile keeps its union's `DeviceImage`
//!   while the viewport holds it, so the GPU's memory ledger counts what
//!   is shown. A tile the viewport refuses on this thread is rendered on
//!   the host instead (the device tile's fallback, run here); one refused
//!   later, when the interface takes it in, is asked for again. An adapter
//!   whose fragment shaders cannot read storage buffers (WebGL 2) takes
//!   the host path from the start;
//! - with a mask shown ([`Shown::mask`]), renders the mask's node over the
//!   same union at the same scale, through the same engine and its cache,
//!   and composites it into each tile: on the device when the union is
//!   there (`crate::mask_display::composite_on_device`, a WGSL kernel, so
//!   the overlay stays zero-copy), else on the host
//!   (`crate::mask_display::composite`, the reference).
//!
//! [`EngineSource::show`] changes what is shown (an edit, another image):
//! the viewport hears that the whole image changed, keeps its tiles on
//! screen and asks for them again. A tile rendered from the old graph
//! answers a request the viewport no longer holds, and is dropped.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use eframe::egui_wgpu::wgpu;
use egui_pop::viewport::device::{BufferLayout, DeviceTile};
use egui_pop::viewport::source::{Pixels, Tile, TileRequest, TileSink, TileSource};
use helios_core::{DeviceImage, Engine, Graph, Image, NodeId, Region};

/// What the render threads did, for tests and the status bar.
#[derive(Default, Debug)]
pub struct Stats {
    /// Engine renders: one per union of neighbouring tiles.
    pub renders: AtomicU64,
    /// Tiles delivered.
    pub tiles: AtomicU64,
    /// Of those, tiles handed on the GPU (device tiles), not read back.
    pub device_tiles: AtomicU64,
    /// Of those, tiles with the mask display composited on the GPU.
    pub device_masks: AtomicU64,
    /// Tiles not rendered: the viewport no longer wanted them.
    pub skipped: AtomicU64,
    /// Tiles the engine could not render.
    pub failed: AtomicU64,
    /// Readouts answered.
    pub readouts: AtomicU64,
    /// Jobs sent to the render threads, and jobs they are done with.
    jobs: Mutex<(u64, u64)>,
    /// The part of the image each graph's render last asked for and its
    /// scale, by graph (the newest few), for the graph view's headers.
    rendered: Mutex<Vec<(u64, Region, f64)>>,
    idle: Condvar,
}

impl Stats {
    fn sent(&self) {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner).0 += 1;
    }

    fn done(&self, n: u64) {
        if n == 0 {
            return;
        }
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner).1 += n;
        self.idle.notify_all();
    }

    /// Waits until the render threads are done with every job sent so
    /// far, their answers waiting for the next frame; false when
    /// `timeout` passes first.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        while jobs.1 < jobs.0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            jobs = self
                .idle
                .wait_timeout(jobs, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }

    /// How many graphs [`Stats::rendered`] remembers.
    const RENDERED: usize = 4;

    fn render_of(&self, graph: u64, union: Region, scale: f64) {
        let mut r = self.rendered.lock().unwrap_or_else(PoisonError::into_inner);
        r.retain(|(g, _, _)| *g != graph);
        r.push((graph, union, scale));
        if r.len() > Self::RENDERED {
            r.remove(0);
        }
    }

    /// The part of the image (in the scaled image's pixels) and the scale
    /// a graph's render last asked for, while it is among the newest.
    pub fn rendered(&self, graph: u64) -> Option<(Region, f64)> {
        self.rendered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(g, _, _)| *g == graph)
            .map(|(_, r, s)| (*r, *s))
    }

    /// A counter's value.
    pub fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }

    fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }
}

/// What a source shows: an output port of a node of a graph.
#[derive(Clone)]
pub struct Shown {
    pub graph: Arc<Graph>,
    pub node: NodeId,
    pub port: String,
    /// The node's extent: the image's size.
    pub extent: Region,
    /// A mask node of the same graph shown over it (the mask display).
    pub mask: Option<crate::mask_display::Layer>,
}

impl Shown {
    /// A node's output, `None` when nothing feeds the node.
    pub fn new(graph: Arc<Graph>, node: NodeId, port: &str) -> Result<Option<Shown>, String> {
        let extent = graph.extent(node).map_err(|e| e.to_string())?;
        Ok(extent.map(|extent| Shown {
            graph,
            node,
            port: port.into(),
            extent,
            mask: None,
        }))
    }

    /// The same output with a mask shown over it, or none.
    #[must_use]
    pub fn with_mask(mut self, mask: Option<crate::mask_display::Layer>) -> Shown {
        self.mask = mask;
        self
    }
}

/// The GPU a source's engine computes on, shared by the sources of a
/// viewport and set once the window's GPU path is attached: the process's
/// one device (`helios_gpu::shared`), which the window draws with.
pub type GpuSlot = Arc<OnceLock<Arc<helios_gpu::Gpu>>>;

enum Job {
    Tiles(Vec<TileRequest>),
    Readout([i64; 2]),
}

/// A [`TileSource`] over Helios's engine: a node's output, rendered on a
/// thread of its own (which ends with the source).
pub struct EngineSource {
    shown: Arc<Mutex<Option<Shown>>>,
    jobs: Sender<Job>,
    sink: TileSink,
    stats: Arc<Stats>,
}

impl EngineSource {
    /// A source delivering to `sink`, rendering with `engine` on a thread
    /// named `name`, counting in `stats`.
    pub fn new(name: &str, engine: Arc<Engine>, sink: TileSink, stats: Arc<Stats>) -> Self {
        Self::on_gpu(name, engine, sink, stats, GpuSlot::default())
    }

    /// [`EngineSource::new`], handing tiles on the device once `gpu` is
    /// set (to the GPU `engine` accelerates on) and the viewport's GPU
    /// path is installed on that GPU's device.
    pub fn on_gpu(
        name: &str,
        engine: Arc<Engine>,
        sink: TileSink,
        stats: Arc<Stats>,
        gpu: GpuSlot,
    ) -> Self {
        let shown: Arc<Mutex<Option<Shown>>> = Arc::default();
        let (jobs, rx) = channel();
        let (s, sk, st) = (shown.clone(), sink.clone(), stats.clone());
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || work(&engine, &gpu, &rx, &s, &sk, &st))
            .expect("a thread for the viewport");
        EngineSource {
            shown,
            jobs,
            sink,
            stats,
        }
    }

    /// Shows a node's output (`None`: nothing); the viewport hears that
    /// the whole image changed.
    pub fn show(&self, shown: Option<Shown>) {
        *self.shown.lock().unwrap_or_else(PoisonError::into_inner) = shown;
        self.sink.changed(None);
    }

    /// What is shown.
    pub fn shown(&self) -> Option<Shown> {
        self.shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn send(&self, job: Job) {
        // Counted before it is sent, so the answer never comes first.
        self.stats.sent();
        if self.jobs.send(job).is_err() {
            // The render thread is gone: nothing will answer it.
            self.stats.done(1);
        }
    }
}

impl TileSource for EngineSource {
    fn size(&self) -> Option<[u32; 2]> {
        self.shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|s| [s.extent.width, s.extent.height])
    }

    fn request(&self, requests: Vec<TileRequest>) {
        self.send(Job::Tiles(requests));
    }

    fn readout(&self, pixel: [i64; 2]) {
        self.send(Job::Readout(pixel));
    }
}

/// The scaled pixels a tile covers.
fn region_of(r: &TileRequest) -> Region {
    Region::new(r.origin[0], r.origin[1], r.size[0], r.size[1])
}

/// An image's pixels as RGBA: a grey spread over the colour channels, an
/// alpha of 1 (the viewport shows an image opaque, as before).
pub fn rgba(image: &Image) -> Vec<f32> {
    let r = image.region();
    let n = image.channels() as usize;
    let mut out = Vec::with_capacity(r.area() * 4);
    for y in r.y..r.bottom() {
        for p in image.row(y).chunks_exact(n) {
            out.extend([p[0], p[1.min(n - 1)], p[2.min(n - 1)], 1.0]);
        }
    }
    out
}

fn work(
    engine: &Arc<Engine>,
    gpu: &GpuSlot,
    jobs: &Receiver<Job>,
    shown: &Mutex<Option<Shown>>,
    sink: &TileSink,
    stats: &Stats,
) {
    while let Ok(first) = jobs.recv() {
        let mut batch = vec![first];
        batch.extend(jobs.try_iter());
        let n = batch.len() as u64;
        let current = shown.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(s) = current {
            let mut tiles = Vec::new();
            let mut readout = None;
            for job in batch {
                match job {
                    Job::Tiles(t) => tiles.extend(t),
                    // Only where the pointer is now matters.
                    Job::Readout(p) => readout = Some(p),
                }
            }
            if let Some(p) = readout {
                read_out(engine, &s, p, sink, stats);
            }
            let device = on_device(gpu, sink);
            render(engine, device.as_ref(), &s, tiles, sink, stats);
        }
        stats.done(n);
    }
}

/// The working-space values of one pixel, at full resolution.
fn read_out(engine: &Engine, s: &Shown, p: [i64; 2], sink: &TileSink, stats: &Stats) {
    let one = Region::new(p[0], p[1], 1, 1);
    let Ok(image) = engine.render(&s.graph, s.node, &s.port, one) else {
        return;
    };
    let px = image.pixel(p[0], p[1]);
    let n = px.len();
    let mut values: Vec<(String, f32)> = ["R", "G", "B"]
        .iter()
        .enumerate()
        .map(|(i, name)| ((*name).to_string(), px[i.min(n - 1)]))
        .collect();
    // The mask shown, under the pointer too.
    if let Some(m) = &s.mask
        && let Ok(mask) = engine.render(&s.graph, m.target.node, &m.target.port, one)
    {
        values.push(("Mask".into(), mask.pixel(p[0], p[1])[0]));
    }
    sink.readout(p, values);
    Stats::add(&stats.readouts, 1);
}

/// The GPU to hand tiles on, and its device: set, and the viewport's GPU
/// path installed on that very device and able to read storage buffers in
/// its fragment shader (else every buffer tile would be refused with
/// `BuffersUnsupported`).
fn on_device(gpu: &GpuSlot, sink: &TileSink) -> Option<(Arc<helios_gpu::Gpu>, wgpu::Device)> {
    let gpu = gpu.get()?;
    let render = sink.render_device().filter(|r| r.buffers)?;
    // A rebuilt device (after a loss) is no longer the window's.
    let device = gpu.handles().device;
    (render.device == device).then(|| (gpu.clone(), device))
}

/// One tile rendered on the host, the mask shown composited over it: a
/// device tile's fallback.
fn host_pixels(engine: &Engine, s: &Shown, region: Region, scale: f64) -> Option<Vec<f32>> {
    let image = engine
        .render_at(&s.graph, s.node, &s.port, region, scale)
        .ok()?;
    let mut pixels = rgba(&image);
    if let Some(m) = &s.mask
        && let Ok(mask) = engine.render_at(&s.graph, m.target.node, &m.target.port, region, scale)
    {
        crate::mask_display::composite(&mut pixels, &mask, m.mode, &m.style);
    }
    Some(pixels)
}

/// The mask shown laid over `image`, on the device: its node rendered
/// there over the same union and scale (usually from the cache the
/// image's render filled), then composited by the WGSL kernel. `None`
/// when either step could not be had on the device.
fn mask_on_device(
    engine: &Engine,
    gpu: &helios_gpu::Gpu,
    s: &Shown,
    m: &crate::mask_display::Layer,
    image: &DeviceImage,
    (union, scale): (Region, f64),
) -> Option<DeviceImage> {
    let mask = engine
        .render_device_at(&s.graph, m.target.node, &m.target.port, union, scale)
        .ok()?;
    crate::mask_display::composite_on_device(gpu, image, &mask, m.mode, &m.style)
        .inspect_err(|e| {
            eprintln!(
                "helios-gui: mask display of {} on the GPU: {e}",
                m.target.name
            );
        })
        .ok()
}

/// Renders `live`, the tiles of `union` at `scale`, into the GPU's memory
/// (the mask shown composited there too) and hands each as a device tile,
/// a view into the union's buffer. False when the union could not be had
/// on the device (the engine lost its GPU, a result of more than four
/// channels, a mask that could not be laid there): the host path renders
/// it then.
fn deliver_on_device(
    engine: &Arc<Engine>,
    (gpu, device): &(Arc<helios_gpu::Gpu>, wgpu::Device),
    s: &Shown,
    (union, scale): (Region, f64),
    live: &[&TileRequest],
    sink: &TileSink,
    stats: &Stats,
) -> bool {
    let Ok(image) = engine.render_device_at(&s.graph, s.node, &s.port, union, scale) else {
        return false;
    };
    let image = match &s.mask {
        None => image,
        Some(m) => match mask_on_device(engine, gpu, s, m, &image, (union, scale)) {
            Some(laid) => laid,
            None => return false,
        },
    };
    let channels = image.channels();
    let Some(buffer) = gpu.buffer(&image).filter(|_| (1..=4).contains(&channels)) else {
        return false;
    };
    let at = image.region();
    let layout = BufferLayout::packed(at.width, channels);
    let masked = s.mask.is_some();
    Stats::add(&stats.renders, 1);
    for r in live {
        if r.is_cancelled() {
            Stats::add(&stats.skipped, 1);
            continue;
        }
        let region = region_of(r);
        let origin = [(region.x - at.x) as u32, (region.y - at.y) as u32];
        // Run only when `deliver_device` refuses the tile, on this render
        // thread (never the interface's): a host render of the one tile,
        // with no lock held.
        let (engine, s) = (engine.clone(), s.clone());
        let tile = DeviceTile::buffer(device, buffer.clone(), layout)
            .origin(origin)
            // The image and its ledger charge, while the tile is shown.
            .keep(image.clone())
            .fallback(move || host_pixels(&engine, &s, region, scale).map(Pixels::Scene));
        sink.deliver_device(r, tile);
        Stats::add(&stats.tiles, 1);
        Stats::add(&stats.device_tiles, 1);
        if masked {
            Stats::add(&stats.device_masks, 1);
        }
    }
    true
}

/// Renders tiles by quality (proxies first) and scale, one union of
/// neighbouring tiles at a time, each tile a view into its union: on the
/// GPU, handed on the device, when `device` says so.
fn render(
    engine: &Arc<Engine>,
    device: Option<&(Arc<helios_gpu::Gpu>, wgpu::Device)>,
    s: &Shown,
    tiles: Vec<TileRequest>,
    sink: &TileSink,
    stats: &Stats,
) {
    let mut groups: BTreeMap<_, Vec<TileRequest>> = BTreeMap::new();
    for r in tiles {
        if r.is_cancelled() {
            Stats::add(&stats.skipped, 1);
            continue;
        }
        groups.entry((r.quality, r.key.scale)).or_default().push(r);
    }
    let mut answered = HashSet::new();
    for ((_, scale), group) in groups {
        let regions = group.iter().map(region_of).collect();
        for union in helios_core::engine::unions(regions) {
            let inside: Vec<&TileRequest> = group
                .iter()
                .filter(|r| !answered.contains(&r.id) && union.contains(&region_of(r)))
                .collect();
            answered.extend(inside.iter().map(|r| r.id));
            let live: Vec<&TileRequest> = inside
                .iter()
                .copied()
                .filter(|r| !r.is_cancelled())
                .collect();
            Stats::add(&stats.skipped, (inside.len() - live.len()) as u64);
            if live.is_empty() {
                continue;
            }
            stats.render_of(s.graph.id(), union, scale.get());
            if let Some(d) = device
                && deliver_on_device(engine, d, s, (union, scale.get()), &live, sink, stats)
            {
                continue;
            }
            match engine.render_at(&s.graph, s.node, &s.port, union, scale.get()) {
                Ok(image) => {
                    Stats::add(&stats.renders, 1);
                    // The mask shown: its node over the same union and
                    // scale, usually from the cache the image's render
                    // filled. Without it, the image alone.
                    let mask = s.mask.as_ref().and_then(|m| {
                        engine
                            .render_at(&s.graph, m.target.node, &m.target.port, union, scale.get())
                            .inspect_err(|e| {
                                eprintln!("helios-gui: mask display of {}: {e}", m.target.name);
                            })
                            .ok()
                            .map(|image| (m, image))
                    });
                    for r in live {
                        if r.is_cancelled() {
                            Stats::add(&stats.skipped, 1);
                            continue;
                        }
                        let tile = image.sample(region_of(r));
                        let mut pixels = rgba(&tile);
                        if let Some((m, mask)) = &mask {
                            crate::mask_display::composite(
                                &mut pixels,
                                &mask.sample(region_of(r)),
                                m.mode,
                                &m.style,
                            );
                        }
                        sink.deliver(Tile::answer(r, Pixels::Scene(pixels)));
                        Stats::add(&stats.tiles, 1);
                    }
                }
                Err(e) => {
                    for r in live {
                        sink.failed(r.id, e.to_string());
                        Stats::add(&stats.failed, 1);
                    }
                }
            }
        }
    }
}
