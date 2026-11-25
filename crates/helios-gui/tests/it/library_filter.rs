//! The Library's filter as one vault query (`docs/design/library.md`,
//! L4): Helios's own keys and the words of the search field run in the
//! vault, the grid shows what it answers, and the facet counts of the
//! filter show under the toolbar, a click on one adding it to the filter.
//! The folders tree (L5): its counts, a click scoping the grid, each
//! row's menu, and the folder chosen kept with the workspace. The albums
//! (L6): made from the selection, filled and emptied, one undo step each,
//! a click scoping the grid, the album chosen kept with the workspace;
//! smart albums made through their form, their rules set by an agent
//! (nested, undone, a fault refused), scoping the grid the same way. The
//! quick collection: B toggles the selection in and out (one step a
//! press, not while a text field is typed in), its count and the grid's
//! badge follow, `collection:quick` scopes the grid, "Save as album"
//! keeps its photos and empties it or not. The recent filters: those
//! used are listed (deduplicated), pinned, kept across a restart, and
//! Ctrl+Shift+K opens them in the window, not while a text field is typed
//! in. Photos dragged from the grid onto an album or the quick collection
//! go in, one step; a smart album or a set refuses them with the reason.
//! The selection commands (L7): Ctrl+I inverts, Ctrl+Shift+A selects
//! none, the photos of the same day selected; a photo's marks copied and
//! pasted on others, merged or overwriting, written to the vault.
//! "Remove from Library" (L8): Delete takes the selection out through the
//! node, the grid follows, and the step undoes. "Move to Trash…" asks
//! first, then the node's journaled job moves the files to a test trash,
//! a backup's copy refused. Rotate (L9): Ctrl+] and Ctrl+[ turn the
//! selection through the node, the thumbnail is shown turned, and the
//! step undoes. Duplicate (L10): Ctrl+' makes a new head through the
//! node, the grid's badge counts the versions, an agent duplicates the
//! original. RAW+JPEG files paired into one cell and unpaired, each
//! one step undone.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::host::Settle as _;
use egui::{Key, vec2};
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
    h.settle();
    h.state_mut()
        .run("view.library", Value::Null, Origin::User)
        .unwrap();
    h.settle();
    h.settle();
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
        h.settle();
        persist.frame_settled(h.state_mut(), at + f64::from(i));
    }
    h.settle();
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
    h.get_by_label("Other folders, 3");
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
    h.get_by_label(&format!("{top}, 3"));
    h.get_by_label("raw, 1").click();
    settle(&mut h, &mut persist, 5.0);
    assert_eq!(grid(&h), ["c.nef"]);
    assert_eq!(h.state().library().folder(), Some(raw.clone()));
    h.get_by_label("raw, 1").click();
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

