//! Importing from the desktop, always within reach: File > Import… and
//! Ctrl+Shift+I, the empty library's "Import photos…", files dropped on
//! the window; without an import source the import dialog asks where the
//! photos go and may remember it for the workspace; darktable sidecars
//! come in as versions, their reports in the Log panel. The library is
//! called the Library, never the vault.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_gui::dialogs::Level;
use helios_gui::volumes::Volume;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_library::import::{Options, import};
use helios_shell::commands::Origin;
use helios_template::workspace::Files;
use helios_template::{ImportDesc, LibraryDesc};
use helios_vault::{ImageId, MemoryStore, Vault};
use serde_json::{Value, json};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-gui-import-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(h: &mut Harness<'static, Shell>, id: &str, args: Value) {
    h.state_mut().run(id, args, Origin::User).unwrap();
    for _ in 0..3 {
        h.run();
    }
}

fn dialog(h: &mut Harness<'static, Shell>) -> Value {
    h.state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap()
}

fn answer(h: &mut Harness<'static, Shell>, values: Value) {
    run(h, "dialog.answer", json!({ "values": values }));
}

fn field_keys(form: &Value) -> Vec<String> {
    form["fields"]
        .as_array()
        .unwrap_or_else(|| panic!("not a form: {form}"))
        .iter()
        .map(|f| f["key"].as_str().unwrap().to_string())
        .collect()
}

/// A file dropped on the window, as the platform hands it over.
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

fn drop_on(h: &mut Harness<'static, Shell>, path: &Path) {
    h.input_mut()
        .dropped_files
        .push(Arc::new(Dropped(path.to_path_buf())));
    for _ in 0..3 {
        h.run();
    }
}

#[test]
fn file_import_is_in_the_file_menu_on_ctrl_shift_i_and_opens_the_import_window() {
    let mut shell = Shell::new(helios_panels(), None);
    let items = shell.menu_items(egui_pop::api::app::MENU_FILE.as_str());
    let import = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "library.importWindow")
        .unwrap_or_else(|| panic!("no File > Import…: {items}"))
        .clone();
    assert_eq!(import["title"], json!("Import photos…"), "{import}");
    assert!(import["keys"].is_string(), "{import}");
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::I);
    for _ in 0..3 {
        h.run();
    }
    assert!(h.state().import_window_open());
    // The command without its arguments still asks with the form: no
    // import source, no workspace, so where the photos go and the
    // darktable edits, nothing to remember it in.
    run(&mut h, "library.importSource", json!(null));
    let form = dialog(&mut h);
    assert_eq!(form["command"], json!("library.importSource"), "{form}");
    assert_eq!(field_keys(&form), ["path", "copy", "darktable"]);
    answer(&mut h, json!({ "path": "/photos/trip" }));
    // Referenced in place, darktable edits read (the host's default).
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({ "path": "/photos/trip" })]
    );
}

#[test]
fn an_empty_library_offers_import_photos_without_an_import_source() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    assert!(h.state().import_source().is_none());
    // The Library panel and the Filmstrip, both empty.
    let buttons = h.query_all_by_label("Import photos…").count();
    assert_eq!(buttons, 2);
    h.query_all_by_label("Import photos…")
        .next()
        .unwrap()
        .click();
    for _ in 0..3 {
        h.run();
    }
    assert!(h.state().import_window_open());
}

/// A tiny JPEG of the fixtures (`a`, `b` or `c`, each its own content).
fn jpeg(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../helios-plugin/tests/fixtures/rated-photos")
        .join(format!("{name}.jpg"))
}

/// Writes `from` (a fixture) or `bytes` at `to`, last modified at noon of
/// `day` of November 2026, local time.
fn put(to: &Path, from: Result<&Path, &[u8]>, day: u32) {
    use chrono::TimeZone as _;
    match from {
        Ok(f) => std::fs::copy(f, to).map(|_| ()).unwrap(),
        Err(bytes) => std::fs::write(to, bytes).unwrap(),
    }
    let at = chrono::Local
        .with_ymd_and_hms(2026, 11, day, 12, 0, 0)
        .single()
        .unwrap();
    let time = std::time::UNIX_EPOCH + Duration::from_secs(at.timestamp() as u64);
    std::fs::File::options()
        .write(true)
        .open(to)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

/// A shell watching one card, `EOS_DIGITAL` under `dir`, with the import
/// window open on it once it is read through.
fn card_window(
    dir: &Path,
    known: Option<helios_gui::import_scan::Known>,
) -> Harness<'static, Shell> {
    let card = dir.join("EOS_DIGITAL");
    let mut shell = Shell::new(helios_panels(), None);
    shell.watch_cards(Arc::new(vec![Volume {
        label: "EOS_DIGITAL".into(),
        root: card,
        capacity: Some(64_000_000_000),
    }]));
    if let Some(known) = known {
        shell.set_import_known(known);
    }
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    // The cards are polled on a thread of their own: the first list
    // comes within a few frames.
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.state().import_cards().is_empty() {
        assert!(Instant::now() < deadline, "no card listed in 10 s");
        std::thread::sleep(Duration::from_millis(20));
        h.run();
    }
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::I);
    h.run();
    // The card is read on a thread of its own too.
    let deadline = Instant::now() + Duration::from_secs(20);
    while !h.state().import_read() {
        assert!(Instant::now() < deadline, "the card not read in 20 s");
        std::thread::sleep(Duration::from_millis(20));
        h.run();
    }
    for _ in 0..3 {
        h.run();
    }
    h
}

