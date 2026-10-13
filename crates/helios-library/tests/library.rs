//! Library operations end to end on fixture files: import with
//! duplicates, copies renamed with their sidecars, the job's events, the
//! perceptual signals, and stack suggestions accepted and dissolved.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::Bus;
use helios_library::import::{self, Duplicates, Options, Placement};
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
    let mut lines = Vec::new();
    let report = import::import(vault, bus, sources, options, 1000, &mut |l: &Value| {
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
    // The copy is the image; the card is left as it was.
    let id = ImageId::from_hex(lines[0]["id"].as_str().unwrap()).unwrap();
    let at: Vec<PathBuf> = vault
        .locations(&id)
        .unwrap()
        .into_iter()
        .map(|l| l.path)
        .collect();
    assert_eq!(at, [first]);
    assert!(dcim.join("IMG_0001.png").exists());
    // The library folder is remembered for the next copy.
    assert_eq!(import::library_root(&vault).unwrap(), Some(library.clone()));

    let events = bus.since(0);
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| e.kind)
        .filter(|k| k.starts_with("job."))
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
    assert_eq!(started["kind"], "library.import");
    assert_eq!(started["total"], 2);
    let progress: Vec<&Value> = events
        .iter()
        .filter(|e| e.kind == "job.progress")
        .map(|e| &e.payload)
        .collect();
    assert_eq!(progress[0]["done"], 0);
    assert_eq!(progress[0]["current"], "IMG_0001.png");
    assert_eq!(progress[2]["done"], 2);
    let finished = &events
        .iter()
        .find(|e| e.kind == "job.finished")
        .unwrap()
        .payload;
    assert_eq!(finished["summary"]["copied"], 2);

    // Renaming in place is refused: a referenced file stays as it is.
    let refused = import::import(
        &vault,
        Some(bus.clone()),
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
