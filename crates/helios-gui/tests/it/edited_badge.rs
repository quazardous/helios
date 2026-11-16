//! A photo whose edit changed it carries an "edited" badge in the Library
//! and the filmstrip (they share their cells' badges): once a step is
//! saved, gone after "Reset all", and still there after a restart, the
//! vault keeping the photo's `edited` field.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::{Bus, ContentHash};
use helios_gui::persist::Persist;
use helios_gui::{Shell, helios_panels};
use helios_shell::commands::Origin;
use helios_vault::{Image, ImageId, Store, Vault};
use helios_vault_sqlite::SqliteStore;
use serde_json::Value;

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-gui-edited-badge-{}", std::process::id()));
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
            .add_to_library(&Image {
                id: id(n),
                size: 10,
                partial: ContentHash([n; 32]),
            })
            .unwrap();
    }
    path
}

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

/// A shell whose library is filled from the vault at `path`.
fn open(path: &Path) -> (Shell, Persist<SqliteStore>) {
    let mut shell = Shell::new(helios_panels(), None);
    let persist = Persist::new(SqliteStore::open(path).unwrap(), Arc::new(Bus::new()));
    persist.fill_library(&mut shell).unwrap();
    (shell, persist)
}

fn badges(shell: &Shell, n: u8) -> Vec<String> {
    shell
        .library()
        .badges_of(&egui::Context::default(), &id(n).hex())
}

fn edited(shell: &Shell, n: u8) -> bool {
    badges(shell, n).iter().any(|b| b == "edited")
}

fn stored(path: &Path) -> Option<Value> {
    let v = Vault::new(SqliteStore::open(path).unwrap());
    v.fields_of(&[id(1)], &[helios_vault::edit::EDITED])
        .unwrap()[0]
        .get(helios_vault::edit::EDITED)
        .cloned()
}

fn set_exposure(shell: &mut Shell, ev: f64) {
    shell
        .values()
        .lock()
        .unwrap()
        .set(helios_values::Set::new(
            "node/exposure/exposure",
            helios_values::Change::To(helios_core::Value::Float(ev)),
            Origin::User,
        ))
        .unwrap();
}

#[test]
fn an_edited_photo_is_badged_until_reset_all_and_after_a_restart() {
    let dir = scratch();
    let path = vault_file(&dir);
    let (mut shell, mut persist) = open(&path);
    // Never edited: no badge (and photos never saved since: unknown, none).
    assert!(!edited(&shell, 1) && !edited(&shell, 2));
    let (graph, exp) = network();
    persist
        .open_image(&mut shell, id(1), graph, exp, "output")
        .unwrap();
    // A step saved: the badge shows, the vault keeps it.
    set_exposure(&mut shell, 1.0);
    persist.frame_settled(&mut shell, 10.0);
    assert!(edited(&shell, 1), "{:?}", badges(&shell, 1));
    assert!(!edited(&shell, 2));
    assert_eq!(stored(&path), Some(Value::Bool(true)));
    // "Reset all": the photo back to its original, the badge gone.
    helios_gui::edit_marks::reset_all(&mut shell.values().lock().unwrap(), &Origin::User).unwrap();
    persist.frame_settled(&mut shell, 11.0);
    assert!(!edited(&shell, 1), "{:?}", badges(&shell, 1));
    assert_eq!(stored(&path), Some(Value::Bool(false)));
    // Edited again, then a restart: read back from the vault.
    set_exposure(&mut shell, 0.5);
    persist.frame_settled(&mut shell, 12.0);
    assert!(edited(&shell, 1));
    drop((shell, persist));
    let (shell, persist) = open(&path);
    assert!(edited(&shell, 1), "{:?}", badges(&shell, 1));
    assert!(!edited(&shell, 2));
    drop((shell, persist));
    let _ = std::fs::remove_dir_all(&dir);
}
