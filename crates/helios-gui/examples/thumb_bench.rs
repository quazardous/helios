//! The thumbnail benchmark (docs/performance.md): what the library's
//! thumbnails cost today, measured against the budgets.
//!
//! Two parts, each a process of its own so peak memory is its own:
//!
//! - `--part grid --n <N> [--start cold|warm]`: a library of N images made
//!   by repeating the raw corpus (entries that refer to its files: nothing
//!   is copied), shown in the Library panel of a headless shell, with the
//!   thumbnails read as `helios-gui` reads them (`helios_gui::thumbnails`:
//!   the thumbnail cache, a pool of threads, the cells in view first).
//!   A cold start empties the cache first (`<out>/thumb-cache`); a warm
//!   one fills it first with every image's entry, as after a first run.
//!   Measured: the time to the first screen of thumbnails, the frame
//!   times while the grid scrolls, the thumbnails read per second, peak
//!   RSS and the thumbnails' textures.
//! - `--part decode`: per corpus raw, the cost of its embedded preview
//!   (`helios_load::preview`, what the library read before the cache, and
//!   `glance`, what the import grid reads), the fast read of the cache
//!   (`helios_thumbs::read`: the smallest embedded JPEG at least 256 px,
//!   DCT-scaled, 8-bit) and a cache hit, the size of a 256 px cache entry,
//!   and the cost of a 256 px render of its edit through the workspace's
//!   network (`head_render::render`).
//!
//! Each result is a JSON line appended to `<out>/thumb-bench.jsonl`.
//!
//! ```text
//! cargo run --release -p helios-gui --example thumb_bench -- \
//!     --corpus <dir> --out <dir> --part grid --n 10000
//! ```

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Event, Modifiers, MouseWheelSource, MouseWheelUnit, TouchPhase, vec2};
use helios_gui::library::Entry;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_thumbs::{Key, SMALL, ThumbCache};
use serde_json::{Value, json};

/// The window the grid is drawn in.
const WINDOW: [f32; 2] = [1920.0, 1080.0];
/// How long the first screen may take before the run gives up on it.
const FIRST_SCREEN_DEADLINE: Duration = Duration::from_mins(2);

struct Args {
    corpus: PathBuf,
    out: PathBuf,
    part: String,
    n: usize,
    frames: usize,
    renders: usize,
    /// The grid's start: `cold` (an empty cache) or `warm` (filled).
    start: String,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        corpus: PathBuf::new(),
        out: PathBuf::from("."),
        part: "grid".into(),
        n: 1000,
        frames: 240,
        renders: 6,
        start: "cold".into(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--corpus" => a.corpus = value()?.into(),
            "--out" => a.out = value()?.into(),
            "--part" => a.part = value()?,
            "--n" => a.n = value()?.parse().map_err(|e| format!("--n: {e}"))?,
            "--frames" => a.frames = value()?.parse().map_err(|e| format!("--frames: {e}"))?,
            "--renders" => a.renders = value()?.parse().map_err(|e| format!("--renders: {e}"))?,
            "--start" => a.start = value()?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.corpus.as_os_str().is_empty() {
        return Err("--corpus is missing".into());
    }
    Ok(a)
}

fn main() {
    let a = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("thumb_bench: {e}");
            std::process::exit(2);
        }
    };
    let files = raws(&a.corpus);
    if files.is_empty() {
        eprintln!("thumb_bench: no raw in {}", a.corpus.display());
        std::process::exit(1);
    }
    let lines = match a.part.as_str() {
        "grid" => vec![grid(
            &files,
            a.n,
            a.frames,
            &a.out.join("thumb-cache"),
            &a.start,
        )],
        "decode" => decode(&files, a.renders, &a.out.join("thumb-cache-decode")),
        other => {
            eprintln!("thumb_bench: unknown part {other}");
            std::process::exit(2);
        }
    };
    if let Err(e) = std::fs::create_dir_all(&a.out) {
        eprintln!("thumb_bench: {}: {e}", a.out.display());
        std::process::exit(1);
    }
    let path = a.out.join("thumb-bench.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    for line in lines {
        println!("{line}");
        writeln!(file, "{line}").expect("the results file");
    }
}

/// The corpus's raw files, sorted (the manifest and checksums left out).
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

/// This process's resident memory and its peak, in bytes (Linux).
fn rss() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .map_or(0, |kb| kb * 1024)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

