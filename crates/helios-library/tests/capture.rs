//! Capture metadata as vault fields (`docs/design/library.md`, slice L2):
//! written at import, filled by the backfill over a vault imported
//! before them (resumable, from the files or from what the vault kept),
//! and measured on a synthetic vault of 50,000 photos, where a date
//! filter is one query.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use helios_core::jobs::Jobs;
use helios_core::{Bus, ContentHash};
use helios_library::capture::{self, Reader};
use helios_library::import::{self, Options};
use helios_vault::capture::{Capture, day_start};
use helios_vault::storage::VolumeId;
use helios_vault::{Image, ImageId, Location, MemoryStore, Presence, Query, Store, Vault};
use helios_vault_sqlite::SqliteStore;
use serde_json::json;

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-library-capture-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn picture(path: &Path, w: u32, h: u32, shade: u8) {
    image::RgbImage::from_pixel(w, h, image::Rgb([shade, 40, 90]))
        .save(path)
        .unwrap();
}

fn jobs() -> Jobs {
    Jobs::new(Arc::new(Bus::new()))
}

#[test]
fn an_import_records_each_photo_s_capture() {
    let dir = scratch("import");
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    picture(&photos.join("a.png"), 30, 20, 1);
    picture(&photos.join("B.JPG"), 16, 12, 200);
    let vault = Vault::new(MemoryStore::default());
    let options = Options {
        signals: false,
        ..Options::default()
    };
    import::import(
        &vault,
        &jobs(),
        std::slice::from_ref(&photos),
        &options,
        1000,
        &mut |_| {},
    )
    .unwrap();
    let ids: Vec<ImageId> = vault.images().unwrap().iter().map(|i| i.id).collect();
    assert_eq!(ids.len(), 2);
    let mut seen: Vec<(String, i64, i64, String)> = vault
        .captures_of(&ids)
        .unwrap()
        .into_iter()
        .map(|c| {
            let c = c.expect("captured at import");
            assert!(c.captured.is_some(), "the file's time stands in");
            (
                c.file_type.unwrap(),
                c.width.unwrap(),
                c.height.unwrap(),
                c.captured_from.unwrap(),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("jpg".into(), 16, 12, "file".into()),
            ("png".into(), 30, 20, "file".into())
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reads like the files, counting, and asks the run to stop after
/// `stop_after` reads.
struct Counting {
    reads: AtomicUsize,
    stop_after: usize,
    cancel: Arc<AtomicBool>,
}

impl Reader for Counting {
    fn read(&self, path: &Path) -> Result<Capture, String> {
        if self.reads.fetch_add(1, Ordering::SeqCst) + 1 >= self.stop_after {
            self.cancel.store(true, Ordering::SeqCst);
        }
        capture::read(path)
    }
}

/// A vault imported before L2 (the vault's own import: no capture), in a
/// SQLite file opened again: the backfill fills it from the files, from
/// the import's signals for a photo whose disk is unplugged, stops when
/// cancelled and resumes after the last batch kept, then never runs
/// again; a capture range then selects in one query.
#[test]
fn the_backfill_fills_an_existing_vault_and_resumes_where_it_stopped() {
    let dir = scratch("backfill");
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    for i in 0..7u8 {
        picture(
            &photos.join(format!("p{i}.png")),
            10 + u32::from(i),
            8,
            i * 30,
        );
    }
    let path = dir.join("vault.db");
    let away;
    {
        let vault = Vault::new(SqliteStore::open(&path).unwrap());
        assert_eq!(vault.import(&photos, 1).unwrap().added, 7);
        let ids: Vec<ImageId> = vault.images().unwrap().iter().map(|i| i.id).collect();
        assert!(vault.captures_of(&ids).unwrap().iter().all(Option::is_none));
        // One photo's copy is on a disk not plugged in; its import
        // measured signals.
        away = ids[3];
        let l = vault.locations(&away).unwrap().remove(0);
        vault
            .update_locations(&[(
                away,
                l.path.clone(),
                Location {
                    volume: Some(VolumeId("uuid:away".into())),
                    rel_path: Some("p.png".into()),
                    presence: Presence::Offline,
                    ..l
                },
            )])
            .unwrap();
        vault
            .set_signals(
                &away,
                Some(&json!({
                    "taken": day_start("2023-05-04").unwrap() as f64 + 3600.0,
                    "taken_from": "exif",
                    "maker": "Canon",
                    "model": "EOS R5",
                    "iso": 200.0,
                    "aperture": 8.0,
                })),
            )
            .unwrap();
    }
    let vault = Vault::new(SqliteStore::open(&path).unwrap());
    let ids: Vec<ImageId> = vault.images().unwrap().iter().map(|i| i.id).collect();
    // Stopped after its first batch of two.
    let cancel = Arc::new(AtomicBool::new(false));
    let reader = Counting {
        reads: AtomicUsize::new(0),
        stop_after: 1,
        cancel: cancel.clone(),
    };
    let first = capture::backfill_by(&vault, &jobs(), &reader, Some(cancel), 2).unwrap();
    assert!(!first.finished);
    assert_eq!((first.done, first.total), (2, 7));
    assert_eq!(first.after, Some(ids[1].hex()));
    let done = vault.captures_of(&ids).unwrap();
    assert!(done[..2].iter().all(Option::is_some));
    assert!(
        done[2..].iter().all(Option::is_none),
        "nothing past the batch kept"
    );
    // Resumed, in a vault opened again: the rest, none read twice.
    drop(vault);
    let vault = Vault::new(SqliteStore::open(&path).unwrap());
    let reader = Counting {
        reads: AtomicUsize::new(0),
        stop_after: usize::MAX,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let rest = capture::backfill_by(&vault, &jobs(), &reader, None, 2).unwrap();
    assert!(rest.finished);
    assert_eq!((rest.done, rest.total), (7, 7));
    assert_eq!(
        reader.reads.load(Ordering::SeqCst),
        4,
        "the offline one is not read"
    );
    assert_eq!((rest.read, rest.kept), (6, 1));
    let all = vault.captures_of(&ids).unwrap();
    for (id, c) in ids.iter().zip(&all) {
        let c = c.as_ref().unwrap();
        if *id == away {
            assert_eq!(c.model.as_deref(), Some("EOS R5"));
            assert_eq!(c.iso, Some(200));
            assert_eq!(c.captured_from.as_deref(), Some("exif"));
            assert_eq!(c.day(), Some((2023, 5, 4)));
        } else {
            assert_eq!(c.file_type.as_deref(), Some("png"));
            assert_eq!(c.height, Some(8));
            assert_eq!(c.captured_from.as_deref(), Some("file"));
        }
    }
    // Finished: it does not run again.
    let again = capture::backfill(&vault, &jobs(), &reader, None).unwrap();
    assert_eq!(again, rest);
    assert_eq!(reader.reads.load(Ordering::SeqCst), 4);
    // The fourth of May 2023, by the camera's clock: one query.
    let q0 = vault.reads().queries;
    let may = vault
        .open_cursor(Query {
            captured_from: day_start("2023-05-04"),
            captured_before: day_start("2023-05-05"),
            ..Query::default()
        })
        .unwrap();
    assert_eq!(vault.reads().queries - q0, 1);
    assert_eq!(vault.cursor_ids(may.id, 0, 9).unwrap(), [away]);
    drop(vault);
    let _ = std::fs::remove_dir_all(&dir);
}

fn id(i: u32) -> ImageId {
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(&i.to_be_bytes());
    bytes[31] = 2;
    ImageId(ContentHash(bytes))
}

/// Day `i % 365` of 2024, at noon, by the camera's clock.
fn taken(i: u32) -> f64 {
    (day_start("2024-01-01").unwrap() + i64::from(i % 365) * 86_400 + 12 * 3600) as f64
}

/// What a file's metadata says, without the disk: the bench measures the
/// backfill's own cost (a metadata read costs what the disk costs).
struct Synthetic;

impl Reader for Synthetic {
    fn read(&self, path: &Path) -> Result<Capture, String> {
        let i: u32 = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("IMG_"))
            .and_then(|s| s.parse().ok())
            .ok_or("no number")?;
        Ok(Capture {
            captured: Some(taken(i)),
            offset: Some(60),
            captured_from: Some("exif".into()),
            make: Some("Nikon".into()),
            model: Some("Z 8".into()),
            lens: Some("NIKKOR Z 50mm f/1.8 S".into()),
            focal_length: Some(50.0),
            aperture: Some(2.8),
            shutter: Some(1.0 / 500.0),
            iso: Some(100),
            width: Some(8256),
            height: Some(5504),
            orientation: Some(1),
            file_type: Some("nef".into()),
        })
    }
}

/// A vault of `n` photos imported before L2: nine in ten present, one in
/// ten on an unplugged disk with the import's signals.
fn synthetic(path: &Path, n: u32) {
    let mut store = SqliteStore::open(path).unwrap();
    store.begin().unwrap();
    for i in 0..n {
        let id = id(i);
        store
            .add_to_library(&Image {
                id,
                size: 10_000 + u64::from(i),
                partial: id.0,
            })
            .unwrap();
        let at = PathBuf::from(format!("/photos/IMG_{i:06}.nef"));
        let seen = Location::seen(at, 10_000 + u64::from(i), 1_700_000_000, 1);
        let location = if i % 10 == 7 {
            store
                .set_document(
                    "signals",
                    &id.hex(),
                    Some(&json!({ "taken": taken(i), "taken_from": "exif", "model": "Z 8" })),
                )
                .unwrap();
            Location {
                volume: Some(VolumeId("uuid:away".into())),
                rel_path: Some(format!("IMG_{i:06}.nef").into()),
                presence: Presence::Offline,
                ..seen
            }
        } else {
            seen
        };
        store.put_location(&id, &location).unwrap();
    }
    store.commit().unwrap();
}

/// The backfill over 50,000 photos, timed, and a date filter on the
/// result: one query, timed. Prints what it cost as a JSON line.
#[test]
fn the_backfill_at_scale_measured_and_a_date_filter_is_one_query() {
    let n = 50_000;
    let dir = scratch("scale");
    let path = dir.join("vault.db");
    synthetic(&path, n);
    let vault = Vault::new(SqliteStore::open(&path).unwrap());
    let q0 = vault.reads().queries;
    let start = Instant::now();
    let done = capture::backfill(&vault, &jobs(), &Synthetic, None).unwrap();
    let backfill = start.elapsed();
    assert!(done.finished);
    assert_eq!((done.done, done.read, done.kept), (50_000, 45_000, 5_000));
    let backfill_queries = vault.reads().queries - q0;
    // One day of 2024: 50,000 photos over 365 days, about 137 a day.
    let q0 = vault.reads().queries;
    let start = Instant::now();
    let day = vault
        .open_cursor(Query {
            captured_from: day_start("2024-03-01"),
            captured_before: day_start("2024-03-02"),
            ..Query::default()
        })
        .unwrap();
    let filter = start.elapsed();
    assert_eq!(
        vault.reads().queries - q0,
        1,
        "the date filter is one query"
    );
    let expected = (0..n).filter(|i| i % 365 == 60).count();
    assert_eq!(day.count, expected);
    println!(
        "capture-backfill {}",
        json!({
            "images": n,
            "backfill_ms": backfill.as_millis(),
            "batch_queries": backfill_queries,
            "date_filter_ms": filter.as_micros() as f64 / 1000.0,
            "date_filter_matches": day.count,
        })
    );
    assert!(
        backfill < Duration::from_secs(90),
        "backfill in {backfill:?}"
    );
    assert!(filter < Duration::from_secs(2), "date filter in {filter:?}");
    drop(vault);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The date filter is served by the `(name, number)` index, never a scan
/// of every field.
#[test]
fn the_date_filter_uses_the_index() {
    let dir = scratch("plan");
    let path = dir.join("vault.db");
    synthetic(&path, 50);
    let vault = Vault::new(SqliteStore::open(&path).unwrap());
    capture::backfill(&vault, &jobs(), &Synthetic, None).unwrap();
    drop(vault);
    let plan = SqliteStore::open(&path)
        .unwrap()
        .explain_select(&Query {
            captured_from: day_start("2024-03-01"),
            captured_before: day_start("2024-03-02"),
            ..Query::default()
        })
        .unwrap();
    assert!(plan.contains("fields_name_number"), "{plan}");
    let _ = std::fs::remove_dir_all(&dir);
}
