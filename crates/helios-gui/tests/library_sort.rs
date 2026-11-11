//! The library's sort (`docs/design/library.md`, L3), driven as a user
//! drives it: the Library toolbar's sort menu and direction button sort
//! the grid in the vault's query, an agent's `library.sort` too; the sort
//! is the workspace's, written to its files (one undo step each), undone
//! with Ctrl+Z, and the window reopened on the workspace sorts the same.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use helios_core::{Bus, ContentHash};
use helios_gui::persist::Persist;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use helios_template::SortDesc;
use helios_template::workspace::Files;
use helios_vault::capture::CAPTURED;
use helios_vault::fields::RATING;
use helios_vault::{Image, ImageId, Location, Order, Sort, Store};
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-sort-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn id(n: u8) -> ImageId {
    ImageId(ContentHash([n; 32]))
}

/// Three photos: their file name, when they were taken, their stars.
const PHOTOS: [(u8, &str, u64, i64); 3] = [
    (1, "c.jpg", 100, 1),
    (2, "a.jpg", 300, 5),
    (3, "b.jpg", 200, 3),
];

/// A vault file holding [`PHOTOS`], each at its path under `dir`.
fn vault_file(dir: &Path) -> PathBuf {
    let path = dir.join("vault.db");
    let mut store = SqliteStore::open(&path).unwrap();
    for (n, name, taken, stars) in PHOTOS {
        store
            .add_to_library(&Image {
                id: id(n),
                size: 10,
                partial: ContentHash([n; 32]),
            })
            .unwrap();
        store
            .put_location(&id(n), &Location::seen(dir.join(name), 10, 1, 1))
            .unwrap();
        store
            .set_field(&id(n), CAPTURED, Some(&json!(taken)))
            .unwrap();
        store
            .set_field(&id(n), RATING, Some(&json!(stars)))
            .unwrap();
    }
    path
}

/// A workspace made from a built-in template under `root`.
fn workspace(root: &Path) -> (PathBuf, PathBuf) {
    let config = root.join("config");
    let shell = Shell::new(helios_panels(), None);
    let catalogue = templates::catalogue(&shell.template_panels(), shell.specs());
    let created = templates::create_workspace(
        &templates::find("helios.import-cull-dedupe").unwrap(),
        &catalogue,
        &root.join("work"),
        &config,
    )
    .unwrap();
    (config, created.workspace)
}

/// A window on the workspace, its library filled from the vault at
/// `path`.
fn window(
    config: &Path,
    workspace: &Path,
    path: &Path,
) -> (Harness<'static, Shell>, Persist<SqliteStore>) {
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(workspace)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(config, workspace).unwrap();
    let persist = Persist::new(SqliteStore::open(path).unwrap(), Arc::new(Bus::new()));
    persist.fill_library(&mut shell).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    (h, persist)
}

/// The file names the grid shows, in order.
fn grid(h: &Harness<'_, Shell>) -> Vec<String> {
    let library = h.state().library();
    library
        .shown()
        .iter()
        .map(|id| library.entry(id).unwrap().name.clone())
        .collect()
}

/// A few frames, the host's turn between them (as the desktop's loop).
fn settle(h: &mut Harness<'static, Shell>, persist: &mut Persist<SqliteStore>, at: f64) {
    for i in 0..3 {
        h.run();
        persist.frame(h.state_mut(), at + f64::from(i));
    }
    h.run();
}

fn kept(config: &Path, workspace: &Path) -> Option<SortDesc> {
    Files::new(config, workspace)
        .load()
        .unwrap()
        .bundle
        .template
        .library
        .and_then(|l| l.sort)
}

#[test]
fn the_sort_orders_the_grid_in_the_vault_and_is_the_workspace_s() {
    let root = temp("scenario");
    let path = vault_file(&root);
    let (config, ws) = workspace(&root);
    let (mut h, mut persist) = window(&config, &ws, &path);
    // By capture time, oldest first: the default.
    assert_eq!(h.state().library().sort(), Order::asc(Sort::Captured));
    assert_eq!(grid(&h), ["c.jpg", "b.jpg", "a.jpg"]);
    assert_eq!(kept(&config, &ws), None, "the default is not written");

    // The toolbar's menu, as a user picks in it, in the library.
    h.state_mut()
        .run("view.library", Value::Null, Origin::User)
        .unwrap();
    h.run();
    h.run();
    h.get_by_label("Sort by").click();
    h.run();
    h.get_by_label("File name").click();
    settle(&mut h, &mut persist, 1.0);
    assert_eq!(h.state().library().sort(), Order::asc(Sort::Name));
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.jpg"]);
    // The direction button turns it around.
    h.get_by_label("Ascending: oldest, smallest or A first")
        .click();
    settle(&mut h, &mut persist, 10.0);
    assert_eq!(grid(&h), ["c.jpg", "b.jpg", "a.jpg"]);
    let by_name = SortDesc {
        by: "name".into(),
        descending: true,
    };
    assert_eq!(kept(&config, &ws), Some(by_name.clone()));

    // An agent sorts by stars; the direction stays.
    h.state_mut()
        .run(
            "library.sort",
            json!({ "by": "rating" }),
            Origin::Agent("test".into()),
        )
        .unwrap();
    settle(&mut h, &mut persist, 20.0);
    assert_eq!(h.state().library().sort(), Order::desc(Sort::Rating));
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.jpg"]);
    // A sort it does not know changes nothing.
    h.state_mut()
        .run("library.sort", json!({ "by": "colour" }), Origin::User)
        .unwrap();
    settle(&mut h, &mut persist, 25.0);
    assert_eq!(h.state().library().sort(), Order::desc(Sort::Rating));

    // Each sort is one undo step of the workspace.
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    settle(&mut h, &mut persist, 30.0);
    assert_eq!(h.state().library().sort(), Order::desc(Sort::Name));
    assert_eq!(grid(&h), ["c.jpg", "b.jpg", "a.jpg"]);
    assert_eq!(kept(&config, &ws), Some(by_name));
    h.state_mut()
        .run("library.sort", json!({ "by": "rating" }), Origin::User)
        .unwrap();
    settle(&mut h, &mut persist, 40.0);
    drop((h, persist));

    // The window opened again on the workspace sorts as it was left.
    let (h, persist) = window(&config, &ws, &path);
    assert_eq!(h.state().library().sort(), Order::desc(Sort::Rating));
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.jpg"]);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}