/// How many images of the library have their thumbnail (or placeholder).
fn shown(h: &egui_kittest::Harness<'_, Shell>) -> usize {
    let lib = h.state().library();
    lib.entries
        .iter()
        .filter(|e| lib.has_thumbnail(&e.id))
        .count()
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e6).round() / 1e3
}

/// The given percentile of sorted values.
fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

fn stats(mut v: Vec<f64>) -> Value {
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
    json!({
        "count": v.len(),
        "mean": (mean * 1e3).round() / 1e3,
        "p50": pct(&v, 0.5),
        "p95": pct(&v, 0.95),
        "p99": pct(&v, 0.99),
        "max": v.last().copied().unwrap_or(0.0),
    })
}

/// The cache for the grid: emptied for a cold start, filled with every
/// image's entry (its file's fast read, written once per entry) for a warm
/// one.
fn grid_cache(files: &[PathBuf], n: usize, dir: &Path, warm: bool) -> Arc<ThumbCache> {
    let _ = std::fs::remove_dir_all(dir);
    let cache = Arc::new(ThumbCache::open(dir, u64::MAX).expect("the thumbnail cache"));
    if warm {
        let jpegs: Vec<Option<Vec<u8>>> = files
            .iter()
            .map(|f| {
                let rgb = helios_thumbs::read::read(f, SMALL).ok().flatten()?;
                let key = Key::preview("probe");
                cache.put(&key, SMALL, &rgb).ok()?;
                std::fs::read(cache.dir().join(cache.file_of(&key, SMALL))).ok()
            })
            .collect();
        for i in 0..n {
            if let Some(jpeg) = &jpegs[i % files.len()] {
                let _ = cache.put_jpeg(&Key::preview(format!("{i:016x}")), SMALL, jpeg);
            }
        }
        // Opened again as the desktop opens it at start: its folder
        // listed by the pool's threads, not here.
        drop(cache);
        return Arc::new(ThumbCache::open_unlisted(dir, u64::MAX).expect("the thumbnail cache"));
    }
    cache
}