/// A card holding two JPEGs on 2 November, one of them in the Library
/// already, and a RAW+JPEG pair on 3 November.
fn fake_card(dir: &Path) -> (PathBuf, helios_gui::import_scan::Known) {
    let dcim = dir.join("EOS_DIGITAL/DCIM/100CANON");
    std::fs::create_dir_all(&dcim).unwrap();
    put(&dcim.join("IMG_0001.JPG"), Ok(&jpeg("a")), 2);
    put(&dcim.join("IMG_0002.JPG"), Ok(&jpeg("b")), 2);
    put(&dcim.join("IMG_0003.CR2"), Err(&b"not a real raw"[..]), 3);
    put(&dcim.join("IMG_0003.JPG"), Ok(&jpeg("c")), 3);
    // IMG_0002 is in the Library, from another folder.
    let library = dir.join("library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::copy(jpeg("b"), library.join("b.jpg")).unwrap();
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    vault.import_file(&library.join("b.jpg"), 0).unwrap();
    let known: helios_gui::import_scan::Known = Arc::new(move || {
        vault
            .images()
            .unwrap()
            .into_iter()
            .map(|i| (i.size, i.partial))
            .collect()
    });
    (dcim, known)
}

#[test]
fn the_import_window_lists_a_card_first_ticks_it_and_imports_it() {
    let dir = temp("card");
    let card = dir.join("EOS_DIGITAL");
    let dcim = card.join("DCIM/100CANON");
    std::fs::create_dir_all(&dcim).unwrap();
    put(&dcim.join("IMG_0001.JPG"), Ok(&jpeg("a")), 2);
    let mut h = card_window(&dir, None);
    assert!(h.state().import_window_open());
    // Listed by its label and capacity, and ticked first.
    let _ = h.get_by_label("EOS_DIGITAL · 64 GB");
    assert_eq!(h.state().import_ticked(), std::slice::from_ref(&card));
    h.get_by_label("Import 1").click();
    for _ in 0..3 {
        h.run();
    }
    // A card's photos are copied into the Library; the window closes at
    // once.
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({ "paths": [dcim.join("IMG_0001.JPG")], "copy": true })]
    );
    assert!(!h.state().import_window_open());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_import_grid_shows_a_card_by_day_greys_what_the_library_holds_and_pairs_raw_and_jpeg() {
    let dir = temp("grid");
    let (_, known) = fake_card(&dir);
    let h = card_window(&dir, Some(known));
    let cells = h.state().import_cells();
    assert_eq!(cells.len(), 3, "{cells:?}");
    let day = |d: u32| chrono::NaiveDate::from_ymd_opt(2026, 11, d);
    let cell = |name: &str| {
        cells
            .iter()
            .find(|c| c.0 == name)
            .unwrap_or_else(|| panic!("{name}: {cells:?}"))
    };
    // Each day with its count.
    assert_eq!(cell("IMG_0001.JPG").1, day(2));
    assert_eq!(cell("IMG_0003.CR2 + IMG_0003.JPG").1, day(3));
    let _ = h.get_by_label("2026-11-02 · 2");
    let _ = h.get_by_label("2026-11-03 · 1");
    // The one the Library holds is greyed, and left out.
    assert!(cell("IMG_0002.JPG").2);
    assert!(!cell("IMG_0001.JPG").2);
    let _ = h.get_by_label("IMG_0002.JPG · in your Library");
    // A RAW and its JPEG are one cell.
    assert!(cell("IMG_0003.CR2 + IMG_0003.JPG").3);
    let _ = h.get_by_label("IMG_0003.CR2 + IMG_0003.JPG · RAW+JPG");
    let counts = h.state().import_counts();
    assert_eq!((counts.photos, counts.new, counts.selected), (3, 2, 2));
    let _ = h.get_by_label_contains("3 photos, 2 new");
    let _ = h.get_by_label("Import 2");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_passes_exactly_the_photos_ticked() {
    let dir = temp("ticked");
    let (dcim, known) = fake_card(&dir);
    let mut h = card_window(&dir, Some(known));
    // A click unticks a photo.
    h.get_by_label("IMG_0001.JPG").click();
    for _ in 0..3 {
        h.run();
    }
    assert_eq!(h.state().import_counts().selected, 1);
    h.get_by_label("Import 1").click();
    for _ in 0..3 {
        h.run();
    }
    // The pair's two files, neither the photo unticked nor the one the
    // Library holds.
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({
            "paths": [dcim.join("IMG_0003.CR2"), dcim.join("IMG_0003.JPG")],
            "copy": true
        })]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_interface_says_library_never_vault() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    assert!(h.query_all_by_label("Library").count() > 0);
    assert_eq!(h.query_all_by_label_contains("Vault").count(), 0);
    assert_eq!(h.query_all_by_label_contains("vault").count(), 0);
    let spec = helios_panels()
        .into_iter()
        .find(|p| p.id.as_str() == "helios.vault")
        .unwrap();
    assert_eq!(spec.title, "Library");
    for c in h.state().specs() {
        assert_ne!(c.category, "Vault", "{}", c.id);
        assert!(
            !c.title.to_lowercase().contains("vault"),
            "{}: {}",
            c.id,
            c.title
        );
    }
}

