//! How long opening a photo takes, step by step, on the raw corpus
//! (docs/performance.md, box job `open-timing`).
//!
//! Each corpus raw (the largest first) is opened `--runs` times, each
//! time on a fresh engine, as the desktop opens it: decoded and its graph
//! built through the `helios.develop` network, then, at once, the
//! viewport's thread (a proxy, then the frame fitted in 1920×1080) and the
//! preview's thread (256 px), the histogram computed on the preview as
//! soon as it lands; then one slider (exposure +0.5 EV) rendered again on
//! the warm engine. The engine's spans (`helios_core::trace`) say what
//! each step cost: the decode, the graph, each demosaic call with its
//! algorithm and megapixels.
//!
//! The times are from the open's start; each cell is the median of the
//! runs. A row whose runs spread more than 15 % (open to fit) is marked
//! unstable, and the run as a whole is said not stable then: the box's
//! own conditions (throttling, other tenants) are in the job's
//! `conditions.json`.
//!
//! Then one return: two mid-sized raws opened in turn on one engine
//! within a budget (1 GiB, the router's 1 GiB too), as the desktop holds
//! the shown photo's full-size step (`preview::hold`, pinned while shown,
//! kept after), then the first opened again: its fit should read the kept
//! step, with no demosaic.
//!
//! Then a warmed neighbour (`--warm`: the first raw whose name holds it,
//! a NEF by default; the largest other raw shown): the shown one opened
//! cold while the neighbour warms (its `developed` state, as the desktop
//! warms a neighbour), background work free then held back from the
//! open's start to its preview; then the neighbour opened: its frame and
//! histogram times, the KPI of a warmed open.
//!
//! Writes `<out>/open-timing.jsonl` (one line per run) and
//! `<out>/open-timing.md` (the table).
//!
//! ```text
//! cargo run --release -p helios-gui --example open_timing -- \
//!     --corpus <dir> --out <dir> --runs 3
//! ```

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use helios_core::trace::Record;
use helios_core::{Bus, Engine, Graph, NodeId, Region, Value as P};
use serde_json::json;

/// A row is unstable past this spread of its runs (open to fit).
const STABLE_SPREAD: f64 = 0.15;

