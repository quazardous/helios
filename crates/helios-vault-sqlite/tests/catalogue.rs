//! Catalogue snapshots: taken while another connection writes, verified,
//! kept to a number, copied to a backup folder and restored.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use helios_core::ContentHash;
use helios_vault::{Image, ImageId, Store};
use helios_vault_sqlite::{Options, SqliteStore, snapshot};
use rusqlite::Connection;
use serde_json::json;

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-vault-catalogue-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn image(n: u8) -> Image {
    Image {
        id: ImageId(ContentHash([n; 32])),
        size: 100,
        partial: ContentHash([n; 32]),
    }
}

fn count(file: &std::path::Path, ns: &str) -> i64 {
    Connection::open(file)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM documents WHERE ns = ?1", [ns], |r| {
            r.get(0)
        })
        .unwrap()
}

#[test]
fn a_snapshot_taken_while_another_connection_writes_is_consistent() {
    let dir = scratch("consistent");
    let path = dir.join("vault.db");
    SqliteStore::open(&path).unwrap().close().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (path, stop) = (path.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut store = SqliteStore::open(&path).unwrap();
            let mut n = 0u32;
            // Each transaction writes one row of each kind: a consistent
            // copy holds as many of one as of the other.
            while !stop.load(Ordering::Relaxed) && n < 20_000 {
                store.begin().unwrap();
                for kind in ["a", "b"] {
                    store
                        .set_document(kind, &format!("{n:06}"), Some(&json!(n)))
                        .unwrap();
                }
                store.commit().unwrap();
                n += 1;
            }
            store.close().unwrap();
            n
        })
    };
    let mut taken = Vec::new();
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        taken.push(snapshot::take_file(&path, "by-hand", &Options::default()).unwrap());
    }
    stop.store(true, Ordering::Relaxed);
    let written = writer.join().unwrap();
    assert!(written > 0);
    for file in &taken {
        snapshot::verify(file).unwrap();
        assert_eq!(count(file, "a"), count(file, "b"), "{}", file.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verification_fails_on_a_corrupted_snapshot() {
    let dir = scratch("corrupted");
    let path = dir.join("vault.db");
    let mut store = SqliteStore::open(&path).unwrap();
    for n in 0..300 {
        store
            .set_document("notes", &format!("{n:04}"), Some(&json!("x".repeat(200))))
            .unwrap();
    }
    let file = snapshot::take_file(&path, "by-hand", &Options::default()).unwrap();
    snapshot::verify(&file).unwrap();
    // Garbage over the pages past the first: the header still opens.
    let mut bytes = std::fs::read(&file).unwrap();
    for b in bytes.iter_mut().skip(4096).take(16_384) {
        *b = 0x5a;
    }
    std::fs::write(&file, &bytes).unwrap();
    assert!(snapshot::verify(&file).is_err());
    // Not a database at all, or missing.
    let junk = dir.join("000099-v1-junk.db");
    std::fs::write(&junk, b"not a database").unwrap();
    assert!(snapshot::verify(&junk).is_err());
    assert!(snapshot::verify(&dir.join("none.db")).is_err());
    // A broken one is refused as a restore.
    assert!(snapshot::request_restore(&path, &file).is_err());
    assert!(snapshot::pending(&path).is_none());
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retention_keeps_the_newest_and_never_none() {
    let dir = scratch("retention");
    let path = dir.join("vault.db");
    let options = Options {
        keep_snapshots: 3,
        ..Options::default()
    };
    SqliteStore::open(&path).unwrap().close().unwrap();
    let taken: Vec<PathBuf> = (0..5)
        .map(|_| snapshot::take_file(&path, "daily", &options).unwrap())
        .collect();
    assert_eq!(snapshot::list(&path), taken[2..]);
    let removed = snapshot::prune(&snapshot::dir(&path), 0).unwrap();
    assert_eq!(removed, taken[2..4]);
    assert_eq!(snapshot::list(&path), taken[4..]);
    let info = snapshot::info(&taken[4]).unwrap();
    assert_eq!(info.sequence, 5);
    assert_eq!(info.reason, "daily");
    assert!(info.version > 0);
    assert!(!snapshot::due(&path, 3600, info.taken + 10));
    assert!(snapshot::due(&path, 3600, info.taken + 3600));
    assert!(snapshot::due(&dir.join("other.db"), 3600, info.taken));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_snapshot_is_copied_verified_to_a_backup_folder_and_kept_to_a_number() {
    let dir = scratch("copy");
    let path = dir.join("vault.db");
    SqliteStore::open(&path).unwrap().close().unwrap();
    let to = dir.join("BACKUP_A/Helios Backup/catalogue/vault");
    let mut copies = Vec::new();
    for _ in 0..3 {
        let file = snapshot::take_file(&path, "daily", &Options::default()).unwrap();
        let copy = snapshot::copy_verified(&file, &to, 2).unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&file).unwrap());
        // Its age is the snapshot's.
        assert_eq!(
            snapshot::info(&copy).unwrap().taken,
            snapshot::info(&file).unwrap().taken
        );
        copies.push(copy);
    }
    assert_eq!(snapshot::list_in(&to), copies[1..]);
    // Copied again: the whole copy there is kept.
    let again = snapshot::copy_verified(&snapshot::list(&path)[2], &to, 2).unwrap();
    assert_eq!(again, copies[2]);
    // No part file left behind.
    assert!(
        std::fs::read_dir(&to)
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().ends_with(".partial"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_restore_round_trip_keeps_the_current_catalogue_aside() {
    let dir = scratch("restore");
    let path = dir.join("vault.db");
    let mut store = SqliteStore::open(&path).unwrap();
    store.add_image(&image(1)).unwrap();
    let file = snapshot::take_file(&path, "by-hand", &Options::default()).unwrap();
    store.add_image(&image(2)).unwrap();
    snapshot::request_restore(&path, &file).unwrap();
    assert_eq!(snapshot::pending(&path).as_ref(), Some(&file));
    store.close().unwrap();
    let restored = snapshot::apply_pending(&path, 1_000).unwrap().unwrap();
    assert_eq!(restored.from, file);
    assert!(snapshot::pending(&path).is_none());
    assert_eq!(snapshot::apply_pending(&path, 1_001).unwrap(), None);
    // The restored catalogue has the snapshot's rows; the one replaced is
    // kept whole beside it, with its last image.
    let back = SqliteStore::open(&path).unwrap();
    assert_eq!(back.images().unwrap(), [image(1)]);
    back.close().unwrap();
    snapshot::verify(&restored.aside).unwrap();
    let aside = SqliteStore::open(&restored.aside).unwrap();
    assert_eq!(aside.images().unwrap(), [image(1), image(2)]);
    aside.close().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
