//! Drawn-mask parity with darktable (`docs/masks.md`, "Parity with
//! darktable"): each fixture of `tests/masks/` (an exposure of +1 EV
//! masked by a drawn shape or group, on a raw of the corpus) is rendered by
//! darktable-cli and imported and rendered by Helios.
//!
//! The mask is judged alone, in the render's own terms: darktable's render
//! without the exposure, blended through Helios's mask, against
//! darktable's masked render, in Delta E 2000 (darktable's thresholds, the
//! max at the 99.9th percentile). The raw chains of the two (decoders,
//! demosaic) stay out of it; how far they are is reported apart.
//!
//! ```text
//! mask-parity --corpus DIR --out DIR [--fixtures DIR] [--only NAME] [--darktable-flexi CLI]
//! ```
//!
//! darktable-cli comes from `HELIOS_DARKTABLE_CLI`, else the PATH. With
//! `--darktable-flexi`, the flexi masks' darktable-cli renders the flexi
//! fixtures, and the classic ones again (`<fixture>@flexi`: darktable
//! migrates them to flexi masks as it reads them). Writes
//! `junit.xml`, `summary.md`, `results.json` and the masks of both, a
//! quarter of their size, as PGM (`masks/<fixture>.{helios,darktable}.pgm`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use helios_core::{Bus, Engine, Image, Region};
use helios_parity::masks::{self, EXPOSURE, Fixture};
use helios_parity::{DEMOSAIC_MARGIN, Darktable, Difference, align, compare};
use helios_xmp::drawn::Frame;
use helios_xmp::{Layouts, Sidecar, import_with};
use serde_json::{Value, json};

/// Pixels whose unmasked luminance is below this are left out of the mask
/// read back from darktable's renders (a ratio of noise).
const DARK: f32 = 2e-3;

struct Args {
    corpus: PathBuf,
    out: PathBuf,
    fixtures: PathBuf,
    only: Option<String>,
    flexi: Option<PathBuf>,
}

fn args() -> Result<Args, String> {
    let mut corpus = None;
    let mut out = None;
    let mut fixtures = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/masks"));
    let mut only = None;
    let mut flexi = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a}: a value expected"));
        match a.as_str() {
            "--corpus" => corpus = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--fixtures" => fixtures = PathBuf::from(value()?),
            "--only" => only = Some(value()?),
            "--darktable-flexi" => flexi = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(Args {
        corpus: corpus.ok_or("--corpus DIR expected")?,
        out: out.ok_or("--out DIR expected")?,
        fixtures,
        only,
        flexi,
    })
}

/// What one fixture gave.
struct Outcome {
    name: String,
    raw: String,
    /// darktable's unmasked render blended through Helios's mask, against
    /// darktable's masked render: the mask alone.
    mask: Difference,
    /// Helios's mask against the one read back from darktable's renders.
    mask_mean: f64,
    mask_p999: f64,
    /// Helios's masked render against darktable's: the whole chain.
    chain: Difference,
    shift: (i64, i64),
    seconds: f64,
}

fn luma(p: &[f32]) -> f32 {
    p[0] * 0.26 + p[1] * 0.68 + p[2] * 0.06
}

/// The `w` x `h` part of `img` from `(x, y)` (relative to its region), at
/// the origin.
fn cut(img: &Image, x: i64, y: i64, w: u32, h: u32) -> Image {
    let r = img.region();
    Image::from_fn(Region::sized(w, h), img.channels(), |i, j, c| {
        img.pixel(r.x + x + i, r.y + y + j)[c as usize]
    })
}

