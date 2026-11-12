//! The Library's filter as one vault query (`docs/design/library.md`,
//! L4): Helios's own keys and the words of the search field run in the
//! vault, the grid shows what it answers, and the facet counts of the
//! filter show under the toolbar, a click on one adding it to the filter.
//! The folders tree (L5): its counts, a click scoping the grid, each
//! row's menu, and the folder chosen kept with the workspace. The albums
//! (L6): made from the selection, filled and emptied, one undo step each,
//! a click scoping the grid, the album chosen kept with the workspace.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_pop::views::library::filter::Filter;
use helios_core::{Bus, ContentHash};
use helios_gui::persist::Persist;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use helios_storage::{Identity, LastSeen, Volume, VolumeKind};
use helios_template::workspace::Files;
use helios_vault::capture::{CAMERA_MODEL, CAPTURED, FILE_TYPE};
use helios_vault::fields::{LABELS, RATING};
use helios_vault::storage::VolumeId;
use helios_vault::{Image, ImageId, Location, Presence, Store};
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
    vault_with(dir, |n, name| {
        Location::seen(folder_of(dir, n).join(name), 10, 1, 1)
    })
}

/// The photos, each copy where `place` puts it.
fn vault_with(dir: &Path, place: impl Fn(u8, &str) -> Location) -> PathBuf {
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
        store.put_location(&id(n), &place(n, name)).unwrap();
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

/// A workspace made from a built-in template under `root`: its
/// configuration folder and the workspace.
fn workspace(root: &Path) -> (PathBuf, PathBuf) {
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
    (config, created.workspace)
}

fn window(root: &Path, path: &Path) -> (Harness<'static, Shell>, Persist<SqliteStore>) {
    let (config, workspace) = workspace(root);
    open(&config, &workspace, path)
}

/// A window on `workspace`, in the Library, filled from the vault at
/// `path`.
fn open(
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
    let volumes = h.state().library().folder_tree().unwrap();
    assert_eq!(volumes.len(), 1, "on no known volume: {volumes:?}");
    assert_eq!((volumes[0].id.as_deref(), volumes[0].count), (None, 3));
    h.get_by_label("Other folders 3");
    let tree = &volumes[0].folders;
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

#[test]
fn the_folders_tree_starts_at_the_volumes_an_offline_one_greyed_and_browsable() {
    let root = temp("volumes");
    let card = root.join("card").join("DCIM");
    // The first two on the system disk, the third on a card not plugged in.
    let path = vault_with(&root, |n, name| {
        let (volume, dir, presence) = if n == 3 {
            ("uuid:card", card.clone(), Presence::Offline)
        } else {
            ("uuid:disk", root.join("photos"), Presence::Present)
        };
        Location {
            volume: Some(VolumeId(volume.into())),
            rel_path: Some(PathBuf::from(name)),
            presence,
            ..Location::seen(dir.join(name), 10, 1, 1)
        }
    });
    let (mut h, mut persist) = window(&root, &path);
    let seen = 1_700_000_000;
    let volume = |id: &str, name: &str, mount: Option<PathBuf>, at: u64| Volume {
        id: VolumeId(id.into()),
        identified_by: Identity::Uuid,
        name: name.into(),
        label: Some(name.into()),
        kind: VolumeKind::Removable,
        mount: mount.clone(),
        last_seen: Some(LastSeen {
            at,
            node: "here".into(),
            mount: mount.or_else(|| Some(card.clone())),
        }),
        ..Volume::remote("x", "y", name)
    };
    h.state_mut().library_mut().set_volumes(&[
        volume("uuid:card", "EOS_CARD", None, seen),
        volume("uuid:disk", "System", Some(PathBuf::from("/")), seen + 100),
    ]);
    h.get_by_label("Folders").click();
    settle(&mut h, &mut persist, 1.0);

    // Both volumes at the top, the one at hand first, with their counts.
    let volumes = h.state().library().folder_tree().unwrap();
    let tops: Vec<_> = volumes
        .iter()
        .map(|v| (v.name.as_str(), v.count, v.online))
        .collect();
    assert_eq!(tops, [("System", 2, true), ("EOS_CARD", 1, false)]);
    h.get_by_label("System 2");
    // The offline one says so and when it was last seen.
    let day = helios_gui::library::day_text(seen as i64).unwrap();
    assert_eq!(volumes[1].last_seen, Some(seen));
    h.get_by_label(&format!("EOS_CARD 1, offline, last seen {day}"));

    // Its folder still filters the grid: the photo is offline, its
    // thumbnail cached.
    let dcim = card.to_string_lossy().into_owned();
    assert_eq!(volumes[1].folders[0].path, dcim);
    let row = format!("{} 1", volumes[1].folders[0].name);
    h.get_by_label(&row).click();
    settle(&mut h, &mut persist, 5.0);
    {
        // The offline photo alone (named by its id: its file is not at hand).
        let library = h.state().library();
        let shown = library.shown();
        assert_eq!(shown.len(), 1, "{:?}", grid(&h));
        let entry = library.entry(&shown[0]).unwrap();
        assert_eq!(entry.presence, Presence::Offline);
    }
    assert_eq!(h.state().library().folder(), Some(dcim));
    h.get_by_label(&row).click();
    settle(&mut h, &mut persist, 9.0);
    assert_eq!(grid(&h).len(), 3);

    // The offline folder's menu: nothing at hand, all greyed.
    row_menu(&mut h, &row);
    assert!(menu_greyed(&h).iter().all(|(_, greyed)| *greyed));
    // A volume at hand's: its mount point, shown, scanned, imported.
    row_menu(&mut h, "System 2");
    assert!(menu_greyed(&h).iter().all(|(_, greyed)| !*greyed));
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The folder the workspace keeps, as written to its files.
fn kept_folder(config: &Path, workspace: &Path) -> Option<String> {
    Files::new(config, workspace)
        .load()
        .unwrap()
        .bundle
        .template
        .library
        .and_then(|l| l.folder)
}

/// A right click on a row of the folders tree: the shell's menu opens.
fn row_menu(h: &mut Harness<'static, Shell>, row: &str) {
    h.get_by_label(row).click_secondary();
    for _ in 0..3 {
        h.run();
    }
}

/// The folder menu's entries, each with whether it is greyed.
fn menu_greyed(h: &Harness<'_, Shell>) -> Vec<(&'static str, bool)> {
    [
        "Show in file manager",
        "Scan this folder",
        "Import new photos here",
    ]
    .into_iter()
    .map(|label| {
        // An entry's label ends with its command's keys, when it has some.
        let mut found = h.query_all_by_label_contains(label);
        let node = found
            .next()
            .unwrap_or_else(|| panic!("{label}: not in the menu"));
        assert!(found.next().is_none(), "{label}: twice");
        (label, node.accesskit_node().is_disabled())
    })
    .collect()
}

#[test]
fn the_folder_chosen_is_the_workspace_s_and_its_menu_imports_there() {
    let root = temp("folder-kept");
    let path = vault_file(&root);
    let (config, ws) = workspace(&root);
    let (mut h, mut persist) = open(&config, &ws, &path);
    let raw = root.join("raw").to_string_lossy().into_owned();
    h.get_by_label("Folders").click();
    settle(&mut h, &mut persist, 1.0);
    assert_eq!(kept_folder(&config, &ws), None);
    h.get_by_label("raw 1").click();
    settle(&mut h, &mut persist, 5.0);
    assert_eq!(grid(&h), ["c.nef"]);
    assert_eq!(kept_folder(&config, &ws), Some(raw.clone()));

    // Another place of the window and back: the folder stays the focus's.
    for place in ["view.network", "view.library"] {
        h.state_mut().run(place, Value::Null, Origin::User).unwrap();
        settle(&mut h, &mut persist, 10.0);
    }
    assert_eq!(h.state().library().folder(), Some(raw.clone()));
    assert_eq!(grid(&h), ["c.nef"]);
    // Another library setting written: the folder stays, here and kept.
    h.state_mut()
        .run("library.sort", json!({ "by": "name" }), Origin::User)
        .unwrap();
    settle(&mut h, &mut persist, 15.0);
    assert_eq!(h.state().library().folder(), Some(raw.clone()));
    assert_eq!(kept_folder(&config, &ws), Some(raw.clone()));
    // A relative path changes nothing.
    h.state_mut()
        .run("library.folder", json!({ "path": "raw" }), Origin::User)
        .unwrap();
    settle(&mut h, &mut persist, 18.0);
    assert_eq!(h.state().library().folder(), Some(raw.clone()));
    assert_eq!(kept_folder(&config, &ws), Some(raw.clone()));
    drop((h, persist));

    // The workspace opened again: the same folder, the same grid.
    let (mut h, mut persist) = open(&config, &ws, &path);
    settle(&mut h, &mut persist, 20.0);
    assert_eq!(h.state().library().folder(), Some(raw.clone()));
    assert_eq!(grid(&h), ["c.nef"]);

    // The row's menu: its three commands, at hand.
    h.get_by_label("Folders").click();
    settle(&mut h, &mut persist, 25.0);
    row_menu(&mut h, "raw 1");
    assert_eq!(
        menu_greyed(&h),
        [
            ("Show in file manager", false),
            ("Scan this folder", false),
            ("Import new photos here", false)
        ]
    );
    // "Import new photos here": the import window, on the folder.
    h.query_all_by_label_contains("Import new photos here")
        .next()
        .unwrap()
        .click();
    h.run_steps(4);
    assert!(h.state().import_window_open());
    assert_eq!(h.state().import_ticked(), [PathBuf::from(&raw)]);
    // The main window's folder is untouched by the import focus.
    assert_eq!(h.state().library().folder(), Some(raw.clone()));
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The collection the workspace keeps, as written to its files.
fn kept_collection(config: &Path, workspace: &Path) -> Option<String> {
    Files::new(config, workspace)
        .load()
        .unwrap()
        .bundle
        .template
        .library
        .and_then(|l| l.collection)
}

/// Runs a command as the user; the shell queues it for its frame.
fn run(h: &mut Harness<'static, Shell>, id: &str, args: Value) {
    h.state_mut().run(id, args, Origin::User).unwrap();
}

/// The albums as the Library shows them: name and count.
fn albums(h: &Harness<'_, Shell>) -> Vec<(String, usize)> {
    let library = h.state().library();
    library
        .albums()
        .unwrap_or_default()
        .iter()
        .map(|a| (a.name.clone(), library.album_count(a)))
        .collect()
}

#[test]
fn an_album_made_from_the_selection_scopes_the_grid_and_each_change_undoes() {
    let root = temp("albums");
    let path = vault_file(&root);
    let (config, ws) = workspace(&root);
    let (mut h, mut persist) = open(&config, &ws, &path);
    // As the desktop wires it: undo and redo move the vault's rows.
    let history = h.state().library_history();
    {
        let vault = persist.vault().clone();
        history.set_apply(Arc::new(move |changes, forward| {
            if forward {
                vault.replay(changes)
            } else {
                vault.revert(changes)
            }
        }));
    }
    h.get_by_label("Albums").click();
    settle(&mut h, &mut persist, 1.0);
    h.get_by_label("No album yet");

    // "New album…" from the selection: the form asks its name.
    h.state_mut()
        .library_mut()
        .select_ids(&[id(1).hex(), id(3).hex()]);
    h.get_by_label("New album…").click();
    settle(&mut h, &mut persist, 2.0);
    run(
        &mut h,
        "dialog.answer",
        json!({ "values": { "name": "Trip 2024" } }),
    );
    settle(&mut h, &mut persist, 3.0);
    assert_eq!(albums(&h), [("Trip 2024".to_string(), 2)]);
    let line = history.line();
    assert_eq!(
        line.undo.last().unwrap().label.text,
        "New album “Trip 2024”"
    );

    // A click scopes the grid to it, kept with the workspace.
    h.get_by_label("Trip 2024 2").click();
    settle(&mut h, &mut persist, 5.0);
    assert_eq!(grid(&h), ["a.jpg", "c.nef"]);
    assert_eq!(
        h.state().library().collection().as_deref(),
        Some("trip-2024")
    );
    assert_eq!(kept_collection(&config, &ws).as_deref(), Some("trip-2024"));

    // Photos added and removed by an agent: the grid follows.
    run(
        &mut h,
        "library.collection.add",
        json!({ "id": "trip-2024", "images": [id(2).hex()] }),
    );
    settle(&mut h, &mut persist, 8.0);
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);
    run(
        &mut h,
        "library.collection.remove",
        json!({ "id": "trip-2024", "images": [id(1).hex()] }),
    );
    settle(&mut h, &mut persist, 11.0);
    assert_eq!(grid(&h), ["b.jpg", "c.nef"]);
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Remove 1 photos from “Trip 2024”"
    );
    // Undone: the photo is back in the album and the grid.
    let undone = history.undo(
        &egui_pop::api::history::Request::new(helios_gui::library_history::LIBRARY),
        &egui_pop::model::commands::Origin::User,
    );
    assert!(
        matches!(undone, egui_pop::api::history::Answer::Done(Ok(_))),
        "{undone:?}"
    );
    settle(&mut h, &mut persist, 14.0);
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);
    assert_eq!(albums(&h), [("Trip 2024".to_string(), 3)]);

    // Renamed; a name taken or an unknown album refused.
    run(
        &mut h,
        "library.collection.rename",
        json!({ "id": "trip-2024", "name": "Summer" }),
    );
    settle(&mut h, &mut persist, 17.0);
    assert_eq!(albums(&h), [("Summer".to_string(), 3)]);
    run(
        &mut h,
        "library.collection.create",
        json!({ "name": "SUMMER" }),
    );
    run(&mut h, "library.collection", json!({ "id": "nope" }));
    settle(&mut h, &mut persist, 20.0);
    assert_eq!(albums(&h).len(), 1);
    assert_eq!(
        h.state().library().collection().as_deref(),
        Some("trip-2024")
    );
    drop((h, persist));

    // The workspace opened again: the same album, the same grid.
    let (mut h, mut persist) = open(&config, &ws, &path);
    settle(&mut h, &mut persist, 22.0);
    assert_eq!(
        h.state().library().collection().as_deref(),
        Some("trip-2024")
    );
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);

    // Deleted: an album that is not there shows no photo; the photos stay
    // in the Library.
    run(
        &mut h,
        "library.collection.delete",
        json!({ "id": "trip-2024" }),
    );
    settle(&mut h, &mut persist, 25.0);
    assert!(albums(&h).is_empty());
    assert!(grid(&h).is_empty());
    run(&mut h, "library.collection", json!({ "id": null }));
    settle(&mut h, &mut persist, 28.0);
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);
    assert_eq!(kept_collection(&config, &ws), None);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}
