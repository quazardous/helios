//! Library operations end to end on fixture files: import with
//! duplicates, copies renamed with their sidecars, the job's events, the
//! perceptual signals, and stack suggestions accepted and dissolved.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_library::import::{self, Duplicates, Layout, Options, Placement};
use helios_library::rename::Pattern;
use helios_library::signals::{self, Signals, distance};
use helios_library::stacks::{self, Resolvers, Status};
use helios_vault::{ImageId, MemoryStore, Vault};
use serde_json::Value;

struct Place(PathBuf);

impl Place {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("helios-library-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A 64 x 48 picture of a scene, stripes over a slope: `kind` 0 rises to
/// the right, any other falls; `gain` scales it, `noise` adds a small
/// pattern of its own.
fn picture(path: &Path, kind: u32, gain: f32, noise: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(64, 48, |x, y| {
        let slope = if kind == 0 { x * 2 } else { (63 - x) * 2 };
        let base = (slope + y) as f32 + if (y / 6) % 2 == 0 { 0.0 } else { 30.0 };
        let wobble = ((x * 31 + y * 17 + noise * 13) % 3) as f32 * noise.min(1) as f32;
        let v = (base * gain + wobble).clamp(0.0, 255.0) as u8;
        image::Rgb([v, v.saturating_add(10), v / 2])
    })
    .save(path)
    .unwrap();
}

fn vault() -> Vault<MemoryStore> {
    Vault::new(MemoryStore::default())
}

fn run(
    vault: &Vault<MemoryStore>,
    bus: Option<Arc<Bus>>,
    sources: &[PathBuf],
    options: &Options,
) -> (import::Report, Vec<Value>) {
    let jobs = Jobs::new(bus.unwrap_or_default());
    let mut lines = Vec::new();
    let report = import::import(vault, &jobs, sources, options, 1000, &mut |l: &Value| {
        lines.push(l.clone());
    })
    .unwrap();
    (report, lines)
}

fn status_of<'a>(lines: &'a [Value], path: &Path) -> &'a str {
    lines
        .iter()
        .find(|l| l["path"] == serde_json::json!(path))
        .unwrap_or_else(|| panic!("no line for {}: {lines:?}", path.display()))["status"]
        .as_str()
        .unwrap()
}

#[test]
fn a_folder_with_duplicates_imports_each_content_once_and_deletes_nothing() {
    let p = Place::new("duplicates");
    let roll = p.join("roll");
    picture(&roll.join("a.png"), 0, 1.0, 0);
    picture(&roll.join("b.png"), 1, 1.0, 0);
    // The same file twice in the source, in a subfolder.
    std::fs::create_dir_all(roll.join("sub")).unwrap();
    std::fs::copy(roll.join("a.png"), roll.join("sub/a-again.png")).unwrap();
    std::fs::write(roll.join("notes.txt"), "not an image").unwrap();
    let vault = vault();
    let (report, lines) = run(
        &vault,
        None,
        std::slice::from_ref(&roll),
        &Options::default(),
    );
    assert_eq!(
        (report.files, report.added, report.skipped),
        (3, 2, 1),
        "{lines:?}"
    );
    let again = lines
        .iter()
        .find(|l| l["status"] == "skipped")
        .expect("the second copy is skipped");
    assert_eq!(again["duplicate"]["of"], "source");
    assert_eq!(
        again["duplicate"]["first"],
        serde_json::json!(roll.join("a.png"))
    );
    assert_eq!(vault.images().unwrap().len(), 2);
    // Every image got its signals.
    for image in vault.images().unwrap() {
        let s = vault.signals(&image.id).unwrap().expect("signals kept");
        assert!(s["phash"].is_string(), "{s}");
        assert_eq!(s["taken_from"], "file");
    }

    // Again: what is known at its place is unchanged, nothing is read twice.
    let (report, lines) = run(
        &vault,
        None,
        std::slice::from_ref(&roll),
        &Options::default(),
    );
    assert_eq!(
        (report.unchanged, report.skipped, report.added),
        (2, 1, 0),
        "{lines:?}"
    );

    // A copy elsewhere of an image the library has: skip, ask, keep.
    let other = p.join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::copy(roll.join("b.png"), other.join("b-copy.png")).unwrap();
    let copy = other.join("b-copy.png");
    let (_, lines) = run(
        &vault,
        None,
        std::slice::from_ref(&other),
        &Options::default(),
    );
    assert_eq!(status_of(&lines, &copy), "skipped");
    assert_eq!(lines[0]["duplicate"]["of"], "library");
    let ask = Options {
        duplicates: Duplicates::Ask,
        ..Options::default()
    };
    let (report, lines) = run(&vault, None, std::slice::from_ref(&other), &ask);
    assert_eq!(report.asked, 1);
    assert_eq!(status_of(&lines, &copy), "asked");
    let id = ImageId::from_hex(lines[0]["id"].as_str().unwrap()).unwrap();
    assert_eq!(vault.locations(&id).unwrap().len(), 1);
    let keep = Options {
        duplicates: Duplicates::Keep,
        ..Options::default()
    };
    let (_, lines) = run(&vault, None, std::slice::from_ref(&other), &keep);
    assert_eq!(status_of(&lines, &copy), "located");
    assert_eq!(vault.locations(&id).unwrap().len(), 2);
    assert_eq!(vault.images().unwrap().len(), 2);
    // Nothing was deleted.
    for f in ["a.png", "b.png", "sub/a-again.png"] {
        assert!(roll.join(f).exists(), "{f}");
    }
    assert!(copy.exists());
}

#[test]
fn a_copy_is_filed_by_its_layout_and_named_by_its_pattern() {
    let noon = signals::exif_time("2024:06:01 12:00:00", None).unwrap();
    for (layout, folder) in [
        (Layout::Day, "2024/2024-06-01"),
        (Layout::Month, "2024/2024-06"),
        (Layout::Year, "2024"),
        (Layout::Flat, ""),
    ] {
        let p = Place::new(&format!("layout-{}", layout.as_str()));
        let src = p.join("photos");
        picture(&src.join("IMG_0042.png"), 0, 1.0, 0);
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(noon);
        std::fs::File::options()
            .write(true)
            .open(src.join("IMG_0042.png"))
            .unwrap()
            .set_modified(when)
            .unwrap();
        let library = p.join("library");
        let options = Options {
            placement: Placement::Copy {
                into: Some(library.clone()),
            },
            rename: Some(Pattern::parse("{date}_{stem}").unwrap()),
            layout,
            ..Options::default()
        };
        let vault = vault();
        let (report, lines) = run(&vault, None, std::slice::from_ref(&src), &options);
        assert_eq!(report.copied, 1, "{lines:?}");
        let to = library.join(folder).join("20240601_IMG_0042.png");
        assert!(to.exists(), "{layout:?}: {lines:?}");
        assert_eq!(layout, Layout::parse(layout.as_str()).unwrap());
    }
    assert_eq!(Layout::Flat.folder(None), PathBuf::new());
    assert_eq!(Layout::Month.folder(None), PathBuf::from("undated"));
    assert!(Layout::parse("week").is_err());
}

