//! Importers against a test plugin (`tests/fixtures/ratings-folder`,
//! Python with its standard library alone, over the JPEGs and
//! `ratings.json` of `tests/fixtures/rated-photos`): the manifest's
//! importers, a scan checked, its progress, a source refused before the
//! plugin starts, a scan cancelled.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helios_plugin::manifest::SourceKind;
use helios_plugin::{Broker, Cancel, Policy, Scope};
use serde_json::Value as Json;

const PLUGIN: &str = "org.example.ratings-folder";
const IMPORTER: &str = "ratings-folder";
/// Only a hang reaches it.
const DEADLINE: Duration = Duration::from_secs(30);

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn python() -> bool {
    let ok = Command::new("python3").arg("--version").output().is_ok();
    if !ok {
        eprintln!("python3 is not installed: skipped");
    }
    ok
}

/// A copy of the rated photos, in a directory of the test's own.
fn photos(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-importer-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(fixtures().join("rated-photos")).unwrap() {
        let path = entry.unwrap().path();
        std::fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
    }
    dir
}

fn broker() -> Broker {
    let mut b = Broker::new(Policy {
        backoff: Duration::ZERO,
        ..Policy::default()
    });
    b.add(&fixtures().join("ratings-folder"), Scope::Builtin)
        .unwrap();
    b
}

fn jpeg(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg"))
}

#[test]
fn importers_are_declared_and_checked_in_the_manifest() {
    let b = broker();
    let importers = b.importers();
    assert_eq!(importers.len(), 1);
    let (plugin, decl) = &importers[0];
    assert_eq!(plugin, PLUGIN);
    assert_eq!(
        (decl.id.as_str(), decl.source, decl.label.as_str()),
        (IMPORTER, SourceKind::Folder, "Folder with ratings.json")
    );
    assert_eq!(b.list()["plugins"][0]["importers"][0]["id"], IMPORTER);
    // Disabled, its importers are not offered.
    b.set_enabled(PLUGIN, false).unwrap();
    assert!(b.importers().is_empty());

    let text =
        std::fs::read_to_string(fixtures().join("ratings-folder/helios-plugin.toml")).unwrap();
    let refused = |from: &str, to: &str, expect: &str| {
        let dir = std::env::temp_dir().join(format!(
            "helios-importer-manifest-{}-{}",
            std::process::id(),
            expect.len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("helios-plugin.toml"), text.replace(from, to)).unwrap();
        let problems = Broker::new(Policy::default())
            .add(&dir, Scope::User)
            .err()
            .unwrap_or_default();
        assert!(
            problems.iter().any(|p| p.contains(expect)),
            "{from} -> {to}: {problems:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    };
    refused(
        r#"name = "source.read""#,
        r#"name = "images.read""#,
        "without the capability `source.read`",
    );
    refused(
        r#"services = ["importer"]"#,
        "services = []",
        "without the service `importer`",
    );
    refused(
        r#"holds = ["ratings.json"]"#,
        r#"patterns = ["*.json"]"#,
        "`patterns` are for a file source",
    );
    refused(
        "onImporter:ratings-folder",
        "onImporter:other",
        "names nothing this plugin declares",
    );
    refused(
        "importer = 1",
        "importer = 3",
        "interface `importer` version 3",
    );
}

#[test]
fn a_scan_lists_the_images_with_their_ratings_and_progress() {
    if !python() {
        return;
    }
    let b = broker();
    let dir = photos("scan");
    let seen = Arc::new(Mutex::new(Vec::<Json>::new()));
    let s = seen.clone();
    let scan = b
        .importer_scan(PLUGIN, IMPORTER, &dir, &jpeg, None, move |p| {
            s.lock().unwrap().push(p.clone());
        })
        .unwrap();
    let dir = dir.canonicalize().unwrap();
    assert_eq!(
        (scan.plugin.as_str(), scan.source.as_path()),
        (PLUGIN, dir.as_path())
    );
    let names: Vec<&str> = scan
        .images
        .iter()
        .map(|i| i.path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(names, ["a.jpg", "b.jpg", "c.jpg"]);
    let a = &scan.images[0];
    assert_eq!(a.path, dir.join("a.jpg"));
    assert_eq!(
        (
            a.rating,
            a.labels.as_slice(),
            a.tags.as_slice(),
            a.title.as_deref()
        ),
        (
            Some(5),
            ["red".to_string()].as_slice(),
            ["trip".to_string(), "beach".to_string()].as_slice(),
            Some("Low tide")
        )
    );
    assert_eq!(
        (scan.images[1].rating, scan.images[1].rejected),
        (Some(2), Some(true))
    );
    assert_eq!(scan.images[2].rating, None);
    assert!(scan.refused.is_empty(), "{:?}", scan.refused);
    assert_eq!(scan.warnings.len(), 1);
    assert!(scan.warnings[0].contains("gone.jpg"), "{:?}", scan.warnings);
    // Its progress, each image then the end, came to this scan's caller.
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 4, "{seen:?}");
    assert_eq!(seen[3]["done"], 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_source_the_importer_does_not_read_is_refused_before_the_plugin_starts() {
    let b = broker();
    let dir = photos("refused");
    std::fs::remove_file(dir.join("ratings.json")).unwrap();
    let e = b
        .importer_scan(PLUGIN, IMPORTER, &dir, &jpeg, None, |_| {})
        .unwrap_err();
    assert!(e.contains("ratings.json"), "{e}");
    assert!(!b.running(PLUGIN));
    let e = b
        .importer_scan(PLUGIN, IMPORTER, &dir.join("a.jpg"), &jpeg, None, |_| {})
        .unwrap_err();
    assert!(e.contains("not a folder"), "{e}");
    let e = b
        .importer_scan(PLUGIN, "nope", &dir, &jpeg, None, |_| {})
        .unwrap_err();
    assert!(e.contains("no importer `nope`"), "{e}");
    assert!(!b.running(PLUGIN));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_scan_is_cancelled_and_the_plugin_goes_on() {
    if !python() {
        return;
    }
    let b = Arc::new(broker());
    let dir = photos("cancel");
    std::fs::write(dir.join("hang"), "").unwrap();
    let cancel = Cancel::new();
    let (waiting, started) = channel();
    let (done, ended) = channel();
    {
        let (b, dir, cancel) = (b.clone(), dir.clone(), cancel.clone());
        let waiting = Mutex::new(waiting);
        std::thread::spawn(move || {
            let r = b.importer_scan(PLUGIN, IMPORTER, &dir, &jpeg, Some(&cancel), move |p| {
                if p["message"] == "waiting" {
                    let _ = waiting.lock().unwrap().send(());
                }
            });
            let _ = done.send(r);
        });
    }
    started
        .recv_timeout(DEADLINE)
        .expect("the plugin started its scan");
    cancel.cancel();
    let r = ended.recv_timeout(DEADLINE).expect("the scan gave up");
    let e = r.unwrap_err();
    assert!(e.contains("cancelled"), "{e}");
    assert!(cancel.is_cancelled());
    // Cancelled before it starts, a scan is never sent.
    assert!(
        b.importer_scan(PLUGIN, IMPORTER, &dir, &jpeg, Some(&cancel), |_| {})
            .is_err()
    );
    // The plugin is still there, and scans again.
    assert!(b.running(PLUGIN));
    std::fs::remove_file(dir.join("hang")).unwrap();
    let scan = b
        .importer_scan(PLUGIN, IMPORTER, &dir, &jpeg, None, |_| {})
        .unwrap();
    assert_eq!(scan.images.len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}
