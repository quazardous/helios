//! The darktable library importer as Helios ships it: its program built
//! beside the tests, written out with its manifest as Helios does
//! (`helios_plugin::bundled`) and run by the broker, on small darktable
//! folders built from darktable's own `CREATE TABLE` statements (4.6 and
//! 5.2, the `fixture` module): what it lists, the schemas it refuses, a
//! library darktable holds open read from a copy, a scan cancelled.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helios_plugin::{Broker, Cancel, Policy, bundled};
use helios_plugin_darktable_library::HOLD;
use helios_plugin_darktable_library::fixture::{self, Options, Schema};
use serde_json::Value as Json;

const PLUGIN: &str = "io.github.quazardous.helios.darktable-library";
const IMPORTER: &str = "darktable-library";
const PROGRAM: &str = env!("CARGO_BIN_EXE_helios-darktable-library");
/// Only a hang reaches it.
const DEADLINE: Duration = Duration::from_secs(30);

/// The test JPEGs of the broker's tests.
fn rated_photos() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../crates/helios-plugin/tests/fixtures/rated-photos")
}

/// A directory of the test's own.
fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-dt-library-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A darktable folder (`<dir>/darktable`) over a copy of the rated photos
/// plus a file with a non-ASCII name (`<dir>/photos`).
fn library(dir: &Path, schema: Schema, options: &Options) -> PathBuf {
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        std::fs::copy(rated_photos().join(name), photos.join(name)).unwrap();
    }
    std::fs::copy(rated_photos().join("c.jpg"), photos.join("Été à Lyon.jpg")).unwrap();
    let out = dir.join("darktable");
    fixture::build(&out, schema, &photos, options).unwrap();
    out
}

/// A broker holding the importer as Helios ships it, written into `dir`.
fn broker(dir: &Path, policy: Policy) -> Broker {
    let mut dirs = Vec::new();
    let programs = Path::new(PROGRAM).parent().unwrap();
    let added = bundled::add_to(&mut dirs, &dir.join("plugins"), programs).unwrap();
    assert!(added.dir.is_some(), "{added:?}");
    let mut b = Broker::new(policy);
    b.discover(&dirs);
    assert!(b.rejected().is_empty(), "{:?}", b.rejected());
    b
}

fn quick() -> Policy {
    Policy {
        backoff: Duration::ZERO,
        ..Policy::default()
    }
}

fn jpeg(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg"))
}

fn name(p: &Path) -> &str {
    p.file_name().unwrap().to_str().unwrap()
}