/// The library grid with N images.
fn grid(files: &[PathBuf], n: usize, frames: usize, cache_dir: &Path, start_kind: &str) -> Value {
    let warm = start_kind == "warm";
    let fill = Instant::now();
    let cache = grid_cache(files, n, cache_dir, warm);
    let filled = fill.elapsed();
    let start = Instant::now();
    let entries: Vec<Entry> = (0..n)
        .map(|i| {
            let file = &files[i % files.len()];
            Entry {
                id: format!("{i:016x}"),
                name: file
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                bytes: std::fs::metadata(file).ok().map(|m| m.len()),
                ..Entry::default()
            }
        })
        .collect();
    let mut shell = Shell::new(helios_panels(), None);
    let built = start.elapsed();
    shell.set_library(entries);
    let set = start.elapsed().saturating_sub(built);

    // The reader, as helios-gui's: the cache first, a pool of threads
    // reading the misses, the cells in view first; a file with no preview
    // shows its placeholder.
    let paths: Vec<(String, PathBuf)> = (0..n)
        .map(|i| (format!("{i:016x}"), files[i % files.len()].clone()))
        .collect();
    let loader = helios_gui::thumbnails::start(&shell, Some(cache.clone()), &paths, |_| {});

    let mut h = headless::harness(shell, vec2(WINDOW[0], WINDOW[1]));
    h.state_mut()
        .run("helios.vault.focus", Value::Null, Origin::User)
        .expect("the Library panel");
    let mut first_frame = None;
    let mut first_screen = None;
    let mut cells = 0;
    let mut ui_frames = Vec::new();
    // The thumbnails taken in at each frame until the first screen.
    let mut per_frame = Vec::new();
    let mut had = 0;
    while start.elapsed() < FIRST_SCREEN_DEADLINE {
        let t = Instant::now();
        h.step();
        ui_frames.push(ms(t.elapsed()));
        let now = shown(&h);
        per_frame.push(now.saturating_sub(had) as f64);
        had = now;
        let lib = h.state().library();
        let in_view = lib.in_view();
        if !in_view.is_empty() {
            first_frame.get_or_insert(start.elapsed());
            cells = in_view.len();
            if in_view.iter().all(|id| lib.has_thumbnail(id)) {
                first_screen = Some(start.elapsed());
                break;
            }
        }
        drop(lib);
        std::thread::sleep(Duration::from_millis(2));
    }
    let read_at_first_screen = shown(&h);
    // The picture once, so the renderer holds the textures uploaded.
    let t = Instant::now();
    let rendered = h.render().is_ok();
    let first_render = t.elapsed();

    // Scrolling: the wheel over the grid, a fast flick each frame.
    let rect = h
        .state()
        .target_rect("helios.library.grid")
        .expect("the grid's rect");
    h.event(Event::PointerMoved(rect.center()));
    h.step();
    let (mut ui_ms, mut render_ms, mut ready) = (Vec::new(), Vec::new(), Vec::new());
    let mut reached = 0usize;
    let scroll_start = Instant::now();
    for _ in 0..frames {
        h.event(Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, -400.0),
            phase: TouchPhase::Move,
            source: MouseWheelSource::Wheel,
            modifiers: Modifiers::NONE,
        });
        let t = Instant::now();
        h.step();
        ui_ms.push(ms(t.elapsed()));
        let t = Instant::now();
        let _ = h.render();
        render_ms.push(ms(t.elapsed()));
        let lib = h.state().library();
        let in_view = lib.in_view();
        let have = in_view.iter().filter(|id| lib.has_thumbnail(id)).count();
        reached = in_view
            .iter()
            .filter_map(|id| usize::from_str_radix(id, 16).ok())
            .fold(reached, usize::max);
        ready.push(have as f64 / in_view.len().max(1) as f64);
    }
    let scrolled = scroll_start.elapsed();
    let reading = start.elapsed();
    let (queued, _) = loader.queued();
    drop(loader);
    // The last thumbnails sent taken in, as the window would.
    h.step();
    let previews = n - queued;
    let (textures, texture_bytes) = h.state().library().thumbnail_textures();
    let all_textures: u64 = h
        .ctx
        .tex_manager()
        .read()
        .allocated()
        .map(|(_, m)| (m.size[0] * m.size[1] * m.bytes_per_pixel) as u64)
        .sum();
    let (rss_now, rss_peak) = rss();
    let per_texture = texture_bytes as f64 / textures.max(1) as f64;
    let rate = previews as f64 / reading.as_secs_f64().max(1e-9);
    json!({
        "part": "grid",
        "n": n,
        "start": start_kind,
        "cache_fill_ms": warm.then(|| ms(filled)),
        "cache_bytes": cache.usage().0,
        "threads": helios_thumbs::workers(),
        "corpus_files": files.len(),
        "window": WINDOW,
        "shell_ms": ms(built),
        "set_library_ms": ms(set),
        "first_frame_ms": first_frame.map(ms),
        "first_screen_ms": first_screen.map(ms),
        "first_screen_cells": cells,
        "previews_read_at_first_screen": read_at_first_screen,
        "frames_to_first_screen": ui_frames.len(),
        "first_frames_ui_ms": stats(ui_frames),
        "first_frames_thumbnails_taken": stats(per_frame),
        "first_render_ms": rendered.then(|| ms(first_render)),
        "scroll": {
            "frames": frames,
            "points_per_frame": 400,
            "seconds": scrolled.as_secs_f64(),
            "ui_ms": stats(ui_ms),
            "render_ms": stats(render_ms),
            "in_view_ready_mean": stats(ready)["mean"],
            "last_image_reached": reached,
        },
        "previews_read": previews,
        "previews_per_second": (rate * 10.0).round() / 10.0,
        "all_read_projected_s": (n as f64 / rate.max(1e-9)).round(),
        "thumbnail_textures": textures,
        "thumbnail_texture_bytes": texture_bytes,
        "bytes_per_thumbnail_texture": per_texture.round(),
        "all_textures_projected_bytes": (per_texture * n as f64).round(),
        "egui_texture_bytes": all_textures,
        "rss_bytes": rss_now,
        "rss_peak_bytes": rss_peak,
    })
}

