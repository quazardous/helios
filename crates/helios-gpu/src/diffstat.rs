//! Per-module diffstat: how far a pipeline's GPU path is from its CPU
//! path, module by module.
//!
//! The idea comes from darktable's `--opencl-diffstat`
//! (darktable pull requests #23072 and #23141,
//! <https://github.com/darktable-org/darktable/pull/23141>): run each
//! module of a pipe on both paths and report its distance, which finds a
//! kernel's parity bug at the module that has it instead of at the end of
//! the pipe.
//!
//! [`run`] takes a graph and two engines: the reference (the CPU, an
//! engine without accelerator) and the one under test (an engine given a
//! [`crate::Gpu`]). For each module node it measures twice:
//! - **alone**: the node rebuilt in a graph of its own, its inputs the
//!   reference's renders of what feeds it, so both engines run it on the
//!   same pixels and the distance is the module's own;
//! - **chain**: the node rendered through the whole graph on each engine,
//!   so the distance includes what the modules before it drifted.
//!
//! Each measure ([`Stats`]) holds the largest absolute difference, the
//! mean one, the share of pixels over a threshold and the worst tile.
//! [`diff_image`] gives the per-pixel distance as an image, to look at.
//! Two reference engines report zero everywhere: the sanity mode.
//!
//! A distance of zero proves nothing when the kernel never ran: a GPU
//! kernel that fails (a shader the driver refuses, say) falls back to the
//! CPU, and the CPU matches itself. So each module also reports how many of
//! its steps fell back ([`ModuleDiff::fallbacks`], from the engine's
//! [`helios_core::Counters::fallbacks`]), and [`problems`] counts a module
//! with a GPU kernel that fell back as a failure. A module without a kernel
//! runs on the CPU on both sides by design: "CPU only", never a failure.

use helios_core::{Engine, Graph, Image, NodeId, Region, graph::SOURCE_PORT};

/// What a measure counts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// A pixel is "over" when its largest channel difference passes this.
    pub threshold: f32,
    /// The side of the square tiles the worst tile is looked for in.
    pub tile: u32,
}

impl Default for Options {
    /// 1/255 (the CPU-oracle bound of 8-bit output) and 64-pixel tiles.
    fn default() -> Self {
        Self {
            threshold: 1.0 / 255.0,
            tile: 64,
        }
    }
}

/// The distance between two renders of the same region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    /// The largest absolute difference over every channel of every pixel
    /// (infinite when one side is not finite where the other is).
    pub max_abs: f32,
    /// The mean absolute difference over every channel of every pixel.
    pub mean_abs: f64,
    /// The share of pixels (0 to 1) whose largest channel difference
    /// passes [`Options::threshold`].
    pub over: f64,
    /// The tile with the largest mean per-pixel difference.
    pub worst_tile: Region,
    /// That tile's mean per-pixel difference.
    pub worst_tile_mean: f64,
}

impl Stats {
    /// True when both renders are identical.
    pub fn is_zero(&self) -> bool {
        self.max_abs == 0.0
    }
}

/// The distance of one channel value from its reference: 0 when equal
/// (infinities and NaN included), infinite when only one is finite.
fn distance(a: f32, b: f32) -> f32 {
    if a == b || (a.is_nan() && b.is_nan()) {
        return 0.0;
    }
    let d = (a - b).abs();
    if d.is_finite() { d } else { f32::INFINITY }
}

/// The per-pixel distance of `other` from `reference`: one channel, the
/// largest channel difference of each pixel.
pub fn diff_image(reference: &Image, other: &Image) -> Result<Image, String> {
    same_shape(reference, other)?;
    let ch = reference.channels() as usize;
    let data = reference
        .data()
        .chunks_exact(ch)
        .zip(other.data().chunks_exact(ch))
        .map(|(a, b)| {
            a.iter()
                .zip(b)
                .map(|(a, b)| distance(*a, *b))
                .fold(0.0, f32::max)
        })
        .collect();
    Ok(Image::new(reference.region(), 1, data))
}

fn same_shape(reference: &Image, other: &Image) -> Result<(), String> {
    if reference.region() != other.region() || reference.channels() != other.channels() {
        return Err(format!(
            "different shapes: {:?} × {} against {:?} × {}",
            reference.region(),
            reference.channels(),
            other.region(),
            other.channels()
        ));
    }
    Ok(())
}

