//! A workshop run over a set of photos (experimental, see
//! `docs/design/workshops-on-the-fly.md`, slice 9), headless: 24
//! synthetic photos of varying backlight in a library, a workshop aiming
//! its brightness at a median with one value junction a fake AI answers.
//! The run flags the photos unlike the test photos, asks the AI once per
//! photo, writes each edit with its provenance as one library step; a
//! second run asks nothing and writes nothing new; `hold` and `stop`
//! keep photos unwritten; undo restores every edit but one edited after
//! the run. The review lists the photos to review first; a fix clears
//! its flag and records provenance `user`; a junction decided by an
//! agent goes into the decision book and a re-run takes it; a held photo
//! accepted, a written one reverted, from the panel too.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use helios_core::jobs::Jobs;
use helios_core::{Bus, Engine, Graph, Image, ModuleRef, Params, Region};
use helios_gui::library_history::LibraryHistory;
use helios_gui::templates;
use helios_gui::workshop_run::{Outcome, RunAsk, Runner};
use helios_template::format::{Junction, Measure, OnBreach};
use helios_template::run::{Asker, Recorded};
use helios_template::{Catalogue, parse_composite};
use helios_vault::{ImageId, MemoryStore, Vault, Version, edit};
use serde_json::{Value as Json, json};

const PHOTOS: usize = 24;
const WORKSHOP: &str = "test.backlit@1";
const NODE: &str = "ws";

/// Brightness aimed at a median of -1 stop; `lift` (the black level)
/// left to the AI, its fallback 0; limits from two test photos around
/// the middle of the set.
fn workshop() -> Json {
    json!({
        "format": 1,
        "id": "test.backlit",
        "version": 1,
        "name": "Backlit",
        "inputs": [ { "name": "input", "channels": 4, "layout": "rgba" } ],
        "outputs": [ { "name": "output", "channels": 4, "layout": "rgba" } ],
        "nodes": [ { "name": "exp", "module": "exposure" } ],
        "connections": [ { "from": "@", "port": "input", "to": "exp", "input": "input" } ],
        "exits": [ { "from": "exp", "port": "output", "output": "output" } ],
        "exposed": [
            { "name": "brightness", "node": "exp", "param": "exposure", "min": -3.0, "max": 3.0 },
            { "name": "lift", "node": "exp", "param": "black", "min": -0.1, "max": 0.1 }
        ],
        "workshop": {
            "intent": "Backlit: bring the photo up to a median of one stop under white",
            "targets": [ { "param": "brightness", "measure": "luma.median", "value": -1.0 } ],
            "junctions": [ { "kind": "value", "name": "lift", "fallback": 0.0 } ],
            "limits": {
                "sliders": {
                    "brightness": { "min": -3.0, "max": 3.0, "value": 1.7, "max_deviation": 0.8 }
                },
                "targets": { "brightness": { "tolerance": 0.1 } },
                "input_span": [ { "measure": "luma.median", "min": -3.4, "max": -2.0 } ],
                "junctions": { "lift": { "min_confidence": 0.7 } }
            }
        }
    })
}

fn catalogue() -> Catalogue {
    templates::modules().composite(parse_composite(&workshop().to_string()).unwrap())
}

/// Photo `i`'s backlight: its light from half to twice the middle one's.
fn light(i: usize) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let t = i as f32 / (PHOTOS - 1) as f32;
    (2f32).powf(2.0 * t - 1.0)
}

/// A bright sky band over a foreground of greys from 0.1 to 0.2, all
/// times the photo's light.
fn photo(i: usize) -> Image {
    let k = light(i);
    Image::from_fn(Region::sized(32, 32), 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let t = x as f32 / 31.0;
        if y < 12 {
            (0.5 + 0.4 * t) * k
        } else {
            (0.1 + 0.1 * t) * k
        }
    })
}