/// A one-channel image a quarter of its size, 0..1 as 8 bits.
fn write_pgm(mask: &Image, path: &Path) -> Result<(), String> {
    let r = mask.region();
    let (w, h) = (r.width / 4, r.height / 4);
    let mut out = format!("P5\n{w} {h}\n255\n").into_bytes();
    for y in 0..i64::from(h) {
        for x in 0..i64::from(w) {
            let mut sum = 0.0;
            for dy in 0..4 {
                for dx in 0..4 {
                    sum += mask.pixel(r.x + x * 4 + dx, r.y + y * 4 + dy)[0];
                }
            }
            out.push(((sum / 16.0).clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}

/// Helios's raw chain, as the raw parity test runs it, then the imported
/// exposure with its mask: the network, its unmasked image, its mask
/// (scaled by the blend's opacity) and its masked render.
fn helios(raw: &Path, xmp: &str) -> Result<(Image, Image, Image), String> {
    let loaded = helios_load::load(raw)?;
    let sensor = loaded.sensor.ok_or("not a raw")?;
    let frame = Frame {
        width: f64::from(sensor.width),
        height: f64::from(sensor.height),
        x0: f64::from(sensor.crop_x),
        y0: f64::from(sensor.crop_y),
    };
    let region = loaded.image.region();
    let sidecar = Sidecar::parse(xmp)?;
    let imported = import_with(&sidecar, &Layouts::built_in(), Some(&frame));
    if let Some(n) = imported
        .report
        .not_applied
        .iter()
        .find(|n| n.operation == "exposure")
    {
        return Err(format!(
            "the exposure was not applied: {}",
            n.reason.describe().english
        ));
    }
    let exposure: Vec<helios_xmp::Node> = imported
        .nodes
        .into_iter()
        .filter(|n| n.operation == "exposure")
        .collect();
    let mask = exposure
        .first()
        .and_then(|n| n.mask.clone())
        .ok_or("the exposure has no mask")?;
    let chain = [
        "rawprepare",
        "whitebalance",
        "highlights",
        "demosaic",
        "colorin",
    ];
    let mut modules: Vec<Value> = chain
        .iter()
        .map(|m| json!({ "name": m, "module": m, "params": {} }))
        .collect();
    let mut connections =
        vec![json!({ "from": "raw", "port": "image", "to": "rawprepare", "input": "input" })];
    for pair in chain.windows(2) {
        connections
            .push(json!({ "from": pair[0], "port": "output", "to": pair[1], "input": "input" }));
    }
    let (more, links, (last, port)) = helios_xmp::import::chain(&exposure, ("colorin", "output"));
    modules.extend(more);
    connections.extend(links);
    let desc = json!({
        "sources": [{ "name": "raw", "file": raw }],
        "modules": modules,
        "connections": connections,
    });
    let desc = serde_json::from_value(desc).map_err(|e| e.to_string())?;
    let net = helios_node::load(desc)?;
    let engine = Engine::new(Arc::new(Bus::new()));
    let render = |node: &str, port: &str| -> Result<Image, String> {
        engine
            .render(&net.graph, net.node(node)?, port, region)
            .map_err(|e| e.to_string())
    };
    let base = render("colorin", "output")?;
    let m = render(&mask.net.mask, "mask")?;
    let opacity = mask.opacity as f32;
    let m = Image::from_fn(m.region(), 1, |x, y, _| {
        m.pixel(x, y)[0].clamp(0.0, 1.0) * opacity
    });
    let masked = render(&last, &port)?;
    Ok((base, m, masked))
}

fn run(
    dt: &Darktable,
    args: &Args,
    fixture: &Fixture,
    label: &str,
    bases: &mut Vec<(String, Image)>,
) -> Result<Outcome, String> {
    let start = Instant::now();
    let raw = args.corpus.join(fixture.raw);
    if !raw.is_file() {
        return Err(format!("{} not in the corpus", raw.display()));
    }
    let xmp_path = args.fixtures.join(format!("{}.xmp", fixture.name));
    let xmp =
        std::fs::read_to_string(&xmp_path).map_err(|e| format!("{}: {e}", xmp_path.display()))?;
    let work = args.out.join("work");
    if !bases.iter().any(|(r, _)| r == fixture.raw) {
        let unmasked = masks::unmasked_sidecar(&Layouts::built_in())?;
        let base = dt.render_raw_sidecar(
            &raw,
            &unmasked,
            &work.join(format!("base-{label}-{}", fixture.raw)),
        )?;
        bases.push((fixture.raw.to_string(), base));
    }
    let theirs_base = &bases
        .iter()
        .find(|(r, _)| r == fixture.raw)
        .expect("rendered")
        .1;
    let theirs = dt.render_raw_sidecar(&raw, &xmp, &work.join(label))?;
    let (ours_base, ours_mask, ours) = helios(&raw, &xmp)?;

    // Where darktable's image sits on Helios's: their crops may differ.
    let (dx, dy, _, _) = align(&ours_base, theirs_base, 24);
    let (a, b) = (ours_base.region(), theirs_base.region());
    let (x0, y0) = (0.max(-dx), 0.max(-dy));
    let x1 = i64::from(b.width).min(i64::from(a.width) - dx);
    let y1 = i64::from(b.height).min(i64::from(a.height) - dy);
    let m = i64::from(DEMOSAIC_MARGIN);
    let (w, h) = ((x1 - x0 - 2 * m) as u32, (y1 - y0 - 2 * m) as u32);
    let ours_at = |img: &Image| cut(img, x0 + dx + m, y0 + dy + m, w, h);
    let theirs_at = |img: &Image| cut(img, x0 + m, y0 + m, w, h);
    let (mask, base, masked) = (
        ours_at(&ours_mask),
        theirs_at(theirs_base),
        theirs_at(&theirs),
    );

    // darktable's unmasked render through Helios's mask.
    let gain = 2f32.powf(EXPOSURE as f32) - 1.0;
    let blended = Image::from_fn(base.region(), 3, |x, y, c| {
        base.pixel(x, y)[c as usize] * (1.0 + gain * mask.pixel(x, y)[0])
    });
    let mask_diff = compare(&blended, &masked)?;
    let chain = compare(&ours_at(&ours), &masked)?;

    // darktable's mask, read back where the image is not too dark.
    let theirs_mask = Image::from_fn(base.region(), 1, |x, y, _| {
        let l = luma(base.pixel(x, y));
        if l > DARK {
            (luma(masked.pixel(x, y)) / l - 1.0) / gain
        } else {
            mask.pixel(x, y)[0]
        }
    });
    let mut diffs: Vec<f64> = mask
        .data()
        .iter()
        .zip(theirs_mask.data())
        .map(|(a, b)| f64::from((a - b).abs()))
        .collect();
    let mean = diffs.iter().sum::<f64>() / diffs.len().max(1) as f64;
    let rank = ((diffs.len() as f64 * 0.999) as usize).min(diffs.len().saturating_sub(1));
    let p999 = *diffs.select_nth_unstable_by(rank, f64::total_cmp).1;
    let dir = args.out.join("masks");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    write_pgm(&mask, &dir.join(format!("{label}.helios.pgm")))?;
    write_pgm(&theirs_mask, &dir.join(format!("{label}.darktable.pgm")))?;
    Ok(Outcome {
        name: label.into(),
        raw: fixture.raw.into(),
        mask: mask_diff,
        mask_mean: mean,
        mask_p999: p999,
        chain,
        shift: (dx, dy),
        seconds: start.elapsed().as_secs_f64(),
    })
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn main() -> ExitCode {
    let args = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("mask-parity: {e}");
            eprintln!("usage: mask-parity --corpus DIR --out DIR [--fixtures DIR] [--only NAME]");
            return ExitCode::from(2);
        }
    };
    let dt = match Darktable::find() {
        Ok(dt) => dt,
        Err(e) => {
            eprintln!("mask-parity: {e}");
            return ExitCode::from(2);
        }
    };
    eprintln!("against {}", dt.version);
    if let Err(e) = std::fs::create_dir_all(&args.out) {
        eprintln!("mask-parity: {}: {e}", args.out.display());
        return ExitCode::from(2);
    }
    let flexi = match args.flexi.as_deref().map(Darktable::at).transpose() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("mask-parity: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some(f) = &flexi {
        eprintln!("and the flexi masks' {}", f.version);
    }
    let mut bases = Vec::new();
    let mut flexi_bases = Vec::new();
    let mut runs: Vec<(String, Fixture, bool)> = Vec::new();
    for fixture in masks::fixtures() {
        if args.only.as_deref().is_some_and(|o| o != fixture.name) {
            continue;
        }
        if !fixture.flexi && flexi.is_some() {
            runs.push((
                format!("{}@flexi", fixture.name),
                masks::fixtures()
                    .into_iter()
                    .find(|f| f.name == fixture.name)
                    .expect("the same"),
                true,
            ));
        }
        let on_flexi = fixture.flexi;
        runs.push((fixture.name.to_string(), fixture, on_flexi));
    }
    runs.sort_by_key(|(_, _, on_flexi)| *on_flexi);
    let mut results: Vec<(String, Result<Outcome, String>)> = Vec::new();
    for (label, fixture, on_flexi) in &runs {
        let r = match (on_flexi, &flexi) {
            (false, _) => run(&dt, &args, fixture, label, &mut bases),
            (true, Some(f)) => run(f, &args, fixture, label, &mut flexi_bases),
            (true, None) => Err("a flexi fixture: needs --darktable-flexi".into()),
        };
        match &r {
            Ok(o) => eprintln!(
                "{}: mask ΔE mean {:.3} p99.9 {:.3} max {:.2}; mask |Δ| mean {:.4} p99.9 {:.4}; chain ΔE mean {:.3} p99.9 {:.3}; shift {:?}; {:.1} s",
                o.name,
                o.mask.mean,
                o.mask.p999,
                o.mask.max,
                o.mask_mean,
                o.mask_p999,
                o.chain.mean,
                o.chain.p999,
                o.shift,
                o.seconds
            ),
            Err(e) => eprintln!("{label}: {e}"),
        }
        results.push((label.clone(), r));
    }
    let _ = std::fs::remove_dir_all(args.out.join("work"));

    let mut junit = String::new();
    let failures = results
        .iter()
        .filter(|(_, r)| !r.as_ref().is_ok_and(|o| o.mask.passes_as_a_port()))
        .count();
    let _ = writeln!(
        junit,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites>\n<testsuite name=\"mask-parity\" tests=\"{}\" failures=\"{failures}\">",
        results.len()
    );
    let mut summary = format!(
        "# Drawn-mask parity\n\nAgainst {}. darktable's unmasked render blended through Helios's mask, against darktable's masked render: Delta E 2000, passing at a mean of at most {} and a 99.9th percentile of at most {} (darktable's thresholds). The mask's own difference (0 to 1) and the whole chain's Delta E are given for the record.\n\n| fixture | raw | mask ΔE mean | mask ΔE p99.9 | mask ΔE max | mask Δ mean | mask Δ p99.9 | chain ΔE mean | chain ΔE p99.9 | result |\n|---|---|---|---|---|---|---|---|---|---|\n",
        dt.version,
        helios_parity::MEAN_DELTA_E,
        helios_parity::MAX_DELTA_E
    );
    let mut json_out = Vec::new();
    for (name, r) in &results {
        match r {
            Ok(o) => {
                let pass = o.mask.passes_as_a_port();
                let _ = writeln!(
                    summary,
                    "| {} | {} | {:.3} | {:.3} | {:.2} | {:.4} | {:.4} | {:.3} | {:.3} | {} |",
                    o.name,
                    o.raw,
                    o.mask.mean,
                    o.mask.p999,
                    o.mask.max,
                    o.mask_mean,
                    o.mask_p999,
                    o.chain.mean,
                    o.chain.p999,
                    if pass { "pass" } else { "FAIL" }
                );
                let _ = write!(
                    junit,
                    "<testcase classname=\"mask-parity\" name=\"{}\" time=\"{:.1}\">",
                    escape(name),
                    o.seconds
                );
                if !pass {
                    let _ = write!(
                        junit,
                        "<failure message=\"mask Delta E mean {:.3}, p99.9 {:.3}\"/>",
                        o.mask.mean, o.mask.p999
                    );
                }
                let _ = writeln!(junit, "</testcase>");
                json_out.push(json!({
                    "fixture": o.name, "raw": o.raw, "pass": pass,
                    "mask_delta_e": { "mean": o.mask.mean, "p999": o.mask.p999, "max": o.mask.max },
                    "mask_difference": { "mean": o.mask_mean, "p999": o.mask_p999 },
                    "chain_delta_e": { "mean": o.chain.mean, "p999": o.chain.p999, "max": o.chain.max },
                    "shift": [o.shift.0, o.shift.1],
                }));
            }
            Err(e) => {
                let _ = writeln!(summary, "| {name} | | | | | | | | | error: {e} |");
                let _ = writeln!(
                    junit,
                    "<testcase classname=\"mask-parity\" name=\"{}\"><error message=\"{}\"/></testcase>",
                    escape(name),
                    escape(e)
                );
                json_out.push(json!({ "fixture": name, "error": e }));
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
            eprintln!("mask-parity: {file}: {e}");
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
