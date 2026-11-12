//! The Library's filter as one vault query (`docs/design/library.md`,
//! L4): Helios's own keys and the words of the search field run in the
//! vault, the grid shows what it answers, and the facet counts of the
//! filter show under the toolbar, a click on one adding it to the filter.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_pop::views::library::filter::Filter;
use helios_core::{Bus, ContentHash};
use helios_gui::persist::Persist;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use helios_vault::capture::{CAMERA_MODEL, CAPTURED, FILE_TYPE};
use helios_vault::fields::{LABELS, RATING};
use helios_vault::{Image, ImageId, Location, Store};
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-filter-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn id(n: u8) -> ImageId {
    ImageId(ContentHash([n; 32]))
}

/// A photo: its id byte, file name, camera, file type, labels, stars.
type Photo = (
    u8,
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    i64,
);

/// Three photos, the third in a `raw` folder.
const PHOTOS: [Photo; 3] = [
    (1, "a.jpg", "Z 8", "jpg", &["red"], 3),
    (2, "b.jpg", "Z 8", "jpg", &[], 1),
    (3, "c.nef", "R5", "nef", &["red"], 0),
];

/// Where a photo's copy is: the third in `raw`, the others at the top.
fn folder_of(dir: &Path, n: u8) -> PathBuf {
    if n == 3 {
        dir.join("raw")
    } else {
        dir.to_path_buf()
    }
}

fn vault_file(dir: &Path) -> PathBuf {
    let path = dir.join("vault.db");
    let mut store = SqliteStore::open(&path).unwrap();
    for (n, name, camera, kind, labels, stars) in PHOTOS {
        store
            .add_to_library(&Image {
                id: id(n),
                size: 10,
                partial: ContentHash([n; 32]),
            })
            .unwrap();
        store
            .put_location(
                &id(n),
                &Location::seen(folder_of(dir, n).join(name), 10, 1, 1),
            )
            .unwrap();
        for (field, value) in [
            (CAPTURED, json!(u64::from(n) * 100)),
            (CAMERA_MODEL, json!(camera)),
            (FILE_TYPE, json!(kind)),
            (RATING, json!(stars)),
        ] {
            store.set_field(&id(n), field, Some(&value)).unwrap();
        }
        if !labels.is_empty() {
            store
                .set_field(&id(n), LABELS, Some(&json!(labels)))
                .unwrap();
        }
    }
    path
}

fn window(root: &Path, path: &Path) -> (Harness<'static, Shell>, Persist<SqliteStore>) {
    let config = root.join("config");
    let probe = Shell::new(helios_panels(), None);
    let catalogue = templates::catalogue(&probe.template_panels(), probe.specs());
    let created = templates::create_workspace(
        &templates::find("helios.import-cull-dedupe").unwrap(),
        &catalogue,
        &root.join("work"),
        &config,
    )
    .unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&created.workspace)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &created.workspace).unwrap();
    let persist = Persist::new(SqliteStore::open(path).unwrap(), Arc::new(Bus::new()));
    persist.fill_library(&mut shell).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h.state_mut()
        .run("view.library", Value::Null, Origin::User)
        .unwrap();
    h.run();
    h.run();
    (h, persist)
}

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

fn filter(h: &mut Harness<'static, Shell>, persist: &mut Persist<SqliteStore>, text: &str) {
    h.state_mut().library_mut().set_filter(Filter::parse(text));
    settle(h, persist, 1.0);
}

#[test]
fn the_filter_runs_in_the_vault_and_its_facets_are_counted() {
    let root = temp("scenario");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);
    assert!(h.query_by_label("Camera").is_none(), "no filter: no counts");

    // A camera, quoted for its space: the vault answers.
    filter(&mut h, &mut persist, "camera:\"Z 8\"");
    assert_eq!(grid(&h), ["a.jpg", "b.jpg"]);
    let facets = h.state().library().facets().cloned().unwrap();
    assert_eq!(facets.count("camera", "Z 8"), 2);
    assert_eq!(facets.count("camera", "R5"), 0, "counted in the filter");
    assert_eq!(facets.count("label", "red"), 1);
    assert_eq!(facets.count("file_type", "jpg"), 2);
    assert_eq!(facets.count("presence", "present"), 2);
    // The counts show under the toolbar; a click adds the value.
    h.get_by_label("Camera");
    h.get_by_label("Z 8 2");
    h.get_by_label("red 1").click();
    settle(&mut h, &mut persist, 5.0);
    assert_eq!(grid(&h), ["a.jpg"]);
    assert_eq!(h.state().library().filter().labels.len(), 1);

    // Words, the file type, presence: in the vault too.
    filter(&mut h, &mut persist, "B.J");
    assert_eq!(grid(&h), ["b.jpg"], "a word, whatever its case");
    filter(&mut h, &mut persist, "type:nef");
    assert_eq!(grid(&h), ["c.nef"]);
    filter(&mut h, &mut persist, "presence:missing");
    assert!(grid(&h).is_empty());
    filter(&mut h, &mut persist, "");
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);
    assert!(h.state().library().facets().is_none());
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_folders_tree_counts_as_the_vault_and_a_click_scopes_the_grid() {
    let root = temp("folders");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
    assert!(
        h.state().library().folder_tree().is_none(),
        "closed: not counted"
    );
    h.get_by_label("Folders").click();
    settle(&mut h, &mut persist, 1.0);
    let tree = h.state().library().folder_tree().unwrap();
    let top = root.to_string_lossy().into_owned();
    let raw = root.join("raw").to_string_lossy().into_owned();
    assert_eq!(tree.len(), 1, "{tree:?}");
    assert_eq!((tree[0].path.as_str(), tree[0].count), (top.as_str(), 3));
    assert_eq!(tree[0].name, top, "a top row names its whole path");
    assert_eq!(tree[0].children.len(), 1);
    let below = &tree[0].children[0];
    assert_eq!(
        (below.path.as_str(), below.name.as_str(), below.count),
        (raw.as_str(), "raw", 1)
    );
    // Each count is what the `folder:` filter selects in the vault.
    for node in [&tree[0], below] {
        let cursor = persist
            .vault()
            .open_cursor(helios_vault::Query {
                folder: Some(node.path.clone()),
                ..helios_vault::Query::default()
            })
            .unwrap();
        persist.vault().close_cursor(cursor.id);
        assert_eq!(cursor.count as u64, node.count, "{}", node.path);
    }

    // A click scopes the grid to the folder; again, to the whole Library.
    h.get_by_label(&format!("{top} 3"));
    h.get_by_label("raw 1").click();
    settle(&mut h, &mut persist, 5.0);
    assert_eq!(grid(&h), ["c.nef"]);
    assert_eq!(h.state().library().folder(), Some(raw.clone()));
    h.get_by_label("raw 1").click();
    settle(&mut h, &mut persist, 9.0);
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);
    assert_eq!(h.state().library().folder(), None);
    // The scope keeps the rest of the filter.
    filter(&mut h, &mut persist, "type:jpg");
    h.state_mut().library_mut().set_folder(Some(&top));
    settle(&mut h, &mut persist, 12.0);
    assert_eq!(grid(&h), ["a.jpg", "b.jpg"]);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}