/// Measures how far `other` is from `reference` (same region, same
/// channels).
pub fn measure(reference: &Image, other: &Image, options: Options) -> Result<Stats, String> {
    let per_pixel = diff_image(reference, other)?;
    let region = per_pixel.region();
    let values = reference.data().len().max(1) as f64;
    let sum: f64 = reference
        .data()
        .iter()
        .zip(other.data())
        .map(|(a, b)| f64::from(distance(*a, *b)))
        .sum();
    let pixels = per_pixel.data();
    let max_abs = pixels.iter().copied().fold(0.0, f32::max);
    let over = pixels.iter().filter(|d| **d > options.threshold).count() as f64
        / pixels.len().max(1) as f64;
    let mut worst = (region, -1.0);
    for tile in region.tiles(options.tile.max(1)) {
        let mut s = 0.0;
        for y in tile.y..tile.bottom() {
            let row = per_pixel.row(y);
            let from = (tile.x - region.x) as usize;
            s += row[from..from + tile.width as usize]
                .iter()
                .map(|d| f64::from(*d))
                .sum::<f64>();
        }
        let mean = s / tile.area().max(1) as f64;
        if mean > worst.1 {
            worst = (tile, mean);
        }
    }
    Ok(Stats {
        max_abs,
        mean_abs: sum / values,
        over,
        worst_tile: worst.0,
        worst_tile_mean: worst.1.max(0.0),
    })
}

/// One module node's distances.
#[derive(Clone, Debug)]
pub struct ModuleDiff {
    pub node: NodeId,
    /// The node's name in the graph.
    pub name: String,
    /// The module's manifest name.
    pub module: String,
    /// Whether the module has a GPU variant: without one, both paths run
    /// its CPU code and only its inputs can differ.
    pub has_kernel: bool,
    /// Steps of the module, run alone, that the engine under test computed
    /// on the CPU because the GPU kernel failed (0 when it has no kernel:
    /// it never tries one).
    pub fallbacks: u64,
    /// Steps of the module, run alone, the engine under test computed on
    /// the GPU.
    pub accelerated: u64,
    /// Why the last of those steps fell back, as the engine said it.
    pub fallback_reason: Option<String>,
    /// The module run alone on the same inputs, or why it could not be.
    pub alone: Result<Stats, String>,
    /// The node rendered through the whole graph, or why it could not be.
    pub chain: Result<Stats, String>,
}

/// Measures every module node of `graph` (in the order they were added,
/// bypassed ones left out; `only` keeps the nodes or modules of that
/// name) over `region`, `other` against `reference`. A node that fails to
/// render reports why; the others still run.
///
/// The fallbacks are counted on `other` while the module runs alone, and
/// that run comes first: the module is computed there for the first time,
/// never served from the cache, and nothing else is (its inputs are
/// sources).
pub fn run(
    graph: &Graph,
    region: Region,
    reference: &Engine,
    other: &Engine,
    options: Options,
    only: Option<&str>,
) -> Result<Vec<ModuleDiff>, String> {
    let mut out = Vec::new();
    for id in graph.ids() {
        let Some(module) = graph.module(id).map_err(|e| e.to_string())? else {
            continue;
        };
        if graph.is_bypassed(id).map_err(|e| e.to_string())? {
            continue;
        }
        let name = graph.name(id).map_err(|e| e.to_string())?.to_string();
        let manifest = module.manifest();
        if only.is_some_and(|o| o != name && o != manifest.name) {
            continue;
        }
        let Some(port) = manifest.outputs.first().map(|p| p.name.clone()) else {
            continue;
        };
        let before = other.counters();
        let alone = alone(graph, id, region)
            .and_then(|(g, n)| both(reference, other, &g, n, &port, region))
            .and_then(|(r, o)| measure(&r, &o, options));
        let ran = other.counters().since(&before);
        let chain = both(reference, other, graph, id, &port, region)
            .and_then(|(r, o)| measure(&r, &o, options));
        out.push(ModuleDiff {
            node: id,
            name,
            module: manifest.name.clone(),
            has_kernel: module.gpu_kernel().is_some(),
            fallbacks: ran.fallbacks,
            accelerated: ran.accelerated,
            fallback_reason: (ran.fallbacks > 0)
                .then(|| other.fallback_reason())
                .flatten(),
            alone,
            chain,
        });
    }
    Ok(out)
}

