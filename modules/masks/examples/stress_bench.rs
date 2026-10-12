//! The masks bench: renders the stress network once, in one mode, and
//! prints one JSON line. Run through `cargo xtask bench masks`, which
//! starts one process per mode so each peak memory is its own.
//!
//! ```text
//! stress_bench <full|tiled|proxy|edit> [--modules N] [--width W] [--height H] [--tile T] [--budget MIB]
//! ```
//!
//! - `full`: the whole image in one render.
//! - `tiled`: the image in square tiles of `T` pixels, one render each.
//! - `proxy`: the whole image at a quarter of its size (scale 0.25).
//! - `edit`: a full render, then one value of the last module's chroma
//!   member changed and the image rendered again; the line measures the
//!   second render.
//!
//! `--budget` gives the engine's cache a budget in MiB (none by default,
//! to measure what the network holds).
//!
//! The line holds the time in milliseconds, the peak resident memory in
//! KiB (`VmHWM` of `/proc/self/status`, `null` where it is not available),
//! and the engine's counters for the measured renders: steps asked for,
//! computed and shared (cache hits), content keys computed while
//! planning, steps cropped from wider cached content, and cache entries
//! evicted to stay within the budget.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use helios_core::{Bus, Counters, Engine, Image, ModuleRef, Params, Region, Value};
use helios_module_exposure::Exposure;
use helios_module_masks::stress::{self, Stress};

const USAGE: &str = "usage: stress_bench <full|tiled|proxy|edit> [--modules N] [--width W] [--height H] [--tile T] [--budget MIB]";

struct Options {
    mode: String,
    modules: usize,
    width: u32,
    height: u32,
    tile: u32,
    /// The cache budget in MiB, `None` for no limit.
    budget: Option<usize>,
}

fn options() -> Option<Options> {
    let mut args = std::env::args().skip(1);
    let mode = args.next()?;
    if !matches!(mode.as_str(), "full" | "tiled" | "proxy" | "edit") {
        return None;
    }
    let mut o = Options {
        mode,
        modules: 3,
        width: 1536,
        height: 1024,
        tile: 256,
        budget: None,
    };
    while let Some(flag) = args.next() {
        let value = args.next()?;
        match flag.as_str() {
            "--modules" => o.modules = value.parse().ok().filter(|m| *m > 0)?,
            "--width" => o.width = value.parse().ok().filter(|w| *w > 0)?,
            "--height" => o.height = value.parse().ok().filter(|h| *h > 0)?,
            "--tile" => o.tile = value.parse().ok().filter(|t| *t > 0)?,
            "--budget" => o.budget = Some(value.parse().ok()?),
            _ => return None,
        }
    }
    Some(o)
}

fn network(o: &Options) -> Stress {
    let full = Region::sized(o.width, o.height);
    stress::build(stress::picture(full), o.modules, |k| {
        (
            ModuleRef::new(Exposure),
            Params::new().with("exposure", Value::Float(0.5 - k as f64 * 0.4)),
        )
    })
    .expect("the stress network builds")
}

/// The peak resident memory of this process, in KiB.
fn peak_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn main() -> ExitCode {
    let Some(o) = options() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut n = network(&o);
    let engine = Engine::with_budget(Arc::new(Bus::new()), o.budget.map(|mib| mib << 20));
    let full = Region::sized(o.width, o.height);
    let render = |n: &Stress, region: Region, scale: f64| -> Image {
        engine
            .render_at(&n.graph, n.output(), "output", region, scale)
            .expect("the stress network renders")
    };
    let mut before = Counters::default();
    let start;
    match o.mode.as_str() {
        "full" => {
            start = Instant::now();
            render(&n, full, 1.0);
        }
        "tiled" => {
            start = Instant::now();
            for tile in full.tiles(o.tile) {
                render(&n, tile, 1.0);
            }
        }
        "proxy" => {
            let proxy = Region::sized(o.width.div_ceil(4), o.height.div_ceil(4));
            start = Instant::now();
            render(&n, proxy, 0.25);
        }
        _ => {
            render(&n, full, 1.0);
            let chroma = n.graph.find(&format!("m{}.chroma", o.modules - 1));
            n.graph
                .set_param(
                    chroma.expect("the last module's chroma member"),
                    "chroma",
                    Value::List(vec![0.0, 0.3, 0.0, 0.1]),
                )
                .expect("a valid range");
            before = engine.counters();
            start = Instant::now();
            render(&n, full, 1.0);
        }
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let c = engine.counters().since(&before);
    let peak = peak_kib().map_or_else(|| "null".into(), |k| k.to_string());
    println!(
        "{{\"mode\":\"{}\",\"modules\":{},\"mask_nodes\":{},\"width\":{},\"height\":{},\"tile\":{},\"ms\":{ms:.1},\"peak_kib\":{peak},\"requests\":{},\"computed\":{},\"shared\":{},\"keys\":{},\"crops\":{},\"budget_mib\":{},\"evictions\":{}}}",
        o.mode,
        o.modules,
        n.mask_nodes,
        o.width,
        o.height,
        o.tile,
        c.requests,
        c.computed,
        c.shared(),
        c.keys,
        c.crops,
        o.budget.map_or_else(|| "null".into(), |b| b.to_string()),
        c.evictions,
    );
    ExitCode::SUCCESS
}