#[test]
fn a_card_is_copied_renamed_with_its_sidecars_as_a_job() {
    let p = Place::new("card");
    let card = p.join("card");
    let dcim = card.join("DCIM/100TEST");
    picture(&dcim.join("IMG_0001.png"), 0, 1.0, 0);
    picture(&dcim.join("IMG_0002.png"), 1, 1.0, 0);
    std::fs::write(dcim.join("IMG_0001.png.xmp"), "<x:xmpmeta/>").unwrap();
    std::fs::write(dcim.join("IMG_0001_01.png.xmp"), "<x:xmpmeta/>").unwrap();
    // Shot on 2024-06-01 at noon, as the files' dates say.
    let noon = signals::exif_time("2024:06:01 12:00:00", None).unwrap();
    for f in ["IMG_0001.png", "IMG_0002.png"] {
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(noon);
        std::fs::File::options()
            .write(true)
            .open(dcim.join(f))
            .unwrap()
            .set_modified(when)
            .unwrap();
    }
    assert_eq!(import::source_kind(&card).0, "card");
    let library = p.join("library");
    // A name taken by another file is never written over.
    let day = library.join("2024/2024-06-01");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(day.join("20240601-0001.png"), "someone else's").unwrap();
    let bus = Arc::new(Bus::new());
    let options = Options {
        placement: Placement::Copy {
            into: Some(library.clone()),
        },
        rename: Some(Pattern::parse("{date}-{seq}").unwrap()),
        ..Options::default()
    };
    let vault = vault();
    let (report, lines) = run(
        &vault,
        Some(bus.clone()),
        std::slice::from_ref(&card),
        &options,
    );
    assert_eq!(
        (report.added, report.copied, report.sidecars),
        (2, 2, 2),
        "{lines:?}"
    );
    let first = day.join("20240601-0001-1.png");
    assert!(first.exists(), "{lines:?}");
    assert!(day.join("20240601-0002.png").exists());
    assert_eq!(
        std::fs::read_to_string(day.join("20240601-0001.png")).unwrap(),
        "someone else's"
    );
    assert!(day.join("20240601-0001-1.png.xmp").exists());
    assert!(day.join("20240601-0001-1_01.png.xmp").exists());
    // The copy is the image, and the card's file it was read from is
    // noted where it is; the card is left as it was.
    let id = ImageId::from_hex(lines[0]["id"].as_str().unwrap()).unwrap();
    let mut at: Vec<PathBuf> = vault
        .locations(&id)
        .unwrap()
        .into_iter()
        .map(|l| l.path)
        .collect();
    at.sort();
    let mut want = vec![first, dcim.join("IMG_0001.png")];
    want.sort();
    assert_eq!(at, want);
    assert!(dcim.join("IMG_0001.png").exists());
    // The library folder is remembered for the next copy.
    assert_eq!(import::library_root(&vault).unwrap(), Some(library.clone()));

    let events = bus.since(0);
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| e.kind)
        .filter(|k| k.starts_with("job.") && *k != "job.log")
        .collect();
    assert_eq!(
        kinds,
        [
            "job.started",
            "job.progress",
            "job.progress",
            "job.progress",
            "job.finished"
        ]
    );
    let started = &events
        .iter()
        .find(|e| e.kind == "job.started")
        .unwrap()
        .payload;
    assert_eq!(started["kind"], "import");
    assert_eq!(started["total"], 2);
    let progress: Vec<&Value> = events
        .iter()
        .filter(|e| e.kind == "job.progress")
        .map(|e| &e.payload)
        .collect();
    assert_eq!(progress[0]["done"], 0);
    assert_eq!(progress[0]["item"], "IMG_0001.png");
    assert_eq!(progress[2]["done"], 2);
    let log: Vec<&str> = events
        .iter()
        .filter(|e| e.kind == "job.log")
        .filter_map(|e| e.payload["text"].as_str())
        .collect();
    assert!(
        log.iter().any(|l| l.ends_with("IMG_0001.png: added")),
        "{log:?}"
    );

    // Renaming in place is refused: a referenced file stays as it is.
    let refused = import::import(
        &vault,
        &Jobs::new(bus.clone()),
        &[card],
        &Options {
            rename: options.rename.clone(),
            ..Options::default()
        },
        1000,
        &mut |_: &Value| {},
    );
    assert!(refused.is_err());
    assert!(bus.since(0).iter().any(|e| e.kind == "job.failed"));
}

#[test]
fn near_identical_pictures_hash_alike_and_another_scene_does_not() {
    let p = Place::new("signals");
    let (a, b, c, d) = (
        p.join("a.png"),
        p.join("b.png"),
        p.join("c.png"),
        p.join("d.png"),
    );
    picture(&a, 0, 1.0, 0);
    // The same scene with a little noise, then brighter.
    picture(&b, 0, 1.0, 1);
    picture(&c, 0, 1.15, 0);
    picture(&d, 1, 1.0, 0);
    let s: Vec<Signals> = [&a, &b, &c, &d]
        .iter()
        .map(|f| signals::measure(f).unwrap())
        .collect();
    let h: Vec<u64> = s.iter().map(|s| s.hash().unwrap()).collect();
    assert!(distance(h[0], h[1]) <= 4, "noise: {}", distance(h[0], h[1]));
    assert!(
        distance(h[0], h[2]) <= 10,
        "brighter: {}",
        distance(h[0], h[2])
    );
    assert!(
        distance(h[0], h[3]) > 16,
        "another scene: {}",
        distance(h[0], h[3])
    );
    assert_eq!(s[0].hashed.as_deref(), Some("picture"));
    assert!(s.iter().all(|s| s.sharpness.is_some_and(|v| v > 0.0)));
}

/// An image of its own in the vault (a small file), with these signals.
fn image_with(vault: &Vault<MemoryStore>, dir: &Path, n: u32, signals: &Signals) -> ImageId {
    let file = dir.join(format!("{n}.raw"));
    std::fs::write(&file, format!("frame {n}")).unwrap();
    let id = vault.import_file(&file, 1).unwrap();
    let doc = serde_json::to_value(signals).unwrap();
    vault.set_signals(&id, Some(&doc)).unwrap();
    id
}