/// The largest distances a module may show on one adapter (see
/// `crates/helios-gpu/diffstat-budgets.toml`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    pub max_abs: f32,
    pub mean_abs: f64,
    pub over: f64,
}

impl Budget {
    /// What of `stats` passes this budget, `None` when nothing does.
    pub fn exceeded(&self, stats: &Stats) -> Option<String> {
        let mut out = Vec::new();
        // A NaN distance never fits (`<=` is false), an infinite one
        // neither.
        let fits = |v: f64, budget: f64| v <= budget;
        if !fits(f64::from(stats.max_abs), f64::from(self.max_abs)) {
            out.push(format!("max {:.3e} > {:.3e}", stats.max_abs, self.max_abs));
        }
        if !fits(stats.mean_abs, self.mean_abs) {
            out.push(format!(
                "mean {:.3e} > {:.3e}",
                stats.mean_abs, self.mean_abs
            ));
        }
        if !fits(stats.over, self.over) {
            out.push(format!(
                "over {:.3}% > {:.3}%",
                stats.over * 100.0,
                self.over * 100.0
            ));
        }
        (!out.is_empty()).then(|| out.join(", "))
    }
}

/// What fails a run, one line per problem, each naming its node and
/// module: a measure that could not be made, or a module with a GPU kernel
/// that fell back to the CPU (its distance then says nothing about the
/// kernel). Modules without a kernel are not checked for fallbacks.
pub fn problems(diffs: &[ModuleDiff]) -> Vec<String> {
    let mut out = Vec::new();
    for d in diffs {
        let who = format!("{} ({})", d.name, d.module);
        if d.has_kernel && d.fallbacks > 0 {
            out.push(format!(
                "{who}: its GPU kernel fell back to the CPU ({} step{}): {}",
                d.fallbacks,
                if d.fallbacks == 1 { "" } else { "s" },
                d.fallback_reason.as_deref().unwrap_or("no reason given")
            ));
        }
        for (kind, r) in [("alone", &d.alone), ("chain", &d.chain)] {
            if let Err(e) = r {
                out.push(format!("{who}: {kind}: {e}"));
            }
        }
    }
    out
}

/// The renders of `node`'s `port` on both engines.
fn both(
    reference: &Engine,
    other: &Engine,
    graph: &Graph,
    node: NodeId,
    port: &str,
    region: Region,
) -> Result<(Image, Image), String> {
    let r = reference
        .render(graph, node, port, region)
        .map_err(|e| format!("reference: {e}"))?;
    let o = other
        .render(graph, node, port, region)
        .map_err(|e| format!("under test: {e}"))?;
    Ok((r, o))
}

/// `node` rebuilt in a graph of its own: its inputs are sources holding
/// the reference engine's renders of what feeds it, over `region` (a
/// module that reads past it sees the source's edges replicated, on both
/// paths alike). The reference here is a fresh CPU engine, so both paths
/// are handed the same pixels whatever the engines under comparison are.
pub fn alone(graph: &Graph, node: NodeId, region: Region) -> Result<(Graph, NodeId), String> {
    let module = graph
        .module(node)
        .map_err(|e| e.to_string())?
        .ok_or("not a module node")?;
    let params = graph
        .params(node)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let context = graph.context(node).map_err(|e| e.to_string())?.cloned();
    let inputs = graph.input_ports(node).map_err(|e| e.to_string())?;
    let cpu = Engine::new(std::sync::Arc::new(helios_core::Bus::new()));
    let mut g = Graph::new();
    let n = g
        .add_module(
            graph.name(node).map_err(|e| e.to_string())?,
            module.clone(),
            &params,
        )
        .map_err(|e| e.to_string())?;
    if let Some(context) = context {
        g.set_context(n, context).map_err(|e| e.to_string())?;
    }
    for (spec, input) in module.manifest().inputs.iter().zip(inputs) {
        let Some((src, port)) = input else { continue };
        let image = cpu
            .render(graph, src, &port, region)
            .map_err(|e| format!("its input {}: {e}", spec.name))?;
        let s = g.add_source(&format!("in.{}", spec.name), image);
        g.connect(s, SOURCE_PORT, n, &spec.name)
            .map_err(|e| e.to_string())?;
    }
    Ok((g, n))
}
