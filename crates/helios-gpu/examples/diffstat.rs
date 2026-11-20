//! `diffstat`: runs the bench fixtures' pipelines on the CPU and on the GPU
//! and prints, per module, how far the GPU output is from the CPU's (see
//! `helios_gpu::diffstat`; the idea comes from darktable's
//! `--opencl-diffstat`). `cargo xtask engine-diffstat` runs it.
//!
//! ```text
//! diffstat <fixture|all> [--module NAME] [--dump-diff DIR] [--threshold T]
//!          [--tile N] [--json FILE] [--cpu]
//! ```
//!
//! `--cpu` compares the CPU with itself (the sanity mode: every distance
//! is zero). `--dump-diff DIR` writes the per-pixel distance of the module
//! `--module` names, alone and through the chain, as PFM files.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, ModuleRef, Params, Region, Value};
use helios_gpu::diffstat::{self, ModuleDiff, Options, Stats};

const USAGE: &str = "diffstat <fixture|all> [--module NAME] [--dump-diff DIR] [--threshold T] \
                     [--tile N] [--json FILE] [--cpu]";

/// The fixtures: a pipeline over a synthetic linear image.
const FIXTURES: [&str; 2] = ["tone", "vkdt"];

/// The input every fixture reads: a smooth gradient with noise on top and
/// highlights past 1, so curves and blurs see edges, flats and clipping.
fn input() -> Image {
    let region = Region::sized(512, 384);
    let noise = helios_testkit::noise(region, 4, 11);
    Image::from_fn(region, 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        let g = x as f32 / 512.0 * 1.5 + y as f32 / 384.0 * 0.25;
        let n = noise.pixel(x, y)[c as usize];
        (g * (0.6 + 0.2 * c as f32) + 0.05 * n).max(0.0)
    })
}

fn f(name: &str, v: f64) -> Params {
    Params::new().with(name, Value::Float(v))
}

/// A fixture's graph: the source then its modules in a line, each
/// reading the previous one's first output.
fn fixture(name: &str) -> Result<Graph, String> {
    let chain: Vec<(&str, ModuleRef, Params)> = match name {
        "tone" => vec![
            (
                "exposure",
                ModuleRef::new(helios_module_exposure::Exposure),
                f("exposure", 0.7),
            ),
            (
                "blur",
                ModuleRef::new(helios_module_blur::Blur),
                f("radius", 2.0),
            ),
            (
                "sigmoid",
                ModuleRef::new(helios_module_sigmoid::Sigmoid),
                Params::new(),
            ),
        ],
        "vkdt" => vec![
            (
                "exposure",
                ModuleRef::new(helios_module_vkdt::VkdtExposure),
                Params::new(),
            ),
            (
                "grade",
                ModuleRef::new(helios_module_vkdt::VkdtGrade),
                Params::new(),
            ),
            (
                "filmcurv",
                ModuleRef::new(helios_module_vkdt::VkdtFilmcurv),
                Params::new(),
            ),
        ],
        other => {
            return Err(format!(
                "no fixture {other}; known: {}",
                FIXTURES.join(", ")
            ));
        }
    };
    let mut g = Graph::new();
    let mut prev = (g.add_source("src", input()), "image".to_string());
    for (name, module, params) in chain {
        let m = module.manifest().clone();
        let n = g
            .add_module(name, module, &params)
            .map_err(|e| e.to_string())?;
        let input = m.inputs.first().ok_or("a module without input")?;
        g.connect(prev.0, &prev.1, n, &input.name)
            .map_err(|e| e.to_string())?;
        prev = (n, m.outputs[0].name.clone());
    }
    Ok(g)
}

struct Args {
    fixtures: Vec<String>,
    module: Option<String>,
    dump: Option<PathBuf>,
    json: Option<PathBuf>,
    cpu: bool,
    options: Options,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        fixtures: Vec::new(),
        module: None,
        dump: None,
        json: None,
        cpu: false,
        options: Options::default(),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--module" => a.module = Some(value()?),
            "--dump-diff" => a.dump = Some(value()?.into()),
            "--json" => a.json = Some(value()?.into()),
            "--threshold" => a.options.threshold = value()?.parse().map_err(|e| format!("{e}"))?,
            "--tile" => a.options.tile = value()?.parse().map_err(|e| format!("{e}"))?,
            "--cpu" => a.cpu = true,
            "all" => a.fixtures.extend(FIXTURES.iter().map(ToString::to_string)),
            s if !s.starts_with("--") => a.fixtures.push(s.to_string()),
            s => return Err(format!("unknown option {s}")),
        }
    }
    if a.fixtures.is_empty() {
        return Err("no fixture".into());
    }
    if a.dump.is_some() && a.module.is_none() {
        return Err("--dump-diff dumps one module: name it with --module".into());
    }
    Ok(a)
}

