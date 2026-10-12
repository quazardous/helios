//! Integrity: a vault killed while it writes, one corrupted on purpose, and
//! snapshot rotation.

use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use helios_core::ContentHash;
use helios_vault::{Image, ImageId, Store};
use helios_vault_sqlite::{OpenError, Options, SqliteStore, snapshot};
use rusqlite::Connection;
use serde_json::json;

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-vault-integrity-{name}-{}-{}",
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

/// Set for the child process that writes until it is killed.
const WRITER: &str = "HELIOS_VAULT_TEST_WRITER";

/// Not a test of its own: the body of the child process that
/// `a_vault_killed_while_writing_is_checked_and_keeps_its_commits` kills.
/// It writes one document per transaction and says so after each commit.
#[test]
fn writer_child() {
    let Ok(path) = std::env::var(WRITER) else {
        return;
    };
    let mut store = SqliteStore::open(Path::new(&path)).unwrap();
    store.add_image(&image(1)).unwrap();
    let mut out = std::io::stdout();
    // Bounded, so an orphaned child ends by itself.
    for i in 0..100_000 {
        store.begin().unwrap();
        store
            .set_document("crash", &format!("{i:06}"), Some(&json!(i)))
            .unwrap();
        store
            .set_field(&image(1).id, "rating", Some(&json!(i % 6)))
            .unwrap();
        store.commit().unwrap();
        writeln!(out, "committed {i}").unwrap();
        out.flush().unwrap();
    }
}

#[test]
fn a_vault_killed_while_writing_is_checked_and_keeps_its_commits() {
    let dir = scratch("killed");
    let path = dir.join("vault.db");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["writer_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(WRITER, &path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut committed = None;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        if let Some(n) = line.strip_prefix("committed ") {
            let n: u64 = n.parse().unwrap();
            committed = Some(n);
            if n >= 50 {
                break;
            }
        }
    }
    // Killed in the middle of its writes: no clean shutdown.
    child.kill().unwrap();
    child.wait().unwrap();
    let committed = committed.expect("the writer committed nothing");
    let store = SqliteStore::open(&path).unwrap();
    assert!(store.report().unclean_shutdown, "checked before use");
    let kept = store.documents("crash").unwrap();
    assert!(
        kept.len() as u64 > committed,
        "every commit the writer reported is kept: {} of {}",
        kept.len(),
        committed + 1
    );
    // Each transaction is whole: the rating matches the last document.
    let last = kept.last().unwrap().1.as_u64().unwrap();
    assert_eq!(
        store.field(&image(1).id, "rating").unwrap(),
        Some(json!(last % 6))
    );
    store.close().unwrap();
    let again = SqliteStore::open(&path).unwrap();
    assert!(!again.report().unclean_shutdown, "closed cleanly this time");
    drop(again);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Overwrites one page of the database file with garbage.
fn corrupt_page(path: &Path, page: u64, page_size: u64) {
    let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start((page - 1) * page_size)).unwrap();
    file.write_all(&vec![0xA5; page_size as usize]).unwrap();
    file.sync_all().unwrap();
}

#[test]
fn a_vault_corrupted_at_a_page_opens_read_only_with_its_snapshot() {
    let dir = scratch("corrupt");
    let path = dir.join("vault.db");
    let mut store = SqliteStore::open(&path).unwrap();
    store.add_image(&image(1)).unwrap();
    for n in 0..200 {
        store
            .set_document("notes", &format!("{n:04}"), Some(&json!("x".repeat(100))))
            .unwrap();
    }
    let snapshot = store.snapshot("scheduled").unwrap().unwrap();
    store.close().unwrap();
    // Find the documents table, then simulate a crash: no clean-shutdown
    // marker. Closing the last connection folds the log into the file.
    let (page, page_size) = {
        let db = Connection::open(&path).unwrap();
        db.execute("DELETE FROM vault_state", []).unwrap();
        let page: i64 = db
            .query_row(
                "SELECT rootpage FROM sqlite_schema WHERE name = 'documents'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let size: i64 = db.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        (page as u64, size as u64)
    };
    assert!(!dir.join("vault.db-wal").exists());
    corrupt_page(&path, page, page_size);
    let failed = SqliteStore::open(&path).unwrap_err();
    assert!(failed.contains(&snapshot.display().to_string()), "{failed}");
    let Err(OpenError::Damaged(damaged)) = SqliteStore::open_with(&path, &Options::default())
    else {
        panic!("not reported as damaged");
    };
    assert!(!damaged.problems.is_empty());
    assert_eq!(damaged.snapshot.as_ref(), Some(&snapshot));
    // What is still readable can be read; nothing can be written.
    let mut read_only = damaged.read_only.unwrap();
    assert_eq!(read_only.images().unwrap(), [image(1)]);
    let refusal = read_only.add_image(&image(2)).unwrap_err();
    assert!(refusal.contains("read-only"), "{refusal}");
    assert!(
        refusal.contains(&snapshot.display().to_string()),
        "{refusal}"
    );
    // The snapshot offered is sound and holds the rows.
    let db = Connection::open(&snapshot).unwrap();
    let check: String = db
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(check, "ok");
    let notes: i64 = db
        .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(notes, 200);
    drop((db, read_only));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn snapshots_rotate_keeping_the_newest() {
    let dir = scratch("rotate");
    let path = dir.join("vault.db");
    let options = Options {
        keep_snapshots: 2,
        ..Options::default()
    };
    let store = SqliteStore::open_with(&path, &options).unwrap();
    let taken: Vec<PathBuf> = (0..4)
        .map(|_| store.snapshot("scheduled").unwrap().unwrap())
        .collect();
    assert_eq!(snapshot::list(&path), taken[2..]);
    assert!(
        taken[3]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("000004-v")
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