/// The Library's Filters docked above its Results across the window
/// (#5078 A2): a bar's shape, so the search and the sources' drop-down
/// share one line inside it, the sources hidden until it opens; a folder
/// chosen there scopes the grid as in the column.
#[test]
fn the_filters_docked_above_the_results_draw_one_line_with_the_sources_behind_a_drop_down() {
    use egui_pop::model::ids::PanelId;
    use egui_pop::model::layout::Node;
    use egui_pop::model::panels::Area;
    let root = temp("bar");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
    // At the start side, a column: the sections in view, no drop-down.
    h.get_by_label("Folders");
    assert!(
        h.query_by_label("Sources").is_none(),
        "a column has no drop-down"
    );

    let filters = PanelId::new("helios.filters").unwrap();
    let results = PanelId::new("helios.vault").unwrap();
    let mut layout = h.state().layout();
    layout.remove(&filters);
    layout.remove(&results);
    // Across the window's middle: the Filters above, the Results under
    // them beside what the middle held.
    let at = layout
        .areas
        .iter()
        .position(|(area, _)| *area == Area::Center);
    let below = match at.map(|at| layout.areas.remove(at).1) {
        Some(rest) => Node::Split {
            vertical: false,
            ratio: 0.6,
            a: Box::new(Node::tab(results)),
            b: Box::new(rest),
        },
        None => Node::tab(results),
    };
    layout.areas.push((
        Area::Center,
        Node::Split {
            vertical: true,
            ratio: 0.12,
            a: Box::new(Node::tab(filters)),
            b: Box::new(below),
        },
    ));
    h.state_mut().set_layout(layout);
    settle(&mut h, &mut persist, 1.0);

    let panel = h.state().target_rect("helios.filters").unwrap();
    assert!(
        panel.width() >= 2.0 * panel.height(),
        "docked as a bar: {panel:?}"
    );
    let sources = h.query_all_by_label("Sources").next().unwrap().rect();
    let search = h.get_by_label("Search images").rect();
    for (what, r) in [("Sources", sources), ("the search", search)] {
        assert!(panel.contains_rect(r), "{what} at {r:?} out of {panel:?}");
    }
    assert!(
        sources.y_range().intersects(search.y_range()) && sources.max.x <= search.min.x,
        "one line, the sources first: {sources:?} {search:?}"
    );
    assert!(
        h.query_by_label("Folders").is_none(),
        "the sources behind their drop-down"
    );

    // Opened, the same sections; a folder chosen scopes the grid, the
    // list still open to choose again.
    h.query_all_by_label("Sources").next().unwrap().click();
    h.settle();
    h.get_by_label("Folders").click();
    settle(&mut h, &mut persist, 3.0);
    let raw = root.join("raw").to_string_lossy().into_owned();
    h.get_by_label("raw, 1").click();
    settle(&mut h, &mut persist, 5.0);
    assert_eq!(grid(&h), ["c.nef"]);
    assert_eq!(h.state().library().folder(), Some(raw));
    // The scope shows in the bar's line, removable there.
    let chip = h
        .query_all_by_label_contains("Folder: ")
        .next()
        .unwrap()
        .rect();
    assert!(
        chip.y_range().intersects(search.y_range()),
        "the restriction on the bar's line: {chip:?} {search:?}"
    );
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
        // Mounted at the system's root: `/`, or the temporary folder's
        // drive (`C:\`) on Windows.
        volume(
            "uuid:disk",
            "System",
            root.ancestors().last().map(Path::to_path_buf),
            seen + 100,
        ),
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
    h.get_by_label("System, 2");
    // The offline one says so and when it was last seen.
    let day = helios_gui::library::day_text(seen as i64).unwrap();
    assert_eq!(volumes[1].last_seen, Some(seen));
    // Dimmed, said unavailable, its line saying when it was last seen.
    let said = |h: &Harness<'_, Shell>, label: &str| {
        h.get_by_label(label)
            .accesskit_node()
            .description()
            .unwrap_or_default()
    };
    assert_eq!(
        said(&h, "EOS_CARD, 1"),
        format!("Unavailable, Offline, last seen {day}")
    );
    assert_eq!(said(&h, "System, 2"), "", "a volume at hand is not dimmed");

    // Its folder still filters the grid: the photo is offline, its
    // thumbnail cached.
    let dcim = card.to_string_lossy().into_owned();
    assert_eq!(volumes[1].folders[0].path, dcim);
    let row = format!("{}, 1", volumes[1].folders[0].name);
    // Its folders dimmed too, still choosable. The row is named by its
    // whole path, wider than the side bar where the temporary folder's
    // path is long (Windows): chosen as assistive technology does, not at
    // its centre, which may lie past the bar.
    assert_eq!(said(&h, &row), "Unavailable");
    h.get_by_label(&row).click_accesskit();
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
    h.get_by_label(&row).click_accesskit();
    settle(&mut h, &mut persist, 9.0);
    assert_eq!(grid(&h).len(), 3);

    // The offline folder's menu: nothing at hand, all greyed.
    row_menu(&mut h, &row);
    assert!(menu_greyed(&h).iter().all(|(_, greyed)| *greyed));
    // A volume at hand's: its mount point, shown, scanned, imported.
    row_menu(&mut h, "System, 2");
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
/// Near the row's start, as a user would: a row named by a long path
/// (the temporary folder's, on Windows) is wider than the side bar, and
/// its centre may lie past it.
fn row_menu(h: &mut Harness<'static, Shell>, row: &str) {
    let rect = h.get_by_label(row).rect();
    let pos = egui::pos2((rect.min.x + 40.0).min(rect.center().x), rect.center().y);
    h.hover_at(pos);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    for _ in 0..3 {
        h.settle();
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
    h.get_by_label("raw, 1").click();
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
    row_menu(&mut h, "raw, 1");
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
    h.get_by_label("Trip 2024, 2").click();
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

/// The warnings the shell gave so far, as one text.
fn warnings(h: &Harness<'_, Shell>) -> String {
    h.state()
        .notices()
        .iter()
        .map(|n| n.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_smart_album_made_through_its_form_scopes_the_grid_and_its_rules_undo() {
    let root = temp("smart");
    let path = vault_file(&root);
    let (config, ws) = workspace(&root);
    let (mut h, mut persist) = open(&config, &ws, &path);
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

    // "New smart album…": the form asks its name and its conditions.
    h.get_by_label("New smart album…").click();
    settle(&mut h, &mut persist, 2.0);
    run(
        &mut h,
        "dialog.answer",
        json!({ "values": { "name": "Best", "rating": 3 } }),
    );
    settle(&mut h, &mut persist, 3.0);
    assert_eq!(albums(&h), [("Best".to_string(), 1)]);
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "New smart album “Best”"
    );

    // A click on it (marked as smart) scopes the grid, as an album's.
    h.get_by_label("⚙ Best, 1").click();
    settle(&mut h, &mut persist, 5.0);
    assert_eq!(h.state().library().collection().as_deref(), Some("best"));
    assert_eq!(kept_collection(&config, &ws).as_deref(), Some("best"));
    assert_eq!(grid(&h), ["a.jpg"]);

    // An agent sets nested rules: the grid and the count follow.
    run(
        &mut h,
        "library.collection.rules",
        json!({ "id": "best", "rules": {"match": "any", "rules": [
            {"match": "all", "rules": [
                {"field": "camera", "op": "is", "value": "Z 8"},
                {"field": "rating", "op": "<=", "value": 1},
            ]},
            {"field": "file_type", "op": "is", "value": "nef"},
        ]} }),
    );
    settle(&mut h, &mut persist, 8.0);
    assert_eq!(grid(&h), ["b.jpg", "c.nef"]);
    assert_eq!(albums(&h), [("Best".to_string(), 2)]);
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Edit the rules of “Best”"
    );

    // Refused with their reason: an unknown field, photos put in a smart
    // album.
    run(
        &mut h,
        "library.collection.rules",
        json!({ "id": "best", "rules": {"match": "all", "rules": [
            {"field": "colour", "op": "is", "value": "red"},
        ]} }),
    );
    run(
        &mut h,
        "library.collection.add",
        json!({ "id": "best", "images": [id(1).hex()] }),
    );
    settle(&mut h, &mut persist, 11.0);
    let said = warnings(&h);
    assert!(said.contains("unknown field `colour`"), "{said}");
    assert!(said.contains("smart album"), "{said}");
    assert_eq!(grid(&h), ["b.jpg", "c.nef"]);

    // Undone: the first rules again.
    let undone = history.undo(
        &egui_pop::api::history::Request::new(helios_gui::library_history::LIBRARY),
        &egui_pop::model::commands::Origin::User,
    );
    assert!(
        matches!(undone, egui_pop::api::history::Answer::Done(Ok(_))),
        "{undone:?}"
    );
    settle(&mut h, &mut persist, 14.0);
    assert_eq!(grid(&h), ["a.jpg"]);
    assert_eq!(albums(&h), [("Best".to_string(), 1)]);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The open rule editor's state, as `library.collection.rules.edit`
/// answers it (`Value::Null` when closed).
fn editor(h: &Harness<'_, Shell>) -> Value {
    h.state()
        .rule_editor()
        .map_or(Value::Null, helios_gui::rule_editor::RuleEditor::state)
}

/// The rule editor's picture on a nested tree that shows each kind of
/// value box (a number between two bounds, a relative date with its
/// unit, a colour label picked from a list, a typed text) and a fault
/// under its condition. Regenerated by the box's `gui-snapshots` job with
/// `test=library_filter`.
#[test]
fn rule_editor_snapshots() {
    use crate::shot::Shots as _;

    let root = temp("rule-shots");
    let path = vault_file(&root);
    let (config, ws) = workspace(&root);
    let (mut h, mut persist) = open(&config, &ws, &path);
    h.get_by_label("Albums").click();
    settle(&mut h, &mut persist, 1.0);
    run(
        &mut h,
        "library.collection.create",
        json!({ "name": "Best", "kind": "smart", "rules": {"match": "any", "rules": [
            {"field": "rating", "op": "between", "value": [2, 4]},
            {"match": "all", "rules": [
                {"field": "captured", "op": "within", "value": {"last": 2, "unit": "weeks"}},
                {"field": "label", "op": "has", "value": "red"},
            ]},
            {"field": "camera", "op": "is", "value": "Z 8"},
            {"field": "rating", "op": ">=", "value": 3},
        ]} }),
    );
    settle(&mut h, &mut persist, 2.0);
    run(&mut h, "library.collection.rules", json!({ "id": "best" }));
    settle(&mut h, &mut persist, 3.0);
    // A fault said under its condition.
    run(
        &mut h,
        "library.collection.rules.edit",
        json!({ "edit": "value", "at": [3], "value": "9" }),
    );
    settle(&mut h, &mut persist, 4.0);
    assert_eq!(editor(&h)["faults"][0]["at"], json!([3]));
    let mut results = egui_kittest::SnapshotResults::new();
    let rect = h
        .state()
        .target_rect(helios_gui::rule_editor_ui::RECT)
        .unwrap();
    results.add(h.try_shot_of("rule-editor-pop-dark-1x", rect));
    drop(results);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_nested_tree_is_built_in_the_rule_editor_each_edit_undone() {
    let root = temp("rule-editor");
    let path = vault_file(&root);
    let (config, ws) = workspace(&root);
    let (mut h, mut persist) = open(&config, &ws, &path);
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
    run(
        &mut h,
        "library.collection.create",
        json!({ "name": "Best", "kind": "smart", "rules": {"match": "all", "rules": [
            {"field": "rating", "op": ">=", "value": 3},
        ]} }),
    );
    settle(&mut h, &mut persist, 2.0);
    run(&mut h, "library.collection", json!({ "id": "best" }));
    settle(&mut h, &mut persist, 3.0);
    assert_eq!(grid(&h), ["a.jpg"]);

    // "Edit the rules…" opens the editor on the tree; its own button adds
    // a condition.
    run(&mut h, "library.collection.rules", json!({ "id": "best" }));
    settle(&mut h, &mut persist, 4.0);
    assert_eq!(editor(&h)["id"], "best");
    h.get_by_label("+ Condition").click();
    settle(&mut h, &mut persist, 5.0);
    assert_eq!(editor(&h)["draft"]["rules"].as_array().unwrap().len(), 2);
    let edit = "library.collection.rules.edit";
    run(&mut h, edit, json!({ "edit": "undo" }));
    settle(&mut h, &mut persist, 6.0);
    assert_eq!(editor(&h)["draft"]["rules"].as_array().unwrap().len(), 1);
    run(&mut h, edit, json!({ "edit": "redo" }));
    settle(&mut h, &mut persist, 7.0);
    assert_eq!(editor(&h)["draft"]["rules"].as_array().unwrap().len(), 2);

    // An invalid value: its reason under it, and the rules not applied.
    run(
        &mut h,
        edit,
        json!({ "edit": "value", "at": [1], "value": "9" }),
    );
    settle(&mut h, &mut persist, 8.0);
    assert_eq!(editor(&h)["faults"][0]["at"], json!([1]));
    assert!(h.query_by_label_contains("not in [0, 5]").is_some());
    run(&mut h, edit, json!({ "edit": "apply" }));
    settle(&mut h, &mut persist, 9.0);
    let said = warnings(&h);
    assert!(
        said.contains("rules[1]") && said.contains("not in [0, 5]"),
        "{said}"
    );
    assert!(!editor(&h).is_null(), "still open");
    assert_eq!(grid(&h), ["a.jpg"]);

    // A nested tree, edit by edit.
    for args in [
        json!({ "edit": "remove", "at": [1] }),
        json!({ "edit": "remove", "at": [0] }),
        json!({ "edit": "match", "at": [], "match": "any" }),
        json!({ "edit": "add_group", "at": [] }),
        json!({ "edit": "add_condition", "at": [0] }),
        json!({ "edit": "field", "at": [0, 0], "field": "camera" }),
        json!({ "edit": "value", "at": [0, 0], "value": "Z 8" }),
        json!({ "edit": "add_condition", "at": [0] }),
        json!({ "edit": "op", "at": [0, 1], "op": "<=" }),
        json!({ "edit": "value", "at": [0, 1], "value": "1" }),
        json!({ "edit": "add_condition", "at": [] }),
        json!({ "edit": "field", "at": [1], "field": "file_type" }),
        json!({ "edit": "value", "at": [1], "value": "nef" }),
        json!({ "edit": "move", "at": [1], "to": 0 }),
    ] {
        run(&mut h, edit, args);
        h.settle();
    }
    settle(&mut h, &mut persist, 10.0);
    let expected = json!({"match": "any", "rules": [
        {"field": "file_type", "op": "is", "value": "nef"},
        {"match": "all", "rules": [
            {"field": "camera", "op": "is", "value": "Z 8"},
            {"field": "rating", "op": "<=", "value": 1},
        ]},
    ]});
    assert_eq!(editor(&h)["rules"], expected);
    run(&mut h, edit, json!({ "edit": "undo" }));
    settle(&mut h, &mut persist, 11.0);
    assert_eq!(
        editor(&h)["rules"]["rules"][0]["match"],
        "all",
        "the move undone"
    );
    run(&mut h, edit, json!({ "edit": "redo" }));
    settle(&mut h, &mut persist, 12.0);
    assert_eq!(editor(&h)["rules"], expected);
    run(&mut h, edit, json!({ "edit": "apply" }));
    settle(&mut h, &mut persist, 13.0);
    settle(&mut h, &mut persist, 14.0);
    assert!(editor(&h).is_null(), "the editor closed");
    assert!(h.query_by_label("+ Condition").is_none());
    assert_eq!(grid(&h), ["b.jpg", "c.nef"]);
    assert_eq!(albums(&h), [("Best".to_string(), 2)]);
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Edit the rules of “Best”"
    );

    // The whole edit is one step of the library's history.
    let undone = history.undo(
        &egui_pop::api::history::Request::new(helios_gui::library_history::LIBRARY),
        &egui_pop::model::commands::Origin::User,
    );
    assert!(
        matches!(undone, egui_pop::api::history::Answer::Done(Ok(_))),
        "{undone:?}"
    );
    settle(&mut h, &mut persist, 16.0);
    assert_eq!(grid(&h), ["a.jpg"]);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The first quick collection's row in the Albums section, the target
/// ("+"), by its count.
fn quick_row(count: usize) -> String {
    format!("⚡ Quick collection, + {count}")
}

/// The steps of the library's history, newest last.
fn steps(h: &Harness<'_, Shell>) -> Vec<String> {
    h.state()
        .library_history()
        .line()
        .undo
        .iter()
        .map(|s| s.label.text.clone())
        .collect()
}

#[test]
fn b_toggles_the_quick_collection_which_scopes_the_grid_and_saves_as_an_album() {
    let root = temp("quick");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
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
    // Always there, pinned first, even with no album.
    h.get_by_label(&quick_row(0));
    h.get_by_label("No album yet");

    // B in the grid: the selection goes in, one step; the badge shows.
    h.state_mut()
        .library_mut()
        .select_ids(&[id(1).hex(), id(3).hex()]);
    run(&mut h, "helios.vault.focus", json!(null));
    h.settle();
    let before = steps(&h).len();
    h.key_press(Key::B);
    settle(&mut h, &mut persist, 2.0);
    assert_eq!(h.state().library().quick_count(), 2);
    h.get_by_label(&quick_row(2));
    let s = steps(&h);
    assert_eq!(s.len(), before + 1);
    assert_eq!(s.last().unwrap(), "Add 2 photos to the quick collection");
    let badges = |h: &Harness<'_, Shell>, n: u8| {
        h.state()
            .library()
            .badges_of(&h.ctx, &id(n).hex())
            .contains(&"quick".to_string())
    };
    assert!(badges(&h, 1) && badges(&h, 3) && !badges(&h, 2));

    // B again: out, one more step; undone, back in.
    h.key_press(Key::B);
    settle(&mut h, &mut persist, 5.0);
    assert_eq!(h.state().library().quick_count(), 0);
    assert!(!badges(&h, 1));
    let s = steps(&h);
    assert_eq!(s.len(), before + 2);
    assert_eq!(
        s.last().unwrap(),
        "Remove 2 photos from the quick collection"
    );
    let undone = history.undo(
        &egui_pop::api::history::Request::new(helios_gui::library_history::LIBRARY),
        &egui_pop::model::commands::Origin::User,
    );
    assert!(
        matches!(undone, egui_pop::api::history::Answer::Done(Ok(_))),
        "{undone:?}"
    );
    settle(&mut h, &mut persist, 8.0);
    assert_eq!(h.state().library().quick_count(), 2);
    assert!(badges(&h, 3));

    // B typed in a text field does not toggle.
    h.get_by_label("Search images").click();
    h.settle();
    h.key_press(Key::B);
    settle(&mut h, &mut persist, 10.0);
    assert_eq!(h.state().library().quick_count(), 2);
    assert_eq!(steps(&h).len(), before + 1);

    // A click on its row scopes the grid: `collection:quick`.
    h.get_by_label(&quick_row(2)).click();
    settle(&mut h, &mut persist, 12.0);
    assert_eq!(h.state().library().collection().as_deref(), Some("quick"));
    assert_eq!(grid(&h), ["a.jpg", "c.nef"]);
    filter(&mut h, &mut persist, "collection:quick camera:R5");
    assert_eq!(grid(&h), ["c.nef"]);
    filter(&mut h, &mut persist, "collection:quick");
    assert_eq!(grid(&h), ["a.jpg", "c.nef"]);

    // Saved as an album, kept: its photos stay in the quick collection.
    run(&mut h, "library.quick.saveAs", json!({ "name": "Picks" }));
    settle(&mut h, &mut persist, 16.0);
    assert_eq!(albums(&h), [("Picks".to_string(), 2)]);
    assert_eq!(h.state().library().quick_count(), 2);
    assert_eq!(
        steps(&h).last().unwrap(),
        "Save the quick collection as “Picks”"
    );
    // Through its form, emptied: the grid scoped to it shows none.
    run(&mut h, "library.quick.saveAs", json!({}));
    settle(&mut h, &mut persist, 18.0);
    run(
        &mut h,
        "dialog.answer",
        json!({ "values": { "name": "Best", "clear": true } }),
    );
    settle(&mut h, &mut persist, 20.0);
    assert_eq!(
        albums(&h),
        [("Best".to_string(), 2), ("Picks".to_string(), 2)]
    );
    assert_eq!(h.state().library().quick_count(), 0);
    assert!(grid(&h).is_empty());
    // Empty: nothing to save.
    run(&mut h, "library.quick.saveAs", json!({ "name": "Again" }));
    settle(&mut h, &mut persist, 21.0);
    let said = warnings(&h);
    assert!(said.contains("the quick collection is empty"), "{said}");
    assert_eq!(albums(&h).len(), 2);

    // An agent fills it by ids, then clears it, one step each.
    run(
        &mut h,
        "library.quick.toggle",
        json!({ "images": [id(2).hex()] }),
    );
    settle(&mut h, &mut persist, 22.0);
    assert_eq!(grid(&h), ["b.jpg"]);
    assert_eq!(h.state().library().quick_count(), 1);
    run(&mut h, "library.quick.clear", json!({}));
    settle(&mut h, &mut persist, 24.0);
    assert_eq!(h.state().library().quick_count(), 0);
    assert_eq!(steps(&h).last().unwrap(), "Clear the quick collection");
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// Named quick collections with a target: an agent makes,
/// renames, targets and deletes them through `library.quick.*`; B puts
/// the selection in the target; photos dropped on a named one's row go
/// in; each change is one step of the library's history, undone.
#[test]
fn named_quick_collections_take_b_and_drops_through_the_target_each_step_undone() {
    use helios_gui::library::DropOn;
    let root = temp("quicks");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
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
    let undo = |history: &helios_gui::library_history::LibraryHistory| {
        let undone = history.undo(
            &egui_pop::api::history::Request::new(helios_gui::library_history::LIBRARY),
            &egui_pop::model::commands::Origin::User,
        );
        assert!(
            matches!(undone, egui_pop::api::history::Answer::Done(Ok(_))),
            "{undone:?}"
        );
    };
    // An agent's command: queued for the shell's frame, its answer read
    // from the history.
    let agent = |h: &mut Harness<'static, Shell>, id: &str, args: Value| {
        let out = h
            .state_mut()
            .run(id, args, Origin::Agent("mcp".into()))
            .unwrap();
        if out != json!({ "queued": true }) {
            return out;
        }
        h.settle();
        h.state()
            .history()
            .into_iter()
            .rev()
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("{id} never ran"))
            .result
            .unwrap()
    };
    h.get_by_label("Albums").click();
    settle(&mut h, &mut persist, 1.0);
    h.get_by_label(&quick_row(0));

    // An agent makes two, one the target at once.
    let made = agent(
        &mut h,
        "library.quick.create",
        json!({ "name": "Picks", "images": [id(1).hex()] }),
    );
    assert_eq!(made["id"], "picks", "{made}");
    agent(
        &mut h,
        "library.quick.create",
        json!({ "name": "Maybe", "target": true }),
    );
    settle(&mut h, &mut persist, 2.0);
    let listed = agent(&mut h, "library.quick.list", json!({}));
    let rows: Vec<(String, u64, bool)> = listed["quick"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| {
            (
                q["id"].as_str().unwrap().to_string(),
                q["count"].as_u64().unwrap(),
                q["target"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("quick".to_string(), 0, false),
            ("maybe".to_string(), 0, true),
            ("picks".to_string(), 1, false),
        ],
        "{listed}"
    );
    h.get_by_label("⚡ Quick collection, 0");
    h.get_by_label("⚡ Maybe, + 0");
    h.get_by_label("⚡ Picks, 1");
    assert!(answer_err(
        &mut h,
        "library.quick.create",
        json!({ "name": "picks" })
    ));

    // B: the selection goes in the target, not the first one; one step.
    h.state_mut()
        .library_mut()
        .select_ids(&[id(2).hex(), id(3).hex()]);
    run(&mut h, "helios.vault.focus", json!(null));
    h.settle();
    h.key_press(Key::B);
    settle(&mut h, &mut persist, 3.0);
    h.get_by_label("⚡ Maybe, + 2");
    h.get_by_label("⚡ Quick collection, 0");
    assert_eq!(
        steps(&h).last().unwrap(),
        "Add 2 photos to the quick collection “Maybe”"
    );

    // The selection dropped on a named quick collection (not the target)
    // goes in.
    let (from, to) = (
        cell_at(&h, "c.nef"),
        h.get_by_label("⚡ Picks, 1").rect().center(),
    );
    drag_onto(&mut h, from, to, |h| {
        assert_eq!(
            h.state().library().drop_hover(),
            Some((DropOn::Quick("picks".into()), true))
        );
    });
    settle(&mut h, &mut persist, 4.0);
    h.get_by_label("⚡ Picks, 3");
    assert_eq!(
        steps(&h).last().unwrap(),
        "Add 2 photos to the quick collection “Picks”"
    );
    undo(&history);
    settle(&mut h, &mut persist, 5.0);
    h.get_by_label("⚡ Picks, 1");

    // Renamed, targeted back to the first, removed from: one step each.
    agent(
        &mut h,
        "library.quick.rename",
        json!({ "id": "picks", "name": "Keepers" }),
    );
    settle(&mut h, &mut persist, 6.0);
    h.get_by_label("⚡ Keepers, 1");
    agent(&mut h, "library.quick.target", json!({ "id": "quick" }));
    settle(&mut h, &mut persist, 7.0);
    h.get_by_label(&quick_row(0));
    h.get_by_label("⚡ Maybe, 2");
    run(
        &mut h,
        "library.quick.remove",
        json!({ "id": "maybe", "images": [id(2).hex()] }),
    );
    settle(&mut h, &mut persist, 8.0);
    h.get_by_label("⚡ Maybe, 1");
    undo(&history);
    settle(&mut h, &mut persist, 9.0);
    h.get_by_label("⚡ Maybe, 2");

    // The target deleted: the first one is the target again; undone, it
    // comes back as the target with its photos.
    run(&mut h, "library.quick.target", json!({ "id": "maybe" }));
    settle(&mut h, &mut persist, 10.0);
    assert!(answer_err(
        &mut h,
        "library.quick.delete",
        json!({ "id": "quick" })
    ));
    run(&mut h, "library.quick.delete", json!({ "id": "maybe" }));
    settle(&mut h, &mut persist, 11.0);
    assert!(h.query_by_label("⚡ Maybe, + 2").is_none());
    h.get_by_label(&quick_row(0));
    assert_eq!(
        steps(&h).last().unwrap(),
        "Delete the quick collection “Maybe”"
    );
    undo(&history);
    settle(&mut h, &mut persist, 12.0);
    h.get_by_label("⚡ Maybe, + 2");
    assert_eq!(h.state().library().quick_target(), "maybe");

    // A click on a named one scopes the grid to it.
    h.get_by_label("⚡ Maybe, + 2").click();
    settle(&mut h, &mut persist, 13.0);
    assert_eq!(h.state().library().collection().as_deref(), Some("maybe"));
    assert_eq!(grid(&h), ["b.jpg", "c.nef"]);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// A command's answer, run as the user (the shell's queued for its
/// frame, then read from its history).
fn answer(h: &mut Harness<'static, Shell>, id: &str, args: Value) -> Value {
    let out = h.state_mut().run(id, args, Origin::User).unwrap();
    if out != json!({ "queued": true }) {
        return out;
    }
    h.settle();
    h.state()
        .history()
        .into_iter()
        .rev()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("{id} never ran"))
        .result
        .unwrap()
}

/// The queries of a list `library.filter.recent` answers.
fn queries(list: &Value, kind: &str) -> Vec<String> {
    list[kind]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["query"].as_str().unwrap().to_string())
        .collect()
}

/// The dialog open in the main window, if any.
fn main_dialog(h: &Harness<'_, Shell>) -> Option<String> {
    h.state()
        .app()
        .handle()
        .service(&egui_pop::api::dialogs::DIALOGS)
        .unwrap()
        .current_in(egui::ViewportId::ROOT)
        .map(|(_, d)| d.title().to_string())
}

#[test]
fn filters_used_are_recent_pinned_kept_and_applied_from_ctrl_shift_k() {
    let root = temp("recent");
    let path = vault_file(&root);
    let (config, ws) = workspace(&root);
    let (mut h, mut persist) = open(&config, &ws, &path);
    settle(&mut h, &mut persist, 1.0);

    // Applied by an agent: each first of the recent ones, once.
    for (at, q) in [(2.0, "camera:R5"), (3.0, "rating>=1"), (4.0, "camera:R5")] {
        run(&mut h, "library.filter.apply", json!({ "query": q }));
        settle(&mut h, &mut persist, at);
    }
    assert_eq!(grid(&h), ["c.nef"]);
    // The search field set by code is not a filter used.
    filter(&mut h, &mut persist, "label:red");
    let list = answer(&mut h, "library.filter.recent", json!({}));
    assert_eq!(queries(&list, "recent"), ["camera:R5", "rating>=1"]);
    assert_eq!(list["in_force"], json!("label:red"));

    // Pinned: out of the recent ones, listed first.
    run(
        &mut h,
        "library.filter.pin",
        json!({ "query": "rating>=1" }),
    );
    settle(&mut h, &mut persist, 5.0);
    let list = answer(&mut h, "library.filter.recent", json!({}));
    assert_eq!(queries(&list, "pinned"), ["rating>=1"]);
    assert_eq!(queries(&list, "recent"), ["camera:R5"]);

    // Ctrl+Shift+K in the grid: the switcher, in this window; the
    // pinned filter chosen by its words applies to the grid.
    run(&mut h, "helios.vault.focus", json!(null));
    h.settle();
    h.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::K);
    h.settle();
    h.settle();
    assert_eq!(main_dialog(&h).as_deref(), Some("Recent filters"));
    let current = answer(&mut h, "dialog.current", json!(null));
    let shown = current.to_string();
    assert!(
        shown.contains("rating>=1") && shown.contains("camera:R5") && shown.contains("pinned"),
        "{shown}"
    );
    h.input_mut()
        .events
        .push(egui::Event::Text("rating".into()));
    h.settle();
    h.key_press(Key::Enter);
    settle(&mut h, &mut persist, 6.0);
    assert_eq!(main_dialog(&h), None);
    assert_eq!(grid(&h), ["a.jpg", "b.jpg"]);

    // Typed in a text field, Ctrl+Shift+K does not open it.
    h.get_by_label("Search images").click();
    h.settle();
    h.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::K);
    h.settle();
    h.settle();
    assert_eq!(main_dialog(&h), None);

    // Unpinned and pinned again; kept across a restart, in the vault.
    run(
        &mut h,
        "library.filter.pin",
        json!({ "query": "camera:R5", "pinned": true }),
    );
    settle(&mut h, &mut persist, 7.0);
    drop((h, persist));
    let (mut h, mut persist) = open(&config, &ws, &path);
    settle(&mut h, &mut persist, 1.0);
    let list = answer(&mut h, "library.filter.recent", json!({}));
    assert_eq!(queries(&list, "pinned"), ["rating>=1", "camera:R5"]);
    assert!(queries(&list, "recent").is_empty(), "{list}");
    run(
        &mut h,
        "library.filter.pin",
        json!({ "query": "rating>=1", "pinned": false }),
    );
    settle(&mut h, &mut persist, 2.0);
    let list = answer(&mut h, "library.filter.recent", json!({}));
    assert_eq!(queries(&list, "recent"), ["rating>=1"]);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// Album sets (`docs/design/library.md`, L6): nested sets show as groups
/// that open and close, each with the count of its albums' photos
/// together; a click scopes the grid to that union; moves in and out
/// undo; a set never goes inside itself (refused with the reason);
/// deleting a set keeps its albums, moved up.
#[test]
fn album_sets_nest_count_the_union_and_move_with_undo() {
    let root = temp("sets");
    let path = vault_file(&root);
    let (config, ws) = workspace(&root);
    let (mut h, mut persist) = open(&config, &ws, &path);
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
    let undo = |h: &mut Harness<'static, Shell>, persist: &mut Persist<SqliteStore>, at: f64| {
        let undone = history.undo(
            &egui_pop::api::history::Request::new(helios_gui::library_history::LIBRARY),
            &egui_pop::model::commands::Origin::User,
        );
        assert!(
            matches!(undone, egui_pop::api::history::Answer::Done(Ok(_))),
            "{undone:?}"
        );
        settle(h, persist, at);
    };
    h.get_by_label("Albums").click();
    settle(&mut h, &mut persist, 1.0);
    let ids = |n: &[u8]| n.iter().map(|&n| id(n).hex()).collect::<Vec<_>>();
    run(
        &mut h,
        "library.collection.create",
        json!({ "name": "Trip", "images": ids(&[1, 2]) }),
    );
    run(
        &mut h,
        "library.collection.create",
        json!({ "name": "Home", "images": ids(&[2, 3]) }),
    );
    run(
        &mut h,
        "library.collection.create",
        json!({ "kind": "set", "name": "Travel" }),
    );
    settle(&mut h, &mut persist, 2.0);
    run(
        &mut h,
        "library.collection.create",
        json!({ "kind": "set", "name": "Inner", "set": "travel" }),
    );
    settle(&mut h, &mut persist, 3.0);
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "New album set “Inner”"
    );
    // Moved into the nested set: both sets count the album's photos.
    run(
        &mut h,
        "library.collection.move",
        json!({ "id": "trip", "set": "inner" }),
    );
    settle(&mut h, &mut persist, 4.0);
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Move “Trip” into a set"
    );
    h.get_by_label("Travel, 2");
    h.get_by_label("Inner, 2");
    h.get_by_label("Trip, 2");
    h.get_by_label("Show or hide Travel");
    // A second album in the outer set: its count is the union.
    run(
        &mut h,
        "library.collection.move",
        json!({ "id": "home", "set": "travel" }),
    );
    settle(&mut h, &mut persist, 5.0);
    h.get_by_label("Travel, 3");
    h.get_by_label("Travel, 3").click();
    settle(&mut h, &mut persist, 6.0);
    assert_eq!(h.state().library().collection().as_deref(), Some("travel"));
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);
    // Closed: its members hidden.
    h.get_by_label("Show or hide Travel").click();
    settle(&mut h, &mut persist, 7.0);
    assert!(h.query_by_label("Inner, 2").is_none());
    h.get_by_label("Show or hide Travel").click();
    settle(&mut h, &mut persist, 8.0);

    // A set never inside itself, nor inside a set below it.
    run(
        &mut h,
        "library.collection.move",
        json!({ "id": "travel", "set": "inner" }),
    );
    run(
        &mut h,
        "library.collection.move",
        json!({ "id": "inner", "set": "inner" }),
    );
    settle(&mut h, &mut persist, 9.0);
    let said = warnings(&h);
    assert!(
        said.contains("`travel` is inside") || said.contains("cannot hold itself"),
        "{said}"
    );
    assert!(
        said.contains("a set cannot hold itself (`inner`)"),
        "{said}"
    );

    // Undone: Home out of the set again.
    undo(&mut h, &mut persist, 10.0);
    h.get_by_label("Travel, 2");
    h.get_by_label("Home, 2");

    // Moved out, then the set deleted: its albums move up.
    run(
        &mut h,
        "library.collection.move",
        json!({ "id": "trip", "set": null }),
    );
    settle(&mut h, &mut persist, 11.0);
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Move “Trip” out of its set"
    );
    h.get_by_label("Travel, 0");
    undo(&mut h, &mut persist, 12.0);
    h.get_by_label("Travel, 2");
    run(
        &mut h,
        "library.collection.delete",
        json!({ "id": "inner" }),
    );
    settle(&mut h, &mut persist, 13.0);
    let kinds: Vec<(String, Option<String>)> = h
        .state()
        .library()
        .albums()
        .unwrap()
        .iter()
        .map(|a| (a.id.clone(), a.set.clone()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("home".to_string(), None),
            ("travel".to_string(), None),
            ("trip".to_string(), Some("travel".to_string())),
        ]
    );
    h.get_by_label("Travel, 2");
    // Undone: the nested set is back, its album in it.
    undo(&mut h, &mut persist, 14.0);
    h.get_by_label("Inner, 2");
    assert_eq!(
        albums(&h),
        [
            ("Home".to_string(), 2),
            ("Inner".to_string(), 2),
            ("Travel".to_string(), 2),
            ("Trip".to_string(), 2),
        ]
    );
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// A grid cell by its photo's file name.
fn cell_at(h: &Harness<'_, Shell>, name: &str) -> egui::Pos2 {
    h.get_by(|n| {
        n.role() == egui::accesskit::Role::Button && n.label().is_some_and(|l| l.contains(name))
    })
    .rect()
    .center()
}