/// Per corpus raw: its previews' costs, a cache entry's size, and a
/// render of its edit at 256 px (the first `renders` raws).
fn decode(files: &[PathBuf], renders: usize, cache_dir: &Path) -> Vec<Value> {
    use helios_core::{Bus, Engine};
    use helios_vault::MemoryStore;
    let mut lines = Vec::new();
    let (mut preview_ms, mut glance_ms, mut jpeg_bytes, mut render_ms) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut fast_ms, mut hit_ms, mut entry_bytes) = (Vec::new(), Vec::new(), Vec::new());
    let _ = std::fs::remove_dir_all(cache_dir);
    let cache = ThumbCache::open(cache_dir, u64::MAX).expect("the thumbnail cache");
    let persist = helios_gui::persist::Persist::new(MemoryStore::default(), Arc::new(Bus::new()));
    let (vault, reader) = (persist.vault().clone(), persist.reader());
    let engine = Engine::new(Arc::new(Bus::new()));
    let bundle = helios_gui::templates::find("helios.develop-preview").expect("the template");
    for (i, path) in files.iter().enumerate() {
        let name = path.file_name().map(|f| f.to_string_lossy().into_owned());
        let bytes = std::fs::metadata(path).map_or(0, |m| m.len());
        // Twice: the second read comes from the page cache, as a library
        // on a local disk mostly does.
        let _ = helios_load::preview(path);
        let t = Instant::now();
        let preview = helios_load::preview(path);
        let small = preview
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map(helios_gui::library::small);
        let p = t.elapsed();
        let t = Instant::now();
        let glance = helios_load::glance(path);
        let g = t.elapsed();
        let preview_size = preview
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map(|im| [im.region().width, im.region().height]);
        let glance_size = glance
            .as_ref()
            .ok()
            .and_then(|g| g.preview.as_ref())
            .map(|im| [im.region().width, im.region().height]);
        let jpeg = small.as_ref().and_then(|s| {
            helios_export::encode(s, &helios_export::Format::Jpeg { quality: 85 })
                .ok()
                .map(|b| b.len())
        });
        if preview_size.is_some() {
            preview_ms.push(ms(p));
        }
        if glance_size.is_some() {
            glance_ms.push(ms(g));
        }
        if let Some(j) = jpeg {
            jpeg_bytes.push(j as f64);
        }
        // The cache's fast read (a miss: the file in the page cache), then
        // a hit.
        let _ = helios_thumbs::read::read(path, SMALL);
        let t = Instant::now();
        let fast = helios_thumbs::read::read(path, SMALL).ok().flatten();
        let f = t.elapsed();
        let key = Key::preview(format!("{i:016x}"));
        let (mut hit, mut entry) = (None, None);
        if let Some(rgb) = &fast {
            fast_ms.push(ms(f));
            if cache.put(&key, SMALL, rgb).is_ok() {
                entry = std::fs::metadata(cache.dir().join(cache.file_of(&key, SMALL)))
                    .ok()
                    .map(|m| m.len());
                let t = Instant::now();
                let got = cache.get(&key, SMALL);
                let d = t.elapsed();
                if got.is_some() {
                    hit_ms.push(ms(d));
                    hit = Some(ms(d));
                }
            }
        }
        if let Some(e) = entry {
            entry_bytes.push(e as f64);
        }
        let mut render = None;
        if i < renders {
            let t = Instant::now();
            let r = vault.import_file(path, 0).and_then(|image| {
                helios_gui::head_render::render(&reader, &engine, &bundle, path, &image.hex(), 256)
            });
            let d = t.elapsed();
            render = Some(match r {
                Ok((_, im)) => {
                    render_ms.push(ms(d));
                    json!({ "ms": ms(d), "size": [im.region().width, im.region().height] })
                }
                Err(e) => json!({ "error": e }),
            });
        }
        lines.push(json!({
            "part": "decode",
            "file": name,
            "bytes": bytes,
            "preview_ms": ms(p),
            "preview_size": preview_size,
            "preview_error": preview.err(),
            "glance_ms": ms(g),
            "glance_size": glance_size,
            "glance_error": glance.err(),
            "jpeg256_bytes": jpeg,
            "fast_ms": fast.as_ref().map(|_| ms(f)),
            "fast_size": fast.as_ref().map(|r| [r.width, r.height]),
            "cache_hit_ms": hit,
            "cache_entry_bytes": entry,
            "render256": render,
        }));
    }
    let jpeg_mean = jpeg_bytes.iter().sum::<f64>() / jpeg_bytes.len().max(1) as f64;
    let (_, rss_peak) = rss();
    lines.push(json!({
        "part": "decode-summary",
        "files": files.len(),
        "preview_ms": stats(preview_ms),
        "glance_ms": stats(glance_ms),
        "jpeg256_bytes": stats(jpeg_bytes),
        "cache_bytes_per_10000": (jpeg_mean * 10_000.0).round(),
        "fast_ms": stats(fast_ms),
        "cache_hit_ms": stats(hit_ms),
        "cache_entry_bytes": stats(entry_bytes),
        "render256_ms": stats(render_ms),
        "rss_peak_bytes": rss_peak,
    }));
    lines
}
