//! Opening another image in the darkroom while one is edited: the edit
//! open is saved before the next image loads, and a save that fails keeps
//! the image open and says why.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use helios_core::{Bus, ContentHash};
use helios_gui::persist::Persist;
use helios_gui::{Shell, helios_panels};
use helios_vault::{Image, ImageId, Store};
use helios_vault_sqlite::{Options, SqliteStore};

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-image-open-{name}-{}-{}",
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

/// A vault file with images 1 and 2 (synthetic: no pixels, no file).
fn vault_file(dir: &Path) -> PathBuf {
    let path = dir.join("vault.db");
    let mut store = SqliteStore::open(&path).unwrap();
    for n in [1, 2] {
        store
            .add_to_library(&Image {
                id: id(n),
                size: 10,
                partial: ContentHash([n; 32]),
            })
            .unwrap();
    }
    path
}

/// A source and an exposure node: the network the viewport shows.
fn network() -> (helios_core::Graph, helios_core::NodeId) {
    let mut g = helios_core::Graph::new();
    let src = g.add_source(
        "source",
        helios_core::Image::from_fn(helios_core::Region::sized(16, 8), 4, |_, _, _| 0.25),
    );
    let exp = g
        .add_module(
            "exposure",
            helios_core::ModuleRef::new(helios_module_exposure::Exposure),
            &helios_core::Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    (g, exp)
}

const EV: &str = "node/exposure/exposure";

/// The exposure set by the user: a step of the edit, not yet saved (no
/// frame ran since).
fn set(shell: &Shell, ev: f64) {
    let set = helios_values::Set::new(
        EV,
        helios_values::Change::To(helios_core::Value::Float(ev)),
        helios_shell::commands::Origin::User,
    );
    shell.values().lock().unwrap().set(set).unwrap();
}

fn shown(shell: &Shell) -> f64 {
    shell
        .values()
        .lock()
        .unwrap()
        .get(EV)
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

fn open(path: &Path, options: &Options) -> (Shell, Persist<SqliteStore>) {
    let shell = Shell::new(helios_panels(), None);
    let persist = Persist::new(
        SqliteStore::open_with(path, options).unwrap(),
        Arc::new(Bus::new()),
    );
    (shell, persist)
}

/// Image 1 is edited, its last step not saved by a frame yet; opening
/// image 2 saves it before image 2 loads: image 1's head holds the step,
/// whoever asked the switch.
#[test]
fn switching_photos_saves_the_unsaved_edit_before_the_next_loads() {
    let dir = scratch("save-first");
    let path = vault_file(&dir);
    let (mut shell, mut persist) = open(&path, &Options::default());
    let (graph, exp) = network();
    persist
        .open_image(&mut shell, id(1), graph, exp, "output")
        .unwrap();
    set(&shell, 1.5);

    let (graph, exp) = network();
    persist
        .open_image(&mut shell, id(2), graph, exp, "output")
        .unwrap();
    persist.settle(&mut shell);
    let vault = persist.vault().clone();
    let versions = vault.versions(&id(1)).unwrap();
    assert_eq!(
        versions.len(),
        1,
        "image 1's unsaved step is saved by the switch"
    );
    assert_eq!(
        persist.session().map(helios_vault::EditSession::image),
        Some(id(2)),
        "then image 2 is open"
    );
    assert!(vault.versions(&id(2)).unwrap().is_empty());
    drop((shell, persist, vault));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The save of image 1's edit fails (another writer holds the vault): the
/// switch is refused with the save's reason, image 1 stays open with its
/// edit shown, and nothing of image 2 is loaded.
#[test]
fn a_failing_save_keeps_the_current_photo_open_and_says_why() {
    let dir = scratch("save-fails");
    let path = vault_file(&dir);
    let options = Options {
        busy_timeout: Duration::from_millis(20),
        ..Options::default()
    };
    let (mut shell, mut persist) = open(&path, &options);
    let (graph, exp) = network();
    persist
        .open_image(&mut shell, id(1), graph, exp, "output")
        .unwrap();
    set(&shell, 1.5);

    let mut blocker = SqliteStore::open(&path).unwrap();
    blocker.begin().unwrap();
    let (graph, exp) = network();
    let refused = persist
        .open_image(&mut shell, id(2), graph, exp, "output")
        .expect_err("the switch is refused");
    assert!(
        refused.contains("edit") && refused.contains("not saved"),
        "the refusal says why: {refused}"
    );
    assert_eq!(
        persist.session().map(helios_vault::EditSession::image),
        Some(id(1)),
        "image 1 stays open"
    );
    assert!((shown(&shell) - 1.5).abs() < 1e-9, "its edit still shown");
    blocker.rollback().unwrap();
    drop(blocker);

    // Once the vault lets go, the next switch saves it and goes on.
    let (graph, exp) = network();
    persist
        .open_image(&mut shell, id(2), graph, exp, "output")
        .unwrap();
    persist.settle(&mut shell);
    assert_eq!(persist.vault().versions(&id(1)).unwrap().len(), 1);
    assert_eq!(
        persist.session().map(helios_vault::EditSession::image),
        Some(id(2))
    );
    drop((shell, persist));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The save of image 1's edit fails slowly: the vault is held by another
/// writer and the store waits 1.5 s for it before giving up, past the
/// bus's default listener deadline (1 s). The switch is still refused:
/// image 1 stays open, its edit shown and marked unsaved, and its undo
/// history is kept (`history.scope` did not run). Before the fix the bus
/// dropped the late listener's refusal and the photo switched.
#[test]
fn a_slow_failing_save_still_refuses_the_switch() {
    let dir = scratch("save-fails-slowly");
    let path = vault_file(&dir);
    let options = Options {
        busy_timeout: Duration::from_millis(1500),
        ..Options::default()
    };
    let (mut shell, mut persist) = open(&path, &options);
    let (graph, exp) = network();
    persist
        .open_image(&mut shell, id(1), graph, exp, "output")
        .unwrap();
    set(&shell, 1.5);

    let mut blocker = SqliteStore::open(&path).unwrap();
    blocker.begin().unwrap();
    let (graph, exp) = network();
    let refused = persist.open_image(&mut shell, id(2), graph, exp, "output");
    blocker.rollback().unwrap();
    drop(blocker);
    let refused = refused.expect_err("the slow failed save refuses the switch");
    assert!(
        refused.contains("not saved"),
        "the refusal says why: {refused}"
    );
    assert_eq!(
        persist.session().map(helios_vault::EditSession::image),
        Some(id(1)),
        "image 1 stays open"
    );
    assert!(persist.unsaved(), "its edit is marked unsaved");
    assert!((shown(&shell) - 1.5).abs() < 1e-9, "its edit still shown");
    let undone = shell
        .values()
        .lock()
        .unwrap()
        .undo(&helios_shell::commands::Origin::User)
        .unwrap();
    assert!(undone.is_some(), "its undo history is kept");
    drop((shell, persist));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The listeners of `image.open` and `image.close` as the desktop
/// registers them, in the order they run: a reviewed dump
/// (`tests/bus/darkroom.txt`). The edit saved first, then its history
/// left.
#[test]
fn the_darkroom_points_listeners_run_in_a_declared_order() {
    let dir = scratch("order");
    let path = vault_file(&dir);
    let (shell, mut persist) = open(&path, &Options::default());
    assert_eq!(
        persist.darkroom_order(&shell),
        include_str!("../bus/darkroom.txt")
    );
    drop((shell, persist));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A switch is said on the journal: `image.closed` with the listeners
/// that ran in their order, then `image.opened`; a refused one,
/// `image.refused` with who refused.
#[test]
fn a_switch_is_said_on_the_journal_with_the_listeners_that_ran() {
    let dir = scratch("journal");
    let path = vault_file(&dir);
    let (mut shell, mut persist) = open(&path, &Options::default());
    let (graph, exp) = network();
    persist
        .open_image(&mut shell, id(1), graph, exp, "output")
        .unwrap();
    set(&shell, 1.5);
    let (graph, exp) = network();
    persist
        .open_image(&mut shell, id(2), graph, exp, "output")
        .unwrap();
    let said: Vec<(String, serde_json::Value)> = shell
        .app()
        .handle()
        .events_since(0)
        .into_iter()
        .filter(|e| e.name.starts_with("helios.image."))
        .map(|e| (e.name.clone(), e.data.clone()))
        .collect();
    let kinds: Vec<&str> = said.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "helios.image.opened",
            "helios.image.closed",
            "helios.image.opened"
        ]
    );
    assert_eq!(
        said[1].1["ran"],
        serde_json::json!(["helios.edit.save", "helios.history.scope"])
    );
    assert_eq!(said[1].1["from"], serde_json::json!(id(1).hex()));
    assert_eq!(said[1].1["to"], serde_json::json!(id(2).hex()));
    drop((shell, persist));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A switch runs each follower of the image open once, in its declared
/// order, inside the pass: `image.closed` follows nothing (an image is
/// asked), `image.opened` runs `snapshots.book`, `mask.marks`, then
/// `workshop.photo`, which names the image to the workshop session before
/// any frame ran (no compare at a later pass). A close with none after it
/// passes `image.closed` once.
#[test]
fn a_switch_runs_each_follower_once_in_order() {
    let dir = scratch("followers");
    let path = vault_file(&dir);
    let (mut shell, mut persist) = open(&path, &Options::default());
    let (graph, exp) = network();
    persist
        .open_image(&mut shell, id(1), graph, exp, "output")
        .unwrap();
    assert_eq!(shell.edited_image(), Some(id(1).hex()));
    let (graph, exp) = network();
    persist
        .open_image(&mut shell, id(2), graph, exp, "output")
        .unwrap();
    assert_eq!(
        shell.edited_image(),
        Some(id(2).hex()),
        "the workshop follows the switch inside its pass"
    );
    persist.close_image(&mut shell, 0.0).unwrap();
    assert_eq!(shell.edited_image(), None, "and the close");
    let said: Vec<(String, serde_json::Value)> = shell
        .app()
        .handle()
        .events_since(0)
        .into_iter()
        .filter(|e| e.name.starts_with("helios.image."))
        .map(|e| (e.name.clone(), e.data.clone()))
        .collect();
    let kinds: Vec<&str> = said.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "helios.image.opened",
            "helios.image.closed",
            "helios.image.opened",
            "helios.image.closed"
        ]
    );
    let followers = serde_json::json!([
        "helios.snapshots.book",
        "helios.mask.marks",
        "helios.workshop.photo"
    ]);
    assert_eq!(said[0].1["followed"], followers);
    assert_eq!(said[1].1["followed"], serde_json::Value::Null);
    assert_eq!(said[2].1["followed"], followers);
    assert_eq!(said[3].1["followed"], followers);
    drop((shell, persist));
    let _ = std::fs::remove_dir_all(&dir);
}