struct Args {
    corpus: PathBuf,
    out: PathBuf,
    runs: usize,
    /// At most this many raws.
    n: usize,
    /// The warmed neighbour: the first raw whose name holds it (any
    /// case); the shown one is the largest other raw.
    warm: String,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        corpus: PathBuf::new(),
        out: PathBuf::from("."),
        runs: 3,
        n: usize::MAX,
        warm: ".nef".into(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--corpus" => a.corpus = value()?.into(),
            "--out" => a.out = value()?.into(),
            "--runs" => a.runs = value()?.parse().map_err(|e| format!("--runs: {e}"))?,
            "--n" => a.n = value()?.parse().map_err(|e| format!("--n: {e}"))?,
            "--warm" => a.warm = value()?.to_lowercase(),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.corpus.as_os_str().is_empty() {
        return Err("--corpus is missing".into());
    }
    a.runs = a.runs.max(1);
    Ok(a)
}

/// One open of one raw: milliseconds from the open's start, unless said.
#[derive(Clone, Default)]
struct Run {
    decode: f64,
    graph: f64,
    ready: f64,
    proxy: f64,
    preview: f64,
    histogram: f64,
    fit: f64,
    /// The demosaic calls: how many, their total time.
    demosaics: usize,
    demosaic: f64,
    /// The first demosaic's detail (algorithm, size).
    algorithm: String,
    slider_fit: f64,
    slider_preview: f64,
}

fn main() {
    let a = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("open_timing: {e}");
            std::process::exit(2);
        }
    };
    let mut files = raws(&a.corpus);
    if files.is_empty() {
        eprintln!("open_timing: no raw in {}", a.corpus.display());
        std::process::exit(1);
    }
    // The largest first: the most megapixels among them.
    files.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).map_or(0, |m| m.len())));
    if let Err(e) = std::fs::create_dir_all(&a.out) {
        eprintln!("open_timing: {}: {e}", a.out.display());
        std::process::exit(1);
    }
    let bundle =
        helios_gui::templates::find("helios.develop").expect("the helios.develop template");
    let gpu = if std::env::var_os("OPEN_TIMING_NO_GPU").is_some() {
        None
    } else {
        helios_gpu::shared::gpu()
            .inspect_err(|e| eprintln!("open_timing: no GPU: {e}"))
            .ok()
    };
    let adapter = gpu.as_ref().map(|g| g.adapter.clone());
    let threads = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    let (_listener, records) = helios_core::trace::collect();
    let jsonl = a.out.join("open-timing.jsonl");
    let mut lines =
        std::fs::File::create(&jsonl).unwrap_or_else(|e| panic!("{}: {e}", jsonl.display()));
    let mut rows = Vec::new();
    for path in files.iter().take(a.n) {
        let name = path
            .file_name()
            .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
        let _ = std::fs::read(path); // the page cache, as a library on a local disk
        let mut runs = Vec::new();
        let mut mp = 0.0;
        for i in 0..a.runs {
            records
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
            let engine = Arc::new(Engine::new(Arc::new(Bus::new())));
            if let Some(g) = &gpu {
                engine.attach_accelerator(g.clone());
            }
            match open(path, &bundle, &engine) {
                Ok((mut run, extent)) => {
                    mp = f64::from(extent.width) * f64::from(extent.height) / 1e6;
                    let spans = records
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clone();
                    from_spans(&mut run, &spans);
                    let line = json!({
                        "file": name, "run": i, "mp": round(mp),
                        "decode_ms": run.decode, "graph_ms": run.graph, "ready_ms": run.ready,
                        "proxy_ms": run.proxy, "preview_ms": run.preview, "histogram_ms": run.histogram,
                        "fit_ms": run.fit, "demosaic_calls": run.demosaics, "demosaic_ms": run.demosaic,
                        "demosaic": run.algorithm,
                        "slider_fit_ms": run.slider_fit, "slider_preview_ms": run.slider_preview,
                    });
                    eprintln!("{line}");
                    writeln!(lines, "{line}").expect("the results file");
                    runs.push(run);
                }
                Err(e) => {
                    let line = json!({"file": name, "run": i, "error": e});
                    eprintln!("{line}");
                    writeln!(lines, "{line}").expect("the results file");
                    break;
                }
            }
        }
        if !runs.is_empty() {
            rows.push((name, mp, runs));
        }
    }
    let mut table = table(&rows, a.runs, adapter.as_deref(), threads);
    if files.len() >= 2 {
        let mid = files.len() / 2;
        let (first, second) = (&files[mid - 1], &files[mid]);
        let line = match back_to_first(first, second, &bundle, gpu.as_ref(), &records) {
            Ok(line) => line,
            Err(e) => json!({"scenario": "back_to_first", "error": e}),
        };
        eprintln!("{line}");
        writeln!(lines, "{line}").expect("the results file");
        let _ = writeln!(
            table,
            "\nBack to the first photo ({} then {}, 1 GiB budgets): first fit {} ms, its fit again {} ms with {} demosaic call(s).",
            line["first"].as_str().unwrap_or("?"),
            line["second"].as_str().unwrap_or("?"),
            line["first_fit_ms"],
            line["back_fit_ms"],
            line["back_demosaic_calls"],
        );
    }
    if files.len() >= 2 {
        // The neighbour warmed: the first raw named after `--warm` (a
        // Nikon's by default, as the journey's), else the second largest;
        // the largest other raw shown.
        let named = |p: &PathBuf| {
            p.file_name()
                .is_some_and(|f| f.to_string_lossy().to_lowercase().contains(&a.warm))
        };
        let near = files.iter().find(|p| named(p)).unwrap_or(&files[1]);
        let shown = files.iter().find(|p| *p != near).unwrap_or(&files[0]);
        for hold in [false, true] {
            let line = match warmed_neighbour(shown, near, &bundle, gpu.as_ref(), hold, &records) {
                Ok(line) => line,
                Err(e) => json!({"scenario": "warmed_neighbour", "error": e}),
            };
            eprintln!("{line}");
            writeln!(lines, "{line}").expect("the results file");
            let _ = writeln!(
                table,
                "\nWarmed neighbour ({} shown cold while {} warms, background {}): shown fit {} ms, histogram {} ms; the neighbour's warm-up {} ms; then the neighbour opened: frame (fit) {} ms, histogram {} ms, {} demosaic call(s).",
                line["shown"].as_str().unwrap_or("?"),
                line["near"].as_str().unwrap_or("?"),
                if hold {
                    "held back until the shown preview"
                } else {
                    "free"
                },
                line["shown_fit_ms"],
                line["shown_histogram_ms"],
                line["warm_ms"],
                line["warmed_fit_ms"],
                line["warmed_histogram_ms"],
                line["warmed_demosaic_calls"],
            );
        }
    }
    print!("{table}");
    let md = a.out.join("open-timing.md");
    std::fs::write(&md, table).unwrap_or_else(|e| panic!("{}: {e}", md.display()));
}