#[test]
fn a_folder_dropped_on_the_window_is_imported() {
    let dir = temp("drop");
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    // Without an import source, where the photos go is asked first.
    drop_on(&mut h, &dir);
    let form = dialog(&mut h);
    assert_eq!(form["command"], json!("library.importFiles"), "{form}");
    assert_eq!(field_keys(&form), ["copy", "darktable"]);
    assert!(h.state_mut().take_imports().is_empty());
    answer(&mut h, json!({ "copy": false }));
    assert_eq!(h.state_mut().take_imports(), [json!({ "paths": [dir] })]);
    // With one, the drop imports at once, as it says.
    h.state_mut().apply_template_library(Some(&LibraryDesc {
        import: Some(ImportDesc {
            source: "folder".into(),
            placement: "reference".into(),
            rename: None,
            duplicates: "skip".into(),
            suggest_stacks: false,
            darktable: true,
        }),
        ..LibraryDesc::default()
    }));
    drop_on(&mut h, &dir);
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({ "paths": [dir], "duplicates": "skip" })]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn desktop() -> helios_template::Catalogue {
    let shell = Shell::new(helios_panels(), None);
    templates::catalogue(&shell.template_panels(), shell.specs())
}

#[test]
fn the_import_dialog_asks_where_photos_go_and_remembers_it_for_the_workspace() {
    let root = temp("remember");
    let config = root.join("config");
    let created = templates::create_workspace(
        &templates::find("helios.portrait-retouch").unwrap(),
        &desktop(),
        &root.join("work"),
        &config,
    )
    .unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&created.workspace)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &created.workspace).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    assert!(h.state().import_source().is_none());
    run(&mut h, "library.importSource", json!(null));
    let form = dialog(&mut h);
    assert_eq!(field_keys(&form), ["path", "copy", "darktable", "remember"]);
    answer(
        &mut h,
        json!({ "path": "/photos/trip", "copy": true, "remember": true }),
    );
    // Copied, and now the workspace's import source.
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({ "path": "/photos/trip", "copy": true, "duplicates": "skip" })]
    );
    let source = h.state().import_source().cloned().unwrap();
    assert_eq!(
        (
            source.source.as_str(),
            source.placement.as_str(),
            source.darktable
        ),
        ("folder", "copy", true)
    );
    let kept = Files::new(&config, &created.workspace)
        .load()
        .unwrap()
        .bundle
        .template
        .library
        .unwrap()
        .import
        .unwrap();
    assert_eq!(kept.placement, "copy");
    // The next import does not ask where; darktable edits stay a choice.
    run(&mut h, "library.importSource", json!(null));
    let form = dialog(&mut h);
    assert_eq!(field_keys(&form), ["path", "darktable"]);
    answer(
        &mut h,
        json!({ "path": "/photos/next", "darktable": false }),
    );
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({
            "path": "/photos/next",
            "copy": true,
            "duplicates": "skip",
            "darktable": false
        })]
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A darktable sidecar whose history holds a module Helios does not have
/// (reported as not applied); `name` is darktable's name for the version.
fn sidecar(name: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:darktable="http://darktable.sf.net/" darktable:history_end="1" darktable:version_name="{name}">
   <darktable:history>
    <rdf:Seq>
     <rdf:li darktable:num="0" darktable:operation="frobnicate" darktable:enabled="1" darktable:modversion="1" darktable:params="00" darktable:multi_name="" darktable:multi_priority="0"/>
    </rdf:Seq>
   </darktable:history>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#
    )
}

