//! Raw development parity with darktable (`docs/raw-development.md`): a
//! few raws of the corpus developed by Helios's "Develop one image" chain
//! with the parameters it ships (its exposure: darktable's default for a
//! raw, +0.7 EV and the camera's bias taken back), against darktable-cli
//! twice:
//!
//! - **chain**: darktable running the same modules as Helios (rawprepare,
//!   white balance at the camera's D65 reference, highlights clipped, its
//!   default demosaic, colorin's standard matrix, colour calibration from
//!   the camera's illuminant, the template's exposure, sigmoid at its
//!   defaults), judged by
//!   [`helios_parity::AS_A_PORT`] away from the image's edges: whether the
//!   ported modules match;
//! - **defaults**: darktable developing the raw with no edit (its
//!   scene-referred sigmoid workflow: color calibration, +0.7 EV,
//!   highlight reconstruction...): how far Helios's default look is from
//!   darktable's. Measured, not judged.
//!
//! ```text
//! raw-parity --corpus DIR --out DIR [--only ID]...
//! ```
//!
//! darktable-cli comes from `HELIOS_DARKTABLE_CLI`, else the PATH. Writes
//! `junit.xml` (one case per raw: the chain), `summary.md`,
//! `results.json`, and per raw an sRGB PNG side by side (Helios, darktable
//! with Helios's chain, darktable's defaults) in `shots/`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use helios_core::{Bus, Engine, Image, Region};
use helios_parity::{
    AS_A_PORT, DEMOSAIC_MARGIN, Darktable, Difference, Item, align, compare, inset, reduce,
};
use serde_json::{Value, json};

/// The corpus raws, by manifest id: a Bayer CR2, a Bayer NEF and an
/// X-Trans RAF (the ones the `raw-develop` job opens).
const RAWS: [&str; 3] = [
    "canon-eos-350d-cr2-raw-3-2",
    "nikon-d300-nef-12bit-lossless-compression",
    "fujifilm-x-pro1-raf-12bit-12bit-uncompressed-3-2",
];

/// The modules of "Develop one image", in its order.
const DEVELOP: [&str; 8] = [
    "rawprepare",
    "whitebalance",
    "highlights",
    "demosaic",
    "colorin",
    "channelmixerrgb",
    "exposure",
    "sigmoid",
];

/// darktable's white balance preset "camera reference" (`DT_IOP_TEMP_D65`).
const DT_IOP_TEMP_D65: i64 = 3;

/// How far a crop may move between the two decoders, in pixels.
const REACH: i64 = 64;

/// The width of each picture in the side-by-side PNG.
const SHOT: u32 = 640;

struct Args {
    corpus: PathBuf,
    out: PathBuf,
    only: Vec<String>,
}

fn args() -> Result<Args, String> {
    let mut corpus = None;
    let mut out = None;
    let mut only = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a}: a value expected"));
        match a.as_str() {
            "--corpus" => corpus = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--only" => only.push(value()?),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(Args {
        corpus: corpus.ok_or("--corpus DIR expected")?,
        out: out.ok_or("--out DIR expected")?,
        only,
    })
}

/// A corpus file by id, whatever its extension.
fn corpus_file(dir: &Path, id: &str) -> Result<PathBuf, String> {
    std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(id))
        .ok_or_else(|| format!("no {id} in {}", dir.display()))
}

/// The parameters "Develop one image" gives the node `name`, as Helios
/// ships it.
fn develop_params(name: &str) -> Result<Value, String> {
    let develop = helios_template::shipped::templates()
        .into_iter()
        .find(|b| b.template.id == "helios.develop")
        .ok_or("no \"Develop one image\" among the built-in templates")?;
    let node = develop
        .template
        .network
        .nodes
        .iter()
        .find(|n| n.name == name)
        .ok_or_else(|| format!("\"Develop one image\" has no `{name}`"))?;
    serde_json::to_value(&node.params).map_err(|e| e.to_string())
}

