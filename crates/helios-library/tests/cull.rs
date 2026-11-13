//! Culling signals (focus, clipping, exposure) kept with an image's
//! signals: measured at import, the same numbers on every run, and filled
//! by the backfill over a vault imported before them (resumable, the
//! signals already kept left in place).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_library::cull::{self, Cull, Measurer};
use helios_library::import::{self, Options};
use helios_library::signals::{self, Signals};
use helios_vault::{ImageId, MemoryStore, Vault};
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-library-cull-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A 96 x 64 checker, crisp or softened by a box blur of `blur` pixels
/// each way; `seed` shifts it so each picture is its own content.
fn picture(path: &Path, blur: i64, seed: u32) {
    let at = |x: i64, y: i64| -> f32 {
        if ((x + i64::from(seed)) / 6 + y / 6) % 2 == 0 {
            40.0
        } else {
            200.0
        }
    };
    image::RgbImage::from_fn(96, 64, |x, y| {
        let (x, y) = (i64::from(x), i64::from(y));
        let mut s = 0.0;
        let mut n = 0.0;
        for dy in -blur..=blur {
            for dx in -blur..=blur {
                s += at((x + dx).clamp(0, 95), (y + dy).clamp(0, 63));
                n += 1.0;
            }
        }
        let v = (s / n) as u8;
        image::Rgb([v, v, v])
    })
    .save(path)
    .unwrap();
}

fn jobs() -> Jobs {
    Jobs::new(Arc::new(Bus::new()))
}

fn kept(vault: &Vault<impl helios_vault::Store>, id: &ImageId) -> Value {
    vault.signals(id).unwrap().expect("signals kept")
}

