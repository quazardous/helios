//! How the SQLite vault behaves on disk: its journal mode, and several
//! connections sharing one file.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use helios_core::ContentHash;
use helios_vault::fields::{Command, Query};
use helios_vault::{Image, ImageId, Store, Vault};
use helios_vault_sqlite::{Options, SqliteStore};
use serde_json::json;

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-vault-sqlite-{name}-{}-{}",
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

fn pragma(path: &std::path::Path, name: &str) -> String {
    let db = rusqlite::Connection::open(path).unwrap();
    db.query_row(&format!("PRAGMA {name}"), [], |r| {
        r.get::<_, rusqlite::types::Value>(0)
    })
    .map(|v| match v {
        rusqlite::types::Value::Integer(i) => i.to_string(),
        rusqlite::types::Value::Text(t) => t,
        other => format!("{other:?}"),
    })
    .unwrap()
}

#[test]
fn a_vault_database_runs_in_wal_mode() {
    let dir = scratch("pragmas");
    let path = dir.join("vault.db");
    let store = SqliteStore::open(&path).unwrap();
    // WAL is a property of the file: a fresh connection sees it.
    assert_eq!(pragma(&path, "journal_mode"), "wal");
    // The others belong to the connection.
    let pragmas = store.pragmas().unwrap();
    assert_eq!(pragmas.journal_mode, "wal");
    assert_eq!(pragmas.synchronous, 1, "NORMAL");
    assert_eq!(pragmas.busy_timeout_ms, 5000);
    assert!(pragmas.foreign_keys);
    drop(store);
    let custom = SqliteStore::open_with(
        &path,
        &Options {
            busy_timeout: Duration::from_millis(1234),
        },
    )
    .unwrap();
    assert_eq!(custom.pragmas().unwrap().busy_timeout_ms, 1234);
    drop(custom);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two vaults on one file, writing the same field from two threads while a
/// third connection reads: nobody fails as busy, and the audit trail is one
/// chain (each change starts from the value the previous one left), which
/// holds only if reading the old value and writing the new one are one
/// transaction.
#[test]
fn connections_sharing_a_file_never_fail_as_busy() {
    let dir = scratch("concurrent");
    let path = dir.join("vault.db");
    let mut first = SqliteStore::open(&path).unwrap();
    first.add_image(&image(1)).unwrap();
    let a = Vault::new(first);
    let b = Vault::new(SqliteStore::open(&path).unwrap());
    let reader = SqliteStore::open(&path).unwrap();
    let id = image(1).id;
    let rounds: i64 = 60;
    std::thread::scope(|scope| {
        for (vault, offset) in [(&a, 0), (&b, 1)] {
            scope.spawn(move || {
                for i in 0..rounds {
                    let rating = (2 * i + offset) % 6;
                    let command = Command {
                        image: id,
                        field: "rating".into(),
                        value: Some(json!(rating)),
                    };
                    vault.apply(&[command], "user", None, i as u64).unwrap();
                }
            });
        }
        scope.spawn(move || {
            for _ in 0..rounds {
                reader.select(&Query::default()).unwrap();
                reader.fields_of(&[id], &["rating"]).unwrap();
                reader.audit(&id).unwrap();
            }
        });
    });
    let audit = a.audit(&id).unwrap();
    assert!(!audit.is_empty());
    assert_eq!(audit[0].old, None);
    for pair in audit.windows(2) {
        assert_eq!(
            pair[1].old, pair[0].new,
            "change {} did not start from what change {} left",
            pair[1].seq, pair[0].seq
        );
    }
    drop((a, b));
    let _ = std::fs::remove_dir_all(&dir);
}