/// Drags from `from` to `to` in a few moves, as a hand does; `over`
/// runs while the pointer is over `to`, before the drop.
fn drag_onto(
    h: &mut Harness<'static, Shell>,
    from: egui::Pos2,
    to: egui::Pos2,
    over: impl FnOnce(&Harness<'static, Shell>),
) {
    h.drag_at(from);
    h.settle();
    for i in 1..=6 {
        h.hover_at(from + (to - from) * (i as f32 / 6.0));
        h.settle();
    }
    h.settle();
    over(h);
    h.drop_at(to);
    h.settle();
}

#[test]
fn photos_dragged_onto_an_album_or_the_quick_collection_go_in_a_smart_album_refuses() {
    use helios_gui::library::DropOn;
    let root = temp("drop");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
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
    run(
        &mut h,
        "library.collection.create",
        json!({ "name": "Trip", "images": [id(3).hex()] }),
    );
    run(
        &mut h,
        "library.collection.create",
        json!({ "name": "Best", "kind": "smart", "rules": {"match": "all", "rules": [
            {"field": "rating", "op": ">=", "value": 3},
        ]} }),
    );
    run(
        &mut h,
        "library.collection.create",
        json!({ "kind": "set", "name": "Travel" }),
    );
    settle(&mut h, &mut persist, 2.0);
    let row = |h: &Harness<'_, Shell>, label: &str| h.get_by_label(label).rect().center();

    // The two selected photos dragged onto an album: highlighted while
    // over it, both in once dropped, one step, undone as one.
    h.state_mut()
        .library_mut()
        .select_ids(&[id(1).hex(), id(2).hex()]);
    settle(&mut h, &mut persist, 3.0);
    let before = steps(&h).len();
    let (from, to) = (cell_at(&h, "a.jpg"), row(&h, "Trip, 1"));
    drag_onto(&mut h, from, to, |h| {
        assert_eq!(
            h.state().library().drop_hover(),
            Some((DropOn::Album("trip".into()), true))
        );
        h.get_by_label("Add 2 photos");
    });
    settle(&mut h, &mut persist, 4.0);
    assert_eq!(h.state().library().drop_hover(), None);
    h.get_by_label("Trip, 3");
    let s = steps(&h);
    assert_eq!(s.len(), before + 1, "{s:?}");
    assert_eq!(s.last().unwrap(), "Add 2 photos to “Trip”");
    assert_eq!(
        h.state().library().selection(),
        [id(1).hex(), id(2).hex()],
        "the selection stays"
    );
    let undone = history.undo(
        &egui_pop::api::history::Request::new(helios_gui::library_history::LIBRARY),
        &egui_pop::model::commands::Origin::User,
    );
    assert!(
        matches!(undone, egui_pop::api::history::Answer::Done(Ok(_))),
        "{undone:?}"
    );
    settle(&mut h, &mut persist, 5.0);
    h.get_by_label("Trip, 1");

    // Onto a smart album: refused while over it, with the reason, and
    // nothing changes once dropped.
    let before = steps(&h).len();
    let reason = "Not here: a smart album's rules choose its photos";
    let (from, to) = (cell_at(&h, "a.jpg"), row(&h, "⚙ Best, 1"));
    drag_onto(&mut h, from, to, |h| {
        assert_eq!(
            h.state().library().drop_hover(),
            Some((DropOn::Album("best".into()), false))
        );
        h.get_by_label(reason);
    });
    settle(&mut h, &mut persist, 6.0);
    assert_eq!(steps(&h).len(), before);
    assert!(warnings(&h).contains(reason), "{}", warnings(&h));
    h.get_by_label("⚙ Best, 1");

    // Onto a set: refused the same way.
    let (from, to) = (cell_at(&h, "a.jpg"), row(&h, "Travel, 0"));
    drag_onto(&mut h, from, to, |h| {
        assert_eq!(
            h.state().library().drop_hover(),
            Some((DropOn::Album("travel".into()), false))
        );
    });
    settle(&mut h, &mut persist, 7.0);
    assert_eq!(steps(&h).len(), before);

    // A photo not selected dragged alone onto the quick collection: it
    // goes in, the selection stays.
    let (from, to) = (cell_at(&h, "c.nef"), row(&h, &quick_row(0)));
    drag_onto(&mut h, from, to, |h| {
        assert_eq!(
            h.state().library().drop_hover(),
            Some((DropOn::Quick("quick".into()), true))
        );
    });
    settle(&mut h, &mut persist, 8.0);
    assert_eq!(h.state().library().quick(), [id(3).hex()]);
    assert_eq!(
        steps(&h).last().unwrap(),
        "Add 1 photos to the quick collection"
    );
    assert_eq!(h.state().library().selection(), [id(1).hex(), id(2).hex()]);
    // The selection onto it: added, never taken out (B's toggle is not a
    // drop's).
    let before = steps(&h).len();
    let (from, to) = (cell_at(&h, "b.jpg"), row(&h, &quick_row(1)));
    drag_onto(&mut h, from, to, |_| {});
    settle(&mut h, &mut persist, 9.0);
    assert_eq!(h.state().library().quick_count(), 3);
    let (from, to) = (cell_at(&h, "b.jpg"), row(&h, &quick_row(3)));
    drag_onto(&mut h, from, to, |_| {});
    settle(&mut h, &mut persist, 10.0);
    assert_eq!(h.state().library().quick_count(), 3);
    assert_eq!(steps(&h).len(), before + 1);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The search filter's photos and facet counts are read through the
/// desktop's node off the frame: the frame that takes the query does not
/// wait for them, a coming frame shows what the node answered.
#[test]
fn the_filter_is_read_through_the_node_and_shown_at_a_coming_frame() {
    let root = temp("off-frame");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
    h.state_mut()
        .library_mut()
        .set_filter(Filter::parse("camera:R5"));
    for i in 0..3 {
        h.settle();
        persist.frame(h.state_mut(), f64::from(i));
        if persist.busy() {
            break;
        }
    }
    assert!(persist.busy(), "the query's read waits for a coming frame");
    assert!(h.state().library().facets().is_none(), "not shown yet");
    persist.settle(h.state_mut());
    h.settle();
    assert_eq!(grid(&h), ["c.nef"]);
    let facets = h.state().library().facets().cloned().unwrap();
    assert_eq!(facets.count("camera", "R5"), 1);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The grid's photos are read through the node's cursor, each window kept
/// in the read cache until an event names one of its photos.
#[test]
fn the_grid_is_read_through_the_nodes_cursor_and_its_cache() {
    let root = temp("cursor-cache");
    let path = vault_file(&root);
    let (h, persist) = window(&root, &path);
    let reader = persist.reader();
    let read = reader.cache().stats();
    assert!(read.misses >= 1 && read.entries >= 1, "{read:?}");
    // A rating set by another writer of this process: its event drops
    // the window naming the photo.
    persist
        .vault()
        .apply(
            &[helios_vault::Command::set(id(2), RATING, json!(5))],
            "test",
            None,
            1,
        )
        .unwrap();
    let seq = persist.bus().last_seq();
    assert!(
        reader
            .cache()
            .wait_seen(seq, std::time::Duration::from_secs(10)),
        "the cache hears the vault's events"
    );
    assert!(reader.cache().stats().entries < read.entries);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The library read again for the host (an import, a disk plugged in) is
/// read off the frame, and its files handed over once shown.
#[test]
fn the_library_read_again_is_read_off_the_frame() {
    let root = temp("refill");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
    persist.ask_fill(h.state(), true);
    assert!(persist.busy(), "read on a thread of its own");
    assert!(persist.take_filled().is_none(), "not shown yet");
    persist.settle(h.state_mut());
    h.settle();
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);
    assert_eq!(persist.take_filled().map(|f| f.len()), Some(3));
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The Library's filter history and the Folders section's counts go
/// through the desktop's node: while another writer holds the vault
/// file locked, the frames that write the history and ask the counts do
/// not wait for it; once it lets go, the write is kept and the counts
/// shown.
#[test]
fn a_vault_held_locked_does_not_delay_the_filter_history_the_folders_or_the_albums() {
    use helios_vault::Store as _;
    let root = temp("held");
    let path = vault_file(&root);
    let (config, ws) = workspace(&root);
    let (mut h, mut persist) = open(&config, &ws, &path);
    settle(&mut h, &mut persist, 1.0);
    assert!(
        h.state().library().filter_history().is_some(),
        "read through the node"
    );

    // Another writer holds the vault's file.
    let mut other = SqliteStore::open(&path).unwrap();
    other.begin().unwrap();
    other
        .set_document("library/elsewhere", "held", Some(&json!(true)))
        .unwrap();
    run(
        &mut h,
        "library.filter.pin",
        json!({ "query": "camera:R5", "pinned": true }),
    );
    h.get_by_label("Folders").click();
    h.state_mut()
        .library_mut()
        .edit_album(helios_gui::library::AlbumEdit::QuickToggle {
            id: "quick".into(),
            images: vec![id(1).hex()],
        });
    let mut slowest = std::time::Duration::ZERO;
    for i in 0..3 {
        let started = std::time::Instant::now();
        h.settle();
        persist.frame(h.state_mut(), 2.0 + f64::from(i));
        slowest = slowest.max(started.elapsed());
    }
    assert!(
        persist.busy(),
        "the write waits for the vault, not the frame"
    );
    assert!(
        slowest < std::time::Duration::from_secs(1),
        "a frame waited {slowest:?} for the vault"
    );

    // Let go: the history is kept, the folders counted.
    other.rollback().unwrap();
    drop(other);
    settle(&mut h, &mut persist, 10.0);
    assert!(h.state().library().folder_tree().is_some());
    let kept = persist
        .vault()
        .library_document(
            helios_gui::recent_filters::NS,
            helios_gui::recent_filters::KEY,
        )
        .unwrap()
        .unwrap();
    assert!(kept.to_string().contains("camera:R5"), "{kept}");
    // The quick collection changed once, one step of the history.
    assert_eq!(persist.vault().quick_collection().unwrap(), [id(1).hex()]);
    assert_eq!(h.state().library().quick().len(), 1);
    assert_eq!(
        h.state()
            .library_history()
            .line()
            .undo
            .last()
            .unwrap()
            .label
            .text,
        "Add 1 photos to the quick collection"
    );
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// Some stored fields of photo `n`, as the vault holds them.
fn stored(persist: &Persist<SqliteStore>, n: u8, names: &[&str]) -> Vec<Value> {
    let row = persist
        .vault()
        .fields_of(&[id(n)], names)
        .unwrap()
        .remove(0);
    names
        .iter()
        .map(|f| row.get(*f).cloned().unwrap_or(Value::Null))
        .collect()
}

#[test]
fn selection_commands_and_marks_pasted_merged_or_overwriting() {
    use helios_vault::fields::{TAGS, TITLE};
    let root = temp("batch");
    let path = vault_file(&root);
    {
        // The first photo titled and tagged, the third taken a day later
        // and tagged.
        let mut store = SqliteStore::open(&path).unwrap();
        store
            .set_field(&id(1), TITLE, Some(&json!("Dawn")))
            .unwrap();
        store
            .set_field(&id(1), TAGS, Some(&json!(["trip"])))
            .unwrap();
        store
            .set_field(&id(3), CAPTURED, Some(&json!(86_400 * 2)))
            .unwrap();
        store
            .set_field(&id(3), TAGS, Some(&json!(["old"])))
            .unwrap();
    }
    let (mut h, mut persist) = window(&root, &path);
    let selection = |h: &Harness<'_, Shell>| h.state().library().selection();

    // In the grid: Ctrl+I inverts, Ctrl+Shift+A selects none.
    h.state_mut().library_mut().select_ids(&[id(1).hex()]);
    run(&mut h, "helios.vault.focus", json!(null));
    h.settle();
    h.key_press_modifiers(egui::Modifiers::CTRL, Key::I);
    // The key queues the command; the next frame runs it.
    h.settle();
    h.settle();
    assert_eq!(selection(&h), [id(2).hex(), id(3).hex()]);
    h.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, Key::A);
    h.settle();
    h.settle();
    assert!(selection(&h).is_empty());

    // The same day as the first photo: the second, not the third.
    h.state_mut().library_mut().select_ids(&[id(1).hex()]);
    let out = answer(&mut h, "library.select.similar", json!({ "by": "day" }));
    assert_eq!(out["selected"], 2, "{out}");
    assert_eq!(selection(&h), [id(1).hex(), id(2).hex()]);

    // The first photo's marks merged on the third: its tag stays.
    h.state_mut().library_mut().select_ids(&[id(1).hex()]);
    let copied = answer(&mut h, "library.marks.copy", json!({}));
    assert_eq!(copied["title"], "Dawn", "{copied}");
    let out = answer(
        &mut h,
        "library.marks.paste",
        json!({ "images": [id(3).hex()] }),
    );
    assert_eq!(out["images"], 1, "{out}");
    settle(&mut h, &mut persist, 2.0);
    let names = [RATING, LABELS, TAGS, TITLE];
    assert_eq!(
        stored(&persist, 3, &names),
        [
            json!(3),
            json!(["red"]),
            json!(["old", "trip"]),
            json!("Dawn")
        ]
    );

    // The second photo's (one star, nothing else) overwriting the third's
    // labels, tags and title on the selection; its stars stay.
    answer(
        &mut h,
        "library.marks.copy",
        json!({ "image": id(2).hex() }),
    );
    h.state_mut().library_mut().select_ids(&[id(3).hex()]);
    answer(
        &mut h,
        "library.marks.paste",
        json!({ "mode": "overwrite", "marks": ["labels", "tags", "title"] }),
    );
    settle(&mut h, &mut persist, 4.0);
    let after = stored(&persist, 3, &names);
    assert_eq!(after[0], json!(3));
    for v in &after[1..] {
        assert!(
            v.is_null() || v.as_array().is_some_and(Vec::is_empty),
            "{after:?}"
        );
    }
    assert_eq!(h.state().library().entry(&id(3).hex()).unwrap().title, None);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn delete_removes_the_selection_from_the_library_and_undo_puts_it_back() {
    use egui_pop::api::history::{Answer, Request};
    let root = temp("remove");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
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
    // The desktop's loop: the vault's event reads the library again.
    let refill = |h: &mut Harness<'static, Shell>, persist: &mut Persist<SqliteStore>, at| {
        settle(h, persist, at);
        // The vault\'s event heard: `library.changed` reads the Library again.
        settle(h, persist, at + 10.0);
    };
    let in_library =
        |persist: &Persist<SqliteStore>, n| persist.vault().in_library(&id(n)).unwrap();

    // Delete in the grid, on the first and third photos.
    h.state_mut()
        .library_mut()
        .select_ids(&[id(1).hex(), id(3).hex()]);
    run(&mut h, "helios.vault.focus", json!(null));
    h.settle();
    h.key_press(Key::Delete);
    h.settle();
    h.settle();
    refill(&mut h, &mut persist, 1.0);
    assert!(!in_library(&persist, 1) && !in_library(&persist, 3));
    assert!(in_library(&persist, 2));
    assert_eq!(grid(&h), ["b.jpg"]);
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Remove 2 photos from the Library"
    );
    // Its marks and its file stay: only the flag moved.
    assert_eq!(persist.vault().locations(&id(1)).unwrap().len(), 1);

    // Undone: both back in the grid.
    let undone = history.undo(
        &Request::new(helios_gui::library_history::LIBRARY),
        &egui_pop::model::commands::Origin::User,
    );
    assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
    refill(&mut h, &mut persist, 30.0);
    assert!(in_library(&persist, 1) && in_library(&persist, 3));
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);

    // By an agent, naming a photo.
    let out = answer(&mut h, "library.remove", json!({ "images": [id(2).hex()] }));
    assert_eq!(out["removed"], 1, "{out}");
    refill(&mut h, &mut persist, 60.0);
    assert_eq!(grid(&h), ["a.jpg", "c.nef"]);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn move_to_trash_asks_then_trashes_through_a_journaled_job_a_backup_refused() {
    use egui::Modifiers;
    use helios_library::trash::{self, Outcome};
    let root = temp("trash");
    let backup = root.join("backup");
    // The first two at the top, the third on a backup volume.
    let path = vault_with(&root, |n, name| {
        if n == 3 {
            Location {
                volume: Some(VolumeId("uuid:backup".into())),
                ..Location::seen(backup.join(name), 10, 1, 1)
            }
        } else {
            Location::seen(root.join(name), 10, 1, 1)
        }
    });
    std::fs::create_dir_all(&backup).unwrap();
    for file in [root.join("a.jpg"), root.join("b.jpg"), backup.join("c.nef")] {
        std::fs::write(file, b"photo").unwrap();
    }
    // Never the desktop's trash: a folder of the test.
    let bin = root.join("bin");
    trash::redirect(Some(bin.clone()));
    let (mut h, mut persist) = window(&root, &path);
    helios_storage::registry::remember(
        persist.vault(),
        &Volume {
            id: VolumeId("uuid:backup".into()),
            identified_by: Identity::Uuid,
            kind: VolumeKind::Removable,
            roles: [helios_storage::Role::Backup].into(),
            ..Volume::remote("x", "y", "BACKUP_A")
        },
        1,
    )
    .unwrap();
    let refill = |h: &mut Harness<'static, Shell>, persist: &mut Persist<SqliteStore>, at| {
        settle(h, persist, at);
        persist.ask_fill(h.state(), true);
        settle(h, persist, at + 10.0);
    };

    // Ctrl+Alt+Shift+Delete on the three: the question first, naming the
    // count, the place and the refusal; nothing moved yet.
    h.state_mut()
        .library_mut()
        .select_ids(&[id(1).hex(), id(2).hex(), id(3).hex()]);
    run(&mut h, "helios.vault.focus", json!(null));
    h.settle();
    h.key_press_modifiers(
        Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT,
        Key::Delete,
    );
    settle(&mut h, &mut persist, 1.0);
    assert_eq!(
        main_dialog(&h).as_deref(),
        Some("Move 3 photos to the Trash?")
    );
    let detail = answer(&mut h, "dialog.current", json!(null)).to_string();
    assert!(detail.contains("Their 2 files in"), "{detail}");
    assert!(
        detail.contains("BACKUP_A is a backup volume: its copies stay"),
        "{detail}"
    );
    assert!(
        root.join("a.jpg").exists(),
        "nothing moved before the answer"
    );

    // Confirmed: the files in the test trash, the backup's copy kept, the
    // two photos out of the Library.
    run(&mut h, "dialog.answer", json!({ "answer": "primary" }));
    refill(&mut h, &mut persist, 5.0);
    assert!(!root.join("a.jpg").exists() && bin.join("a.jpg").exists());
    assert!(!root.join("b.jpg").exists() && bin.join("b.jpg").exists());
    assert!(backup.join("c.nef").exists(), "a backup copy stays");
    assert_eq!(grid(&h), ["c.nef"]);
    let vault = persist.vault();
    assert!(!vault.in_library(&id(1)).unwrap() && vault.in_library(&id(3)).unwrap());
    assert_eq!(
        vault.locations(&id(1)).unwrap()[0].presence,
        Presence::Missing
    );

    // The job's journal says each file's outcome.
    let jobs = trash::list(&**vault).unwrap();
    assert_eq!(jobs.len(), 1);
    let outcomes: Vec<_> = jobs[0]
        .files
        .iter()
        .map(|f| {
            (
                f.path.file_name().unwrap().to_string_lossy().into_owned(),
                f.outcome,
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            ("a.jpg".to_string(), Outcome::Trashed),
            ("b.jpg".to_string(), Outcome::Trashed),
            ("c.nef".to_string(), Outcome::Refused),
        ]
    );
    assert!(jobs[0].done);
    trash::redirect(None);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// A write the Library asks (a turn) is a deferred request
/// (`library.write`): asked while the Library is borrowed, heard after
/// the drawing in the same pass, its write waiting for the node by the
/// host's turn; then written.
#[test]
fn a_turn_asked_in_the_library_is_heard_in_the_pass_it_is_made() {
    use helios_vault::rotation::{ROTATION, quarter_turns};
    let root = temp("turn-pass");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
    settle(&mut h, &mut persist, 1.0);
    assert!(!persist.busy(), "nothing waits before the ask");
    h.state_mut().library_mut().select_ids(&[id(1).hex()]);
    run(&mut h, "helios.vault.focus", json!(null));
    h.settle();
    settle(&mut h, &mut persist, 1.5);
    assert!(!persist.busy(), "nothing waits before the key");
    // Ctrl+] in the grid, seen while the Library draws: one pass, the
    // ask heard after the drawing.
    h.key_press_modifiers(egui::Modifiers::CTRL, Key::CloseBracket);
    h.step();
    assert_eq!(h.state().app().handle().asks_waiting(), 0, "heard");
    assert!(persist.busy(), "its write waits for the node");
    settle(&mut h, &mut persist, 2.0);
    let rows = persist.vault().fields_of(&[id(1)], &[ROTATION]).unwrap();
    assert_eq!(quarter_turns(rows[0].get(ROTATION)), 1);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ctrl_brackets_rotate_the_selection_and_undo_turns_it_back() {
    use egui_pop::api::history::{Answer, Request};
    use helios_vault::rotation::{ROTATION, quarter_turns};
    let root = temp("rotate");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
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
    let turns = |persist: &Persist<SqliteStore>, n| {
        let rows = persist.vault().fields_of(&[id(n)], &[ROTATION]).unwrap();
        quarter_turns(rows[0].get(ROTATION))
    };
    let shown = |h: &Harness<'static, Shell>, n: u8| {
        h.state().library().entry(&id(n).hex()).unwrap().rotation
    };
    // A landscape thumbnail for the first photo.
    let thumb = |h: &Harness<'static, Shell>| {
        let sink = h.state().library_thumbnails();
        let image =
            helios_core::Image::from_fn(helios_core::Region::sized(600, 400), 4, |_, _, _| 0.2);
        sink.send((id(1).hex(), image)).unwrap();
    };
    thumb(&h);
    settle(&mut h, &mut persist, 0.5);
    let wide = h.state().library().thumb_size(&id(1).hex()).unwrap();
    assert!(wide[0] > wide[1], "{wide:?}");

    // Ctrl+] in the grid, on the first and third photos.
    h.state_mut()
        .library_mut()
        .select_ids(&[id(1).hex(), id(3).hex()]);
    run(&mut h, "helios.vault.focus", json!(null));
    h.settle();
    h.key_press_modifiers(egui::Modifiers::CTRL, Key::CloseBracket);
    settle(&mut h, &mut persist, 1.0);
    settle(&mut h, &mut persist, 5.0);
    assert_eq!(
        (turns(&persist, 1), turns(&persist, 2), turns(&persist, 3)),
        (1, 0, 1)
    );
    assert_eq!(shown(&h, 1), 1, "the vault's event shows the turn");
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Rotate 2 photos right"
    );
    // Its thumbnail is read again, and shown turned.
    assert_eq!(h.state().library().thumb_size(&id(1).hex()), None);
    thumb(&h);
    settle(&mut h, &mut persist, 8.0);
    assert_eq!(
        h.state().library().thumb_size(&id(1).hex()),
        Some([wide[1], wide[0]])
    );

    // Ctrl+[ twice: three quarters clockwise from upright.
    for at in [10.0, 20.0] {
        h.key_press_modifiers(egui::Modifiers::CTRL, Key::OpenBracket);
        settle(&mut h, &mut persist, at);
    }
    assert_eq!(turns(&persist, 1), 3);

    // Undone: one quarter back to the right.
    let undone = history.undo(
        &Request::new(helios_gui::library_history::LIBRARY),
        &egui_pop::model::commands::Origin::User,
    );
    assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
    settle(&mut h, &mut persist, 30.0);
    // The vault\'s event heard: `library.changed` reads the Library again.
    settle(&mut h, &mut persist, 40.0);
    assert_eq!(turns(&persist, 1), 0);
    assert_eq!(shown(&h, 1), 0, "the Library read again shows it");

    // By an agent, naming a photo; then reset.
    let out = answer(
        &mut h,
        "library.rotateLeft",
        json!({ "images": [id(2).hex()] }),
    );
    assert_eq!(out["rotated"], 1, "{out}");
    settle(&mut h, &mut persist, 50.0);
    assert_eq!(turns(&persist, 2), 3);
    answer(
        &mut h,
        "library.resetRotation",
        json!({ "images": [id(2).hex()] }),
    );
    settle(&mut h, &mut persist, 60.0);
    assert_eq!(turns(&persist, 2), 0);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ctrl_quote_duplicates_the_selection_and_the_grid_counts_versions() {
    let root = temp("duplicate");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
    let heads = |persist: &Persist<SqliteStore>, n| persist.vault().heads(&id(n)).unwrap().len();
    let badge = |h: &Harness<'static, Shell>, n: u8| {
        h.state()
            .library()
            .badges_of(&h.ctx, &id(n).hex())
            .into_iter()
            .find(|b| b.ends_with("versions"))
    };
    assert_eq!(badge(&h, 1), None, "one cell, no versions yet");

    // Ctrl+' in the grid, on the first photo.
    h.state_mut().library_mut().select_ids(&[id(1).hex()]);
    run(&mut h, "helios.vault.focus", json!(null));
    h.settle();
    h.key_press_modifiers(egui::Modifiers::CTRL, Key::Quote);
    settle(&mut h, &mut persist, 1.0);
    settle(&mut h, &mut persist, 5.0);
    assert_eq!(heads(&persist, 1), 1, "a photo never edited: its copy");
    h.key_press_modifiers(egui::Modifiers::CTRL, Key::Quote);
    settle(&mut h, &mut persist, 10.0);
    assert_eq!((heads(&persist, 1), heads(&persist, 2)), (2, 0));
    assert_eq!(badge(&h, 1).as_deref(), Some("2 versions"));

    // By an agent, from the original, naming a photo.
    let out = answer(
        &mut h,
        "library.duplicateOriginal",
        json!({ "images": [id(1).hex()] }),
    );
    assert_eq!(out["duplicated"], 1, "{out}");
    settle(&mut h, &mut persist, 20.0);
    assert_eq!(heads(&persist, 1), 3);
    assert_eq!(badge(&h, 1).as_deref(), Some("3 versions"));

    // The Library read again counts them the same.
    persist.ask_fill(h.state(), true);
    settle(&mut h, &mut persist, 30.0);
    assert_eq!(badge(&h, 1).as_deref(), Some("3 versions"));
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The versions strip (L10): the photo under the cursor with two
/// versions lists them, the newest current; a click on the first makes
/// it current through the node; "Remove Version…" asks, removes the copy,
/// and undo brings it back.
#[test]
fn the_versions_strip_switches_and_a_removed_copy_comes_back_with_undo() {
    use egui_pop::api::history::{Answer, Request};
    let root = temp("versions-strip");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
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
    let photo = id(1).hex();
    h.state_mut()
        .library_mut()
        .select_ids(std::slice::from_ref(&photo));
    for at in [1.0, 5.0] {
        answer(&mut h, "library.duplicate", json!({ "images": [photo] }));
        settle(&mut h, &mut persist, at);
    }
    assert_eq!(persist.vault().heads(&id(1)).unwrap().len(), 2);
    // The strip asks the photo's heads, then shows them.
    settle(&mut h, &mut persist, 8.0);
    settle(&mut h, &mut persist, 9.0);
    let listed = h.state().library().versions_of(&photo).map(<[_]>::to_vec);
    let listed = listed.expect("the strip's heads read");
    assert_eq!(listed.len(), 2);
    assert!(listed[1].current, "the newest is current: {listed:?}");
    let first = listed[0].version.clone();

    // A click on the first version makes it current.
    h.get_by_label("1 Copy").click();
    settle(&mut h, &mut persist, 12.0);
    settle(&mut h, &mut persist, 13.0);
    let current = persist.vault().current_head(&id(1)).unwrap().unwrap();
    assert_eq!(current.id.hex(), first);
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Switch version"
    );
    let listed = h.state().library().versions_of(&photo).map(<[_]>::to_vec);
    assert!(listed.is_some_and(|l| l[0].current), "the strip follows");

    // Remove Version… on it: asked first, then the copy goes.
    let out = answer(&mut h, "library.removeVersion", json!({}));
    assert_eq!(out["asked"], true, "{out}");
    assert_eq!(out["version"], json!(first));
    settle(&mut h, &mut persist, 15.0);
    assert_eq!(
        persist.vault().heads(&id(1)).unwrap().len(),
        2,
        "asked only"
    );
    let out = answer(
        &mut h,
        "library.removeVersion",
        json!({ "image": photo, "version": first, "confirmed": true }),
    );
    assert_eq!(out["asked"], false, "{out}");
    settle(&mut h, &mut persist, 20.0);
    assert_eq!(persist.vault().heads(&id(1)).unwrap().len(), 1);
    assert_eq!(h.state().library().heads_of(&photo), None, "no badge left");
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Remove version"
    );
    assert!(
        answer_err(&mut h, "library.removeVersion", json!({ "image": photo })),
        "a photo keeps one version"
    );

    // Undone: the copy is back.
    let undone = history.undo(
        &Request::new(helios_gui::library_history::LIBRARY),
        &egui_pop::model::commands::Origin::User,
    );
    assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
    settle(&mut h, &mut persist, 30.0);
    assert_eq!(persist.vault().heads(&id(1)).unwrap().len(), 2);
    // The vault\'s event heard: `library.changed` reads the Library again.
    settle(&mut h, &mut persist, 40.0);
    settle(&mut h, &mut persist, 45.0);
    assert_eq!(
        h.state().library().heads_of(&photo),
        Some(2),
        "the Library read again counts the copy"
    );
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn photos_are_grouped_topped_and_ungrouped_each_undone_and_redone() {
    use egui_pop::api::history::{Answer, Request};
    let root = temp("manual-stacks");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
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
    let (a, b) = (id(1).hex(), id(2).hex());
    let stack_of = |h: &Harness<'static, Shell>, image: &str| {
        h.state()
            .library()
            .entry(image)
            .and_then(|e| e.stack.as_ref())
            .map(|s| (s.top.clone(), s.count))
    };
    let request = Request::new(helios_gui::library_history::LIBRARY);
    let user = egui_pop::model::commands::Origin::User;

    // Group into Stack: the selection, the cursor's photo on top.
    h.state_mut()
        .library_mut()
        .select_ids(&[a.clone(), b.clone()]);
    let out = answer(&mut h, "library.stack.group", json!({}));
    assert_eq!(out["grouped"], 2, "{out}");
    for at in [1.0, 5.0, 9.0] {
        settle(&mut h, &mut persist, at);
    }
    let stack = persist.vault().stack_of(&id(1)).unwrap().expect("grouped");
    assert_eq!(stack.top, b, "the cursor's photo on top");
    assert_eq!(stack_of(&h, &a), Some((b.clone(), 2)), "the grid shows it");
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Group 2 photos into a stack"
    );

    // Move to Top of Stack.
    let out = answer(&mut h, "library.stack.top", json!({ "image": a }));
    assert_eq!(out["stack"], json!(stack.id), "{out}");
    for at in [12.0, 15.0, 18.0] {
        settle(&mut h, &mut persist, at);
    }
    assert_eq!(persist.vault().stack(&stack.id).unwrap().unwrap().top, a);
    assert_eq!(stack_of(&h, &b), Some((a.clone(), 2)));
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Move to top of stack"
    );
    // Undone: the former top is back; redone: the new one.
    let undone = history.undo(&request, &user);
    assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
    assert_eq!(persist.vault().stack(&stack.id).unwrap().unwrap().top, b);
    let redone = history.redo(&request, &user);
    assert!(matches!(redone, Answer::Done(Ok(_))), "{redone:?}");
    assert_eq!(persist.vault().stack(&stack.id).unwrap().unwrap().top, a);

    // Unstack: the stack goes, the photos stay.
    let out = answer(&mut h, "library.stack.ungroup", json!({ "images": [b] }));
    assert_eq!(out["ungrouped"], 1, "{out}");
    for at in [21.0, 24.0, 27.0] {
        settle(&mut h, &mut persist, at);
    }
    assert!(persist.vault().stacks().unwrap().is_empty());
    assert_eq!(stack_of(&h, &a), None, "the grid follows");
    assert_eq!(history.line().undo.last().unwrap().label.text, "Unstack");
    assert!(
        answer_err(&mut h, "library.stack.ungroup", json!({ "images": [b] })),
        "in no stack now"
    );

    // Undone: the stack is back, and the Library read again shows it.
    let undone = history.undo(&request, &user);
    assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
    assert_eq!(persist.vault().stacks().unwrap().len(), 1);
    // The vault\'s event heard: `library.changed` reads the Library again.
    for at in [30.0, 35.0, 40.0] {
        settle(&mut h, &mut persist, at);
    }
    assert_eq!(stack_of(&h, &b), Some((a.clone(), 2)));
    // Undone twice more: the top, then the grouping.
    for _ in 0..2 {
        let undone = history.undo(&request, &user);
        assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
    }
    assert!(persist.vault().stacks().unwrap().is_empty(), "ungrouped");
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// "Pair RAW+JPEG Files" on two photos: one cell remains, the step
/// undone shows two again and redone one; "Unpair Files" splits them.
#[test]
fn two_photos_are_paired_into_one_cell_and_the_step_undone() {
    use egui_pop::api::history::{Answer, Request};
    let root = temp("pairs");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
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
    // a.jpg at the top, c.nef in `raw`: no pair by name, two photos
    // given are paired as they are, the raw the photo.
    let (jpeg, raw) = (id(1).hex(), id(3).hex());
    let request = Request::new(helios_gui::library_history::LIBRARY);
    let user = egui_pop::model::commands::Origin::User;
    let mut clock = 1.0;
    let mut refill = |h: &mut Harness<'static, Shell>, persist: &mut Persist<SqliteStore>| {
        for _ in 0..3 {
            settle(h, persist, clock);
            clock += 4.0;
        }
        // The vault\'s event heard: `library.changed` reads the Library again.
        for _ in 0..3 {
            settle(h, persist, clock);
            clock += 4.0;
        }
    };
    let shown =
        |h: &Harness<'static, Shell>, image: &str| h.state().library().entry(image).is_some();

    h.state_mut()
        .library_mut()
        .select_ids(&[jpeg.clone(), raw.clone()]);
    let out = answer(&mut h, "library.files.pair", json!({}));
    assert_eq!(out["asked"], 2, "{out}");
    refill(&mut h, &mut persist);
    assert_eq!(
        persist.vault().photo_of(&id(1)).unwrap().map(|f| f.photo),
        Some(id(3)),
        "the JPEG is the raw's file"
    );
    assert!(shown(&h, &raw) && !shown(&h, &jpeg), "one cell remains");
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Pair 1 RAW+JPEG files"
    );

    // Undone: two cells; redone: one.
    let undone = history.undo(&request, &user);
    assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
    refill(&mut h, &mut persist);
    assert_eq!(persist.vault().photo_of(&id(1)).unwrap(), None);
    assert!(shown(&h, &raw) && shown(&h, &jpeg), "two cells again");
    let redone = history.redo(&request, &user);
    assert!(matches!(redone, Answer::Done(Ok(_))), "{redone:?}");
    refill(&mut h, &mut persist);
    assert!(!shown(&h, &jpeg), "one cell again");

    // Unpair Files on the photo: two photos, one step.
    let out = answer(&mut h, "library.files.unpair", json!({ "images": [raw] }));
    assert_eq!(out["asked"], 1, "{out}");
    refill(&mut h, &mut persist);
    assert!(shown(&h, &raw) && shown(&h, &jpeg), "unpaired");
    assert_eq!(
        history.line().undo.last().unwrap().label.text,
        "Unpair files"
    );
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// Whether a command, run as `answer` runs it, failed.
fn answer_err(h: &mut Harness<'static, Shell>, id: &str, args: Value) -> bool {
    match h.state_mut().run(id, args, Origin::User) {
        Err(_) => true,
        Ok(out) if out != json!({ "queued": true }) => false,
        Ok(_) => {
            h.settle();
            h.state()
                .history()
                .into_iter()
                .rev()
                .find(|r| r.id == id)
                .is_some_and(|r| r.result.is_err())
        }
    }
}