#[test]
fn an_import_keeps_the_culling_signals_and_they_repeat() {
    let dir = scratch("import");
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    picture(&photos.join("sharp.png"), 0, 0);
    picture(&photos.join("soft.png"), 3, 1);
    let vault = Vault::new(MemoryStore::default());
    import::import(
        &vault,
        &jobs(),
        std::slice::from_ref(&photos),
        &Options::default(),
        1000,
        &mut |_| {},
    )
    .unwrap();
    let mut focus = Vec::new();
    for image in vault.images().unwrap() {
        let s = kept(&vault, &image.id);
        assert!(Cull::current(&s), "{s}");
        assert!(s["phash"].is_string(), "the other signals are kept: {s}");
        assert_eq!(s["clip_white"], json!([0.0, 0.0, 0.0]));
        assert!(s["lum_median"].as_f64().unwrap() > 0.0);
        let at = s["focus_at"].as_array().unwrap();
        assert_eq!(at.len(), 2);
        let name = vault.sightings(&image.id).unwrap()[0].path.clone();
        focus.push((name, s["focus"].as_f64().unwrap()));
    }
    focus.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(focus[0].1 > 2.0 * focus[1].1, "sharp over soft: {focus:?}");
    // Measured again, the same numbers to the last bit.
    let once = signals::measure(&photos.join("sharp.png")).unwrap();
    let twice = signals::measure(&photos.join("sharp.png")).unwrap();
    assert_eq!(
        serde_json::to_string(&once.cull).unwrap(),
        serde_json::to_string(&twice.cull).unwrap()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Measures like the files, counting, and asks the run to stop after
/// `stop_after` measures.
struct Counting {
    measures: AtomicUsize,
    stop_after: usize,
    cancel: Arc<AtomicBool>,
}

impl Measurer for Counting {
    fn measure(&self, path: &Path) -> Result<Signals, String> {
        if self.measures.fetch_add(1, Ordering::SeqCst) + 1 >= self.stop_after {
            self.cancel.store(true, Ordering::SeqCst);
        }
        signals::measure(path)
    }
}

/// A vault imported before the culling signals (the vault's own import:
/// no signals; one photo with older signals), in a SQLite file opened
/// again: the backfill stops when cancelled, resumes after the last batch
/// kept, adds the culling keys beside the older ones, then never runs
/// again.
#[test]
fn the_cull_backfill_fills_old_photos_and_resumes_where_it_stopped() {
    let dir = scratch("backfill");
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    for i in 0..5 {
        picture(&photos.join(format!("p{i}.png")), i64::from(i % 2), i);
    }
    let path = dir.join("vault.db");
    let older;
    {
        let vault = Vault::new(SqliteStore::open(&path).unwrap());
        assert_eq!(vault.import(&photos, 1).unwrap().added, 5);
        let ids: Vec<ImageId> = vault.images().unwrap().iter().map(|i| i.id).collect();
        assert!(ids.iter().all(|id| vault.signals(id).unwrap().is_none()));
        older = ids[2];
        vault
            .set_signals(
                &older,
                Some(&json!({ "phash": "00ff00ff00ff00ff", "sharpness": 1.5 })),
            )
            .unwrap();
    }
    let vault = Vault::new(SqliteStore::open(&path).unwrap());
    let mut ids: Vec<ImageId> = vault.images().unwrap().iter().map(|i| i.id).collect();
    ids.sort();
    let cancel = Arc::new(AtomicBool::new(false));
    let measurer = Counting {
        measures: AtomicUsize::new(0),
        stop_after: 1,
        cancel: cancel.clone(),
    };
    let first = cull::backfill_by(&vault, &jobs(), &measurer, Some(cancel), 2).unwrap();
    assert!(!first.finished);
    assert_eq!((first.done, first.total, first.measured), (2, 5, 2));
    assert_eq!(first.after, Some(ids[1].hex()));
    assert_eq!(cull::progress(&vault).unwrap(), Some(first.clone()));
    for (i, id) in ids.iter().enumerate() {
        let current = vault
            .signals(id)
            .unwrap()
            .is_some_and(|s| Cull::current(&s));
        assert_eq!(current, i < 2, "only the first batch is kept");
    }
    // Resumed, in a vault opened again: the rest, none measured twice.
    drop(vault);
    let vault = Vault::new(SqliteStore::open(&path).unwrap());
    let measurer = Counting {
        measures: AtomicUsize::new(0),
        stop_after: usize::MAX,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let rest = cull::backfill_by(&vault, &jobs(), &measurer, None, 2).unwrap();
    assert!(rest.finished);
    assert_eq!((rest.done, rest.total), (5, 5));
    assert_eq!(measurer.measures.load(Ordering::SeqCst), 3);
    assert_eq!((rest.measured, rest.missing, rest.failed), (5, 0, 0));
    for id in &ids {
        let s = kept(&vault, id);
        assert!(Cull::current(&s), "{s}");
        assert!(s["focus"].as_f64().unwrap() > 0.0);
        if *id == older {
            assert_eq!(s["phash"], "00ff00ff00ff00ff", "older signals kept");
            assert_eq!(s["sharpness"], 1.5);
        } else {
            assert!(s["phash"].is_string(), "a photo with none gets them all");
        }
    }
    // The backfilled numbers are the import's numbers.
    let p0 = vault.sightings(&ids[0]).unwrap()[0].path.clone();
    let direct = signals::measure(&p0).unwrap().cull;
    let stored: Cull = serde_json::from_value(kept(&vault, &ids[0])).unwrap();
    let near = |a: f64, b: f64| (a - b).abs() <= 1e-12 * a.abs().max(1e-12);
    assert_eq!(stored.version, direct.version);
    assert!(near(stored.focus.unwrap(), direct.focus.unwrap()));
    assert!(near(stored.lum_mean.unwrap(), direct.lum_mean.unwrap()));
    // Finished: it does not run again.
    let again = cull::backfill(&vault, &jobs(), &measurer, None).unwrap();
    assert_eq!(again, rest);
    assert_eq!(measurer.measures.load(Ordering::SeqCst), 3);
    // The culling fields followed the signals.
    let focus = helios_vault::cull::FOCUS;
    assert!(
        vault
            .fields_of(&ids, &[focus])
            .unwrap()
            .iter()
            .all(|f| f.contains_key(focus))
    );
    // A run finished before the fields existed: the next one fills them
    // from the signals kept, measuring nothing.
    drop(vault);
    {
        use helios_vault::Store as _;
        let mut store = SqliteStore::open(&path).unwrap();
        store.set_field(&ids[3], focus, None).unwrap();
        let mut old = serde_json::to_value(&rest).unwrap();
        old.as_object_mut().unwrap().remove("fields");
        store
            .set_document("library/cull", "backfill", Some(&old))
            .unwrap();
    }
    let vault = Vault::new(SqliteStore::open(&path).unwrap());
    let filled = cull::backfill(&vault, &jobs(), &measurer, None).unwrap();
    assert!(filled.finished && filled.fields);
    assert_eq!(
        measurer.measures.load(Ordering::SeqCst),
        3,
        "nothing measured"
    );
    let f = vault.fields_of(&ids[3..4], &[focus]).unwrap();
    assert_eq!(f[0].get(focus), Some(&kept(&vault, &ids[3])["focus"]));
    drop(vault);
    let _ = std::fs::remove_dir_all(&dir);
}
