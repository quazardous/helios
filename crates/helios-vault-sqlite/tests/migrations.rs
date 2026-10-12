//! Schema versions: new, old, current, newer and messy databases.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use helios_vault::Store;
use helios_vault_sqlite::{OpenError, Options, Progress, SCHEMA_VERSION, SqliteStore, snapshot};
use rusqlite::Connection;

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-vault-migrations-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A database as the first Helios releases wrote it: the version 1 tables,
/// frozen here, with one image.
const VERSION_1: &str = "
CREATE TABLE images (id TEXT PRIMARY KEY, size INTEGER NOT NULL, partial TEXT NOT NULL);
CREATE INDEX images_size ON images (size);
CREATE TABLE locations (
    path TEXT PRIMARY KEY, image TEXT NOT NULL REFERENCES images (id),
    size INTEGER NOT NULL, modified INTEGER NOT NULL,
    last_verified INTEGER NOT NULL, present INTEGER NOT NULL);
CREATE INDEX locations_image ON locations (image);
CREATE TABLE fields (
    image TEXT NOT NULL REFERENCES images (id), name TEXT NOT NULL,
    value TEXT NOT NULL, number REAL, PRIMARY KEY (image, name));
CREATE INDEX fields_name_number ON fields (name, number);
CREATE TABLE audit (
    seq INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, who TEXT NOT NULL,
    job INTEGER, image TEXT NOT NULL, field TEXT NOT NULL, old TEXT, new TEXT);
CREATE INDEX audit_image ON audit (image);
CREATE TABLE versions (
    id TEXT PRIMARY KEY, image TEXT NOT NULL REFERENCES images (id),
    parents TEXT NOT NULL, edit TEXT NOT NULL, origin TEXT NOT NULL,
    at INTEGER NOT NULL, label TEXT);
CREATE INDEX versions_image ON versions (image);
CREATE TABLE documents (ns TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (ns, key));
INSERT INTO images VALUES (
    '0101010101010101010101010101010101010101010101010101010101010101', 100,
    '0202020202020202020202020202020202020202020202020202020202020202');
INSERT INTO fields VALUES (
    '0101010101010101010101010101010101010101010101010101010101010101',
    'rating', '3', 3);
";

fn old_database(path: &Path, user_version: u32, extra: &str) {
    let db = Connection::open(path).unwrap();
    db.execute_batch(VERSION_1).unwrap();
    db.execute_batch(extra).unwrap();
    db.pragma_update(None, "user_version", user_version)
        .unwrap();
}

fn user_version(path: &Path) -> u32 {
    Connection::open(path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

fn recording() -> (Options, Arc<Mutex<Vec<Progress>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let options = Options {
        on_progress: Some(Arc::new(move |p: &Progress| {
            sink.lock().unwrap().push(p.clone());
        })),
        ..Options::default()
    };
    (options, seen)
}

#[test]
fn a_new_database_gets_the_whole_schema_without_a_snapshot() {
    let dir = scratch("fresh");
    let path = dir.join("vault.db");
    let store = SqliteStore::open(&path).unwrap();
    let migrated = store.report().migrated.clone().unwrap();
    assert_eq!((migrated.from, migrated.to), (0, SCHEMA_VERSION));
    assert_eq!(migrated.snapshot, None, "nothing to keep");
    assert!(migrated.dropped.is_empty() && migrated.orphans.is_empty());
    assert_eq!(user_version(&path), SCHEMA_VERSION);
    assert!(snapshot::list(&path).is_empty());
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_old_database_is_upgraded_after_a_verified_snapshot() {
    for from in [0, 1] {
        let dir = scratch("old");
        let path = dir.join("vault.db");
        // Version 0 is a database written before versions existed.
        old_database(&path, from, "");
        let (options, seen) = recording();
        let store = SqliteStore::open_with(&path, &options).unwrap();
        assert_eq!(user_version(&path), SCHEMA_VERSION);
        let image = store.images().unwrap()[0].id;
        assert_eq!(
            store.field(&image, "rating").unwrap(),
            Some(serde_json::json!(3)),
            "rows are kept"
        );
        let Some(migrated) = store.report().migrated.clone() else {
            assert_eq!(
                from, SCHEMA_VERSION,
                "only a current database is left alone"
            );
            continue;
        };
        assert_eq!((migrated.from, migrated.to), (from, SCHEMA_VERSION));
        // The snapshot is the database as it was, and it is sound.
        let copy = migrated.snapshot.unwrap();
        assert_eq!(snapshot::list(&path), std::slice::from_ref(&copy));
        assert_eq!(user_version(&copy), from);
        let db = Connection::open(&copy).unwrap();
        let images: i64 = db
            .query_row("SELECT COUNT(*) FROM images", [], |r| r.get(0))
            .unwrap();
        assert_eq!(images, 1);
        let check: String = db
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(check, "ok");
        let seen = seen.lock().unwrap();
        assert!(seen.iter().any(|p| p.stage == "snapshot"), "{seen:?}");
        let last = seen.last().unwrap();
        assert_eq!(
            (last.stage.as_str(), last.done, last.total),
            (
                "migration",
                u64::from(SCHEMA_VERSION - from),
                u64::from(SCHEMA_VERSION - from)
            )
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_current_database_opens_twice_untouched() {
    let dir = scratch("current");
    let path = dir.join("vault.db");
    drop(SqliteStore::open(&path).unwrap());
    let first = SqliteStore::open(&path).unwrap();
    let second = SqliteStore::open(&path).unwrap();
    assert_eq!(first.report().migrated, None);
    assert_eq!(second.report().migrated, None);
    assert!(
        snapshot::list(&path).is_empty(),
        "no snapshot without a change"
    );
    drop((first, second));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_database_from_a_newer_helios_is_refused_untouched() {
    let dir = scratch("newer");
    let path = dir.join("vault.db");
    old_database(&path, SCHEMA_VERSION + 1, "");
    let refused = SqliteStore::open_with(&path, &Options::default()).unwrap_err();
    assert!(
        matches!(refused, OpenError::TooNew { found, supported }
            if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION),
        "{refused:?}"
    );
    assert!(refused.to_string().contains("newer Helios"), "{refused}");
    assert_eq!(user_version(&path), SCHEMA_VERSION + 1, "no downgrade");
    assert!(snapshot::list(&path).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Years of drift: rows pointing to images that are gone, a view and a
/// trigger no version of Helios defines. The upgrade goes through, drops
/// what is stale and reports the orphans.
#[test]
fn a_messy_database_migrates_and_reports_what_it_found() {
    let dir = scratch("messy");
    let path = dir.join("vault.db");
    old_database(
        &path,
        0,
        "PRAGMA foreign_keys = OFF;
         INSERT INTO locations VALUES ('/gone.cr3', 'f00d', 1, 1, 1, 1);
         INSERT INTO fields VALUES ('f00d', 'rating', '5', 5);
         CREATE VIEW v_images AS SELECT id, size, partial FROM images;
         CREATE TRIGGER t_images AFTER INSERT ON images BEGIN SELECT 1; END;",
    );
    let store = SqliteStore::open(&path).unwrap();
    let migrated = store.report().migrated.clone().unwrap();
    assert_eq!(migrated.dropped, ["trigger t_images", "view v_images"]);
    assert_eq!(
        migrated.orphans,
        [
            "fields row 2 points to a missing images row",
            "locations row 1 points to a missing images row"
        ]
    );
    assert_eq!(store.images().unwrap().len(), 1, "the vault is usable");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