/// Opens `first`, then `second`, then `first` again on one engine, the
/// shown photo's full-size step held as the desktop's preview follower
/// holds it: the return's fit time and demosaic calls.
fn back_to_first(
    first: &Path,
    second: &Path,
    bundle: &helios_template::Bundle,
    gpu: Option<&Arc<helios_gpu::Gpu>>,
    records: &std::sync::Mutex<Vec<Record>>,
) -> Result<serde_json::Value, String> {
    use helios_node_core::image_states::ImageStates;
    const GIB: u64 = 1 << 30;
    let engine = Arc::new(Engine::with_budget(
        Arc::new(Bus::new()),
        Some(GIB as usize),
    ));
    if let Some(g) = gpu {
        engine.attach_accelerator(g.clone());
    }
    let states = ImageStates::new(
        engine.bus().clone(),
        helios_node_core::image_states::THREADS,
    );
    states.set_budget(Some(GIB));
    let mut pin = None;
    let mut show = |path: &Path| -> Result<(Arc<Graph>, NodeId, Region, f64), String> {
        let start = Instant::now();
        let (graph, node) = helios_gui::templates::develop_file(path, bundle)?;
        let extent = graph
            .extent(node)
            .map_err(|e| e.to_string())?
            .ok_or("no extent")?;
        let graph = Arc::new(graph);
        let held = helios_gui::preview::hold(
            &states,
            &engine,
            &graph,
            node,
            "output",
            extent,
            helios_core::Priority::Interactive,
        )
        .map_err(|e| e.to_string())?;
        let (_, fit, _, _) = concurrent(&engine, &graph, node, extent, true, start);
        if let Some(mut held) = held {
            held.wait(std::time::Duration::from_mins(10))
                .map_err(|e| e.to_string())?;
            // The new pin first, then the old one goes.
            pin = Some(states.pin(held.key()));
        }
        Ok((graph, node, extent, fit))
    };
    let (graph, node, extent, first_fit) = show(first)?;
    show(second)?;
    records
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    let start = Instant::now();
    let (_, back_fit, _, _) = concurrent(&engine, &graph, node, extent, true, start);
    let mut run = Run::default();
    from_spans(
        &mut run,
        &records.lock().unwrap_or_else(PoisonError::into_inner),
    );
    let name = |p: &Path| {
        p.file_name()
            .map_or_else(String::new, |f| f.to_string_lossy().into_owned())
    };
    Ok(json!({
        "scenario": "back_to_first", "first": name(first), "second": name(second),
        "first_fit_ms": first_fit, "back_fit_ms": back_fit,
        "back_demosaic_calls": run.demosaics, "states": states.usage(),
        "evicted_states": states.counters().evicted,
    }))
}