fn shot(taken: f64, hash: u64, sharpness: f64, bias: f64) -> Signals {
    Signals {
        phash: Some(format!("{hash:016x}")),
        taken: Some(taken),
        taken_from: Some("exif".into()),
        exposure_time: Some(0.01),
        aperture: Some(4.0),
        iso: Some(100.0),
        exposure_bias: Some(bias),
        sharpness: Some(sharpness),
        ..Signals::default()
    }
}

#[test]
fn a_burst_and_a_bracket_are_suggested_accepted_and_dissolved() {
    let p = Place::new("stacks");
    let vault = vault();
    let bus = Bus::new();
    let base: u64 = 0xF0F0_F0F0_0F0F_0F0F;
    // A burst: three frames half a second apart, a bit or two apart, the
    // middle one sharpest.
    let burst = [
        image_with(&vault, &p.0, 1, &shot(0.0, base, 1.0, 0.0)),
        image_with(&vault, &p.0, 2, &shot(0.5, base ^ 0b1, 3.0, 0.0)),
        image_with(&vault, &p.0, 3, &shot(1.0, base ^ 0b11, 2.0, 0.0)),
    ];
    // Alone, a minute later.
    let alone = image_with(&vault, &p.0, 4, &shot(60.0, base, 5.0, 0.0));
    // A bracket: -1, 0, +1 EV, hashes further apart than a burst allows
    // but within the bracket's threshold.
    let wide = 0x0000_0000_0000_0FFF_u64; // 12 bits
    let bracket = [
        image_with(&vault, &p.0, 5, &shot(120.0, base, 1.0, -1.0)),
        image_with(&vault, &p.0, 6, &shot(120.3, base ^ wide, 4.0, 0.0)),
        image_with(&vault, &p.0, 7, &shot(120.6, base, 2.0, 1.0)),
    ];
    // Right after the bracket, another scene entirely.
    let other = image_with(&vault, &p.0, 8, &shot(121.0, !base, 9.0, 0.0));

    let resolvers = Resolvers::default();
    let d = stacks::suggest(&vault, Some(&bus), &resolvers, "rules", &[], 50).unwrap();
    assert_eq!(d.status, Status::Proposed);
    assert_eq!(d.proposal.model.name, "rules");
    let groups = &d.proposal.groups;
    assert_eq!(groups.len(), 2, "{groups:?}");
    let hex = |ids: &[ImageId]| ids.iter().map(ImageId::hex).collect::<Vec<_>>();
    assert_eq!(groups[0].members, hex(&burst[..]));
    assert_eq!(groups[0].kind, "burst");
    assert_eq!(groups[0].best, burst[1].hex());
    assert_eq!(groups[1].members, hex(&bracket[..]));
    assert_eq!(groups[1].kind, "bracket");
    assert_eq!(groups[1].best, bracket[1].hex());
    for loner in [alone, other] {
        assert!(groups.iter().all(|g| !g.members.contains(&loner.hex())));
    }
    // Nothing applied yet.
    assert!(vault.stacks().unwrap().is_empty());
    assert!(stacks::suggest(&vault, None, &resolvers, "a-model", &[], 50).is_err());

    // Only the burst is taken: a correction of the proposal.
    let answered = stacks::accept(&vault, Some(&bus), &d.id, Some(&[0][..]), "user", 60).unwrap();
    assert_eq!(answered.status, Status::Accepted);
    let answer = answered.answer.as_ref().unwrap();
    assert_eq!(answer.groups, [0]);
    let recorded = serde_json::to_value(answer).unwrap();
    assert_eq!(recorded["by"], "corrected");
    assert_eq!(recorded["model"]["name"], "rules");
    let made = vault.stacks().unwrap();
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].top, burst[1].hex());
    assert_eq!(made[0].who, format!("decision:{}", d.id));
    assert!(stacks::accept(&vault, None, &d.id, None, "user", 61).is_err());
    assert_eq!(stacks::decisions(&vault).unwrap(), [answered]);
    assert!(
        bus.since(0)
            .iter()
            .filter(|e| e.kind == "library.decision")
            .count()
            >= 2
    );

    // Asked again, the stacked burst is left out; declining makes nothing.
    let again = stacks::suggest(&vault, None, &resolvers, "rules", &[], 70).unwrap();
    assert_eq!(again.proposal.groups.len(), 1);
    assert_eq!(again.left_out.len(), 3);
    let declined = stacks::decline(&vault, None, &again.id, "user", 71).unwrap();
    assert_eq!(declined.status, Status::Declined);
    assert_eq!(vault.stacks().unwrap().len(), 1);

    // Dissolving keeps every image.
    let gone = vault.dissolve_stack(&made[0].id).unwrap();
    assert_eq!(gone.members, hex(&burst[..]));
    assert!(vault.stacks().unwrap().is_empty());
    assert_eq!(vault.images().unwrap().len(), 8);
}

/// An import is a job egui-pop's jobs model follows: its progress, then
/// cancelled from it after its first file, with the files done imported
/// and no half-written copy left.
#[test]
fn an_import_shows_in_the_jobs_model_and_is_cancelled_midway() {
    use egui_pop_model::jobs::{Incoming, JobState, Jobs as Model};

    let p = Place::new("cancel");
    let roll = p.join("roll");
    for (name, kind, gain) in [("a.png", 0, 1.0), ("b.png", 1, 1.25), ("c.png", 0, 1.5)] {
        picture(&roll.join(name), kind, gain, 0);
    }
    let library = p.join("library");
    let bus = Arc::new(Bus::new());
    let jobs = Jobs::new(bus.clone());
    let options = Options {
        placement: Placement::Copy {
            into: Some(library.clone()),
        },
        ..Options::default()
    };
    let vault = vault();
    let mut lines = Vec::new();
    let report = import::import(
        &vault,
        &jobs,
        std::slice::from_ref(&roll),
        &options,
        1000,
        &mut |l: &Value| {
            lines.push(l.clone());
            if lines.len() == 1 {
                // The jobs centre's Cancel, once the first file is in.
                let started = bus
                    .since(0)
                    .into_iter()
                    .find(|e| e.kind == "job.started")
                    .expect("the import started");
                jobs.cancel(started.payload["id"].as_str().unwrap())
                    .unwrap();
            }
        },
    )
    .unwrap();
    assert!(report.cancelled, "{report:?}");
    assert_eq!((report.files, report.added, report.copied), (1, 1, 1));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(vault.images().unwrap().len(), 1);

    // Copies only of what was imported, and no part left anywhere.
    let mut copies = Vec::new();
    let mut dirs = vec![library.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                copies.push(path.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
    }
    assert_eq!(copies, ["a.png"], "no half-written copy");

    let mut model = Model::new();
    for e in bus.since(0) {
        model.apply(
            &Incoming::new(e.seq, e.kind, &e.payload).source(&e.source),
            0.0,
        );
    }
    let all = model.all();
    assert_eq!(all.len(), 1);
    let job = all[0];
    assert_eq!(job.kind, "import");
    assert!(job.title.starts_with("Import "), "{}", job.title);
    assert_eq!(job.state, JobState::Cancelled);
    assert!(
        model
            .log(&job.id)
            .iter()
            .any(|l| l.contains("cancelled after 1 of 3")),
        "{:?}",
        model.log(&job.id)
    );
}

/// A darktable sidecar: exposure at `ev`, then a module Helios does not
/// have (reported as not applied).
fn darktable_sidecar(ev: f64) -> String {
    let layouts = helios_xmp::Layouts::built_in();
    let version = *layouts.versions("exposure").iter().max().unwrap();
    let layout = layouts.get("exposure", version).unwrap();
    let bytes = helios_xmp::encode(layout, &serde_json::json!({ "exposure": ev })).unwrap();
    let hex = bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .concat();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/" darktable:history_end="2">
   <darktable:history>
    <rdf:Seq>
     <rdf:li darktable:num="0" darktable:operation="exposure" darktable:enabled="1" darktable:modversion="{version}" darktable:params="{hex}" darktable:multi_name="" darktable:multi_priority="0"/>
     <rdf:li darktable:num="1" darktable:operation="frobnicate" darktable:enabled="1" darktable:modversion="1" darktable:params="00" darktable:multi_name="" darktable:multi_priority="0"/>
    </rdf:Seq>
   </darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#
    )
}