/// Writes a one-channel image as a PFM (bottom row first, little-endian).
fn write_pfm(image: &Image, path: &Path) -> Result<(), String> {
    let r = image.region();
    let mut out = format!("Pf\n{} {}\n-1.0\n", r.width, r.height).into_bytes();
    for y in (r.y..r.bottom()).rev() {
        for v in image.row(y) {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}

fn cell(s: &Result<Stats, String>) -> String {
    match s {
        Ok(s) => format!(
            "{:>10.3e} {:>10.3e} {:>7.3}% {:>18}",
            s.max_abs,
            s.mean_abs,
            s.over * 100.0,
            format!(
                "{},{} {}x{}",
                s.worst_tile.x, s.worst_tile.y, s.worst_tile.width, s.worst_tile.height
            )
        ),
        Err(e) => format!("error: {e}"),
    }
}

fn stats_json(s: &Result<Stats, String>) -> serde_json::Value {
    match s {
        Ok(s) => serde_json::json!({
            "max_abs": s.max_abs,
            "mean_abs": s.mean_abs,
            "over": s.over,
            "worst_tile": [s.worst_tile.x, s.worst_tile.y, s.worst_tile.width, s.worst_tile.height],
            "worst_tile_mean": s.worst_tile_mean,
        }),
        Err(e) => serde_json::json!({ "error": e }),
    }
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(&raw) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("diffstat: {e}\nusage: {USAGE}");
            return ExitCode::from(2);
        }
    };
    let bus = Arc::new(Bus::new());
    let reference = Engine::new(bus.clone());
    let (other, adapter) = if args.cpu {
        (Engine::new(bus), "cpu (sanity mode)".to_string())
    } else {
        match helios_gpu::shared::gpu() {
            Ok(gpu) => {
                let adapter = gpu.adapter.clone();
                (Engine::new(bus).with_accelerator(gpu), adapter)
            }
            Err(e) => {
                eprintln!("diffstat: no GPU: {e}");
                return ExitCode::FAILURE;
            }
        }
    };
    println!("GPU: {adapter}");
    println!(
        "threshold {:.3e}, tiles of {} px; alone = the module on the CPU's input, chain = through the pipeline",
        args.options.threshold, args.options.tile
    );
    let mut lines = Vec::new();
    let mut failed = false;
    for name in &args.fixtures {
        let graph = match fixture(name) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("diffstat: {e}");
                return ExitCode::from(2);
            }
        };
        let region = input().region();
        let diffs = match diffstat::run(
            &graph,
            region,
            &reference,
            &other,
            args.options,
            args.module.as_deref(),
        ) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("diffstat: {name}: {e}");
                return ExitCode::FAILURE;
            }
        };
        println!("\n{name}");
        println!(
            "  {:<10} {:<14} {:<6} | alone: {:>10} {:>10} {:>8} {:>18} | chain: {:>10} {:>10} {:>8} {:>18}",
            "node",
            "module",
            "kernel",
            "max",
            "mean",
            "over",
            "worst tile",
            "max",
            "mean",
            "over",
            "worst tile"
        );
        for d in &diffs {
            failed |= d.alone.is_err() || d.chain.is_err();
            println!(
                "  {:<10} {:<14} {:<6} | {} | {}",
                d.name,
                d.module,
                if d.has_kernel { "gpu" } else { "cpu" },
                cell(&d.alone),
                cell(&d.chain)
            );
            lines.push(serde_json::json!({
                "fixture": name,
                "node": d.name,
                "module": d.module,
                "kernel": d.has_kernel,
                "adapter": adapter,
                "alone": stats_json(&d.alone),
                "chain": stats_json(&d.chain),
            }));
            if let Some(dir) = &args.dump
                && let Err(e) = dump(&graph, d, region, &reference, &other, dir, name)
            {
                eprintln!("diffstat: dump: {e}");
                failed = true;
            }
        }
    }
    if let Some(path) = &args.json {
        let mut text = String::new();
        for l in &lines {
            text.push_str(&l.to_string());
            text.push('\n');
        }
        if let Err(e) = std::fs::File::create(path).and_then(|mut f| f.write_all(text.as_bytes())) {
            eprintln!("diffstat: {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Writes the module's per-pixel distance, alone and through the chain.
fn dump(
    graph: &Graph,
    d: &ModuleDiff,
    region: Region,
    reference: &Engine,
    other: &Engine,
    dir: &Path,
    fixture: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let port = graph
        .manifest(d.node)
        .map_err(|e| e.to_string())?
        .ok_or("not a module")?
        .outputs[0]
        .name
        .clone();
    let (alone, n) = diffstat::alone(graph, d.node, region)?;
    for (kind, g, node) in [("alone", &alone, n), ("chain", graph, d.node)] {
        let r = reference
            .render(g, node, &port, region)
            .map_err(|e| e.to_string())?;
        let o = other
            .render(g, node, &port, region)
            .map_err(|e| e.to_string())?;
        let image = diffstat::diff_image(&r, &o)?;
        let path = dir.join(format!("{fixture}-{}-{kind}.pfm", d.name));
        write_pfm(&image, &path)?;
        println!("  wrote {}", path.display());
    }
    Ok(())
}