#[test]
fn a_folder_with_darktable_sidecars_comes_in_with_versions_and_its_report_logged() {
    let dir = temp("darktable");
    let edited = dir.join("edited.png");
    image::RgbImage::from_fn(16, 16, |x, y| {
        image::Rgb([(x * 9) as u8, (y * 13) as u8, 60])
    })
    .save(&edited)
    .unwrap();
    std::fs::write(dir.join("edited.png.xmp"), sidecar("base")).unwrap();
    std::fs::write(dir.join("edited_01.png.xmp"), sidecar("bw")).unwrap();
    let vault = Vault::new(MemoryStore::default());
    let shell = Shell::new(helios_panels(), None);
    // As the desktop's `library.import` runs it: each line to the shell,
    // then the end.
    let args = json!({ "path": dir });
    let sink = shell.import_sink();
    let jobs = helios_core::jobs::Jobs::new(Arc::new(helios_core::Bus::new()));
    let options = Options {
        darktable: true,
        signals: false,
        ..Options::default()
    };
    let mut lines = Vec::new();
    let report = import(
        &vault,
        &jobs,
        std::slice::from_ref(&dir),
        &options,
        1,
        &mut |line: &Value| {
            lines.push(line.clone());
            sink.send((args.clone(), line.clone())).unwrap();
        },
    )
    .unwrap();
    sink.send((args.clone(), json!({ "ended": report })))
        .unwrap();
    // The base and its duplicate: two versions of the one photo.
    let id = ImageId::from_hex(lines[0]["id"].as_str().unwrap()).unwrap();
    assert_eq!(vault.versions(&id).unwrap().len(), 2, "{lines:?}");
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    // Each sidecar's report in the Log panel, then the import's end.
    let log = h.state().log();
    let entries = log.entries();
    let reports: Vec<&str> = entries
        .iter()
        .filter(|e| e.level == Level::Warning)
        .map(|e| e.message.english.as_str())
        .collect();
    assert_eq!(reports.len(), 3, "{reports:?}");
    assert!(reports[0].contains("edited.png.xmp"), "{reports:?}");
    assert!(reports[0].contains("not applied"), "{reports:?}");
    assert!(reports[1].contains("edited_01.png.xmp"), "{reports:?}");
    assert!(
        reports[2].starts_with("1 photos imported, 2 darktable edits"),
        "{reports:?}"
    );
    drop(log);
    // A short notice says so, pointing at the Log panel.
    let notice = h.state().notices().last().unwrap().clone();
    assert_eq!(notice.level, Level::Warning);
    assert!(notice.text.contains("Log panel"), "{}", notice.text);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_import_is_one_undo_step_and_ctrl_z_on_the_library_takes_it_back() {
    let dir = temp("undo");
    for (n, name) in ["a.png", "b.png"].iter().enumerate() {
        image::RgbImage::from_fn(16, 16, |x, y| {
            image::Rgb([(x * 9) as u8, (y * 13) as u8, 40 * n as u8])
        })
        .save(dir.join(name))
        .unwrap();
    }
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let shell = Shell::new(helios_panels(), None);
    // As the desktop wires it: undo and redo move the vault's rows.
    let history = shell.library_history();
    {
        let vault = vault.clone();
        history.set_apply(Arc::new(move |changes, forward| {
            if forward {
                vault.replay(changes)
            } else {
                vault.revert(changes)
            }
        }));
    }
    let jobs = helios_core::jobs::Jobs::new(Arc::new(helios_core::Bus::new()));
    let mut report = import(
        &vault,
        &jobs,
        std::slice::from_ref(&dir),
        &Options::default(),
        1,
        &mut |_: &Value| {},
    )
    .unwrap();
    assert_eq!(report.added, 2);
    let label = helios_gui::library_history::import_label(report.added + report.located);
    history.record(label, std::mem::take(&mut report.changes), 1.0);
    let line = history.line();
    assert_eq!(line.undo.len(), 1);
    assert_eq!(line.undo[0].label.text, "Import 2 photos");

    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    // The step is in the history the interface reads, as the library's.
    let scope = helios_gui::library_history::LIBRARY;
    let seen = egui_pop::api::history::line(h.state().app().handle(), &scope)
        .expect("the library's scope");
    assert_eq!(seen.undo.len(), 1, "{seen:?}");
    assert_eq!(seen.undo[0].label.text, "Import 2 photos");
    // Ctrl+Z with the Library shown takes the import back; the files stay.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    for _ in 0..3 {
        h.run();
    }
    assert!(vault.images().unwrap().is_empty(), "the entries are gone");
    assert!(dir.join("a.png").is_file() && dir.join("b.png").is_file());
    let line = history.line();
    assert_eq!((line.undo.len(), line.redo.len()), (0, 1));
    let _ = std::fs::remove_dir_all(&dir);
}