#[test]
fn darktable_edits_come_in_as_versions_with_the_import_when_asked() {
    let p = Place::new("darktable");
    let roll = p.join("roll");
    let (edited, bare) = (roll.join("edited.png"), roll.join("bare.png"));
    picture(&edited, 0, 1.0, 0);
    picture(&bare, 1, 1.0, 0);
    std::fs::write(roll.join("edited.png.xmp"), darktable_sidecar(1.0)).unwrap();
    std::fs::write(roll.join("edited_01.png.xmp"), darktable_sidecar(2.0)).unwrap();
    let vault = vault();
    // Off by default: the files only.
    let (report, lines) = run(
        &vault,
        None,
        std::slice::from_ref(&roll),
        &Options::default(),
    );
    assert_eq!((report.added, report.edits), (2, 0), "{lines:?}");
    assert!(
        lines.iter().all(|l| l.get("darktable").is_none()),
        "{lines:?}"
    );
    let id = ImageId::from_hex(
        lines
            .iter()
            .find(|l| l["path"] == serde_json::json!(edited))
            .unwrap()["id"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(vault.versions(&id).unwrap().is_empty());
    // Asked: the base and its duplicate, two versions side by side, each
    // with its report (the module Helios does not have).
    let options = Options {
        darktable: true,
        signals: false,
        ..Options::default()
    };
    let (report, lines) = run(&vault, None, std::slice::from_ref(&roll), &options);
    assert_eq!(
        (
            report.unchanged,
            report.edits,
            report.edits_reported,
            report.edits_failed
        ),
        (2, 2, 2, 0),
        "{lines:?}"
    );
    let line = lines
        .iter()
        .find(|l| l["path"] == serde_json::json!(edited))
        .unwrap();
    let kept = line["darktable"].as_array().unwrap();
    assert_eq!(kept.len(), 2, "{line}");
    assert!(kept.iter().all(|k| k["warning"].is_string()), "{line}");
    let mut labels: Vec<String> = vault
        .versions(&id)
        .unwrap()
        .into_iter()
        .map(|v| v.label.unwrap_or_default())
        .collect();
    labels.sort();
    assert_eq!(labels, ["import:xmp", "import:xmp duplicate 01"]);
    // A file without sidecars has none to keep.
    let bare_line = lines
        .iter()
        .find(|l| l["path"] == serde_json::json!(bare))
        .unwrap();
    assert!(bare_line.get("darktable").is_none(), "{bare_line}");
    // Again: the same sidecars add nothing.
    let (report, _) = run(&vault, None, std::slice::from_ref(&roll), &options);
    assert_eq!(report.edits, 0);
    assert_eq!(vault.versions(&id).unwrap().len(), 2);
}

#[test]
fn files_an_importer_listed_take_its_values_without_overwriting_the_library() {
    use helios_library::import::{Listed, Metadata};
    use helios_vault::fields::Command;
    use serde_json::json;

    let place = Place::new("listed");
    let (a, b) = (place.join("a.png"), place.join("b.png"));
    picture(&a, 0, 1.0, 0);
    picture(&b, 1, 1.0, 0);
    let vault = vault();
    let listed = |rating: u8| Listed {
        by: "importer:org.example/test".into(),
        files: [
            (
                a.clone(),
                Metadata {
                    rating: Some(rating),
                    labels: vec!["red".into()],
                    tags: vec!["trip".into()],
                    title: Some("Low tide".into()),
                    ..Metadata::default()
                },
            ),
            (
                b.clone(),
                Metadata {
                    rejected: Some(true),
                    ..Metadata::default()
                },
            ),
        ]
        .into(),
    };
    let options = Options {
        signals: false,
        listed: Some(listed(4)),
        ..Options::default()
    };
    let (report, lines) = run(&vault, None, &[a.clone(), b.clone()], &options);
    assert_eq!((report.added, report.metadata), (2, 2), "{lines:?}");
    let id = |l: &Value| ImageId::from_hex(l["id"].as_str().unwrap()).unwrap();
    let (ia, ib) = (id(&lines[0]), id(&lines[1]));
    let fields = vault
        .fields_of(
            &[ia, ib],
            &["rating", "rejected", "labels", "tags", "title"],
        )
        .unwrap();
    assert_eq!(fields[0]["rating"], json!(4));
    assert_eq!(fields[0]["labels"], json!(["red"]));
    assert_eq!(fields[0]["tags"], json!(["trip"]));
    assert_eq!(fields[0]["title"], json!("Low tide"));
    assert_eq!(fields[1]["rejected"], json!(true));
    assert!(fields[1].get("rating").is_none_or(Value::is_null));
    // Written under the importer's name.
    let audit = vault.audit(&ia).unwrap();
    assert!(audit.iter().all(|e| e.who == "importer:org.example/test"));
    // The user's own rating and labels stay: a second import only adds.
    vault
        .apply(&[Command::set(ia, "rating", json!(1))], "user", None, 1)
        .unwrap();
    let mut again = listed(5);
    again.files.get_mut(&a).unwrap().labels = vec!["blue".into()];
    let options = Options {
        listed: Some(again),
        ..options
    };
    let (report, lines) = run(&vault, None, std::slice::from_ref(&a), &options);
    assert_eq!(report.unchanged, 1, "{lines:?}");
    let fields = vault.fields_of(&[ia], &["rating", "labels"]).unwrap();
    assert_eq!(fields[0]["rating"], json!(1));
    assert_eq!(fields[0]["labels"], json!(["blue", "red"]));
}

/// A batch export planned over library images: each image's file and
/// head version, names from the import's renaming fields with the
/// format's extension, a name taken on disk numbered, an image without a
/// copy refused in the request.
#[test]
fn an_export_is_planned_with_each_image_s_head_and_its_name() {
    use helios_export::batch::{Clash, Settings};
    use helios_export::{Format, Output};
    use helios_vault::EditSession;

    let place = Place::new("export-plan");
    let (a, b) = (place.join("in/IMG_0001.png"), place.join("in/IMG_0002.png"));
    picture(&a, 0, 1.0, 0);
    picture(&b, 1, 1.0, 1);
    let v = vault();
    let ia = v.import_file(&a, 10).unwrap();
    let ib = v.import_file(&b, 10).unwrap();
    // `a` was edited: its head goes in the request.
    let (mut session, _) = EditSession::open(&v, ia).unwrap();
    let saved = session
        .save(
            &v,
            serde_json::json!({ "nodes": {} }),
            "exposure",
            "user",
            11,
        )
        .unwrap()
        .expect("a version");
    let out = place.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("IMG_0001-x.jpg"), b"taken").unwrap();
    let missing = ImageId::from_hex(&"ee".repeat(32)).unwrap();
    let settings = Settings {
        output: Output::from(Format::Jpeg { quality: 90 }),
        folder: out.clone(),
        pattern: "{stem}-x".into(),
        clash: Clash::Number,
    };
    let request =
        helios_library::export::plan(&v, &[ia, ib, missing], settings, Some("exposure".into()))
            .unwrap();
    assert_eq!(request.node.as_deref(), Some("exposure"));
    let items = &request.items;
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].source, a);
    assert_eq!(items[0].version, Some(saved.id.hex()));
    assert_eq!(items[0].output, out.join("IMG_0001-x-2.jpg"), "numbered");
    assert_eq!(items[1].version, None, "never edited");
    assert_eq!(items[1].output, out.join("IMG_0002-x.jpg"));
    assert!(items[2].error.as_deref().unwrap().contains("no copy"));
    // `{seq}` is the image's place in the batch; a bad pattern is refused.
    let numbered = Settings {
        output: Output::from(Format::Png { bits: 8 }),
        folder: out.clone(),
        pattern: "trip-{seq:3}".into(),
        clash: Clash::Number,
    };
    let request = helios_library::export::plan(&v, &[ib, ia], numbered, None).unwrap();
    assert_eq!(request.items[0].output, out.join("trip-001.png"));
    assert_eq!(request.items[1].output, out.join("trip-002.png"));
    let bad = Settings {
        output: Output::from(Format::Tiff),
        folder: out,
        pattern: "{nope}".into(),
        clash: Clash::Number,
    };
    assert!(helios_library::export::plan(&v, &[ia], bad, None).is_err());
}