/// Helios's "Develop one image" chain with its parameters, the whole
/// image.
fn helios(raw: &Path) -> Result<Image, String> {
    let modules = DEVELOP
        .iter()
        .map(|m| Ok(json!({ "name": m, "module": m, "params": develop_params(m)? })))
        .collect::<Result<Vec<Value>, String>>()?;
    let mut connections =
        vec![json!({ "from": "raw", "port": "image", "to": DEVELOP[0], "input": "input" })];
    for pair in DEVELOP.windows(2) {
        connections
            .push(json!({ "from": pair[0], "port": "output", "to": pair[1], "input": "input" }));
    }
    let desc = json!({
        "sources": [{ "name": "raw", "file": raw }],
        "modules": modules,
        "connections": connections,
    });
    let net = helios_node::load(serde_json::from_value(desc).map_err(|e| e.to_string())?)?;
    let last = net.node(DEVELOP[DEVELOP.len() - 1])?;
    let region = net
        .graph
        .extent(last)
        .map_err(|e| e.to_string())?
        .ok_or("no extent")?;
    Engine::new(Arc::new(Bus::new()))
        .render(&net.graph, last, "output", region)
        .map_err(|e| e.to_string())
}

/// darktable running Helios's chain: what Helios's modules default to,
/// in darktable's terms, and the template's exposure (the rest are
/// darktable's defaults with no workflow: its default demosaic, sigmoid's
/// own defaults, between colorin's standard matrix and a linear Rec.2020
/// output). The white balance is at the camera's D65 reference, its
/// coefficients from the matrix Helios reads, and colour calibration
/// adapts from the camera's illuminant, as "Develop one image" does.
fn darktable_chain(dt: &Darktable, raw: &Path, dir: &Path) -> Result<Image, String> {
    let exposure = develop_params("exposure")?;
    let float = |k: &str| exposure[k]["float"].as_f64().unwrap_or(0.0);
    let camera = helios_load::load(raw)?
        .camera
        .ok_or_else(|| format!("{}: no camera", raw.display()))?;
    let d65 = helios_module_whitebalance::d65_coefficients(&camera.xyz_to_camera)
        .ok_or_else(|| format!("{}: no D65 coefficients from its matrix", raw.display()))?;
    let identity = |c: usize| {
        let mut row = [0.0; 4];
        row[c] = 1.0;
        row
    };
    let items = [
        Item::new(
            "temperature",
            json!({
                "red": d65[0],
                "green": d65[1],
                "blue": d65[2],
                "various": d65[1],
                "preset": DT_IOP_TEMP_D65,
            }),
        ),
        Item::new(
            "channelmixerrgb",
            json!({
                "red": identity(0),
                "green": identity(1),
                "blue": identity(2),
                "illuminant": "DT_ILLUMINANT_CAMERA",
                "adaptation": "DT_ADAPTATION_CAT16",
            }),
        ),
        Item::new(
            "highlights",
            json!({"mode": "DT_IOP_HIGHLIGHTS_CLIP", "clip": 1.0}),
        ),
        Item::new(
            "exposure",
            json!({
                "exposure": float("exposure"),
                "black": float("black"),
                "compensate_exposure_bias": exposure["compensate_bias"]["bool"].as_bool().unwrap_or(false),
            }),
        ),
        Item::new("sigmoid", json!({})),
    ];
    dt.render_raw(raw, &items, &dt.layouts(), dir)
}

/// The mean of the luminance (Rec.2020's Y), linear.
fn mean_y(img: &Image) -> f64 {
    let n = img.channels() as usize;
    let (mut sum, mut count) = (0.0f64, 0u64);
    for p in img.data().chunks_exact(n) {
        sum += f64::from(p[0]) * 0.2627 + f64::from(p[1]) * 0.6780 + f64::from(p[2]) * 0.0593;
        count += 1;
    }
    if count == 0 { 0.0 } else { sum / count as f64 }
}

/// Two renders compared: aligned, judged inside the demosaic margin, and
/// with detail averaged out (1/8: colour and tone alone).
struct Pair {
    full: Difference,
    coarse: Difference,
    shift: (i64, i64),
    /// log2 of the ratio of mean luminances, darktable's over Helios's.
    ev: f64,
}

fn pair(ours: &Image, theirs: &Image) -> Result<Pair, String> {
    let (a, b) = (ours.region(), theirs.region());
    if (a.width > a.height) != (b.width > b.height) {
        return Err(format!(
            "orientations differ: {a} against {b} (darktable rotates by the EXIF orientation)"
        ));
    }
    let (dx, dy, ours, theirs) = align(ours, theirs, REACH);
    let (ours, theirs) = (
        inset(&ours, DEMOSAIC_MARGIN),
        inset(&theirs, DEMOSAIC_MARGIN),
    );
    let full = compare(&ours, &theirs)?;
    let coarse = compare(&reduce(&ours, 8), &reduce(&theirs, 8))?;
    let ev = (mean_y(&theirs) / mean_y(&ours).max(1e-9)).log2();
    Ok(Pair {
        full,
        coarse,
        shift: (dx, dy),
        ev,
    })
}