/// The desktop's warm-up of a neighbour, timed: `shown` opened cold (its
/// `decoded` state asked on the open's thread) while `near`'s `developed`
/// state is asked at Prefetch, as the warmer asks it once the neighbour
/// settles (`hold`: background work held back from the open's start to
/// its preview, as the desktop holds it); then `near` opened as a warmed
/// photo: its frame and histogram times, its demosaic calls.
fn warmed_neighbour(
    shown: &Path,
    near: &Path,
    bundle: &helios_template::Bundle,
    gpu: Option<&Arc<helios_gpu::Gpu>>,
    hold: bool,
    records: &std::sync::Mutex<Vec<Record>>,
) -> Result<serde_json::Value, String> {
    use helios_node_core::image_states::ImageStates;
    let engine = Arc::new(Engine::new(Arc::new(Bus::new())));
    if let Some(g) = gpu {
        engine.attach_accelerator(g.clone());
    }
    let states = ImageStates::new(
        engine.bus().clone(),
        helios_node_core::image_states::THREADS,
    );
    let name = |p: &Path| {
        p.file_name()
            .map_or_else(String::new, |f| f.to_string_lossy().into_owned())
    };
    let wait = std::time::Duration::from_mins(10);
    let open = |path: &Path, start: Instant| -> Result<(f64, f64, f64), String> {
        let held = hold.then(|| states.hold_background());
        let (graph, node) = helios_gui::warm::open(&states, &name(path), path, bundle)?;
        let decoded = ms(start);
        let extent = graph
            .extent(node)
            .map_err(|e| e.to_string())?
            .ok_or("no extent")?;
        let graph = Arc::new(graph);
        let (_, fit, _, histogram) = concurrent_held(&engine, &graph, node, extent, start, held);
        Ok((decoded, fit, histogram))
    };
    // The neighbour's warm-up, asked as the open starts.
    let decoded = (
        helios_gui::warm::key(&name(near), near, bundle),
        helios_gui::warm::compute(near.to_path_buf(), bundle.clone()),
    );
    let key = helios_gui::warm::developed_key(&decoded.0, None);
    let compute = helios_gui::warm::developed(&states, decoded, engine.clone(), None);
    let start = Instant::now();
    let mut warm = states.ask(key, helios_core::Priority::Prefetch, compute);
    let (_, shown_fit, shown_histogram) = open(shown, start)?;
    warm.wait(wait).map_err(|e| e.to_string())?;
    let warm_ms = ms(start);
    records
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    let start = Instant::now();
    let (warmed_decoded, warmed_fit, warmed_histogram) = open(near, start)?;
    let mut run = Run::default();
    from_spans(
        &mut run,
        &records.lock().unwrap_or_else(PoisonError::into_inner),
    );
    Ok(json!({
        "scenario": "warmed_neighbour", "hold": hold, "shown": name(shown), "near": name(near),
        "shown_fit_ms": round(shown_fit), "shown_histogram_ms": round(shown_histogram),
        "warm_ms": round(warm_ms), "warmed_decoded_ms": round(warmed_decoded),
        "warmed_fit_ms": round(warmed_fit), "warmed_histogram_ms": round(warmed_histogram),
        "warmed_demosaic_calls": run.demosaics, "states": states.usage(),
    }))
}