/// Waits up to `seconds` for an event `kind` on `bus`: its payload.
fn event(bus: &Bus, kind: &str, seconds: u64) -> Option<Value> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        if let Some(e) = bus.since(0).into_iter().find(|e| e.kind == kind) {
            return Some(e.payload.clone());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    None
}

/// A card taken out mid-import pauses the job, saying to reinsert it;
/// put back, the import goes on and every file comes in.
#[test]
fn a_card_taken_out_pauses_the_import_until_it_is_back() {
    let p = Place::new("lost-card");
    let card = p.join("EOS_DIGITAL");
    let away = p.join("away");
    for (name, kind, gain) in [("a.png", 0, 1.0), ("b.png", 1, 1.25), ("c.png", 0, 1.5)] {
        picture(&card.join("DCIM/100CANON").join(name), kind, gain, 0);
    }
    let bus = Arc::new(Bus::new());
    let jobs = Jobs::new(bus.clone());
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let options = Options {
        placement: Placement::Copy {
            into: Some(p.join("library")),
        },
        cancel: Some(cancel.clone()),
        ..Options::default()
    };
    let vault = vault();
    let (report, paused) = std::thread::scope(|scope| {
        let importing = scope.spawn(|| {
            let mut lines = 0;
            import::import(
                &vault,
                &jobs,
                std::slice::from_ref(&card),
                &options,
                1000,
                &mut |_: &Value| {
                    lines += 1;
                    if lines == 1 {
                        // Taken out after its first file.
                        std::fs::rename(&card, &away).unwrap();
                    }
                },
            )
        });
        let paused = event(&bus, "job.paused", 30);
        if paused.is_some() {
            std::fs::rename(&away, &card).unwrap();
        }
        if paused.is_none() || event(&bus, "job.finished", 30).is_none() {
            // Never hang: a job that did not pause or end is stopped.
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        (importing.join().unwrap(), paused)
    });
    let paused = paused.expect("the import paused");
    assert_eq!(paused["reason"], "Reinsert EOS_DIGITAL to go on");
    let report = report.unwrap();
    assert!(!report.cancelled, "{report:?}");
    assert_eq!((report.added, report.failed), (3, 0), "{report:?}");
    let kinds: Vec<&str> = bus
        .since(0)
        .iter()
        .map(|e| e.kind)
        .filter(|k| ["job.paused", "job.resumed", "job.finished"].contains(k))
        .collect();
    assert_eq!(kinds, ["job.paused", "job.resumed", "job.finished"]);
}

/// A paused import can still be cancelled.
#[test]
fn a_paused_import_can_be_cancelled() {
    let p = Place::new("lost-cancel");
    let card = p.join("EOS_DIGITAL");
    for (name, kind) in [("a.png", 0), ("b.png", 1)] {
        picture(&card.join("DCIM").join(name), kind, 1.0, 0);
    }
    let bus = Arc::new(Bus::new());
    let jobs = Jobs::new(bus.clone());
    let options = Options::default();
    let vault = vault();
    let report = std::thread::scope(|scope| {
        let importing = scope.spawn(|| {
            import::import(
                &vault,
                &jobs,
                std::slice::from_ref(&card),
                &options,
                1000,
                &mut |_: &Value| {
                    let _ = std::fs::remove_dir_all(card.join("DCIM"));
                },
            )
        });
        let started = event(&bus, "job.started", 30).expect("started");
        let id = started["id"].as_str().unwrap().to_string();
        // Cancelled whether it paused or not: never left waiting.
        let paused = event(&bus, "job.paused", 30);
        jobs.cancel(&id).unwrap();
        (importing.join().unwrap(), paused)
    });
    let (report, paused) = report;
    assert!(paused.is_some(), "the import paused");
    let report = report.unwrap();
    assert!(report.cancelled, "{report:?}");
    assert_eq!(report.added, 1);
    assert!(event(&bus, "job.cancelled", 1).is_some());
}

/// After a copy, a card is safe to format only when every photo on it has
/// a copy in the library folder, by content: a duplicate of a referenced
/// file left out keeps it unsafe.
#[test]
fn a_card_is_safe_to_format_once_every_photo_is_copied() {
    let p = Place::new("format");
    let card = p.join("EOS_DIGITAL");
    let dcim = card.join("DCIM/100CANON");
    picture(&dcim.join("a.png"), 0, 1.0, 0);
    picture(&dcim.join("b.png"), 1, 1.25, 0);
    // b is in the library already, referenced where it is.
    let elsewhere = p.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::copy(dcim.join("b.png"), elsewhere.join("b.png")).unwrap();
    let vault = vault();
    run(&vault, None, &[elsewhere], &Options::default());
    let copy = |duplicates| Options {
        placement: Placement::Copy {
            into: Some(p.join("library")),
        },
        duplicates,
        ..Options::default()
    };
    let bus = Arc::new(Bus::new());
    let (report, lines) = run(
        &vault,
        Some(bus.clone()),
        std::slice::from_ref(&card),
        &copy(Duplicates::Skip),
    );
    assert_eq!((report.added, report.skipped), (1, 1), "{lines:?}");
    let [first] = report.cards.as_slice() else {
        panic!("one card: {report:?}");
    };
    assert_eq!(
        (first.label.as_str(), first.photos, first.missing),
        ("EOS_DIGITAL", 2, 1)
    );
    assert!(!first.safe_to_format);
    assert!(
        bus.since(0).iter().any(|e| e.kind == "job.log"
            && e.payload["text"] == "EOS_DIGITAL: 1 photo not in the Library"),
        "the job says it"
    );
    // Copied too, b makes the card safe; a folder is never a card.
    let (report, _) = run(
        &vault,
        None,
        std::slice::from_ref(&card),
        &copy(Duplicates::Keep),
    );
    assert_eq!(report.cards.len(), 1);
    assert!(report.cards[0].safe_to_format, "{report:?}");
    assert_eq!(
        report.cards[0].said(),
        "EOS_DIGITAL: safe to format (1 copy, in the Library: no backup set up)"
    );
    let (report, _) = run(&vault, None, &[dcim], &copy(Duplicates::Skip));
    assert!(report.cards.is_empty());
}

#[test]
fn an_import_is_undone_and_redone_its_files_kept() {
    let p = Place::new("undo");
    let roll = p.join("roll");
    let (edited, bare) = (roll.join("edited.png"), roll.join("bare.png"));
    picture(&edited, 0, 1.0, 0);
    picture(&bare, 1, 1.0, 0);
    std::fs::write(roll.join("edited.png.xmp"), darktable_sidecar(1.0)).unwrap();
    let library = p.join("library");
    let vault = vault();
    // One image the library had before: its rows come back as they were.
    let (first, _) = run(
        &vault,
        None,
        std::slice::from_ref(&bare),
        &Options::default(),
    );
    assert_eq!(first.added, 1);
    let options = Options {
        placement: Placement::Copy {
            into: Some(library.clone()),
        },
        duplicates: Duplicates::Keep,
        darktable: true,
        ..Options::default()
    };
    let before: Vec<_> = vault.images().unwrap();
    let (report, lines) = run(&vault, None, std::slice::from_ref(&roll), &options);
    assert_eq!(
        (report.added, report.located, report.edits),
        (1, 1, 1),
        "{lines:?}"
    );
    // The copy of the image the library had is a sighting, a fact: not a
    // change an undo takes back.
    assert_eq!(report.changes.images.len(), 1, "{:?}", report.changes);
    assert_eq!(report.changes.added(), 1);
    assert!(!report.copies.is_empty());
    let after: Vec<_> = vault
        .images()
        .unwrap()
        .iter()
        .map(|i| vault.rows(&i.id).unwrap())
        .collect();
    let mut files: Vec<PathBuf> = report.copies.clone();
    files.extend([edited.clone(), bare.clone(), roll.join("edited.png.xmp")]);

    // Undo: the new image leaves the Library, what Helios saw stays (the
    // image, its sightings), and so do the files.
    let undone = vault.revert(&report.changes).unwrap();
    assert_eq!((undone.removed, undone.changed, undone.kept), (0, 1, 0));
    assert_eq!(vault.images().unwrap(), before);
    let bare_id = before[0].id;
    assert_eq!(
        vault.locations(&bare_id).unwrap().len(),
        2,
        "the copy's sighting stays"
    );
    let new_id = report.changes.images[0].image;
    assert!(!vault.in_library(&new_id).unwrap());
    assert!(vault.image(&new_id).unwrap().is_some(), "still known");
    assert!(!vault.sightings(&new_id).unwrap().is_empty());
    assert!(
        vault.versions(&new_id).unwrap().is_empty(),
        "its edit taken back"
    );
    for f in &files {
        assert!(f.is_file(), "{} stays on disk", f.display());
    }

    // Redo: everything back, versions and signals included.
    vault.replay(&report.changes).unwrap();
    let redone: Vec<_> = vault
        .images()
        .unwrap()
        .iter()
        .map(|i| vault.rows(&i.id).unwrap())
        .collect();
    assert_eq!(redone, after);

    // Undo again: the same import later finds its copies still in the
    // library folder, by their hash, and copies nothing again.
    vault.revert(&report.changes).unwrap();
    assert_eq!(vault.images().unwrap(), before);
    let (again, lines) = run(&vault, None, std::slice::from_ref(&roll), &options);
    assert_eq!((again.added, again.copied), (1, 0), "{lines:?}");
    for f in &files {
        assert!(f.is_file(), "{} stays on disk", f.display());
    }
}

/// The modification time of `path`, in seconds, as a listing records it.
fn modified_of(path: &Path) -> u64 {
    std::fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Records what the import window's scan and the map job know of the file
/// at `path`: a sighting with its image (the map job confirmed it).
fn seen_as(vault: &Vault<MemoryStore>, path: &Path, id: ImageId) {
    let size = std::fs::metadata(path).unwrap().len();
    let mut s =
        helios_vault::Sighting::seen(path.to_path_buf(), size, modified_of(path), 900, "import");
    s.image = Some(id);
    s.partial = Some(vault.image(&id).unwrap().unwrap().partial);
    vault.record_sighting(&s).unwrap();
}

/// Overwrites the file at `path` with other bytes of the same size,
/// keeping its modification time: a read of it would see another content,
/// so a file read shows (a reader that tells on itself).
fn tamper(path: &Path) {
    let meta = std::fs::metadata(path).unwrap();
    let modified = meta.modified().unwrap();
    std::fs::write(path, vec![0x5au8; meta.len() as usize]).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
}

#[test]
fn an_import_of_files_seen_before_sets_the_flag_without_reading_them() {
    let p = Place::new("flag-seen");
    let (first, again) = (p.join("roll/a.png"), p.join("roll2/a.png"));
    picture(&first, 0, 1.0, 0);
    std::fs::create_dir_all(again.parent().unwrap()).unwrap();
    std::fs::copy(&first, &again).unwrap();
    let vault = vault();
    // Imported once, then taken out of the Library: known, off it.
    let (report, _) = run(&vault, None, &[p.join("roll")], &Options::default());
    let id = report.changes.images[0].image;
    vault.set_in_library(&[id], false, "user", 1100).unwrap();
    assert!(vault.images().unwrap().is_empty());
    // The scan listed its copy in roll2, the map job confirmed it.
    seen_as(&vault, &again, id);
    tamper(&again);
    let reads = vault.reads();
    let (report, lines) = run(&vault, None, &[p.join("roll2")], &Options::default());
    assert_eq!(report.added, 1, "{lines:?}");
    assert_eq!(report.read, 0, "nothing read: {lines:?}");
    assert_eq!(
        (vault.reads().full_hashes, vault.reads().partial_hashes),
        (reads.full_hashes, reads.partial_hashes)
    );
    // Flagged by the lookup: had the file been read, its other bytes
    // would be another image.
    let flag = vault.library_flag(&id).unwrap().unwrap();
    assert!(flag.in_library);
    assert_eq!(flag.at, Some(1000));
    assert!(
        flag.by.as_deref().is_some_and(|b| b.starts_with("import-")),
        "{flag:?}"
    );
    assert_eq!(vault.images().unwrap().len(), 1);

    // Undo clears the flag and keeps the sightings and the files; redo sets
    // it again.
    let undone = vault.revert(&report.changes).unwrap();
    assert_eq!(undone.changed, 1);
    assert!(!vault.in_library(&id).unwrap());
    assert_eq!(vault.sightings(&id).unwrap().len(), 2);
    assert!(first.is_file() && again.is_file());
    vault.replay(&report.changes).unwrap();
    assert!(vault.in_library(&id).unwrap());
    assert_eq!(vault.sightings(&id).unwrap().len(), 2);
}

#[test]
fn a_copy_records_a_sighting_where_it_lands_and_an_add_in_place_none() {
    let p = Place::new("flag-places");
    let (here, there) = (p.join("disk/a.png"), p.join("card/DCIM/100/b.png"));
    picture(&here, 0, 1.0, 0);
    picture(&there, 1, 1.0, 0);
    let library = p.join("library");
    let vault = vault();
    // Added where it is: zero copies, its one sighting at its place.
    let (added, _) = run(&vault, None, &[p.join("disk")], &Options::default());
    assert_eq!((added.added, added.copied), (1, 0));
    let a = added.changes.images[0].image;
    let sightings = vault.sightings(&a).unwrap();
    assert_eq!(sightings.len(), 1);
    assert_eq!(sightings[0].path, here);
    assert!(!library.exists(), "nothing copied");
    // Copied into the Library: one copy, a sighting where it landed, seen
    // by the import and verified; the card's file, read whole while
    // copied, is a sighting too.
    let options = Options {
        placement: Placement::Copy {
            into: Some(library.clone()),
        },
        ..Options::default()
    };
    let (copied, _) = run(&vault, None, &[p.join("card")], &options);
    assert_eq!((copied.added, copied.copied), (1, 1));
    let b = copied.changes.images[0].image;
    let sightings = vault.sightings(&b).unwrap();
    assert_eq!(sightings.len(), 2, "{sightings:?}");
    let landed = sightings
        .iter()
        .find(|s| s.path.starts_with(&library))
        .expect("a sighting in the library folder");
    assert_eq!(landed.seen_by, "import");
    assert!(landed.last_verified.is_some());
    assert!(sightings.iter().any(|s| s.path == there), "{sightings:?}");
    assert!(vault.in_library(&a).unwrap() && vault.in_library(&b).unwrap());
}

#[test]
fn a_copy_import_run_again_reads_none_of_what_it_copied() {
    let p = Place::new("copy-again");
    let card = p.join("card/DCIM/100");
    let (a, b) = (card.join("a.png"), card.join("b.png"));
    picture(&a, 0, 1.0, 0);
    picture(&b, 1, 1.0, 0);
    let vault = vault();
    let options = Options {
        placement: Placement::Copy {
            into: Some(p.join("library")),
        },
        ..Options::default()
    };
    let sources = [p.join("card")];
    let (first, lines) = run(&vault, None, &sources, &options);
    assert_eq!((first.added, first.copied), (2, 2), "{lines:?}");
    assert!(first.read > 0);

    // Run again (a resumed card, say): each file is known by its place,
    // size and time, left out as already in the Library, nothing read.
    // `a` now holds other bytes of the same size and time: a read would
    // see another image.
    tamper(&a);
    let reads = vault.reads();
    let (again, lines) = run(&vault, None, &sources, &options);
    assert_eq!(
        (again.skipped, again.added, again.copied, again.read),
        (2, 0, 0, 0),
        "{lines:?}"
    );
    assert_eq!(
        (vault.reads().full_hashes, vault.reads().partial_hashes),
        (reads.full_hashes, reads.partial_hashes)
    );
    assert_eq!(vault.images().unwrap().len(), 2);

    // A file changed since (another time) is read and copied again.
    picture(&b, 1, 0.5, 3);
    std::fs::File::options()
        .write(true)
        .open(&b)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(2_000_000_000))
        .unwrap();
    let (changed, lines) = run(&vault, None, &sources, &options);
    assert_eq!(
        (changed.added, changed.copied, changed.skipped),
        (1, 1, 1),
        "{lines:?}"
    );
    assert_eq!(status_of(&lines, &b), "added");
    assert!(changed.read > 0);
    let held: Vec<ImageId> = first.changes.images.iter().map(|c| c.image).collect();
    let new_b = changed
        .changes
        .images
        .iter()
        .map(|c| c.image)
        .find(|id| !held.contains(id))
        .expect("the changed file is a new image");

    // An image taken out of the Library is imported again, as an import in
    // place does: its file is read and copied anew.
    vault.set_in_library(&[new_b], false, "user", 1100).unwrap();
    let (back, lines) = run(&vault, None, &sources, &options);
    assert_eq!((back.added, back.skipped), (1, 1), "{lines:?}");
    assert_eq!(status_of(&lines, &b), "added");
    assert!(back.read > 0);
    assert!(vault.in_library(&new_b).unwrap());
}

#[test]
fn a_duplicate_of_the_library_is_found_by_a_lookup_not_a_read() {
    let p = Place::new("flag-duplicate");
    let (kept, copy) = (p.join("library/a.png"), p.join("roll/a-copy.png"));
    picture(&kept, 0, 1.0, 0);
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::copy(&kept, &copy).unwrap();
    let vault = vault();
    let (report, _) = run(&vault, None, &[p.join("library")], &Options::default());
    let id = report.changes.images[0].image;
    seen_as(&vault, &copy, id);
    tamper(&copy);
    let (report, lines) = run(&vault, None, &[p.join("roll")], &Options::default());
    // The map knows it holds a Library image: already in the Library, at
    // that place, nothing read and nothing added.
    assert_eq!(
        (report.unchanged, report.added, report.read),
        (1, 0, 0),
        "{lines:?}"
    );
    assert_eq!(vault.images().unwrap().len(), 1);
    // An image only seen, off the Library, is no duplicate: it is imported.
    vault.set_in_library(&[id], false, "user", 1100).unwrap();
    let (report, lines) = run(&vault, None, &[p.join("roll")], &Options::default());
    assert_eq!((report.added, report.skipped), (1, 0), "{lines:?}");
}

/// A cull measurer noting the files it reads.
struct Noting(std::sync::Mutex<Vec<PathBuf>>);

impl helios_library::cull::Measurer for Noting {
    fn measure(&self, path: &Path) -> Result<Signals, String> {
        self.0.lock().unwrap().push(path.to_path_buf());
        Ok(Signals::default())
    }
}

/// A photo copied from a card into the Library, in a vault of its own:
/// the card's file, the library copy, and the vault knowing both.
fn copied_from_a_card(name: &str) -> (Place, Vault<MemoryStore>, ImageId, PathBuf, PathBuf) {
    let p = Place::new(name);
    let card = p.join("card/DCIM/100");
    let on_card = card.join("IMG_0001.png");
    picture(&on_card, 0, 1.0, 0);
    let vault = vault();
    let options = Options {
        placement: Placement::Copy {
            into: Some(p.join("library")),
        },
        ..Options::default()
    };
    let (report, lines) = run(&vault, None, &[p.join("card")], &options);
    assert_eq!((report.added, report.copied), (1, 1), "{lines:?}");
    let id = report.changes.images[0].image;
    let at: Vec<PathBuf> = vault
        .locations(&id)
        .unwrap()
        .into_iter()
        .map(|l| l.path)
        .collect();
    assert_eq!(at.len(), 2, "the copy and the card's file: {at:?}");
    // Measured by the import: forgotten, for the cull backfill to measure.
    vault.set_signals(&id, None).unwrap();
    let copy = at
        .iter()
        .find(|a| a.starts_with(p.join("library")))
        .expect("a library copy")
        .clone();
    (p, vault, id, std::path::absolute(&on_card).unwrap(), copy)
}

fn export_source(
    vault: &Vault<MemoryStore>,
    id: ImageId,
    out: &Path,
) -> helios_export::batch::Item {
    use helios_export::batch::{Clash, Settings};
    use helios_export::{Format, Output};
    let settings = Settings {
        output: Output::from(Format::Jpeg { quality: 90 }),
        folder: out.to_path_buf(),
        pattern: "{stem}".into(),
        clash: Clash::Number,
    };
    helios_library::export::plan(vault, &[id], settings, None)
        .unwrap()
        .items
        .remove(0)
}

fn culled(vault: &Vault<MemoryStore>) -> (helios_library::cull::Backfill, Vec<PathBuf>) {
    let measurer = Noting(std::sync::Mutex::default());
    let done =
        helios_library::cull::backfill_by(vault, &Jobs::new(Arc::default()), &measurer, None, 8)
            .unwrap();
    (done, measurer.0.into_inner().unwrap())
}

/// An image copied from a card has two locations, the library copy and
/// the card's file: export, cull and a library view read the library
/// copy, the card plugged in or not; without the library copy, the
/// card's file; without either, the image is said missing.
#[test]
fn an_image_copied_from_a_card_is_read_from_its_library_copy() {
    let (p, vault, id, on_card, copy) = copied_from_a_card("read-library-copy");
    let out = p.join("out");
    assert!(on_card.is_file() && copy.is_file());

    // Both at hand: the library copy, for export, cull and a view.
    assert_eq!(export_source(&vault, id, &out).source, copy);
    assert_eq!(vault.file_to_read(&id).unwrap().unwrap().path, copy);
    let (done, read) = culled(&vault);
    assert_eq!((done.measured, read), (1, vec![copy.clone()]));
    let rows = vault.library_rows(&[id], &[]).unwrap();
    let first = rows[0].locations.iter().find(|l| l.present()).unwrap();
    assert_eq!(first.path, copy, "a view's first present copy");

    // The library copy gone: the card's file.
    std::fs::remove_file(&copy).unwrap();
    assert_eq!(export_source(&vault, id, &out).source, on_card);

    // Neither: a clear "no copy" in the request.
    std::fs::remove_file(&on_card).unwrap();
    let item = export_source(&vault, id, &out);
    assert!(
        item.error.as_deref().is_some_and(|e| e.contains("no copy")),
        "{item:?}"
    );
    assert_eq!(vault.file_to_read(&id).unwrap(), None);
}

/// Cull without the library copy measures the card's file, and with
/// neither counts the photo missing.
#[test]
fn cull_falls_back_to_the_card_then_says_missing() {
    let (_p, vault, _, on_card, copy) = copied_from_a_card("cull-card-fallback");
    std::fs::remove_file(&copy).unwrap();
    let (done, read) = culled(&vault);
    assert_eq!((done.measured, read), (1, vec![on_card]));

    let (_p, vault, _, on_card, copy) = copied_from_a_card("cull-none-at-hand");
    std::fs::remove_file(&copy).unwrap();
    std::fs::remove_file(&on_card).unwrap();
    let (done, read) = culled(&vault);
    assert_eq!((done.measured, done.missing, read.len()), (0, 1, 0));
}