/// The pictures side by side, each `SHOT` wide, as an 8-bit sRGB PNG.
fn side_by_side(images: &[&Image], path: &Path) -> Result<(), String> {
    let fitted: Vec<Image> = images
        .iter()
        .map(|img| {
            let r = img.region();
            let long = (f64::from(SHOT) * f64::from(r.width.max(r.height)) / f64::from(r.width))
                .round() as u32;
            let small = helios_export::fit(img, long.max(1));
            let s = small.region();
            Image::from_fn(Region::sized(s.width, s.height), 4, |x, y, c| {
                let p = small.pixel(s.x + x, s.y + y);
                if c == 3 { 1.0 } else { p[c as usize] }
            })
        })
        .collect();
    let height = fitted.iter().map(|i| i.region().height).max().unwrap_or(1);
    let gap = 8u32;
    let width = fitted.iter().map(|i| i.region().width + gap).sum::<u32>() - gap;
    let mut starts = Vec::new();
    let mut x0 = 0u32;
    for img in &fitted {
        starts.push(x0);
        x0 += img.region().width + gap;
    }
    let canvas = Image::from_fn(Region::sized(width, height), 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        for (img, start) in fitted.iter().zip(&starts) {
            let r = img.region();
            let ix = x - i64::from(*start);
            if ix >= 0 && ix < i64::from(r.width) && y < i64::from(r.height) {
                return img.pixel(ix, y)[c as usize];
            }
        }
        0.0
    });
    let bytes = helios_export::encode(&canvas, &helios_export::Format::Png { bits: 8 })
        .map_err(|m| m.english)?;
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// What one raw gave.
struct Outcome {
    id: String,
    size: (u32, u32),
    /// darktable's size, its defaults' render.
    theirs: (u32, u32),
    chain: Pair,
    defaults: Pair,
    seconds: f64,
}