/// Ctrl+Z walks the library's history off the frame: while another
/// writer holds the vault, each frame stays short, the step shows undone
/// and in progress, a second Ctrl+Z queues behind the first, and both land
/// in order once the vault is released. A walk the vault refuses is put
/// back, said in a toast.
#[test]
fn ctrl_z_walks_the_library_off_the_frame_while_the_vault_is_held() {
    use helios_vault::rotation::{ROTATION, quarter_turns};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    let root = temp("held-undo");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
    // As the desktop wires it, through a writer another one may hold.
    let history = h.state().library_history();
    let held = Arc::new(Mutex::new(()));
    let refuse = Arc::new(AtomicBool::new(false));
    {
        let (vault, held, refuse) = (persist.vault().clone(), held.clone(), refuse.clone());
        history.set_apply(Arc::new(move |changes, forward| {
            let _writer = held.lock().unwrap();
            if refuse.load(Ordering::SeqCst) {
                return Err("the vault is read-only".into());
            }
            if forward {
                vault.replay(changes)
            } else {
                vault.revert(changes)
            }
        }));
    }
    let turns = |persist: &Persist<SqliteStore>| {
        let rows = persist.vault().fields_of(&[id(1)], &[ROTATION]).unwrap();
        quarter_turns(rows[0].get(ROTATION))
    };
    h.state_mut().library_mut().select_ids(&[id(1).hex()]);
    run(&mut h, "helios.vault.focus", json!(null));
    h.settle();
    for at in [1.0, 5.0] {
        h.key_press_modifiers(egui::Modifiers::CTRL, Key::CloseBracket);
        settle(&mut h, &mut persist, at);
    }
    assert_eq!(turns(&persist), 2);
    assert_eq!(history.line().undo.len(), 2);

    // Held: two Ctrl+Z, each frame short; the line moves at once.
    let writer = held.lock().unwrap();
    for _ in 0..2 {
        let start = Instant::now();
        h.key_press_modifiers(egui::Modifiers::CTRL, Key::Z);
        h.settle();
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "a frame waited on the vault: {:?}",
            start.elapsed()
        );
    }
    let line = history.line();
    assert_eq!((line.undo.len(), line.redo.len()), (0, 2), "{line:?}");
    assert!(line.redo.iter().all(|s| s.in_progress), "{line:?}");
    assert_eq!(history.in_flight().len(), 2);
    assert!(
        h.state()
            .status_text()
            .is_some_and(|t| t.starts_with("Undoing")),
        "{:?}",
        h.state().status_text()
    );
    assert_eq!(turns(&persist), 2, "nothing written while held");
    drop(writer);
    assert!(history.wait_idle(Duration::from_secs(30)));
    settle(&mut h, &mut persist, 10.0);
    assert_eq!(turns(&persist), 0, "both undone, in order");
    assert!(history.line().redo.iter().all(|s| !s.in_progress));

    // Redone, refused: the step goes back where it was, with a toast.
    refuse.store(true, Ordering::SeqCst);
    run(&mut h, "edit.redo", json!(null));
    h.settle();
    assert!(history.wait_idle(Duration::from_secs(30)));
    settle(&mut h, &mut persist, 20.0);
    let line = history.line();
    assert_eq!((line.undo.len(), line.redo.len()), (0, 2), "{line:?}");
    assert!(
        h.query_all_by_label_contains("Undo refused: the vault is read-only")
            .next()
            .is_some(),
        "the refusal toasted"
    );
    assert_eq!(turns(&persist), 0);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// A photo's tags, as the vault holds them.
