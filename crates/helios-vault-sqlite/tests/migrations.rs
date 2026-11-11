//! Schema versions: new, old, current, newer and messy databases.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use helios_vault::{ImageId, LibraryFlag, Location, Presence, Store};
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
            "sightings row 1 points to a missing images row"
        ]
    );
    assert_eq!(store.images().unwrap().len(), 1, "the vault is usable");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Version 3 takes the reject out of the rating: a rating of -1 becomes
/// the `rejected` field, other ratings stay as they were.
#[test]
fn a_reject_kept_as_a_rating_becomes_its_own_field() {
    let dir = scratch("reject");
    let path = dir.join("vault.db");
    old_database(
        &path,
        2,
        "CREATE TABLE vault_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         INSERT INTO images VALUES (
             '0303030303030303030303030303030303030303030303030303030303030303', 100,
             '0404040404040404040404040404040404040404040404040404040404040404');
         INSERT INTO fields VALUES (
             '0303030303030303030303030303030303030303030303030303030303030303',
             'rating', '-1', -1);",
    );
    let store = SqliteStore::open(&path).unwrap();
    let kept = helios_vault::ImageId::from_hex(&"01".repeat(32)).unwrap();
    let rejected = helios_vault::ImageId::from_hex(&"03".repeat(32)).unwrap();
    assert_eq!(
        store.field(&kept, "rating").unwrap(),
        Some(serde_json::json!(3))
    );
    assert_eq!(store.field(&kept, "rejected").unwrap(), None);
    assert_eq!(store.field(&rejected, "rating").unwrap(), None);
    assert_eq!(
        store.field(&rejected, "rejected").unwrap(),
        Some(serde_json::json!(true))
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Version 4 puts locations on volumes: an old row keeps its absolute
/// path, has no volume nor path inside one until a scan relinks it, and
/// its `present` flag becomes a presence.
#[test]
fn old_locations_keep_their_path_and_wait_for_a_volume() {
    let dir = scratch("volumes");
    let path = dir.join("vault.db");
    let image = "01".repeat(32);
    old_database(
        &path,
        3,
        &format!(
            "CREATE TABLE vault_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO locations VALUES ('/photos/here.nef', '{image}', 100, 5, 7, 1);
             INSERT INTO locations VALUES ('/photos/gone.nef', '{image}', 100, 5, 7, 0);"
        ),
    );
    let store = SqliteStore::open(&path).unwrap();
    let migrated = store.report().migrated.clone().unwrap();
    assert_eq!((migrated.from, migrated.to), (3, SCHEMA_VERSION));
    let id = helios_vault::ImageId::from_hex(&image).unwrap();
    let locations = store.locations(&id).unwrap();
    let seen: Vec<_> = locations
        .iter()
        .map(|l| {
            (
                l.path.to_str().unwrap(),
                l.presence,
                l.volume.clone(),
                l.rel_path.clone(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("/photos/gone.nef", Presence::Missing, None, None),
            ("/photos/here.nef", Presence::Present, None, None),
        ]
    );
    // A relinked row round-trips.
    let mut store = store;
    let relinked = Location {
        volume: Some(helios_vault::storage::VolumeId("uuid:photos".into())),
        rel_path: Some("here.nef".into()),
        ..locations[1].clone()
    };
    store.put_location(&id, &relinked).unwrap();
    assert_eq!(
        store.at_path(Path::new("/photos/here.nef")).unwrap(),
        Some((id, relinked))
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Schema 4 as it was released, after `VERSION_1`: the vault state and
/// locations on volumes.
const VERSION_4: &str = "
CREATE TABLE vault_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
ALTER TABLE locations ADD COLUMN volume TEXT;
ALTER TABLE locations ADD COLUMN rel_path TEXT;
ALTER TABLE locations ADD COLUMN presence TEXT NOT NULL DEFAULT 'present';
ALTER TABLE locations DROP COLUMN present;
CREATE INDEX locations_volume ON locations (volume);
";

/// Version 5, the image map: every image is flagged in the Library
/// (`migrated`, since its first location's check), every location becomes
/// a sighting with its image, partial hash, presence and last check, and
/// fields and versions stay as they were.
#[test]
fn schema_4_moves_to_the_image_map_keeping_every_row() {
    let dir = scratch("map");
    let path = dir.join("vault.db");
    let (one, two, three) = ("01".repeat(32), "05".repeat(32), "07".repeat(32));
    let version = "09".repeat(32);
    old_database(
        &path,
        4,
        &format!(
            "{VERSION_4}
             INSERT INTO images VALUES ('{two}', 200, '{}');
             INSERT INTO images VALUES ('{three}', 300, '{}');
             INSERT INTO locations VALUES ('/photos/a.nef', '{one}', 100, 5, 70, NULL, NULL, 'present');
             INSERT INTO locations VALUES ('/media/b/x.cr3', '{one}', 100, 5, 30, 'uuid:b', 'x.cr3', 'offline');
             INSERT INTO locations VALUES ('/photos/gone.nef', '{two}', 200, 6, 50, NULL, NULL, 'missing');
             INSERT INTO versions VALUES ('{version}', '{one}', '[]', '{{\"n\":1}}', 'user', 3, 'first');",
            "06".repeat(32),
            "08".repeat(32)
        ),
    );
    let store = SqliteStore::open(&path).unwrap();
    let migrated = store.report().migrated.clone().unwrap();
    assert_eq!((migrated.from, migrated.to), (4, SCHEMA_VERSION));
    assert!(migrated.snapshot.is_some(), "a snapshot first");
    assert!(migrated.orphans.is_empty(), "{:?}", migrated.orphans);
    let id = |hex: &str| ImageId::from_hex(hex).unwrap();
    // Every image, flagged.
    let images: Vec<_> = store.images().unwrap().iter().map(|i| i.id).collect();
    assert_eq!(images, [id(&one), id(&two), id(&three)]);
    let flag = |hex: &str| store.library_flag(&id(hex)).unwrap().unwrap();
    assert_eq!(
        flag(&one),
        LibraryFlag {
            in_library: true,
            at: Some(30),
            by: Some("migrated".into())
        }
    );
    assert_eq!(flag(&two).at, Some(50));
    assert_eq!(flag(&three).at, None, "no location, no date");
    // Fields and versions untouched.
    assert_eq!(
        store.field(&id(&one), "rating").unwrap(),
        Some(serde_json::json!(3))
    );
    assert_eq!(store.versions(&id(&one)).unwrap().len(), 1);
    // Every location, as before.
    let all: Vec<_> = store
        .all_locations()
        .unwrap()
        .into_iter()
        .map(|(i, l)| {
            (
                i,
                l.path.to_str().unwrap().to_string(),
                l.size,
                l.modified,
                l.last_verified,
                l.volume.map(|v| v.0),
                l.rel_path.map(|p| p.to_str().unwrap().to_string()),
                l.presence,
            )
        })
        .collect();
    assert_eq!(
        all,
        [
            (
                id(&one),
                "/media/b/x.cr3".into(),
                100,
                5,
                30,
                Some("uuid:b".into()),
                Some("x.cr3".into()),
                Presence::Offline
            ),
            (
                id(&one),
                "/photos/a.nef".into(),
                100,
                5,
                70,
                None,
                None,
                Presence::Present
            ),
            (
                id(&two),
                "/photos/gone.nef".into(),
                200,
                6,
                50,
                None,
                None,
                Presence::Missing
            ),
        ]
    );
    // As sightings, with their partial hash and when they were seen.
    let seen = store.sightings_of(&id(&one)).unwrap();
    assert_eq!(seen.len(), 2);
    let partial = helios_vault::parse_hash(&"02".repeat(32)).unwrap();
    assert!(seen.iter().all(|s| s.partial == Some(partial)));
    assert_eq!(
        (seen[1].first_seen, seen[1].last_seen, seen[1].last_verified),
        (70, 70, Some(70))
    );
    assert_eq!(store.sightings_like(100, &partial).unwrap(), seen);
    // The old table is gone, the folders' is there.
    let db = Connection::open(&path).unwrap();
    let tables: Vec<String> = db
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!tables.contains(&"locations".to_string()), "{tables:?}");
    assert!(tables.contains(&"folders".to_string()), "{tables:?}");
    drop((db, store));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Schema 6 indexes the text fields (capture's camera, lens, file type)
/// beside the numbers; a vault at schema 5 keeps every row, and its
/// capture fields wait for the backfill job (they are read from files).
#[test]
fn schema_5_gains_the_text_index_and_keeps_its_rows() {
    let dir = scratch("capture");
    let path = dir.join("vault.db");
    let one = ImageId::from_hex(&"01".repeat(32)).unwrap();
    {
        let mut store = SqliteStore::open(&path).unwrap();
        store
            .add_to_library(&helios_vault::Image {
                id: one,
                size: 100,
                partial: one.0,
            })
            .unwrap();
        store
            .set_field(&one, "rating", Some(&serde_json::json!(4)))
            .unwrap();
    }
    // Back to schema 5 as an older Helios left it.
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("DROP INDEX fields_name_text; PRAGMA user_version = 5;")
            .unwrap();
    }
    let mut store = SqliteStore::open(&path).unwrap();
    let migrated = store.report().migrated.clone().unwrap();
    assert_eq!((migrated.from, migrated.to), (5, SCHEMA_VERSION));
    assert!(migrated.snapshot.is_some(), "a snapshot first");
    assert_eq!(
        store.field(&one, "rating").unwrap(),
        Some(serde_json::json!(4))
    );
    assert_eq!(store.images().unwrap().len(), 1);
    // Text fields are looked up through their index.
    store
        .set_field(&one, "camera_model", Some(&serde_json::json!("Z 8")))
        .unwrap();
    drop(store);
    let db = Connection::open(&path).unwrap();
    let plan: Vec<String> = db
        .prepare(
            "EXPLAIN QUERY PLAN SELECT image FROM fields \
             WHERE name = 'camera_model' AND value = '\"Z 8\"' AND number IS NULL",
        )
        .unwrap()
        .query_map([], |r| r.get(3))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        plan.iter().any(|s| s.contains("fields_name_text")),
        "{plan:?}"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}