/// [`concurrent`] with a proxy, dropping `held` (the open's hold on
/// background work) once the preview and its histogram are done.
fn concurrent_held(
    engine: &Arc<Engine>,
    graph: &Arc<Graph>,
    node: NodeId,
    extent: Region,
    start: Instant,
    held: Option<helios_node_core::image_states::BackgroundHold>,
) -> (f64, f64, f64, f64) {
    let release = std::sync::Mutex::new(held);
    concurrent_then(engine, graph, node, extent, true, start, &|| {
        drop(
            release
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
    })
}

/// Opens `path` on `engine` as the desktop does, and times it.
fn open(
    path: &Path,
    bundle: &helios_template::Bundle,
    engine: &Arc<Engine>,
) -> Result<(Run, Region), String> {
    let start = Instant::now();
    let (graph, node) = helios_gui::templates::develop_file(path, bundle)?;
    let ready = ms(start);
    let extent = graph
        .extent(node)
        .map_err(|e| e.to_string())?
        .ok_or("no extent")?;
    let graph = Arc::new(graph);
    let (proxy, fit, preview, histogram) = concurrent(engine, &graph, node, extent, true, start);
    let mut run = Run {
        ready,
        proxy,
        preview,
        histogram,
        fit,
        ..Run::default()
    };
    // One slider, on the warm engine.
    let mut moved = (*graph).clone();
    if let Some(id) = moved.find("exposure") {
        moved
            .set_param(id, "exposure", P::Float(0.5))
            .map_err(|e| e.to_string())?;
        let at = Instant::now();
        let (_, fit, preview, _) = concurrent(engine, &Arc::new(moved), node, extent, false, at);
        run.slider_fit = fit;
        run.slider_preview = preview;
    }
    Ok((run, extent))
}

/// The viewport's thread (a proxy unless `with_proxy` is false, then the
/// fit) and the preview's thread at once, the histogram on the preview:
/// (proxy, fit, preview, histogram) in ms from `start`.
fn concurrent(
    engine: &Arc<Engine>,
    graph: &Arc<Graph>,
    node: NodeId,
    extent: Region,
    with_proxy: bool,
    start: Instant,
) -> (f64, f64, f64, f64) {
    concurrent_then(engine, graph, node, extent, with_proxy, start, &|| {})
}

/// [`concurrent`], calling `histogram_done` once the preview's histogram
/// is computed.
fn concurrent_then(
    engine: &Arc<Engine>,
    graph: &Arc<Graph>,
    node: NodeId,
    extent: Region,
    with_proxy: bool,
    start: Instant,
    histogram_done: &(dyn Fn() + Sync),
) -> (f64, f64, f64, f64) {
    // The viewport at 1920×1080: the fit scale, and a proxy at a power of
    // two below a quarter of it.
    let fit = (1920.0 / f64::from(extent.width))
        .min(1080.0 / f64::from(extent.height))
        .min(1.0);
    let mut proxy = 1.0;
    while proxy > fit / 4.0 {
        proxy /= 2.0;
    }
    // Placed as the viewport places it (`engine_source::region_of`): a
    // turned image's extent starts off the origin.
    let at = move |s: f64| {
        Region::new(
            (extent.x as f64 * s).floor() as i64,
            (extent.y as f64 * s).floor() as i64,
            ((f64::from(extent.width) * s).ceil() as u32).max(1),
            ((f64::from(extent.height) * s).ceil() as u32).max(1),
        )
    };
    // The preview is a state of the image-state router, as in the desktop.
    let states = helios_node_core::image_states::ImageStates::new(
        engine.bus().clone(),
        helios_node_core::image_states::THREADS,
    );
    let (e1, g1) = (engine.clone(), graph.clone());
    let view = std::thread::Builder::new()
        .name("open-timing-viewport".into())
        .spawn(move || {
            let mut p = 0.0;
            if with_proxy {
                let _ = e1.render_at(&g1, node, "output", at(proxy), proxy);
                p = ms(start);
            }
            let _ = e1.render_at(&g1, node, "output", at(fit), fit);
            (p, ms(start))
        })
        .expect("a viewport thread");
    let (e2, g2) = (engine.clone(), graph.clone());
    let preview = std::thread::Builder::new()
        .name("open-timing-preview".into())
        .spawn(move || {
            let image = helios_gui::preview::ask(
                &states,
                &e2,
                &g2,
                node,
                "output",
                extent,
                helios_core::Priority::Interactive,
            )
            .ok()
            .and_then(|mut h| h.wait(std::time::Duration::from_mins(10)).ok())
            .and_then(helios_gui::preview::image);
            let at = ms(start);
            if let Some(image) = &image {
                let _ = helios_scopes::histogram(image, 256, helios_scopes::Scale::default());
            }
            (at, ms(start))
        })
        .expect("a preview thread");
    let (pv, hist) = preview.join().expect("the preview thread");
    histogram_done();
    let (px, ft) = view.join().expect("the viewport thread");
    (px, ft, pv, hist)
}

/// What the engine's spans say: the decode, the graph, the demosaic.
fn from_spans(run: &mut Run, spans: &[Record]) {
    // Only the open's own spans: the slider's run computes no demosaic
    // (its input is cached), and decode / graph happen once.
    for r in spans {
        match r.name {
            "photo.decode" => run.decode += r.ms(),
            "photo.graph" => run.graph += r.ms(),
            "demosaic" => {
                run.demosaics += 1;
                run.demosaic += r.ms();
                if run.algorithm.is_empty() {
                    run.algorithm = r.detail.split_whitespace().next().unwrap_or("").into();
                }
            }
            _ => {}
        }
    }
    run.decode = (run.decode * 10.0).round() / 10.0;
    run.graph = (run.graph * 10.0).round() / 10.0;
    run.demosaic = (run.demosaic * 10.0).round() / 10.0;
}

/// The median of `f` over the runs.
fn median(runs: &[Run], f: impl Fn(&Run) -> f64) -> f64 {
    let mut v: Vec<f64> = runs.iter().map(f).collect();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        f64::midpoint(v[n / 2 - 1], v[n / 2])
    }
}