fn tags_of(persist: &Persist<SqliteStore>, n: u8) -> Value {
    let rows = persist.vault().fields_of(&[id(n)], &["tags"]).unwrap();
    rows[0].get("tags").cloned().unwrap_or(Value::Null)
}

#[test]
fn the_sources_fold_by_keyboard_and_keep_one_choice_per_section() {
    let root = temp("sources");
    let path = vault_file(&root);
    {
        let mut store = SqliteStore::open(&path).unwrap();
        store
            .set_field(&id(1), "tags", Some(&json!(["Animals|Dog", "sea"])))
            .unwrap();
    }
    let (mut h, mut persist) = window(&root, &path);
    let selected = |h: &Harness<'_, Shell>, label: &str| {
        let node = h.get_by_label(label).accesskit_node();
        node.toggled() == Some(egui::accesskit::Toggled::True) || node.is_selected() == Some(true)
    };

    // A branch of the keyword tree opens with Right and folds with Left
    // while its row has the keyboard.
    h.get_by_label("Keywords").click();
    settle(&mut h, &mut persist, 1.0);
    assert!(h.query_by_label("Dog, 1").is_none(), "closed at first");
    h.get_by_label("Animals, 1").focus();
    h.settle();
    h.key_press(Key::ArrowRight);
    settle(&mut h, &mut persist, 2.0);
    h.get_by_label("Dog, 1");
    h.key_press(Key::ArrowLeft);
    settle(&mut h, &mut persist, 3.0);
    assert!(h.query_by_label("Dog, 1").is_none(), "folded again");

    // A folder and a keyword chosen together: both scopes hold, each
    // row highlighted in its section.
    h.get_by_label("Folders").click();
    settle(&mut h, &mut persist, 4.0);
    let top = root.to_string_lossy().into_owned();
    let folder = format!("{top}, 3");
    // A row named by its whole path, wider than the side bar where the
    // temporary folder's path is long (Windows): chosen as assistive
    // technology does, not at its centre, which may lie past the bar.
    h.get_by_label(&folder).click_accesskit();
    settle(&mut h, &mut persist, 5.0);
    h.get_by_label("sea, 1").click();
    settle(&mut h, &mut persist, 6.0);
    assert_eq!(h.state().library().folder(), Some(top.clone()));
    assert_eq!(h.state().library().keyword().as_deref(), Some("sea"));
    assert_eq!(grid(&h), ["a.jpg"]);
    assert!(selected(&h, &folder), "the folder stays highlighted");
    assert!(selected(&h, "sea, 1"), "beside the keyword");
    assert!(!selected(&h, "Animals, 1"));
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_keyword_tree_counts_scopes_tags_by_drag_and_renames_and_merges_with_undo() {
    use egui_pop::api::history::{Answer, Request};
    use helios_gui::library::DropOn;
    let root = temp("keywords");
    let path = vault_file(&root);
    {
        let mut store = SqliteStore::open(&path).unwrap();
        store
            .set_field(&id(1), "tags", Some(&json!(["Animals|Dog", "sea"])))
            .unwrap();
        store
            .set_field(&id(2), "tags", Some(&json!(["Animals|Cat"])))
            .unwrap();
    }
    let (mut h, mut persist) = window(&root, &path);
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
    let request = Request::new(helios_gui::library_history::LIBRARY);
    let user = egui_pop::model::commands::Origin::User;
    let row = |h: &Harness<'_, Shell>, label: &str| h.get_by_label(label).rect().center();

    // The tree: the top branches with their counts, a photo counted once
    // under Animals; a branch opens on its own branches.
    h.get_by_label("Keywords").click();
    settle(&mut h, &mut persist, 1.0);
    h.get_by_label("Animals, 2");
    h.get_by_label("sea, 1");
    assert!(h.query_by_label("Dog, 1").is_none(), "closed");
    h.get_by_label("Show or hide Animals").click();
    settle(&mut h, &mut persist, 2.0);
    h.get_by_label("Dog, 1");
    h.get_by_label("Cat, 1");

    // A click scopes the grid to a branch, with the other filters; again,
    // to the whole Library.
    h.get_by_label("Dog, 1").click();
    settle(&mut h, &mut persist, 3.0);
    assert_eq!(grid(&h), ["a.jpg"]);
    assert_eq!(
        h.state().library().keyword().as_deref(),
        Some("Animals|Dog")
    );
    h.get_by_label("Dog, 1").click();
    settle(&mut h, &mut persist, 4.0);
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);
    h.get_by_label("Animals, 2").click();
    settle(&mut h, &mut persist, 5.0);
    assert_eq!(grid(&h), ["a.jpg", "b.jpg"], "the branch and below");
    let listed = answer(&mut h, "library.keyword.list", json!({}));
    assert_eq!(listed["shown"], "Animals", "{listed}");
    assert_eq!(
        listed["keywords"][0],
        json!({ "path": "Animals", "count": 2 })
    );
    answer(&mut h, "library.keyword", json!({ "path": null }));
    settle(&mut h, &mut persist, 6.0);
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);

    // A branch dragged onto a photo tags it; photos dragged onto a
    // branch take it.
    let (from, to) = (row(&h, "sea, 1"), cell_at(&h, "c.nef"));
    drag_onto(&mut h, from, to, |h| {
        h.get_by_label("Tag 1 photos with sea");
    });
    settle(&mut h, &mut persist, 7.0);
    assert_eq!(tags_of(&persist, 3), json!(["sea"]));
    h.get_by_label("sea, 2");
    let (from, to) = (cell_at(&h, "c.nef"), row(&h, "Cat, 1"));
    drag_onto(&mut h, from, to, |h| {
        assert_eq!(
            h.state().library().drop_hover(),
            Some((DropOn::Keyword("Animals|Cat".into()), true))
        );
        h.get_by_label("Tag 1 photos with Animals|Cat");
    });
    settle(&mut h, &mut persist, 8.0);
    assert_eq!(tags_of(&persist, 3), json!(["Animals|Cat", "sea"]));
    h.get_by_label("Animals, 3");
    // Taken off through the branch's command (its menu's).
    let out = answer(
        &mut h,
        "library.keyword.remove",
        json!({ "keyword": "Animals", "images": [id(3).hex()] }),
    );
    assert_eq!(out["untagged"], 1, "{out}");
    settle(&mut h, &mut persist, 9.0);
    assert_eq!(tags_of(&persist, 3), json!(["sea"]));

    // Renamed, then merged: each one step for every photo, undone and
    // redone.
    let before = steps(&h).len();
    assert!(answer_err(
        &mut h,
        "library.keyword.rename",
        json!({ "from": "Animals", "to": "sea" })
    ));
    let out = answer(
        &mut h,
        "library.keyword.rename",
        json!({ "from": "Animals", "to": "Fauna" }),
    );
    assert_eq!(out["photos"], 2, "{out}");
    settle(&mut h, &mut persist, 10.0);
    assert_eq!(tags_of(&persist, 1), json!(["Fauna|Dog", "sea"]));
    assert_eq!(tags_of(&persist, 2), json!(["Fauna|Cat"]));
    assert_eq!(
        steps(&h).last().map(String::as_str),
        Some("Rename keyword Animals to Fauna")
    );
    h.get_by_label("Fauna, 2");
    let out = answer(
        &mut h,
        "library.keyword.merge",
        json!({ "from": "Fauna|Cat", "into": "sea" }),
    );
    assert_eq!(out["photos"], 1, "{out}");
    settle(&mut h, &mut persist, 12.0);
    assert_eq!(tags_of(&persist, 2), json!(["sea"]));
    assert_eq!(steps(&h).len(), before + 2, "{:?}", steps(&h));
    assert_eq!(
        steps(&h).last().map(String::as_str),
        Some("Merge keyword Fauna|Cat into sea")
    );
    let undone = history.undo(&request, &user);
    assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
    assert_eq!(tags_of(&persist, 2), json!(["Fauna|Cat"]));
    let undone = history.undo(&request, &user);
    assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
    assert_eq!(tags_of(&persist, 1), json!(["Animals|Dog", "sea"]));
    assert_eq!(tags_of(&persist, 2), json!(["Animals|Cat"]));
    let redone = history.redo(&request, &user);
    assert!(matches!(redone, Answer::Done(Ok(_))), "{redone:?}");
    assert_eq!(tags_of(&persist, 1), json!(["Fauna|Dog", "sea"]));
    // The Library read again shows the tree as the vault holds it.
    settle(&mut h, &mut persist, 13.0);
    persist.ask_fill(h.state(), true);
    for at in [14.0, 16.0, 18.0] {
        settle(&mut h, &mut persist, at);
    }
    h.get_by_label("Fauna, 2");
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// A photo's field, as the vault holds it.
fn field_of(persist: &Persist<SqliteStore>, n: u8, name: &str) -> Value {
    let rows = persist.vault().fields_of(&[id(n)], &[name]).unwrap();
    rows[0].get(name).cloned().unwrap_or(Value::Null)
}

