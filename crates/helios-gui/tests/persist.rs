//! The library and the vault behind it: what the user changes is written
//! as field commands, a failed write puts the stored value back on screen,
//! and other writers' changes show.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use helios_core::{Bus, ContentHash};
use helios_gui::persist::Persist;
use helios_gui::{Shell, helios_panels};
use helios_vault::fields::{Command, LABELS, RATING, REJECTED};
use helios_vault::{Image, ImageId, Store, Vault};
use helios_vault_sqlite::{Options, SqliteStore};
use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-persist-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn id(n: u8) -> ImageId {
    ImageId(ContentHash([n; 32]))
}

/// A vault file with images 1 and 2.
fn vault_file(dir: &Path) -> PathBuf {
    let path = dir.join("vault.db");
    let mut store = SqliteStore::open(&path).unwrap();
    for n in [1, 2] {
        store
            .add_image(&Image {
                id: id(n),
                size: 10,
                partial: ContentHash([n; 32]),
            })
            .unwrap();
    }
    path
}

/// A shell whose library is filled from the vault at `path`, the first
/// image selected.
fn open(path: &Path, options: &Options) -> (Shell, Persist<SqliteStore>) {
    let mut shell = Shell::new(helios_panels(), None);
    let persist = Persist::new(
        SqliteStore::open_with(path, options).unwrap(),
        Arc::new(Bus::new()),
    );
    persist.fill_library(&mut shell).unwrap();
    shell.library_mut().select(0);
    (shell, persist)
}

fn stored(path: &Path, field: &str) -> Option<Value> {
    let v = Vault::new(SqliteStore::open(path).unwrap());
    v.fields_of(&[id(1)], &[field]).unwrap()[0]
        .get(field)
        .cloned()
}

#[test]
fn a_label_toggled_from_a_stale_view_keeps_the_label_another_writer_added() {
    let dir = scratch("stale-label");
    let path = vault_file(&dir);
    let (mut shell, mut persist) = open(&path, &Options::default());
    assert!(shell.library().entries[0].labels.is_empty());
    // Another process labels the image red; this library still shows no
    // label.
    let other = Vault::new(SqliteStore::open(&path).unwrap());
    other
        .apply(&[Command::add(id(1), LABELS, "red")], "agent", None, 1)
        .unwrap();
    assert!(shell.library().entries[0].labels.is_empty(), "stale view");
    // The user toggles blue: one label added, red kept.
    shell.library_mut().toggle_label("blue").unwrap();
    persist.frame(&mut shell, 2);
    assert_eq!(stored(&path, LABELS), Some(json!(["blue", "red"])));
    // And the library now shows both, read back from the vault.
    assert_eq!(shell.library().entries[0].labels, ["red", "blue"]);
    // Toggling blue again takes off blue only.
    shell.library_mut().toggle_label("blue").unwrap();
    persist.frame(&mut shell, 3);
    assert_eq!(stored(&path, LABELS), Some(json!(["red"])));
    drop((shell, persist, other));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn taking_a_reject_back_restores_the_stars() {
    let dir = scratch("reject");
    let path = vault_file(&dir);
    let (mut shell, mut persist) = open(&path, &Options::default());
    shell.library_mut().rate(4).unwrap();
    persist.frame(&mut shell, 1);
    shell.library_mut().toggle_reject().unwrap();
    persist.frame(&mut shell, 2);
    assert_eq!(stored(&path, REJECTED), Some(json!(true)));
    assert_eq!(stored(&path, RATING), Some(json!(4)), "the stars stay");
    shell.library_mut().toggle_reject().unwrap();
    persist.frame(&mut shell, 3);
    assert_eq!(stored(&path, REJECTED), None);
    assert_eq!(stored(&path, RATING), Some(json!(4)));
    let entry = shell.library().entries[0].clone();
    assert_eq!((entry.rating, entry.rejected), (4, false));
    // Reopened, the library shows what was saved.
    drop((shell, persist));
    let (shell, _persist) = open(&path, &Options::default());
    assert_eq!(shell.library().entries[0].rating, 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_write_puts_the_stored_value_back_and_says_why() {
    let dir = scratch("failed");
    let path = vault_file(&dir);
    // A short busy timeout, so the blocked write gives up quickly.
    let options = Options {
        busy_timeout: Duration::from_millis(20),
        ..Options::default()
    };
    let (mut shell, mut persist) = open(&path, &options);
    shell.library_mut().rate(2).unwrap();
    persist.frame(&mut shell, 1);
    assert_eq!(stored(&path, RATING), Some(json!(2)));
    // Another process holds the write lock: the next write fails.
    let mut blocker = SqliteStore::open(&path).unwrap();
    blocker.begin().unwrap();
    shell.library_mut().rate(5).unwrap();
    assert_eq!(shell.library().entries[0].rating, 5, "shown at once");
    persist.frame(&mut shell, 2);
    let notes = shell.notices();
    let last = notes.last().expect("a notice says why");
    assert!(last.text.contains("Not saved"), "{}", last.text);
    assert!(
        last.text.contains("locked") || last.text.contains("busy"),
        "{}",
        last.text
    );
    assert_eq!(
        shell.library().entries[0].rating,
        2,
        "the grid shows the stored value again"
    );
    blocker.rollback().unwrap();
    assert_eq!(stored(&path, RATING), Some(json!(2)));
    drop((shell, persist, blocker));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_change_by_another_writer_on_the_bus_shows_in_the_library() {
    let dir = scratch("events");
    let path = vault_file(&dir);
    let (mut shell, mut persist) = open(&path, &Options::default());
    // An agent in this process, writing through its own handle on the
    // same bus.
    let agent = Vault::new(SqliteStore::open(&path).unwrap()).with_bus(persist.bus().clone());
    agent
        .apply(
            &[
                Command::set(id(2), RATING, json!(3)),
                Command::add(id(2), LABELS, "green"),
            ],
            "agent",
            None,
            1,
        )
        .unwrap();
    persist.frame(&mut shell, 2);
    let entry = shell
        .library()
        .entries
        .iter()
        .find(|e| e.id == id(2).hex())
        .cloned()
        .unwrap();
    assert_eq!(entry.rating, 3);
    assert_eq!(entry.labels, ["green"]);
    drop((shell, persist, agent));
    let _ = std::fs::remove_dir_all(&dir);
}