/// The fake AI: a small lift, sure of it except on the darkest photos
/// (what enters measures under -3.3 stops); it counts its calls.
struct Fake(Arc<Mutex<u64>>);

impl Asker for Fake {
    fn ask(&mut self, _: &Junction, input: &Image) -> Option<Recorded> {
        *self.0.lock().unwrap() += 1;
        let median = helios_template::workshop::measure(input, Measure::LumaMedian)?;
        Some(Recorded {
            value: Some(0.01),
            confidence: if median < -3.3 { 0.5 } else { 0.9 },
        })
    }
}

struct Library {
    vault: Arc<Vault<MemoryStore>>,
    runner: Arc<Runner<MemoryStore>>,
    history: LibraryHistory,
    calls: Arc<Mutex<u64>>,
    images: Vec<ImageId>,
    /// Each photo's version before any run.
    first: Vec<Version>,
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-run-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The photos' index from their file: `p<i>.bin`.
fn index(path: &Path) -> usize {
    path.file_stem().unwrap().to_str().unwrap()[1..]
        .parse()
        .unwrap()
}

/// The library: each photo a file (its bytes its index) and a first
/// version; a runner loading photo `i` as a source into the workshop.
fn library(name: &str) -> Library {
    let dir = temp(name);
    let bus = Arc::new(Bus::new());
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let mut images = Vec::new();
    let mut first = Vec::new();
    for i in 0..PHOTOS {
        let path = dir.join(format!("p{i}.bin"));
        std::fs::write(&path, format!("photo {i}")).unwrap();
        let id = vault.import_file(&path, 1).unwrap();
        let v = Version::new(id, vec![], json!({ "modules": [] }), "user", 1).labelled("first");
        vault.add_version(&v).unwrap();
        images.push(id);
        first.push(v);
    }
    let built = catalogue()
        .build_composite(
            &helios_template::network::composite_ref(WORKSHOP).unwrap(),
            0,
        )
        .unwrap();
    let history = LibraryHistory::default();
    let calls = Arc::new(Mutex::new(0));
    let counted = calls.clone();
    let runner = Arc::new(
        Runner::new(
            vault.clone(),
            Arc::new(Engine::new(bus.clone())),
            Jobs::new(bus),
            history.clone(),
        )
        .with_load(Arc::new(move |path, _| {
            let mut graph = Graph::new();
            let src = graph.add_source("input", photo(index(path)));
            let node = graph
                .add_module(NODE, ModuleRef::composite(built.clone()), &Params::new())
                .map_err(|e| e.to_string())?;
            graph
                .connect(src, helios_template::SOURCE_PORT, node, "input")
                .map_err(|e| e.to_string())?;
            Ok(graph)
        }))
        .with_askers(Arc::new(move || Box::new(Fake(counted.clone())))),
    );
    Library {
        vault,
        runner,
        history,
        calls,
        images,
        first,
    }
}

fn ask(lib: &Library, on_breach: Option<OnBreach>) -> RunAsk {
    RunAsk {
        node: NODE.into(),
        workshop: WORKSHOP.into(),
        name: "Backlit".into(),
        catalogue: catalogue(),
        bundle: helios_template::workspace::blank(),
        images: lib.images.clone(),
        on_breach,
        who: "user".into(),
    }
}

fn run(lib: &Library, on_breach: Option<OnBreach>) -> Outcome {
    lib.runner
        .run(&ask(lib, on_breach), "run", &AtomicBool::new(false), None)
        .unwrap()
}

fn head(lib: &Library, i: usize) -> Version {
    let (_, head) = edit::EditSession::open(&lib.vault, lib.images[i]).unwrap();
    head.unwrap()
}

fn flags(o: &Outcome, i: usize, lib: &Library) -> Vec<String> {
    let hex = lib.images[i].hex();
    o.flagged
        .iter()
        .chain(&o.held)
        .filter(|h| h.image == hex)
        .flat_map(|h| {
            h.flags
                .iter()
                .map(|r| json!(r.flag).as_str().unwrap().to_string())
        })
        .collect()
}

#[test]
fn a_run_flags_asks_once_per_photo_writes_one_step_and_a_second_run_asks_nothing() {
    let lib = library("flag");
    // Before: nothing answered, one call per photo expected.
    let plan = lib.runner.plan(&ask(&lib, None)).unwrap();
    assert_eq!(plan["photos"], json!(PHOTOS), "{plan}");
    assert_eq!(plan["J"], json!(1), "{plan}");
    assert_eq!(plan["answered_by_book"], json!(0), "{plan}");
    assert_eq!(plan["calls"], json!(PHOTOS), "{plan}");

    let o = run(&lib, None);
    assert!(o.failed.is_empty(), "{:?}", o.failed);
    assert_eq!(o.written.len(), PHOTOS, "every photo written under `flag`");
    assert_eq!(o.calls, PHOTOS as u64);
    assert_eq!(*lib.calls.lock().unwrap(), PHOTOS as u64);
    // The ends of the set are unlike the test photos; the middle is not.
    assert!(
        flags(&o, 0, &lib).contains(&"unlike_test_cases".to_string()),
        "{:?}",
        flags(&o, 0, &lib)
    );
    assert!(
        flags(&o, PHOTOS - 1, &lib).contains(&"unlike_test_cases".to_string()),
        "{:?}",
        flags(&o, PHOTOS - 1, &lib)
    );
    assert!(flags(&o, PHOTOS / 2, &lib).is_empty(), "{:?}", o.flagged);
    // The darkest photo's answer is under its confidence: the fallback.
    assert!(flags(&o, 0, &lib).contains(&"low_confidence".to_string()));

    // Each edit: the workshop node's values, their provenance in the
    // version, continuing the photo's first version.
    let mid = head(&lib, PHOTOS / 2);
    assert_eq!(mid.parents, vec![lib.first[PHOTOS / 2].id]);
    assert_eq!(mid.label.as_deref(), Some("workshop Backlit (run 1)"));
    let meta = &mid.meta.as_ref().unwrap()["workshop_run"];
    assert_eq!(
        meta["provenance"]["brightness"]["kind"],
        json!("aimed"),
        "{meta}"
    );
    assert_eq!(meta["provenance"]["lift"]["kind"], json!("ai"), "{meta}");
    let dark = head(&lib, 0);
    let dark_run = &dark.meta.as_ref().unwrap()["workshop_run"];
    assert_eq!(dark_run["provenance"]["lift"]["kind"], json!("fallback"));
    let modules = edit::modules(&mid.edit).unwrap();
    let params = &modules[NODE].1;
    assert_eq!(
        params.get("lift").and_then(helios_core::Value::as_f64),
        Some(0.01)
    );
    let brightness = params
        .get("brightness")
        .and_then(helios_core::Value::as_f64)
        .unwrap();
    assert!(brightness > 0.5 && brightness < 3.0, "{brightness}");

    // One library step for the run.
    let line = lib.history.line();
    assert_eq!(line.undo.len(), 1);
    assert_eq!(line.undo[0].label.text, "Workshop “Backlit” on 24 photos");

    // The book answers every photo now: the cache's share in A is all.
    let plan = lib.runner.plan(&ask(&lib, None)).unwrap();
    assert_eq!(plan["answered_by_book"], json!(PHOTOS), "{plan}");
    assert_eq!(plan["calls"], json!(0), "{plan}");
    assert_eq!(plan["runs"], json!(1), "{plan}");

    // A second run: no call, the same edits, nothing written.
    let heads: Vec<Version> = (0..PHOTOS).map(|i| head(&lib, i)).collect();
    let again = run(&lib, None);
    assert_eq!(again.calls, 0);
    assert_eq!(again.from_book, PHOTOS as u64);
    assert_eq!(*lib.calls.lock().unwrap(), PHOTOS as u64);
    assert!(again.written.is_empty(), "{:?}", again.written);
    assert_eq!(again.unchanged.len(), PHOTOS);
    for (i, h) in heads.iter().enumerate() {
        assert_eq!(head(&lib, i).id, h.id);
    }
}

#[test]
fn hold_keeps_breaching_photos_unwritten_and_stop_ends_the_run() {
    let lib = library("hold");
    let o = run(&lib, Some(OnBreach::Hold));
    assert!(!o.held.is_empty());
    assert_eq!(o.held.len() + o.written.len(), PHOTOS);
    let first: Vec<_> = lib.first.iter().map(|v| v.id).collect();
    for h in &o.held {
        assert!(
            h.flags.iter().any(|r| r.flag.is_breach()),
            "held with a reason: {h:?}"
        );
        let i = lib
            .images
            .iter()
            .position(|id| id.hex() == h.image)
            .unwrap();
        assert_eq!(head(&lib, i).id, first[i], "a held photo is not written");
    }
    // Stop: the run ends at the first breach, before writing it.
    let lib = library("stop");
    let o = run(&lib, Some(OnBreach::Stop));
    let stopped = o.stopped.expect("stopped");
    assert_eq!(stopped.image, lib.images[0].hex());
    assert!(o.written.is_empty());
}

#[test]
fn undo_restores_every_edit_but_one_edited_after_the_run() {
    let lib = library("undo");
    let o = run(&lib, None);
    assert_eq!(o.written.len(), PHOTOS);
    // The user edits photo 3 after the run.
    let after = head(&lib, 3);
    let later = Version::new(
        lib.images[3],
        vec![after.id],
        json!({ "modules": [ { "name": NODE, "module": "x", "params": {} } ] }),
        "user",
        2,
    );
    lib.vault.add_version(&later).unwrap();
    lib.vault.revert(&o.changes).unwrap();
    for i in 0..PHOTOS {
        if i == 3 {
            assert_eq!(head(&lib, i).id, later.id, "edited after: left alone");
        } else {
            assert_eq!(head(&lib, i).id, lib.first[i].id, "photo {i} restored");
        }
    }
}

fn review(lib: &Library, args: Json, who: &str) -> Result<Json, String> {
    lib.runner.review(&args, who, Some(ask(lib, None)))
}

fn photo_of<'a>(review: &'a Json, lib: &Library, i: usize) -> &'a Json {
    let hex = lib.images[i].hex();
    review["photos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["image"] == hex.as_str())
        .unwrap()
}