#[test]
fn the_metadata_section_edits_a_mixed_selection_as_one_step_with_templates() {
    use egui_pop::api::history::{Answer, Request};
    let root = temp("metadata");
    let path = vault_file(&root);
    {
        let mut store = SqliteStore::open(&path).unwrap();
        for (n, title) in [(1, "one"), (2, "two"), (3, "three")] {
            store
                .set_field(&id(n), "title", Some(&json!(title)))
                .unwrap();
        }
        store
            .set_field(&id(1), "tags", Some(&json!(["Animals|Dog", "sea"])))
            .unwrap();
    }
    let (mut h, mut persist) = window(&root, &path);
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
    let request = Request::new(helios_gui::library_history::LIBRARY);
    let user = egui_pop::model::commands::Origin::User;
    let all: Vec<String> = (1..=3).map(|n| id(n).hex()).collect();
    h.state_mut().library_mut().select_ids(&all);
    settle(&mut h, &mut persist, 1.0);

    // Three titles differ: mixed; no creator yet: shared (empty).
    h.get_by_label("3 photos selected");
    let got = answer(&mut h, "library.metadata.get", json!({}));
    assert_eq!(got["title"], "mixed", "{got}");
    assert_eq!(got["creator"], "", "{got}");
    assert_eq!(got["keywords"]["some"], json!(["Animals|Dog", "sea"]));

    // A creator typed for all, a keyword added and a branch taken off:
    // one step, the mixed titles kept.
    let before = steps(&h).len();
    let out = answer(
        &mut h,
        "library.metadata.set",
        json!({ "fields": { "creator": "Ann" },
                "keywords": { "add": ["Trips|2026"], "remove": ["Animals"] } }),
    );
    assert_eq!(out["photos"], 3, "{out}");
    settle(&mut h, &mut persist, 2.0);
    for n in 1..=3 {
        assert_eq!(field_of(&persist, n, "creator"), "Ann");
    }
    assert_eq!(field_of(&persist, 2, "title"), "two", "a mixed field kept");
    assert_eq!(field_of(&persist, 1, "tags"), json!(["Trips|2026", "sea"]));
    assert_eq!(field_of(&persist, 3, "tags"), json!(["Trips|2026"]));
    assert_eq!(steps(&h).len(), before + 1, "{:?}", steps(&h));
    assert_eq!(
        steps(&h).last().map(String::as_str),
        Some("Edit the metadata of 3 photos")
    );
    let got = answer(&mut h, "library.metadata.get", json!({}));
    assert_eq!(got["creator"], "Ann", "the vault's events shown: {got}");
    assert_eq!(got["keywords"]["all"], json!(["Trips|2026"]));

    // Undone and redone as one step.
    let undone = history.undo(&request, &user);
    assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
    for n in 1..=3 {
        assert_eq!(field_of(&persist, n, "creator"), Value::Null);
    }
    assert_eq!(field_of(&persist, 1, "tags"), json!(["Animals|Dog", "sea"]));
    let redone = history.redo(&request, &user);
    assert!(matches!(redone, Answer::Done(Ok(_))), "{redone:?}");
    assert_eq!(field_of(&persist, 2, "creator"), "Ann");

    // Templates: read from the library, saved from what the selection
    // shares, applied to one photo as one step, deleted.
    settle(&mut h, &mut persist, 4.0);
    let listed = answer(&mut h, "library.metadata.template.list", json!({}));
    assert_eq!(listed["templates"], json!([]), "{listed}");
    let saved = answer(
        &mut h,
        "library.metadata.template.save",
        json!({ "name": "Wedding" }),
    );
    assert_eq!(saved["templates"], 1, "{saved}");
    let listed = answer(&mut h, "library.metadata.template.list", json!({}));
    assert_eq!(
        listed["templates"][0],
        json!({ "name": "Wedding", "fields": { "creator": "Ann" }, "keywords": ["Trips|2026"] })
    );
    answer(
        &mut h,
        "library.metadata.template.save",
        json!({ "name": "Rights", "fields": { "copyright": "© Ann" } }),
    );
    settle(&mut h, &mut persist, 5.0);
    let kept = persist
        .vault()
        .library_document("meta", "templates")
        .unwrap();
    assert_eq!(
        kept.as_ref()
            .map(|d| d["templates"].as_array().map_or(0, Vec::len)),
        Some(2),
        "{kept:?}"
    );
    let out = answer(
        &mut h,
        "library.metadata.template.apply",
        json!({ "name": "Rights", "images": [id(3).hex()] }),
    );
    assert_eq!(out["photos"], 1, "{out}");
    settle(&mut h, &mut persist, 6.0);
    assert_eq!(field_of(&persist, 3, "copyright"), "© Ann");
    assert_eq!(field_of(&persist, 2, "copyright"), Value::Null);
    let undone = history.undo(&request, &user);
    assert!(matches!(undone, Answer::Done(Ok(_))), "{undone:?}");
    assert_eq!(field_of(&persist, 3, "copyright"), Value::Null);
    let left = answer(
        &mut h,
        "library.metadata.template.delete",
        json!({ "name": "Rights" }),
    );
    assert_eq!(left["templates"], 1, "{left}");
    assert!(answer_err(
        &mut h,
        "library.metadata.template.apply",
        json!({ "name": "Rights" })
    ));
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// Whether the window paints its bars and the grid's cells: the main
/// menu, the status bar's zone and a cell per name are in the
/// accessibility tree.
fn paints(h: &Harness<'_, Shell>, cells: &[&str]) -> Result<(), String> {
    let button = |name: &str| {
        h.query_by(|n| {
            n.role() == egui::accesskit::Role::Button && n.label().is_some_and(|l| l.contains(name))
        })
        .is_some()
    };
    if h.query_all_by_label_contains("File").next().is_none() {
        return Err("no main menu".into());
    }
    let named = h
        .query_all_by(|n| n.label().is_some_and(|l| !l.is_empty()))
        .count();
    if named < 12 {
        return Err(format!("{named} named nodes only: the bars are not drawn"));
    }
    for name in cells {
        if !button(name) {
            return Err(format!("no cell for {name}"));
        }
    }
    Ok(())
}

/// The health check's fix for RAW+JPEG pairs (`library.files.pair` with
/// `all`) on a Library of three pairs and a lone photo, a paired JPEG
/// selected: the JPEGs leave the grid, and the window keeps painting its
/// bars and cells; a second window on the same workspace and vault (the
/// next launch) paints them too.
#[test]
fn the_window_keeps_painting_once_every_pair_is_made_and_on_the_next_launch() {
    let root = temp("pairs-all");
    let path = root.join("vault.db");
    {
        let mut store = SqliteStore::open(&path).unwrap();
        let photos: [(u8, &str, &str); 7] = [
            (1, "p1.nef", "nef"),
            (2, "p1.jpg", "jpg"),
            (3, "p2.nef", "nef"),
            (4, "p2.jpg", "jpg"),
            (5, "p3.nef", "nef"),
            (6, "p3.jpg", "jpg"),
            (7, "lone.jpg", "jpg"),
        ];
        for (n, name, kind) in photos {
            store
                .add_to_library(&Image {
                    id: id(n),
                    size: 10,
                    partial: ContentHash([n; 32]),
                })
                .unwrap();
            store
                .put_location(&id(n), &Location::seen(root.join(name), 10, 1, 1))
                .unwrap();
            for (field, value) in [
                (CAPTURED, json!(u64::from(n) * 100)),
                (CAMERA_MODEL, json!("Z 8")),
                (FILE_TYPE, json!(kind)),
            ] {
                store.set_field(&id(n), field, Some(&value)).unwrap();
            }
        }
    }
    let (config, workspace) = workspace(&root);
    let (mut h, mut persist) = open(&config, &workspace, &path);
    let mut clock = 1.0;
    let mut refill = |h: &mut Harness<'static, Shell>, persist: &mut Persist<SqliteStore>| {
        for _ in 0..3 {
            settle(h, persist, clock);
            clock += 4.0;
        }
        // The vault\'s event heard: `library.changed` reads the Library again.
        for _ in 0..3 {
            settle(h, persist, clock);
            clock += 4.0;
        }
    };
    refill(&mut h, &mut persist);
    assert_eq!(grid(&h).len(), 7, "{:?}", grid(&h));
    paints(&h, &["p1.nef", "p1.jpg", "lone.jpg"]).unwrap();

    // A JPEG about to leave the Library is selected.
    h.state_mut().library_mut().select_ids(&[id(2).hex()]);
    let out = answer(&mut h, "library.files.pair", json!({ "all": true }));
    assert!(out.get("asked").is_some(), "{out}");
    refill(&mut h, &mut persist);
    for n in [2, 4, 6] {
        assert!(
            persist.vault().photo_of(&id(n)).unwrap().is_some(),
            "JPEG {n} is its RAW's file"
        );
    }
    let mut left = grid(&h);
    left.sort();
    assert_eq!(left, ["lone.jpg", "p1.nef", "p2.nef", "p3.nef"]);
    paints(&h, &["p1.nef", "p2.nef", "p3.nef", "lone.jpg"]).unwrap();
    drop(h);
    drop(persist);

    // The next launch: the same workspace and vault.
    let (mut h, mut persist) = open(&config, &workspace, &path);
    let mut clock = 100.0;
    for _ in 0..4 {
        settle(&mut h, &mut persist, clock);
        clock += 4.0;
    }
    let mut left = grid(&h);
    left.sort();
    assert_eq!(left, ["lone.jpg", "p1.nef", "p2.nef", "p3.nef"]);
    paints(&h, &["p1.nef", "p2.nef", "p3.nef", "lone.jpg"]).unwrap();
}