/// The table: medians per raw, and whether the runs agreed.
fn table(
    rows: &[(String, f64, Vec<Run>)],
    runs: usize,
    adapter: Option<&str>,
    threads: usize,
) -> String {
    let mut out = String::new();
    let mut stable = true;
    let mut body = String::new();
    for (name, mp, rs) in rows {
        let fits: Vec<f64> = rs.iter().map(|r| r.fit).collect();
        let lo = fits.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = fits.iter().copied().fold(0.0, f64::max);
        let mid = median(rs, |r| r.fit);
        let spread = if mid > 0.0 { (hi - lo) / mid } else { 0.0 };
        let ok = spread <= STABLE_SPREAD;
        stable &= ok;
        let m = |f: fn(&Run) -> f64| format!("{:.0}", median(rs, f));
        let _ = writeln!(
            body,
            "| {name} | {mp:.1} | {} | {} | {} | {} | {} | {} | {} | {} | {} × {} | {} / {} | {:.0} %{} |",
            rs[0].algorithm,
            m(|r| r.decode),
            m(|r| r.graph),
            m(|r| r.ready),
            m(|r| r.proxy),
            m(|r| r.preview),
            m(|r| r.histogram),
            m(|r| r.fit),
            rs[0].demosaics,
            m(|r| r.demosaic),
            m(|r| r.slider_fit),
            m(|r| r.slider_preview),
            spread * 100.0,
            if ok { "" } else { " unstable" },
        );
    }
    let _ = writeln!(
        out,
        "Open timing: median of {runs} runs per raw, ms from the open's start; {threads} threads; accelerator {}; runs {}.\n",
        adapter.unwrap_or("none"),
        if stable {
            "stable (every raw within 15 %)"
        } else {
            "NOT stable (a raw spread past 15 %)"
        },
    );
    out.push_str(
        "| raw | MP | demosaic | decode | graph build | graph ready | proxy | preview | histogram | fit | demosaic calls × ms | slider fit / preview | spread |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    out.push_str(&body);
    out
}

/// Milliseconds since `t`, to the microsecond.
fn ms(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

fn round(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// The corpus's raw files (the manifest and checksums left out).
fn raws(dir: &Path) -> Vec<PathBuf> {
    const RAW: &[&str] = &[
        "3fr", "ari", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "iiq", "kdc", "mef", "mos",
        "mrw", "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
    ];
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in read.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| RAW.contains(&x.to_ascii_lowercase().as_str()))
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}
