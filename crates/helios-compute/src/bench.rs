//! Calibration: a short benchmark of a device on reference modules, one per
//! family, over a fixed tile, through the engine as real renders go.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use helios_core::{
    Accelerator, Bus, Engine, Graph, Image, ModuleRef, Params, Region, Value, graph::SOURCE_PORT,
};

/// The side of the square tile every reference module runs over.
pub const TILE: u32 = 512;

/// Timed runs per module, after one to warm up (kernels compile then); the
/// best is kept.
const RUNS: usize = 3;

/// The reference modules: (family, module, parameters).
fn references() -> Vec<(&'static str, ModuleRef, Params)> {
    vec![
        (
            "tone",
            ModuleRef::new(helios_module_exposure::Exposure),
            Params::new().with("exposure", Value::Float(1.0)),
        ),
        (
            "detail",
            ModuleRef::new(helios_module_blur::Blur),
            Params::new().with("radius", Value::Float(4.0)),
        ),
    ]
}

/// A fixed, deterministic tile: the same work on every device.
fn tile(side: u32) -> Image {
    Image::from_fn(Region::sized(side, side), 4, |x, y, c| {
        (((x * 7 + y * 13 + i64::from(c) * 3) % 97) as f32) / 96.0
    })
}

/// Megapixels per second of each family on the CPU (`None`) or a GPU,
/// over a square tile of `side` pixels ([`TILE`] normally). A GPU that
/// computes nothing is an error: the speed would be the CPU's.
pub fn calibrate(
    accelerator: Option<Arc<dyn Accelerator>>,
    side: u32,
) -> Result<BTreeMap<String, f64>, String> {
    let input = tile(side);
    let region = input.region();
    let mut speeds = BTreeMap::new();
    for (family, module, params) in references() {
        let mut g = Graph::new();
        let src = g.add_source("src", input.clone());
        let name = module.manifest().name.clone();
        let node = g
            .add_module(&name, module.clone(), &params)
            .map_err(|e| e.to_string())?;
        g.connect(src, SOURCE_PORT, node, "input")
            .map_err(|e| e.to_string())?;
        let mut best = Duration::MAX;
        for run in 0..=RUNS {
            // A fresh engine each run: its cache would answer otherwise.
            let mut engine = Engine::new(Arc::new(Bus::new()));
            if let Some(a) = &accelerator {
                engine = engine.with_accelerator(a.clone());
            }
            let start = Instant::now();
            engine
                .render(&g, node, "output", region)
                .map_err(|e| format!("{name}: {e}"))?;
            let took = start.elapsed();
            if accelerator.is_some() {
                // A step that fell back ran on the CPU: its time is not the GPU's.
                if let Some(why) = engine.fallback_reason() {
                    return Err(format!("{name}: the GPU failed: {why}"));
                }
                if engine.counters().accelerated == 0 {
                    return Err(format!("{name}: the GPU computed nothing"));
                }
            }
            if run > 0 {
                best = best.min(took);
            }
        }
        let seconds = best.as_secs_f64().max(1e-9);
        let mpx = region.area() as f64 / 1e6;
        speeds.insert(family.to_string(), mpx / seconds);
    }
    Ok(speeds)
}
