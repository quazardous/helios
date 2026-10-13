//! Library operations end to end on fixture files: import with
//! duplicates, copies renamed with their sidecars, the job's events, the
//! perceptual signals.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::Bus;
use helios_library::import::{self, Duplicates, Options, Placement};
use helios_library::rename::Pattern;
use helios_library::signals::{self, Signals, distance};
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