/// A workspace kept on an empty album opens on it (the owner's
/// "0 photos" of 2026-11-15): what limits the grid shows as a chip by
/// the search, the empty grid says the album is empty, and its action
/// or the chip's removal shows every photo again.
#[test]
fn a_workspace_on_an_empty_album_says_so_and_shows_all_in_one_click() {
    let root = temp("empty-album");
    let path = vault_file(&root);
    let (config, ws) = workspace(&root);
    let (mut h, mut persist) = open(&config, &ws, &path);
    h.state_mut().library_mut().select_ids(&[]);
    run(
        &mut h,
        "library.collection.create",
        json!({ "name": "test" }),
    );
    settle(&mut h, &mut persist, 1.0);
    run(&mut h, "library.collection", json!({ "id": "test" }));
    settle(&mut h, &mut persist, 2.0);
    assert_eq!(kept_collection(&config, &ws).as_deref(), Some("test"));
    drop((h, persist));

    for lift in ["Show all photos", "Remove Album: test", "All photos"] {
        let (mut h, mut persist) = open(&config, &ws, &path);
        settle(&mut h, &mut persist, 3.0);
        assert!(grid(&h).is_empty(), "{lift}: the album holds no photo");
        h.get_by_label("Album: test");
        h.get_by_label("This album is empty");
        h.get_by_label(lift).click();
        settle(&mut h, &mut persist, 4.0);
        assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"], "{lift}");
        assert!(h.query_by_label("Album: test").is_none(), "{lift}");
        assert_eq!(h.state().library().collection(), None, "{lift}");
        drop((h, persist));
        // Opened on the album again for the next way out.
        let (mut h, mut persist) = open(&config, &ws, &path);
        run(&mut h, "library.collection", json!({ "id": "test" }));
        settle(&mut h, &mut persist, 5.0);
        drop((h, persist));
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A workspace kept on an album that is no longer there still opens,
/// with the same hint and way out.
#[test]
fn a_workspace_on_a_missing_album_opens_with_the_hint() {
    let root = temp("missing-album");
    let path = vault_file(&root);
    let (config, ws) = workspace(&root);
    let (mut h, mut persist) = open(&config, &ws, &path);
    h.state_mut().library_mut().set_collection(Some("gone"));
    settle(&mut h, &mut persist, 1.0);
    assert!(grid(&h).is_empty());
    h.get_by_label("Album: gone");
    h.get_by_label("Show all photos").click();
    settle(&mut h, &mut persist, 2.0);
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The photos a gallery shows, by name.
fn gallery(h: &mut Harness<'static, Shell>, id: &str) -> Vec<String> {
    h.state_mut().library_mut().with_gallery(id, |library| {
        library
            .shown()
            .iter()
            .map(|id| library.entry(id).unwrap().name.clone())
            .collect()
    })
}

/// Each Library view keeps its own scope (owner, 2026-11-15: the
/// active filters of the Library views are apart). A workspace written
/// before the views were apart, its album at the top of `library`,
/// scopes the main window's panel only; a folder chosen in the Library
/// window is kept under `library.views.window` and scopes that window
/// only.
#[test]
fn an_old_workspace_s_album_scopes_the_main_panel_only_and_the_window_keeps_its_own() {
    use helios_gui::library::{MAIN_GALLERY, WINDOW_GALLERY};
    let root = temp("views-apart");
    let path = vault_file(&root);
    let (config, ws) = workspace(&root);
    let (mut h, mut persist) = open(&config, &ws, &path);
    h.state_mut().library_mut().select_ids(&[]);
    run(
        &mut h,
        "library.collection.create",
        json!({ "name": "test" }),
    );
    settle(&mut h, &mut persist, 1.0);
    run(&mut h, "library.collection", json!({ "id": "test" }));
    settle(&mut h, &mut persist, 2.0);
    // Chosen in the main panel: where a workspace has always kept it.
    assert_eq!(kept_collection(&config, &ws).as_deref(), Some("test"));
    drop((h, persist));

    let (mut h, mut persist) = open(&config, &ws, &path);
    settle(&mut h, &mut persist, 3.0);
    assert!(grid(&h).is_empty(), "the main panel opens on the album");
    assert_eq!(
        gallery(&mut h, WINDOW_GALLERY),
        ["a.jpg", "b.jpg", "c.nef"],
        "the Library window shows every photo"
    );

    // In the Library window (its search field clicked: the input is
    // there), a folder: the window's scope, kept as the window's.
    run(&mut h, "library.window", Value::Null);
    settle(&mut h, &mut persist, 4.0);
    assert!(
        h.query_by_label("Album: test").is_none(),
        "not the window's"
    );
    h.get_by_role_and_label(egui::accesskit::Role::Window, "Library")
        .get_by_label("Search images")
        .click();
    h.settle();
    let folder = folder_of(&root, 1).display().to_string();
    run(&mut h, "library.folder", json!({ "path": folder }));
    settle(&mut h, &mut persist, 5.0);
    let kept = Files::new(&config, &ws)
        .load()
        .unwrap()
        .bundle
        .template
        .library
        .unwrap();
    assert_eq!(
        kept.collection.as_deref(),
        Some("test"),
        "the panel's stays"
    );
    assert_eq!(kept.folder, None, "the window's folder is not the panel's");
    assert_eq!(
        kept.views[WINDOW_GALLERY].folder.as_deref(),
        Some(folder.as_str())
    );
    h.state_mut().library_mut().with_gallery(MAIN_GALLERY, |l| {
        assert_eq!(l.collection().as_deref(), Some("test"));
        assert_eq!(l.folder(), None);
    });
    drop((h, persist));

    // Opened again: each view on its own scope.
    let (mut h, mut persist) = open(&config, &ws, &path);
    settle(&mut h, &mut persist, 6.0);
    assert!(grid(&h).is_empty(), "the panel, on its album");
    h.state_mut()
        .library_mut()
        .with_gallery(WINDOW_GALLERY, |l| {
            assert_eq!(l.folder().as_deref(), Some(folder.as_str()));
            assert_eq!(l.collection(), None);
        });
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The Library window (Window › Library) in a system window of its own,
/// its passes run as eframe runs a deferred window's, where the system
/// hides where windows are (Wayland): no position in their input.
struct LibraryWindow {
    viewport: egui::ViewportId,
    time: f64,
    /// The accessible names its last pass drew, with their rectangles.
    nodes: Vec<(String, egui::Rect)>,
}

impl LibraryWindow {
    /// Opens the Library window, the main window's own viewports real
    /// (not embedded), and runs its first passes.
    fn open(h: &mut Harness<'static, Shell>, persist: &mut Persist<SqliteStore>) -> Self {
        use helios_gui::shell::{LIBRARY_WINDOW, LIBRARY_WINDOW_COMMAND};
        h.ctx.set_embed_viewports(false);
        run(h, LIBRARY_WINDOW_COMMAND, Value::Null);
        settle(h, persist, 1.0);
        let viewport = h
            .state()
            .app()
            .handle()
            .window_viewport(&egui_pop::api::windows::WindowId::new(LIBRARY_WINDOW))
            .expect("the Library window is open");
        let mut w = Self {
            viewport,
            time: h.ctx.input(|i| i.time),
            nodes: Vec::new(),
        };
        for _ in 0..3 {
            w.pass(h, Vec::new());
            settle(h, persist, 1.0);
        }
        w
    }

    /// One pass of the window, given these input events.
    fn pass(&mut self, h: &Harness<'static, Shell>, events: Vec<egui::Event>) {
        use egui::{Pos2, RawInput, Rect, ViewportId, ViewportInfo};
        let cb = h.output().viewport_output[&self.viewport]
            .viewport_ui_cb
            .clone()
            .expect("the main window's last pass declared the Library window");
        self.time += 1.0 / 60.0;
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(1100.0, 750.0));
        let mut input = RawInput {
            viewport_id: self.viewport,
            screen_rect: Some(rect),
            time: Some(self.time),
            focused: true,
            events,
            ..Default::default()
        };
        input.viewports.insert(
            self.viewport,
            ViewportInfo {
                focused: Some(true),
                ..Default::default()
            },
        );
        input
            .viewports
            .insert(ViewportId::ROOT, ViewportInfo::default());
        let mut out = h.ctx.run_ui(input, |ui| cb(ui));
        out.textures_delta.clear();
        if let Some(update) = &out.platform_output.accesskit_update {
            self.nodes = update
                .nodes
                .iter()
                .filter_map(|(_, n)| {
                    let name = n.label().or_else(|| n.value())?.to_string();
                    let b = n.bounds()?;
                    Some((
                        name,
                        Rect::from_min_max(
                            Pos2::new(b.x0 as f32, b.y0 as f32),
                            Pos2::new(b.x1 as f32, b.y1 as f32),
                        ),
                    ))
                })
                .collect();
        }
    }

    /// The centre of the node its last pass drew named `name`, else of
    /// the first whose name contains it.
    fn at(&self, name: &str) -> egui::Pos2 {
        self.nodes
            .iter()
            .find(|(n, _)| n == name)
            .or_else(|| self.nodes.iter().find(|(n, _)| n.contains(name)))
            .unwrap_or_else(|| {
                let names: Vec<_> = self.nodes.iter().map(|(n, _)| n).collect();
                panic!("no {name:?} in the Library window: {names:?}")
            })
            .1
            .center()
    }

    /// A click of the hand at `name`, each step a pass of its own.
    fn click(
        &mut self,
        h: &mut Harness<'static, Shell>,
        persist: &mut Persist<SqliteStore>,
        name: &str,
    ) {
        let at = self.at(name);
        self.pass(h, vec![egui::Event::PointerMoved(at)]);
        for pressed in [true, false] {
            self.pass(
                h,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        settle(h, persist, 1.0);
        self.pass(h, vec![egui::Event::PointerGone]);
    }
}

/// Drags from `from` in the main window out past its right edge, and
/// lets go there: the main window keeps the pointer while the button is
/// held, then the system gives it to the Library window, at `there` of
/// its points, which sees no release.
fn drag_into_window(
    h: &mut Harness<'static, Shell>,
    persist: &mut Persist<SqliteStore>,
    w: &mut LibraryWindow,
    from: egui::Pos2,
    there: impl Fn(&LibraryWindow) -> egui::Pos2,
) {
    let out = egui::pos2(1400.0, from.y);
    h.drag_at(from);
    h.settle();
    for i in 1..=8 {
        h.hover_at(from + (out - from) * (i as f32 / 8.0));
        h.settle();
    }
    h.drop_at(out);
    h.event(egui::Event::PointerGone);
    // One pass of the main window only: the drag waits a few for another
    // window to see the pointer.
    h.run_steps(1);
    let at = there(w);
    w.pass(h, vec![egui::Event::PointerMoved(at)]);
    w.pass(h, Vec::new());
    settle(h, persist, 2.0);
    w.pass(h, vec![egui::Event::PointerGone]);
    settle(h, persist, 3.0);
}

/// Photos dragged from the grid of the main window and let go over an
/// album of the Library window, where the system hides where windows are
/// (Wayland): they go in the album, once.
#[test]
fn photos_dragged_from_the_main_window_onto_an_album_of_the_library_window_go_in() {
    let root = temp("drag-windows");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
    run(
        &mut h,
        "library.collection.create",
        json!({ "name": "Trip", "images": [id(3).hex()] }),
    );
    // The main window's selection, made before the Library window has
    // the input (each window's gallery has its own).
    h.state_mut()
        .library_mut()
        .select_ids(&[id(1).hex(), id(2).hex()]);
    settle(&mut h, &mut persist, 1.0);
    let mut w = LibraryWindow::open(&mut h, &mut persist);
    w.click(&mut h, &mut persist, "Albums");
    w.at("Trip, 1");
    let from = cell_at(&h, "a.jpg");
    drag_into_window(&mut h, &mut persist, &mut w, from, |w| w.at("Trip, 1"));
    assert_eq!(albums(&h), [("Trip".to_string(), 3)]);
    assert_eq!(
        steps(&h).last().map(String::as_str),
        Some("Add 2 photos to “Trip”")
    );
    w.pass(&h, Vec::new());
    w.at("Trip, 3");
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// A keyword branch dragged from the main window's Keywords section and
/// let go over a photo of the Library window's grid tags the photo.
#[test]
fn a_keyword_dragged_from_the_main_window_onto_a_photo_of_the_library_window_tags_it() {
    let root = temp("drag-keyword-windows");
    let path = vault_file(&root);
    {
        let mut store = SqliteStore::open(&path).unwrap();
        store
            .set_field(&id(1), "tags", Some(&json!(["sea"])))
            .unwrap();
    }
    let (mut h, mut persist) = window(&root, &path);
    h.get_by_label("Keywords").click();
    settle(&mut h, &mut persist, 1.0);
    let from = h.get_by_label("sea, 1").rect().center();
    let mut w = LibraryWindow::open(&mut h, &mut persist);
    w.at("c.nef");
    drag_into_window(&mut h, &mut persist, &mut w, from, |w| w.at("c.nef"));
    assert_eq!(tags_of(&persist, 3), json!(["sea"]));
    assert_eq!(
        tags_of(&persist, 2),
        Value::Null,
        "only the photo let go on"
    );
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The centre of the cell showing `name` inside the panel `panel`.
fn cell_in(h: &Harness<'_, Shell>, panel: &str, name: &str) -> egui::Pos2 {
    let area = h.state().target_rect(panel).unwrap();
    h.query_all_by(|n| {
        n.role() == egui::accesskit::Role::Button && n.label().is_some_and(|l| l.contains(name))
    })
    .map(|n| n.rect())
    .find(|r| area.contains_rect(*r))
    .unwrap_or_else(|| panic!("no cell {name} in {panel}"))
    .center()
}

/// The photos (ids) a quick collection holds, as the Library last read.
fn quick_holds(h: &Harness<'_, Shell>, quick: &str) -> Vec<String> {
    let mut held = h
        .state()
        .library()
        .quick_collection(quick)
        .map(|q| q.images.clone())
        .unwrap_or_default();
    held.sort();
    held
}

/// Quick-collection panels (#5255 S6c): two quick collections open in
/// panels of their own; a photo dragged from one to the other moves
/// (one step, undone), Ctrl copies, a photo dragged from a panel onto
/// Compare joins the comparison; the main view's cursor and selection
/// never move; a restart from the saved layout gives both panels back.
/// Each panel's tab is titled with its collection's name, and follows a
/// rename.
#[test]
fn quick_collection_panels_move_copy_and_feed_compare_and_come_back() {
    use helios_gui::library_views::{QUICK_OPEN_COMMAND, quick_views};
    let root = temp("quick-panels");
    let path = vault_file(&root);
    let (config, workspace) = workspace(&root);
    let (mut h, mut persist) = open(&config, &workspace, &path);
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
    let (a, b, c) = (id(1).hex(), id(2).hex(), id(3).hex());
    run(
        &mut h,
        "library.quick.create",
        json!({ "name": "Picks", "images": [a, b] }),
    );
    run(
        &mut h,
        "library.quick.create",
        json!({ "name": "Maybe", "images": [c] }),
    );
    settle(&mut h, &mut persist, 1.0);
    // The main view browsed to c.
    h.state_mut()
        .library_mut()
        .select_ids(std::slice::from_ref(&c));
    h.settle();
    let main = |h: &Harness<'_, Shell>| {
        h.state()
            .library_mut()
            .with_gallery(helios_gui::library::MAIN_GALLERY, |l| {
                (l.cursor(), l.selection())
            })
    };
    let browsing = main(&h);
    assert_eq!(browsing.0.as_deref(), Some(c.as_str()));

    // Compared: two photos, then the two quick collections in panels.
    run(&mut h, "library.compare", json!({ "images": [b, c] }));
    settle(&mut h, &mut persist, 2.0);
    run(
        &mut h,
        QUICK_OPEN_COMMAND,
        json!({ "id": "picks", "place": "start_side" }),
    );
    run(
        &mut h,
        QUICK_OPEN_COMMAND,
        json!({ "id": "maybe", "place": "end_side" }),
    );
    settle(&mut h, &mut persist, 3.0);
    let views = quick_views(h.state().app().handle());
    let [(picks, _), (maybe, _)] = &views[..] else {
        panic!("two quick panels: {views:?}");
    };
    let (picks, maybe) = (picks.clone(), maybe.clone());
    assert_eq!(
        views,
        vec![
            (picks.clone(), "picks".to_string()),
            (maybe.clone(), "maybe".to_string())
        ]
    );
    let shown = |h: &Harness<'_, Shell>, view: &str| {
        h.state()
            .library_mut()
            .with_gallery(view, |l| l.filter().text.clone())
    };
    assert_eq!(shown(&h, &picks), "collection:picks");
    assert_eq!(shown(&h, &maybe), "collection:maybe");
    // Each tab is titled with its collection's name, not "Results 2".
    let title = |h: &Harness<'_, Shell>, view: &str| {
        h.state().app().handle().describe_panel(view).unwrap().title
    };
    assert_eq!(title(&h, &picks), "Picks");
    assert_eq!(title(&h, &maybe), "Maybe");

    // a dragged from Picks to Maybe: it moves, one step.
    let to = h.state().target_rect(&maybe).unwrap().center();
    let from = cell_in(&h, &picks, "a.jpg");
    drag_onto(&mut h, from, to, |_| {});
    settle(&mut h, &mut persist, 4.0);
    assert_eq!(quick_holds(&h, "picks"), vec![b.clone()]);
    let mut both = vec![a.clone(), c.clone()];
    both.sort();
    assert_eq!(quick_holds(&h, "maybe"), both);
    assert_eq!(
        steps(&h).last().unwrap(),
        "Move 1 photos to the quick collection “Maybe”"
    );
    assert_eq!(main(&h), browsing, "the main view's cursor stays");
    // Undone: back where it was, in one step.
    let undone = history.undo(
        &egui_pop::api::history::Request::new(helios_gui::library_history::LIBRARY),
        &egui_pop::model::commands::Origin::User,
    );
    assert!(
        matches!(undone, egui_pop::api::history::Answer::Done(Ok(_))),
        "{undone:?}"
    );
    settle(&mut h, &mut persist, 5.0);
    let mut first = vec![a.clone(), b.clone()];
    first.sort();
    assert_eq!(quick_holds(&h, "picks"), first);
    assert_eq!(quick_holds(&h, "maybe"), vec![c.clone()]);

    // b dragged with Ctrl: copied, it stays in Picks too.
    let from = cell_in(&h, &picks, "b.jpg");
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
    drag_onto(&mut h, from, to, |_| {});
    h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    settle(&mut h, &mut persist, 6.0);
    assert_eq!(quick_holds(&h, "picks"), first);
    let mut copied = vec![b.clone(), c.clone()];
    copied.sort();
    assert_eq!(quick_holds(&h, "maybe"), copied);
    assert_eq!(main(&h), browsing, "the main view's cursor stays");

    // a dragged from Picks onto Compare: it joins the comparison.
    let compare = h
        .state()
        .target_rect("helios.library.compare")
        .unwrap()
        .center();
    let from = cell_in(&h, &picks, "a.jpg");
    drag_onto(&mut h, from, compare, |_| {});
    settle(&mut h, &mut persist, 7.0);
    let mut compared = h
        .state_mut()
        .run("compare.active", json!({}), Origin::User)
        .unwrap();
    if compared == json!({ "queued": true }) {
        h.settle();
        compared = h
            .state()
            .history()
            .into_iter()
            .rev()
            .find(|r| r.id == "compare.active")
            .expect("compare.active ran")
            .result
            .unwrap();
    }
    let photos: Vec<&str> = compared["images"]
        .as_array()
        .unwrap_or_else(|| panic!("{compared}"))
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(photos.contains(&a.as_str()), "{compared}");
    assert_eq!(main(&h), browsing, "the main view's cursor stays");

    // A restart from the layout as saved: both panels come back, each on
    // its quick collection.
    let saved = h.state().app().handle().layout().to_json();
    drop((h, persist));
    let (mut h, mut persist) = open(&config, &workspace, &path);
    h.state_mut()
        .set_layout(egui_pop::model::layout::Layout::from_json(&saved).unwrap());
    settle(&mut h, &mut persist, 8.0);
    assert_eq!(
        quick_views(h.state().app().handle()),
        vec![
            (picks.clone(), "picks".into()),
            (maybe.clone(), "maybe".into())
        ]
    );
    assert_eq!(shown(&h, &picks), "collection:picks");
    assert_eq!(shown(&h, &maybe), "collection:maybe");
    assert_eq!(
        title(&h, &picks),
        "Picks",
        "the title comes back with the layout"
    );
    // Renamed: its panel's tab follows.
    run(
        &mut h,
        "library.quick.rename",
        json!({ "id": "maybe", "name": "Perhaps" }),
    );
    settle(&mut h, &mut persist, 9.0);
    assert_eq!(title(&h, &maybe), "Perhaps");
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}

/// The filter the library registry publishes, as the bus's
/// `library.filter` step left it.
fn published_filter(h: &Harness<'_, Shell>) -> String {
    let libraries = h
        .state()
        .app()
        .handle()
        .service(&egui_pop::views::library::plugin::LIBRARIES)
        .expect("the library plugin");
    libraries.describe()["libraries"][helios_gui::library::LIBRARY]["filter"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The search field's suggestions and chips (`docs/design/library-search.md`,
/// G4): typing `ca` offers the `camera:` key, Down and Enter take it; a key
/// term typed in full becomes a chip in words, its rule in the filter;
/// Escape closes the list first, the chips kept; Backspace twice in the
/// empty field removes the chip and the rule.
#[test]
fn the_search_suggests_keys_and_shows_its_rules_as_chips() {
    let root = temp("chips");
    let path = vault_file(&root);
    let (mut h, mut persist) = window(&root, &path);
    let type_in = |h: &mut Harness<'static, Shell>, text: &str| {
        h.input_mut().events.push(egui::Event::Text(text.into()));
        h.run();
        h.run();
    };
    h.get_by_label("Search images").click();
    h.run();

    // `ca`: the camera key offered, in a list named "Suggestions".
    type_in(&mut h, "ca");
    h.get_by_label("Suggestions");
    h.get_by_label("Camera — camera:");
    h.key_press(Key::ArrowDown);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert!(h.query_by_label("Suggestions").is_none(), "taken: closed");

    // Its value and a space: the term is a chip, the rule filters.
    type_in(&mut h, "R5 ");
    settle(&mut h, &mut persist, 2.0);
    h.get_by_label("camera is R5");
    assert_eq!(h.state().library().filter().text, "camera:R5");
    assert_eq!(grid(&h), ["c.nef"]);

    // Escape closes the list first, the text and the chip kept.
    type_in(&mut h, "ca");
    h.get_by_label("Suggestions");
    h.key_press(Key::Escape);
    h.run();
    assert!(h.query_by_label("Suggestions").is_none());
    h.get_by_label("camera is R5");
    assert_eq!(h.state().library().filter().text, "camera:R5 ca");
    // The bus's `library.filter` saw the whole search, the chip's rule
    // included, as its registry publishes it.
    assert_eq!(published_filter(&h), "camera:R5 ca");
    assert!(
        h.get_by_label("Search images").is_focused(),
        "the list closed, the field keeps the keyboard"
    );

    // Escape again clears the words typed, the field still focused; a
    // third leaves the field.
    h.key_press(Key::Escape);
    h.run();
    settle(&mut h, &mut persist, 3.0);
    assert_eq!(h.state().library().filter().text, "camera:R5");
    assert_eq!(published_filter(&h), "camera:R5");
    assert!(
        h.get_by_label("Search images").is_focused(),
        "cleared, kept"
    );
    h.key_press(Key::Escape);
    h.run();
    assert!(!h.get_by_label("Search images").is_focused(), "left");
    h.get_by_label("camera is R5");
    h.get_by_label("Search images").click();
    h.run();

    // Backspace in the empty field selects the chip, again removes it
    // and its rule.
    h.key_press(Key::Backspace);
    h.run();
    h.get_by_label("camera is R5");
    h.key_press(Key::Backspace);
    settle(&mut h, &mut persist, 4.0);
    assert!(h.query_by_label("camera is R5").is_none());
    assert!(h.state().library().filter().text.is_empty());
    assert_eq!(grid(&h), ["a.jpg", "b.jpg", "c.nef"]);

    // A chip again, then a filter set by the bus's command: its whole
    // text is typed, the chips dropped.
    type_in(&mut h, "camera:R5 ");
    settle(&mut h, &mut persist, 5.0);
    h.get_by_label("camera is R5");
    run(
        &mut h,
        "library.filter",
        json!({ "library": helios_gui::library::LIBRARY, "filter": { "text": "camera:\"Z 8\"" } }),
    );
    settle(&mut h, &mut persist, 6.0);
    assert!(h.query_by_label("camera is R5").is_none());
    assert_eq!(h.state().library().filter().text, "camera:\"Z 8\"");
    assert_eq!(grid(&h), ["a.jpg", "b.jpg"]);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&root);
}