fn run(dt: &Darktable, args: &Args, id: &str) -> Result<Outcome, String> {
    let start = Instant::now();
    let raw = corpus_file(&args.corpus, id)?;
    let work = args.out.join("work").join(id);
    let ours = helios(&raw)?;
    let chain = darktable_chain(dt, &raw, &work.join("chain"))?;
    let defaults = dt.render_raw_defaults(&raw, &work.join("defaults"))?;
    let _ = std::fs::remove_dir_all(&work);
    let shots = args.out.join("shots");
    std::fs::create_dir_all(&shots).map_err(|e| format!("{}: {e}", shots.display()))?;
    side_by_side(
        &[&ours, &chain, &defaults],
        &shots.join(format!("{id}.png")),
    )?;
    let (r, t) = (ours.region(), defaults.region());
    Ok(Outcome {
        id: id.to_string(),
        size: (r.width, r.height),
        theirs: (t.width, t.height),
        chain: pair(&ours, &chain).map_err(|e| format!("chain: {e}"))?,
        defaults: pair(&ours, &defaults).map_err(|e| format!("defaults: {e}"))?,
        seconds: start.elapsed().as_secs_f64(),
    })
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn difference_json(d: &Difference) -> Value {
    json!({ "identical": d.identical, "mean": d.mean, "p999": d.p999, "p9999": d.p9999, "max": d.max })
}

fn pair_json(p: &Pair) -> Value {
    json!({
        "full": difference_json(&p.full),
        "coarse": difference_json(&p.coarse),
        "shift": [p.shift.0, p.shift.1],
        "ev": p.ev,
    })
}

fn main() -> ExitCode {
    let args = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("raw-parity: {e}");
            eprintln!("usage: raw-parity --corpus DIR --out DIR [--only ID]...");
            return ExitCode::from(2);
        }
    };
    let dt = match Darktable::find() {
        Ok(dt) => dt,
        Err(e) => {
            eprintln!("raw-parity: {e}");
            return ExitCode::from(2);
        }
    };
    eprintln!("against {}", dt.version);
    if let Err(e) = std::fs::create_dir_all(&args.out) {
        eprintln!("raw-parity: {}: {e}", args.out.display());
        return ExitCode::from(2);
    }
    let mut results: Vec<(String, Result<Outcome, String>)> = Vec::new();
    for id in RAWS {
        if !args.only.is_empty() && !args.only.iter().any(|o| o == id) {
            continue;
        }
        let r = run(&dt, &args, id);
        match &r {
            Ok(o) => eprintln!(
                "{id}: chain ΔE mean {:.3} p99.9 {:.3} (1/8 mean {:.3}), defaults ΔE mean {:.3} p99.9 {:.3} (1/8 mean {:.3}, {:+.2} EV); {:.1} s",
                o.chain.full.mean,
                o.chain.full.p999,
                o.chain.coarse.mean,
                o.defaults.full.mean,
                o.defaults.full.p999,
                o.defaults.coarse.mean,
                o.defaults.ev,
                o.seconds
            ),
            Err(e) => eprintln!("{id}: {e}"),
        }
        results.push((id.to_string(), r));
    }
    let _ = std::fs::remove_dir_all(args.out.join("work"));

    let failures = results
        .iter()
        .filter(|(_, r)| !r.as_ref().is_ok_and(|o| o.chain.full.within(&AS_A_PORT)))
        .count();
    let mut junit = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites>\n<testsuite name=\"raw-parity\" tests=\"{}\" failures=\"{failures}\">\n",
        results.len()
    );
    let mut summary = format!(
        "# Raw development parity\n\nAgainst {}. Helios's \"Develop one image\" at its defaults, Delta E 2000 inside a {DEMOSAIC_MARGIN}-pixel margin, aligned within {REACH} pixels; 1/8: the mean of 8 x 8 blocks (colour and tone, without demosaicing detail); EV: log2 of darktable's mean luminance over Helios's.\n\n- **chain**: darktable running Helios's modules, judged by darktable's thresholds with the max at the 99.9th percentile (mean <= {}, p99.9 <= {}).\n- **defaults**: darktable's own defaults (scene-referred sigmoid, modern chromatic adaptation): measured, not judged.\n\n| raw | Helios size | darktable size | chain shift | chain mean | chain p99.9 | chain 1/8 mean | chain | defaults mean | defaults p99.9 | defaults 1/8 mean | defaults 1/8 p99.9 | defaults EV |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
        dt.version, AS_A_PORT.mean, AS_A_PORT.p999
    );
    let mut json_out = Vec::new();
    for (id, r) in &results {
        match r {
            Ok(o) => {
                let pass = o.chain.full.within(&AS_A_PORT);
                let _ = writeln!(
                    summary,
                    "| {} | {}x{} | {}x{} | {:?} | {:.3} | {:.3} | {:.3} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {:+.2} |",
                    o.id,
                    o.size.0,
                    o.size.1,
                    o.theirs.0,
                    o.theirs.1,
                    o.chain.shift,
                    o.chain.full.mean,
                    o.chain.full.p999,
                    o.chain.coarse.mean,
                    if pass { "pass" } else { "FAIL" },
                    o.defaults.full.mean,
                    o.defaults.full.p999,
                    o.defaults.coarse.mean,
                    o.defaults.coarse.p999,
                    o.defaults.ev
                );
                let _ = write!(
                    junit,
                    "<testcase classname=\"raw-parity\" name=\"{}\" time=\"{:.1}\">",
                    escape(id),
                    o.seconds
                );
                if !pass {
                    let _ = write!(
                        junit,
                        "<failure message=\"chain Delta E {}\"/>",
                        escape(&o.chain.full.beyond(&AS_A_PORT).join(", "))
                    );
                }
                let _ = writeln!(junit, "</testcase>");
                json_out.push(json!({
                    "raw": o.id, "size": [o.size.0, o.size.1], "darktable_size": [o.theirs.0, o.theirs.1], "chain_pass": pass,
                    "chain": pair_json(&o.chain), "defaults": pair_json(&o.defaults),
                    "seconds": o.seconds,
                }));
            }
            Err(e) => {
                let _ = writeln!(summary, "| {id} | | | | | | | error: {e} | | | | | |");
                let _ = writeln!(
                    junit,
                    "<testcase classname=\"raw-parity\" name=\"{}\"><error message=\"{}\"/></testcase>",
                    escape(id),
                    escape(e)
                );
                json_out.push(json!({ "raw": id, "error": e }));
            }
        }
    }
    let _ = writeln!(junit, "</testsuite>\n</testsuites>");
    for (file, text) in [
        ("junit.xml", junit),
        ("summary.md", summary.clone()),
        (
            "results.json",
            serde_json::to_string_pretty(&json_out).expect("JSON"),
        ),
    ] {
        if let Err(e) = std::fs::write(args.out.join(file), text) {
            eprintln!("raw-parity: {file}: {e}");
            return ExitCode::from(2);
        }
    }
    print!("{summary}");
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