#[test]
fn the_review_lists_flagged_first_a_fix_records_user_and_a_decided_junction_is_rerun() {
    let lib = library("review");
    let o = run(&lib, None);
    assert_eq!(o.written.len(), PHOTOS);
    let r = review(&lib, json!({ "run": "run" }), "user").unwrap();
    assert_eq!(r["counts"]["photos"], json!(PHOTOS), "{r}");
    assert_eq!(r["counts"]["written"], json!(PHOTOS), "{r}");
    // The photos to review first, each with its flags.
    let photos = r["photos"].as_array().unwrap();
    let flagged = |p: &Json| !p["flags"].as_array().unwrap().is_empty();
    let first_clean = photos.iter().position(|p| !flagged(p)).unwrap();
    assert!(first_clean > 0, "{r}");
    assert!(
        photos[first_clean..].iter().all(|p| !flagged(p)),
        "flagged first"
    );
    assert_eq!(r["counts"]["flagged"], json!(first_clean), "{r}");
    // Before and after, and where each value comes from.
    let mid = photo_of(&r, &lib, PHOTOS / 2);
    assert_eq!(
        mid["provenance"]["brightness"]["kind"],
        json!("aimed"),
        "{mid}"
    );
    assert!(mid["after"]["brightness"].as_f64().unwrap() > 0.5, "{mid}");
    assert!(mid["before"].is_object(), "{mid}");
    assert_ne!(
        mid["before"]["brightness"], mid["after"]["brightness"],
        "{mid}"
    );
    // A filter per flag.
    let low = review(
        &lib,
        json!({ "run": "run", "flag": "low_confidence" }),
        "user",
    )
    .unwrap();
    let low_photos = low["photos"].as_array().unwrap();
    assert!(!low_photos.is_empty());
    assert!(low_photos.iter().all(|p| {
        p["flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["flag"] == "low_confidence")
    }));
    // The darkest photos' junction is pending: the AI was unsure.
    let pending = r["pending"].as_array().unwrap();
    assert!(
        pending
            .iter()
            .any(|j| j["image"] == lib.images[0].hex().as_str() && j["junction"] == "lift"),
        "{r}"
    );

    // An agent decides photo 0's junction: in the book, with its author.
    let steps = lib.history.line().undo.len();
    let d = review(
        &lib,
        json!({ "run": "run", "action": "decide", "image": lib.images[0].hex(), "junction": "lift", "value": 0.03 }),
        "agent:test",
    )
    .unwrap();
    assert!(
        !d["pending"]
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["image"] == lib.images[0].hex().as_str()),
        "{d}"
    );
    assert!(
        d["rerun"]
            .as_array()
            .unwrap()
            .contains(&json!(lib.images[0].hex())),
        "{d}"
    );
    let book = lib.vault.workshop_book(WORKSHOP).unwrap().unwrap();
    assert!(
        book["decided"]
            .as_object()
            .unwrap()
            .values()
            .any(|w| w == "agent:test"),
        "{book}"
    );
    assert_eq!(lib.history.line().undo.len(), steps + 1, "one step");
    // Accept, revert and rerun are the user's alone.
    let e = review(
        &lib,
        json!({ "run": "run", "action": "revert", "image": lib.images[1].hex() }),
        "agent:test",
    )
    .unwrap_err();
    assert!(e.contains("user's alone"), "{e}");
    // A re-run of photo 0 takes the decision from the book: no call, the
    // value decided, its flag gone.
    let calls = *lib.calls.lock().unwrap();
    let mut again = ask(&lib, None);
    again.images = vec![lib.images[0]];
    let o = lib
        .runner
        .run(&again, "rerun", &AtomicBool::new(false), None)
        .unwrap();
    assert_eq!(o.calls, 0);
    assert_eq!(*lib.calls.lock().unwrap(), calls);
    let rr = review(&lib, json!({ "run": "rerun" }), "user").unwrap();
    let p0 = photo_of(&rr, &lib, 0);
    assert_eq!(p0["after"]["lift"].as_f64(), Some(0.03), "{p0}");
    assert_eq!(p0["provenance"]["lift"]["kind"], json!("ai"), "{p0}");
    assert!(
        !p0["flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "lift"),
        "{p0}"
    );

    // A fix by hand: a version, provenance `user`, its flag cleared.
    let i = (0..PHOTOS)
        .find(|&i| {
            photo_of(&r, &lib, i)["flags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["name"] == "lift")
                && i != 0
        })
        .expect("another photo with the junction flagged");
    let f = review(
        &lib,
        json!({ "run": "run", "action": "fix", "image": lib.images[i].hex(), "values": { "lift": 0.02 } }),
        "user",
    )
    .unwrap();
    let p = photo_of(&f, &lib, i);
    assert_eq!(p["state"], json!("fixed"), "{p}");
    assert_eq!(p["provenance"]["lift"]["kind"], json!("user"), "{p}");
    assert!(
        !p["flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "lift"),
        "{p}"
    );
    let h = head(&lib, i);
    assert_eq!(
        h.meta.as_ref().unwrap()["workshop_run"]["provenance"]["lift"]["kind"],
        json!("user")
    );
    let params = &edit::modules(&h.edit).unwrap()[NODE].1;
    assert_eq!(
        params.get("lift").and_then(helios_core::Value::as_f64),
        Some(0.02)
    );
    // Out of range: refused.
    let e = review(
        &lib,
        json!({ "run": "run", "action": "fix", "image": lib.images[i].hex(), "values": { "lift": 5.0 } }),
        "user",
    )
    .unwrap_err();
    assert!(e.contains("outside its range"), "{e}");

    // Revert a written photo: its first version is current again.
    let m = PHOTOS / 2;
    let v = review(
        &lib,
        json!({ "run": "run", "action": "revert", "image": lib.images[m].hex() }),
        "user",
    )
    .unwrap();
    assert_eq!(photo_of(&v, &lib, m)["state"], json!("reverted"));
    assert_eq!(head(&lib, m).id, lib.first[m].id);
}

#[test]
fn a_held_photo_is_accepted_and_the_held_ones_rerun() {
    let lib = library("accept");
    let o = run(&lib, Some(OnBreach::Hold));
    let held = &o.held[0].image;
    let i = lib.images.iter().position(|id| id.hex() == *held).unwrap();
    let a = review(
        &lib,
        json!({ "run": "run", "action": "accept", "image": held }),
        "user",
    )
    .unwrap();
    assert_eq!(photo_of(&a, &lib, i)["state"], json!("accepted"));
    let h = head(&lib, i);
    assert_eq!(h.parents, vec![lib.first[i].id]);
    assert_eq!(
        h.label.as_deref(),
        Some("workshop Backlit (run 1, accepted)")
    );
    // The other held photos re-run: a new run, its record when it ends.
    let left = a["rerun"].as_array().unwrap().len();
    assert_eq!(left, o.held.len() - 1, "{a}");
    let started = review(&lib, json!({ "run": "run", "action": "rerun" }), "user").unwrap();
    let id = started["run"].as_str().unwrap().to_string();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let record = loop {
        if let Some(r) = lib.vault.workshop_run(&id).unwrap() {
            break r;
        }
        assert!(std::time::Instant::now() < deadline, "the re-run ends");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(record["photos"].as_array().unwrap().len(), left);
    let after = review(&lib, json!({ "run": "run" }), "user").unwrap();
    assert!(after["rerun"].as_array().unwrap().is_empty(), "{after}");
}

#[test]
fn the_panel_shows_the_review_and_accepts_a_held_photo() {
    use egui_kittest::kittest::Queryable;
    use helios_gui::{Experimental, Shell, headless, helios_panels_with};
    use helios_shell::commands::Origin;

    let lib = library("panel");
    let o = run(&lib, Some(OnBreach::Hold));
    assert!(!o.held.is_empty());
    let config = temp("panel-config");
    let panels = helios_panels_with(Experimental { workshops: true });
    let mut shell = Shell::builder(panels, Some(config.join("ui"))).build();
    shell.load_layout().unwrap();
    let runner = lib.runner.clone();
    let fixed = ask(&lib, None);
    shell.set_workshop_reviewer(Arc::new(move |args, who, _| {
        runner.review(args, who, Some(fixed.clone()))
    }));
    let mut h = headless::harness(shell, egui::vec2(1280.0, 1400.0));
    h.state_mut()
        .run("helios.workshops.focus", json!(null), Origin::User)
        .unwrap();
    h.state_mut()
        .run("workshop.review", json!({ "run": "run" }), Origin::User)
        .unwrap();
    for _ in 0..3 {
        h.run();
    }
    assert!(
        h.query_by_label_contains("Review: Backlit, run 1")
            .is_some()
    );
    assert!(h.query_by_label_contains("held").is_some());
    h.get_all_by_label("Accept").next().unwrap().click();
    for _ in 0..4 {
        h.run();
    }
    let review = h.state().workshop_review().unwrap();
    let accepted = review["photos"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["state"] == "accepted")
        .count();
    assert_eq!(accepted, 1, "{review}");
}