#[test]
fn the_importer_is_declared_as_the_study_says() {
    let dir = scratch("declared");
    let b = broker(&dir, quick());
    let importers = b.importers();
    assert_eq!(importers.len(), 1);
    let (plugin, decl) = &importers[0];
    assert_eq!(plugin, PLUGIN);
    assert_eq!(
        (decl.id.as_str(), decl.label.as_str(), decl.anywhere),
        (IMPORTER, "darktable library", true)
    );
    assert_eq!(decl.holds, ["library.db"]);
    // A folder without library.db is refused before the plugin starts.
    let e = b
        .importer_scan(PLUGIN, IMPORTER, &dir, &jpeg, None, |_| {})
        .unwrap_err();
    assert!(e.contains("library.db"), "{e}");
    assert!(!b.running(PLUGIN));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_scan_lists_each_file_with_darktable_s_marks_in_both_schemas() {
    for schema in [Schema::V4_6, Schema::V5_2] {
        let dir = scratch("scan");
        let source = library(&dir, schema, &Options::default());
        let b = broker(&dir, quick());
        let seen = Arc::new(Mutex::new(Vec::<Json>::new()));
        let s = seen.clone();
        let scan = b
            .importer_scan(PLUGIN, IMPORTER, &source, &jpeg, None, move |p| {
                s.lock().unwrap().push(p.clone());
            })
            .unwrap_or_else(|e| panic!("{schema:?}: {e}"));
        assert!(scan.refused.is_empty(), "{schema:?}: {:?}", scan.refused);
        // One entry per file: a.jpg's duplicate is not a second one; the
        // file on the missing drive is left out.
        let names: Vec<&str> = scan.images.iter().map(|i| name(&i.path)).collect();
        assert_eq!(
            names,
            ["a.jpg", "b.jpg", "c.jpg", "Été à Lyon.jpg"],
            "{schema:?}"
        );
        let photos = dir.join("photos").canonicalize().unwrap();
        assert_eq!(scan.images[0].path, photos.join("a.jpg"), "{schema:?}");
        let a = &scan.images[0];
        assert_eq!(
            (a.rating, a.rejected, a.title.as_deref()),
            (Some(3), Some(false), Some("Low tide")),
            "{schema:?}"
        );
        // The labels and tags of the original and its duplicate; the
        // whole tag path, darktable's own tags left out.
        assert_eq!(a.labels, ["red", "blue"], "{schema:?}");
        assert_eq!(a.tags, ["people|Ann", "places|France|Lyon"], "{schema:?}");
        let b_jpg = &scan.images[1];
        assert_eq!(
            (b_jpg.rating, b_jpg.rejected, b_jpg.description.as_deref()),
            (Some(2), Some(true), Some("Rocks")),
            "{schema:?}: a reject keeps its stars"
        );
        assert_eq!(b_jpg.labels, ["green"], "{schema:?}");
        let c = &scan.images[2];
        assert_eq!((c.rating, c.rejected), (Some(0), Some(false)), "{schema:?}");
        assert!(c.labels.is_empty() && c.tags.is_empty() && c.title.is_none());
        assert_eq!(scan.images[3].rating, Some(1), "{schema:?}");
        // Warned: the missing drive, the duplicate whose stars differ, the
        // creator Helios has no field for.
        let warned = |what: &str| scan.warnings.iter().any(|w| w.contains(what));
        assert!(
            warned("1 photo of the library left out") && warned("drive-not-plugged-in"),
            "{schema:?}: {:?}",
            scan.warnings
        );
        assert!(
            warned("duplicates whose stars or reject differ") && warned("a.jpg"),
            "{schema:?}: {:?}",
            scan.warnings
        );
        assert!(
            warned("Xmp.dc.creator (1)"),
            "{schema:?}: {:?}",
            scan.warnings
        );
        assert!(!warned("darktable was running"), "{schema:?}");
        // Progress, from nothing to every file.
        let seen = seen.lock().unwrap();
        assert_eq!(seen.first().unwrap()["done"], 0, "{schema:?}: {seen:?}");
        assert_eq!(seen.last().unwrap()["done"], 5, "{schema:?}: {seen:?}");
        assert_eq!(seen.last().unwrap()["total"], 5, "{schema:?}: {seen:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn without_data_db_the_tags_are_skipped_and_said() {
    let dir = scratch("no-data");
    let source = library(
        &dir,
        Schema::V4_6,
        &Options {
            no_data: true,
            ..Options::default()
        },
    );
    let b = broker(&dir, quick());
    let scan = b
        .importer_scan(PLUGIN, IMPORTER, &source, &jpeg, None, |_| {})
        .unwrap();
    assert_eq!(scan.images.len(), 4);
    assert!(scan.images.iter().all(|i| i.tags.is_empty()));
    // The title still comes, by darktable's fixed keys.
    assert_eq!(scan.images[0].title.as_deref(), Some("Low tide"));
    assert!(
        scan.warnings.iter().any(|w| w.contains("no data.db")),
        "{:?}",
        scan.warnings
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_schema_the_importer_does_not_know_is_refused_with_its_number() {
    for (schema, options, expect) in [
        (
            Schema::V5_2,
            Options {
                library_version: Some(58),
                ..Options::default()
            },
            "newer than this importer knows (library schema 58; it reads 36 to 57)",
        ),
        (
            Schema::V4_6,
            Options {
                library_version: Some(34),
                ..Options::default()
            },
            "darktable library from an older darktable (schema 34): open it once in darktable 4.0",
        ),
        (
            Schema::V5_2,
            Options {
                data_version: Some(14),
                ..Options::default()
            },
            "(data schema 14; it reads 9 to 13)",
        ),
    ] {
        let dir = scratch("refused");
        let source = library(&dir, schema, &options);
        let b = broker(&dir, quick());
        let e = b
            .importer_scan(PLUGIN, IMPORTER, &source, &jpeg, None, |_| {})
            .unwrap_err();
        assert!(e.contains(expect), "{options:?}: {e}");
        assert!(e.contains("-32001"), "{options:?}: {e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_library_darktable_holds_open_is_read_from_a_copy_and_never_written() {
    // darktable's lock holds its pid: this test's own, alive.
    let dir = scratch("locked");
    let source = library(
        &dir,
        Schema::V5_2,
        &Options {
            lock_pid: Some(std::process::id()),
            ..Options::default()
        },
    );
    let files = ["library.db", "data.db", "library.db.lock"];
    let read = || -> Vec<Vec<u8>> {
        files
            .iter()
            .map(|f| std::fs::read(source.join(f)).unwrap())
            .collect()
    };
    let before = read();
    let b = broker(&dir, quick());
    let scan = b
        .importer_scan(PLUGIN, IMPORTER, &source, &jpeg, None, |_| {})
        .unwrap();
    assert_eq!(scan.images.len(), 4);
    assert_eq!(scan.images[0].rating, Some(3));
    assert!(
        scan.warnings
            .iter()
            .any(|w| w.starts_with("darktable was running")),
        "{:?}",
        scan.warnings
    );
    assert!(before == read(), "the darktable folder was changed");
    let mut left: Vec<String> = std::fs::read_dir(&source)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["data.db", "library.db", "library.db.lock"]);
    let _ = std::fs::remove_dir_all(&dir);

    // A lock left by a darktable that is gone (a crash) is stale: the
    // files are read in place. Where a process cannot be probed
    // (Windows), every lock counts as held: a copy is harmless.
    if !cfg!(unix) {
        return;
    }
    let mut child = Command::new(PROGRAM)
        .stdin(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let gone = child.id();
    child.wait().unwrap();
    let dir = scratch("stale");
    let source = library(
        &dir,
        Schema::V5_2,
        &Options {
            lock_pid: Some(gone),
            ..Options::default()
        },
    );
    let b = broker(&dir, quick());
    let scan = b
        .importer_scan(PLUGIN, IMPORTER, &source, &jpeg, None, |_| {})
        .unwrap();
    assert!(
        !scan
            .warnings
            .iter()
            .any(|w| w.starts_with("darktable was running")),
        "{:?}",
        scan.warnings
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_scan_is_cancelled_and_the_plugin_goes_on() {
    let dir = scratch("cancel");
    let source = library(&dir, Schema::V5_2, &Options::default());
    // The importer's test hook: the scan waits after its first progress
    // notice until it is cancelled.
    let b = Arc::new(broker(
        &dir,
        Policy {
            env: vec![(HOLD.into(), "1".into())],
            ..quick()
        },
    ));
    let cancel = Cancel::new();
    let (waiting, started) = channel();
    let (done, ended) = channel();
    {
        let (b, source, cancel) = (b.clone(), source.clone(), cancel.clone());
        let waiting = Mutex::new(waiting);
        std::thread::spawn(move || {
            let r = b.importer_scan(PLUGIN, IMPORTER, &source, &jpeg, Some(&cancel), move |_| {
                let _ = waiting.lock().unwrap().send(());
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
    // The plugin is still there.
    assert!(b.running(PLUGIN));
    let _ = std::fs::remove_dir_all(&dir);
}
