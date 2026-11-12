//! Importing from the desktop, always within reach: File > Import… and
//! Ctrl+Shift+I, the empty library's "Import photos…", files dropped on
//! the window, all through the import window; for scripts, without an import source the import dialog asks where the
//! photos go and may remember it for the workspace; darktable sidecars
//! come in as versions, their reports in the Log panel. The library is
//! called the Library, never the vault.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use helios_gui::dialogs::Level;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_library::import::{Options, import};
use helios_shell::commands::Origin;
use helios_storage::Card as Volume;
use helios_template::workspace::Files;
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
    // The window opens and reads the drop: a spinner turns meanwhile.
    h.run_steps(3);
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
    card_window_with(dir, known, |_| {})
}

/// [`card_window`], the shell set up by `setup` first.
fn card_window_with(
    dir: &Path,
    known: Option<helios_gui::import_scan::Known>,
    setup: impl FnOnce(&mut Shell),
) -> Harness<'static, Shell> {
    card_window_at(dir, known, setup, vec2(1280.0, 800.0))
}

/// [`card_window_with`], in a main window of `size`.
fn card_window_at(
    dir: &Path,
    known: Option<helios_gui::import_scan::Known>,
    setup: impl FnOnce(&mut Shell),
    size: egui::Vec2,
) -> Harness<'static, Shell> {
    let card = dir.join("EOS_DIGITAL");
    let mut shell = Shell::new(helios_panels(), None);
    setup(&mut shell);
    shell.watch_cards(Arc::new(vec![Volume {
        label: "EOS_DIGITAL".into(),
        root: card,
        capacity: Some(64_000_000_000),
    }]));
    if let Some(known) = known {
        shell.set_import_known(known);
    }
    let mut h = headless::harness(shell, size);
    // The cards are polled on a thread of their own: the first list
    // comes within a few frames.
    let deadline = Instant::now() + Duration::from_secs(10);
    while h.state().import_cards().is_empty() {
        assert!(Instant::now() < deadline, "no card listed in 10 s");
        std::thread::sleep(Duration::from_millis(20));
        h.run();
    }
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::I);
    // The card is read on a thread of its own too; a spinner turns
    // meanwhile, so frames are stepped, not run to rest.
    h.step();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !h.state().import_read() {
        assert!(Instant::now() < deadline, "the card not read in 20 s");
        std::thread::sleep(Duration::from_millis(20));
        h.step();
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
    let known = helios_gui::import_scan::known_of(vault);
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
        [json!({
            "paths": [dcim.join("IMG_0001.JPG")],
            "copy": true,
            "layout": "day",
            "rename": "{stem}",
            "duplicates": "skip"
        })]
    );
    assert!(!h.state().import_window_open());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_import_grid_shows_a_card_by_day_greys_what_the_library_holds_and_pairs_raw_and_jpeg() {
    let dir = temp("grid");
    let (_, known) = fake_card(&dir);
    let mut h = card_window(&dir, Some(known));
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
    // The agenda of the file browser groups them by day.
    assert!(
        h.state_mut()
            .import_browser_mut()
            .unwrap()
            .set_mode("agenda")
    );
    h.run_steps(3);
    let _ = h.get_by_label("2026-11-02");
    let _ = h.get_by_label("2026-11-03");
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
    // Ctrl and a click take a photo out of the selection.
    h.get_by_label("IMG_0001.JPG")
        .click_modifiers(Modifiers::COMMAND);
    h.run_steps(3);
    assert_eq!(h.state().import_counts().selected, 1);
    h.get_by_label("Import 1").click();
    h.run_steps(3);
    // The pair's two files, neither the photo unticked nor the one the
    // Library holds.
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({
            "paths": [dcim.join("IMG_0003.CR2"), dcim.join("IMG_0003.JPG")],
            "copy": true,
            "layout": "day",
            "rename": "{stem}",
            "duplicates": "skip"
        })]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_import_choices_show_live_examples_and_reach_the_import() {
    let dir = temp("choices");
    let (dcim, known) = fake_card(&dir);
    let mut h = card_window(&dir, Some(known));
    let kept = dir.join("choices.json");
    h.state_mut().remember_import_choices_in(kept.clone());
    h.run_steps(3);
    // A card is copied, with a note on why.
    assert!(h.state().import_copies());
    let _ = h.get_by_label_contains("the card is formatted later");
    // The examples come from the first photo ticked, of 2 November.
    let _ = h.get_by_label("e.g. 2026/2026-11-02/");
    let _ = h.get_by_label("e.g. IMG_0001.JPG");
    // Into's layout and the Name change them.
    h.get_by_label("By month").click();
    h.run_steps(3);
    let _ = h.get_by_label("e.g. 2026/2026-11/");
    h.get_by_label("Date and number").click();
    h.run_steps(3);
    let _ = h.get_by_label("e.g. 20261102-0001.JPG");
    h.get_by_label("Keep").click();
    h.run_steps(3);
    assert_eq!(
        h.state().import_choices().duplicates.as_str(),
        "keep",
        "Keep clicked"
    );
    h.get_by_label("Read darktable edits").click();
    h.run_steps(3);
    // Added where they are, the photos are neither filed nor renamed:
    // Into and Name go away.
    h.get_by_label("Add where they are").click();
    h.run_steps(3);
    assert!(!h.state().import_copies());
    assert!(h.query_by_label("Into").is_none());
    assert!(h.query_by_label_contains("e.g.").is_none());
    // Back to a copy: Import passes every choice.
    h.get_by_label("Copy into the Library").click();
    h.run_steps(3);
    let _ = h.get_by_label("Into");
    h.get_by_label("Import 2").click();
    h.run_steps(3);
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({
            "paths": [
                dcim.join("IMG_0001.JPG"),
                dcim.join("IMG_0003.CR2"),
                dcim.join("IMG_0003.JPG")
            ],
            "copy": true,
            "layout": "month",
            "rename": "{date}-{seq}",
            "duplicates": "keep",
            "darktable": false
        })]
    );
    // They are kept as the last used.
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&kept).unwrap()).unwrap();
    assert_eq!(
        (
            &saved["layout"],
            &saved["rename"],
            &saved["duplicates"],
            &saved["darktable"]
        ),
        (
            &json!("month"),
            &json!("{date}-{seq}"),
            &json!("keep"),
            &json!(false)
        )
    );
    let mut again = Shell::new(helios_panels(), None);
    again.remember_import_choices_in(kept);
    assert_eq!(again.import_choices(), h.state().import_choices());
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
fn files_and_folders_dropped_on_the_window_open_the_import_window_on_them() {
    let dir = temp("drop");
    let (dcim, _) = fake_card(&dir);
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    // A folder: it is the source, every photo of it ticked.
    drop_on(&mut h, &dcim);
    assert!(h.state().import_window_open());
    assert!(h.state_mut().take_imports().is_empty(), "nothing yet");
    assert_eq!(h.state().import_ticked(), std::slice::from_ref(&dcim));
    read_through(&mut h);
    assert_eq!(h.state().import_counts().selected, 3);
    // A file: its folder is the source, that file alone ticked.
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    drop_on(&mut h, &dcim.join("IMG_0002.JPG"));
    assert_eq!(h.state().import_ticked(), std::slice::from_ref(&dcim));
    read_through(&mut h);
    let ticked: Vec<String> = h
        .state()
        .import_cells()
        .into_iter()
        .filter(|c| c.4)
        .map(|c| c.0)
        .collect();
    assert_eq!(ticked, ["IMG_0002.JPG"]);
    let _ = h.get_by_label("Import 1");
    // A folder's photos are added where they are, by default.
    assert!(!h.state().import_copies());
    h.get_by_label("Import 1").click();
    h.run_steps(3);
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({
            "paths": [dcim.join("IMG_0002.JPG")],
            "duplicates": "skip"
        })]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_files_run_with_ask_still_asks_with_the_form() {
    let dir = temp("ask");
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    // A script's import without an import source nor `copy`: the form.
    run(
        &mut h,
        "library.importFiles",
        json!({ "paths": [dir], "ask": true }),
    );
    let form = dialog(&mut h);
    assert_eq!(form["command"], json!("library.importFiles"), "{form}");
    assert_eq!(field_keys(&form), ["copy", "darktable"]);
    answer(&mut h, json!({ "copy": false }));
    assert_eq!(h.state_mut().take_imports(), [json!({ "paths": [dir] })]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Steps frames until the import window's sources are read through.
fn read_through(h: &mut Harness<'static, Shell>) {
    let deadline = Instant::now() + Duration::from_secs(20);
    h.step();
    while !h.state().import_read() {
        assert!(Instant::now() < deadline, "the sources not read in 20 s");
        std::thread::sleep(Duration::from_millis(20));
        h.step();
    }
    for _ in 0..3 {
        h.run();
    }
}

#[test]
fn the_footer_totals_the_photos_ticked() {
    let dir = temp("totals");
    let (dcim, known) = fake_card(&dir);
    let h = card_window(&dir, Some(known));
    let size = |names: &[&str]| -> u64 {
        names
            .iter()
            .map(|n| std::fs::metadata(dcim.join(n)).unwrap().len())
            .sum()
    };
    let counts = h.state().import_counts();
    assert_eq!(counts.selected, 2);
    assert_eq!(
        counts.selected_bytes,
        size(&["IMG_0001.JPG", "IMG_0003.CR2", "IMG_0003.JPG"])
    );
    let _ = h.get_by_label_contains("2 photos, ");
    let _ = h.get_by_label("Import 2");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn presets_are_named_choices_saved_applied_and_deleted() {
    let dir = temp("presets");
    let (_, known) = fake_card(&dir);
    let mut h = card_window(&dir, Some(known));
    h.state_mut()
        .remember_import_choices_in(dir.join("lib/import-choices.json"));
    h.run_steps(3);
    let _ = h.get_by_value("Default");
    // A monthly preset, saved from the menu under a name asked in place.
    h.get_by_label("By month").click();
    h.run_steps(3);
    h.get_by_value("Last used").click();
    h.run_steps(3);
    h.get_by_label("Save as preset…").click();
    h.run_steps(3);
    // The name field takes the focus (the Library's search is another).
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .find(egui_kittest::Node::is_focused)
        .expect("the preset name field focused")
        .type_text("Monthly");
    h.run_steps(3);
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert_eq!(h.state().import_presets(), ["Monthly"]);
    let kept: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("lib/import-presets.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(kept["Monthly"]["layout"], json!("month"), "{kept}");
    // Default sets the Choices column back; the preset sets it again.
    h.get_by_value("Monthly").click();
    h.run_steps(3);
    h.get_by_label("Default").click();
    h.run_steps(3);
    assert_eq!(
        h.state().import_choices(),
        helios_gui::shell::Choices::default()
    );
    h.state_mut().apply_import_preset(Some("Monthly"));
    h.run_steps(3);
    let _ = h.get_by_label("e.g. 2026/2026-11/");
    // Copy or add for folders is part of a preset; a card is still
    // copied.
    let mut add = h.state().import_choices();
    add.copy_folders = false;
    h.state_mut().set_import_choices(add);
    h.state_mut().save_import_preset("Monthly");
    h.state_mut().apply_import_preset(Some("Monthly"));
    assert!(h.state().import_copies(), "a card is copied");
    // Deleted from the menu: the choices stay as they are.
    h.run_steps(3);
    h.get_by_value("Monthly").click();
    h.run_steps(3);
    h.get_by_label("Delete preset").click();
    h.run_steps(3);
    assert!(h.state().import_presets().is_empty());
    assert_eq!(h.state().import_choices().layout.as_str(), "month");
    let mut again = Shell::new(helios_panels(), None);
    again.remember_import_choices_in(dir.join("lib/import-choices.json"));
    assert!(again.import_presets().is_empty());
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

/// The host's `library.import`, as `main.rs` registers it: the import
/// run as a job into `vault`, its lines to the shell, its changes one
/// undo step; `done` once it ended.
fn host_import<S: helios_vault::Store + Send + 'static>(
    shell: &mut Shell,
    vault: &Arc<Vault<S>>,
    done: &Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    let history = shell.library_history();
    let v = vault.clone();
    history.set_apply(Arc::new(move |changes, forward| {
        if forward {
            v.replay(changes)
        } else {
            v.revert(changes)
        }
    }));
    let (v, done, lines) = (vault.clone(), done.clone(), shell.import_sink());
    let jobs = shell.jobs();
    shell
        .register(
            helios_shell::commands::CommandSpec::new("library.import", "Import", "Library"),
            move |args, _| {
                let options = helios_gui::importers::options(args)?;
                let sources: Vec<PathBuf> = args["paths"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect();
                let (v, jobs, done, lines, history, asked) = (
                    v.clone(),
                    jobs.clone(),
                    done.clone(),
                    lines.clone(),
                    history.clone(),
                    args.clone(),
                );
                std::thread::spawn(move || {
                    let mut emit = |line: &Value| {
                        let _ = lines.send((asked.clone(), line.clone()));
                    };
                    let end = match import(&v, &jobs, &sources, &options, 1, &mut emit) {
                        Ok(mut report) => {
                            let label = helios_gui::library_history::import_label(
                                report.added + report.located,
                            );
                            history.record(label, std::mem::take(&mut report.changes), 1.0);
                            json!({ "ended": report })
                        }
                        Err(e) => json!({ "failed": e }),
                    };
                    let _ = lines.send((asked, end));
                    done.store(true, Ordering::Relaxed);
                });
                Ok(json!({ "queued": true }))
            },
        )
        .unwrap();
}

/// The whole path, as the desktop runs it: the window opened on a card,
/// Import, the host's job (`library.import`, as `main.rs` registers it),
/// then the photos in the Library, selected, and Ctrl+Z taking the import
/// back as one step.
#[test]
fn a_card_imported_from_the_window_lands_in_the_library_selected_as_one_undo_step() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = temp("end-to-end");
    let dcim = dir.join("EOS_DIGITAL/DCIM/100CANON");
    std::fs::create_dir_all(&dcim).unwrap();
    put(&dcim.join("IMG_0001.JPG"), Ok(&jpeg("a")), 2);
    put(&dcim.join("IMG_0002.JPG"), Ok(&jpeg("c")), 3);
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let done = Arc::new(AtomicBool::new(false));
    let mut h = card_window_with(&dir, None, |shell| host_import(shell, &vault, &done));
    // Copies go to a folder of the test's.
    let mut choices = h.state().import_choices();
    choices.into = Some(dir.join("photos"));
    h.state_mut().set_import_choices(choices);
    h.run_steps(3);
    h.get_by_label("Import 2").click();
    h.run_steps(3);
    assert!(!h.state().import_window_open());
    // The host takes the import asked for and runs it as a job.
    for args in h.state_mut().take_imports() {
        h.state_mut()
            .run("library.import", args, Origin::User)
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done.load(Ordering::Relaxed) {
        assert!(Instant::now() < deadline, "the import not done in 30 s");
        std::thread::sleep(Duration::from_millis(20));
    }
    // The desktop fills the Library again once the job is done.
    let ids: Vec<String> = vault
        .images()
        .unwrap()
        .into_iter()
        .map(|i| i.id.hex())
        .collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    h.state_mut().set_library(
        ids.iter()
            .map(|id| helios_gui::library::Entry {
                id: id.clone(),
                name: format!("{id}.jpg"),
                ..helios_gui::library::Entry::default()
            })
            .collect(),
    );
    h.run_steps(3);
    let mut selected = h.state().library().selection();
    selected.sort();
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(selected, expected, "the photos imported are selected");
    assert!(dir.join("photos/2026/2026-11-02/IMG_0001.JPG").is_file());
    // One undo step: Ctrl+Z with the Library shown takes it back.
    let scope = helios_gui::library_history::LIBRARY;
    let line = egui_pop::api::history::line(h.state().app().handle(), &scope).unwrap();
    assert_eq!(line.undo.len(), 1, "{line:?}");
    assert_eq!(line.undo[0].label.text, "Import 2 photos");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert!(vault.images().unwrap().is_empty(), "the entries are gone");
    assert!(dcim.join("IMG_0001.JPG").is_file(), "the card untouched");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The import window keeps the size it opened at, frame after frame, in
/// a 960x640 main window: on a card, on a folder and with no source. Its
/// content lays out in the room it is given and never asks for more.
#[test]
fn the_import_window_keeps_its_size() {
    const TITLE: &str = "Import photos";
    let size = vec2(960.0, 640.0);
    let dir = temp("stable");
    let (dcim, known) = fake_card(&dir);
    // A card, ticked, with its grid.
    let mut h = card_window_at(&dir, Some(known), |_| {}, size);
    headless::assert_window_stable(&mut h, TITLE, 120);
    let rect = headless::window_rect(&h, TITLE).unwrap();
    assert!(
        rect.width() <= 0.9 * size.x && rect.height() <= 0.9 * size.y,
        "smaller than the main window: {rect:?}"
    );
    // A folder.
    let mut h = headless::harness(Shell::new(helios_panels(), None), size);
    h.run();
    h.state_mut()
        .run(
            "library.importWindow",
            json!({ "paths": [dcim] }),
            Origin::User,
        )
        .unwrap();
    read_through(&mut h);
    headless::assert_window_stable(&mut h, TITLE, 120);
    // No source, from the frame it opens: it grew a few points a frame.
    let mut h = headless::harness(Shell::new(helios_panels(), None), size);
    h.run();
    h.state_mut()
        .run("library.importWindow", json!(null), Origin::User)
        .unwrap();
    headless::assert_window_stable(&mut h, TITLE, 120);
    // egui-pop's folder chooser, opened from it.
    h.get_by_label("Choose a folder…").click();
    h.run_steps(3);
    headless::assert_window_stable(&mut h, "Choose a folder…", 60);
    let _ = std::fs::remove_dir_all(&dir);
}

const NO_LIBRARY: &str =
    "No library is open: start Helios with one (--vault FILE) to import photos";

/// No toast nor note says a command is missing (the host's
/// `library.import` without a library, as it was).
fn no_missing_command(shell: &Shell) {
    assert!(
        !shell.notes.iter().any(|n| n.contains("no command")),
        "{:?}",
        shell.notes
    );
}

/// Why the shell refused `id`: a queued command's error comes in the
/// frame it applies, as a warning.
fn refusal(h: &mut Harness<'static, Shell>, id: &str, args: Value) -> String {
    h.state_mut().notes.clear();
    h.state_mut().run(id, args, Origin::User).unwrap();
    h.run_steps(3);
    let notes = &h.state().notes;
    notes
        .last()
        .unwrap_or_else(|| panic!("{id} was not refused"))
        .clone()
}

/// Helios started without a library (no `--vault`): the window opens on a
/// card and says why nothing can be imported, its Import button off;
/// nothing is queued for a command the host does not have.
#[test]
fn without_a_library_the_import_window_says_so_and_its_import_button_is_off() {
    let dir = temp("no-library");
    let dcim = dir.join("EOS_DIGITAL/DCIM/100CANON");
    std::fs::create_dir_all(&dcim).unwrap();
    put(&dcim.join("IMG_0001.JPG"), Ok(&jpeg("a")), 2);
    let mut h = card_window_with(&dir, None, |shell| shell.set_library_open(false));
    assert!(h.state().import_window_open());
    let _ = h.get_by_label(NO_LIBRARY);
    let import = h.get_by_label("Import 1");
    assert!(import.accesskit_node().is_disabled());
    import.click();
    h.run_steps(3);
    assert!(h.state_mut().take_imports().is_empty());
    assert!(h.state().import_window_open(), "the window stays");
    no_missing_command(h.state());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A folder dropped without a library: the window opens on it and says
/// so; the scripted imports refuse with the same reason.
#[test]
fn without_a_library_a_drop_and_every_import_command_say_why() {
    let dir = temp("no-library-drop");
    put(&dir.join("a.jpg"), Ok(&jpeg("a")), 2);
    put(&dir.join("c.jpg"), Ok(&jpeg("c")), 2);
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library_open(false);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    drop_on(&mut h, &dir);
    assert!(h.state().import_window_open());
    read_through(&mut h);
    let _ = h.get_by_label(NO_LIBRARY);
    assert!(h.get_by_label("Import 2").accesskit_node().is_disabled());
    for (id, args) in [
        ("library.importFiles", json!({ "paths": [dir] })),
        ("library.importSource", json!({ "path": dir })),
    ] {
        assert_eq!(refusal(&mut h, id, args), NO_LIBRARY, "{id}");
    }
    assert!(h.state_mut().take_imports().is_empty());
    no_missing_command(h.state());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The commands the import path dispatches: the shell's own are
/// registered; the one it hands imports to (`library.import`) is the
/// host's, registered only with a library open, so a shell whose host
/// has none never queues one.
#[test]
fn the_commands_the_import_path_dispatches_are_registered() {
    let shell = Shell::new(helios_panels(), None);
    for id in [
        "library.importWindow",
        "library.importFiles",
        "library.importSource",
        "library.importWith",
    ] {
        assert!(shell.has_command(id), "{id} is not registered");
    }
    assert!(
        !shell.has_command(helios_gui::shell::IMPORT_COMMAND),
        "the host's command, not the shell's"
    );
    assert!(shell.library_open(), "open unless the host says otherwise");
}

/// A library that could not be opened: imports refuse with its reason.
#[test]
fn a_library_that_could_not_be_opened_gives_its_reason() {
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_no_library(Some("library.db: read-only file system".into()));
    assert!(!shell.library_open());
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let e = refusal(&mut h, "library.importFiles", json!({ "paths": ["/x"] }));
    assert_eq!(e, "No library is open: library.db: read-only file system");
    assert!(h.state_mut().take_imports().is_empty());
    no_missing_command(h.state());
}

/// Helios started without `--vault`: the default library, `library.db`
/// in Helios's data directory (here a temporary `XDG_DATA_HOME`, never the
/// user's), is made on the first launch; a folder of two JPEGs imported
/// from the import window lands in it, and it keeps them once reopened.
#[test]
fn without_vault_a_folder_imports_into_the_default_library() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = temp("default-library");
    let data = dir.join("data");
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    put(&photos.join("a.jpg"), Ok(&jpeg("a")), 2);
    put(&photos.join("c.jpg"), Ok(&jpeg("c")), 3);
    // Every variable a platform reads for its data directory points into
    // the test's folder.
    let env = |name: &str| match name {
        "XDG_DATA_HOME" | "LOCALAPPDATA" => Some(data.to_string_lossy().into_owned()),
        "HOME" => Some(dir.join("home").to_string_lossy().into_owned()),
        _ => None,
    };
    let args = ["helios-gui".to_string()];
    let file = helios_gui::platform::library_file_with(&args, env)
        .unwrap()
        .expect("a default library");
    assert!(file.starts_with(&dir), "{file:?}");
    assert!(file.ends_with(helios_gui::platform::LIBRARY_FILE));
    assert!(!file.exists(), "not there before the first launch");
    let store = helios_gui::platform::open_library(&file).unwrap();
    assert!(file.is_file(), "made on the first launch");
    let vault = Arc::new(Vault::new(store));
    let done = Arc::new(AtomicBool::new(false));
    let mut shell = Shell::new(helios_panels(), None);
    host_import(&mut shell, &vault, &done);
    assert!(shell.library_open());
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h.state_mut()
        .run(
            "library.importWindow",
            json!({ "paths": [photos] }),
            Origin::User,
        )
        .unwrap();
    read_through(&mut h);
    h.get_by_label("Import 2").click();
    h.run_steps(3);
    assert!(!h.state().import_window_open());
    for args in h.state_mut().take_imports() {
        h.state_mut()
            .run("library.import", args, Origin::User)
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done.load(Ordering::Relaxed) {
        assert!(Instant::now() < deadline, "the import not done in 30 s");
        std::thread::sleep(Duration::from_millis(20));
    }
    no_missing_command(h.state());
    assert_eq!(vault.images().unwrap().len(), 2);
    drop(h);
    drop(vault);
    // The next launch finds them in the same library.
    let again = Vault::new(helios_gui::platform::open_library(&file).unwrap());
    assert_eq!(again.images().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The photos the import window's file browser lists, by name.
fn listed(h: &Harness<'static, Shell>) -> Vec<String> {
    h.state()
        .import_browser()
        .map(|b| b.entries().iter().map(|e| e.name.clone()).collect())
        .unwrap_or_default()
}

/// The names of the photos selected in the import window's browser.
fn selected(h: &Harness<'static, Shell>) -> Vec<String> {
    let b = h.state().import_browser().unwrap();
    b.entries()
        .iter()
        .filter(|e| b.selection().contains(&e.path))
        .map(|e| e.name.clone())
        .collect()
}

#[test]
fn the_import_contents_are_a_file_browser_that_lists_selects_and_imports() {
    let dir = temp("browser");
    let (dcim, known) = fake_card(&dir);
    let mut h = card_window(&dir, Some(known));
    // A pair is one entry, by its raw; every photo selected, as ticked.
    assert_eq!(
        listed(&h),
        [
            "IMG_0001.JPG",
            "IMG_0002.JPG",
            "IMG_0003.CR2 + IMG_0003.JPG"
        ]
    );
    assert_eq!(selected(&h).len(), 3);
    let browser = h.state().import_browser().unwrap();
    assert_eq!(browser.mode(), "thumbnails", "an import opens on them");
    assert_eq!(
        browser.modes(),
        [
            "details",
            "thumbnails",
            "filmstrip",
            "columns",
            "carousel",
            "agenda"
        ]
    );
    // The browser is the import window's, shared with its window: let go.
    drop(browser);
    // A click selects that photo alone: one to import.
    h.get_by_label("IMG_0001.JPG").click();
    h.run_steps(3);
    assert_eq!(selected(&h), ["IMG_0001.JPG"]);
    assert_eq!(h.state().import_counts().selected, 1);
    // Ctrl+A selects them all again; the one the Library holds stays out
    // of the import while New only is on.
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(3);
    assert_eq!(selected(&h).len(), 3);
    assert_eq!(h.state().import_counts().selected, 2);
    // Select: none, then the focused photo's day.
    h.get_by_label("none").click();
    h.run_steps(3);
    assert!(selected(&h).is_empty());
    h.get_by_label("IMG_0003.CR2 + IMG_0003.JPG · RAW+JPG")
        .click();
    h.run_steps(3);
    h.get_by_label("day").click();
    h.run_steps(3);
    assert_eq!(selected(&h), ["IMG_0003.CR2 + IMG_0003.JPG"]);
    h.get_by_label("Import 1").click();
    h.run_steps(3);
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({
            "paths": [dcim.join("IMG_0003.CR2"), dcim.join("IMG_0003.JPG")],
            "copy": true,
            "layout": "day",
            "rename": "{stem}",
            "duplicates": "skip"
        })]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_mode_of_the_import_browser_shows_the_photos_and_the_mode_is_kept_for_the_window() {
    let dir = temp("modes");
    let (_, known) = fake_card(&dir);
    let mut h = card_window(&dir, Some(known));
    // The keyboard goes to the entries with a click; Ctrl and a digit
    // switch modes, in the switcher's order.
    h.get_by_label("IMG_0001.JPG").click();
    h.run_steps(3);
    for (n, mode) in [
        (Key::Num1, "details"),
        (Key::Num2, "thumbnails"),
        (Key::Num3, "filmstrip"),
        (Key::Num4, "columns"),
        (Key::Num5, "carousel"),
        (Key::Num6, "agenda"),
    ] {
        h.key_press_modifiers(Modifiers::COMMAND, n);
        h.run_steps(3);
        assert_eq!(h.state().import_browser().unwrap().mode(), mode);
        // The focused photo is there, said with its name, in every mode.
        assert!(
            h.query_all_by_label_contains("IMG_0001.JPG").count() > 0,
            "{mode}"
        );
        // The selection holds across modes.
        assert_eq!(selected(&h), ["IMG_0001.JPG"], "{mode}");
    }
    // The last mode is kept for the import window.
    assert_eq!(
        egui_pop::api::files::view_for(h.state().app().handle(), "helios.import").as_deref(),
        Some("agenda")
    );
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::I);
    h.run_steps(3);
    assert_eq!(h.state().import_browser().unwrap().mode(), "agenda");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_dropped_on_the_import_browser_opens_as_its_source() {
    let dir = temp("drop-browser");
    let (_, known) = fake_card(&dir);
    let other = dir.join("Trip");
    std::fs::create_dir_all(&other).unwrap();
    put(&other.join("TRIP_1.JPG"), Ok(&jpeg("c")), 4);
    let mut h = card_window(&dir, Some(known));
    let card = dir.join("EOS_DIGITAL");
    assert_eq!(h.state().import_ticked(), std::slice::from_ref(&card));
    // Dragged over the photos: the browser says what a drop does.
    let over = h.get_by_label("IMG_0001.JPG").rect().center();
    h.hover_at(over);
    h.input_mut().hovered_files.push(egui::HoveredFile {
        path: Some(other.clone()),
        ..Default::default()
    });
    h.run_steps(3);
    let hover = h.state().import_browser().unwrap().drop_hover().cloned();
    assert!(hover.as_ref().is_some_and(|(_, taken)| *taken), "{hover:?}");
    assert!(h.query_by_label_contains("Drop into").is_some());
    // Dropped: the folder is the import's source.
    h.input_mut().hovered_files.clear();
    h.input_mut()
        .dropped_files
        .push(Arc::new(Dropped(other.clone())));
    h.run_steps(3);
    assert_eq!(h.state().import_ticked(), std::slice::from_ref(&other));
    read_through(&mut h);
    assert_eq!(listed(&h), ["TRIP_1.JPG"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_import_browser_asks_helios_thumbnails_for_the_photos_in_view() {
    let dir = temp("thumbs");
    let (dcim, known) = fake_card(&dir);
    let mut h = card_window(&dir, Some(known));
    // The scan's previews (the JPEGs' own) answer the browser's requests.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !h.state().import_thumbnail_ready(&dcim.join("IMG_0001.JPG")) {
        assert!(Instant::now() < deadline, "no thumbnail in 10 s");
        std::thread::sleep(Duration::from_millis(20));
        h.step();
    }
    let mut on_screen = h.state().import_thumbnails_on_screen();
    on_screen.sort();
    assert_eq!(
        on_screen,
        [
            dcim.join("IMG_0001.JPG"),
            dcim.join("IMG_0002.JPG"),
            dcim.join("IMG_0003.CR2")
        ],
        "every photo in view is asked, as visible"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_offline_folder_lists_what_the_image_map_recorded_with_cached_thumbnails() {
    let dir = temp("offline");
    // A photo of the Library, seen on a disk now unplugged: its folder is
    // gone, the image map and the thumbnail cache remember it.
    let library = dir.join("library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::copy(jpeg("a"), library.join("a.jpg")).unwrap();
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let id = vault.import_file(&library.join("a.jpg"), 0).unwrap();
    let gone = dir.join("Unplugged/DCIM");
    let mut imported = helios_vault::Sighting::seen(gone.join("A.JPG"), 1_000, 50, 60, "import");
    imported.image = Some(id);
    let other = helios_vault::Sighting::seen(gone.join("B.JPG"), 2_000, 50, 60, "import");
    vault.record_listed(&[imported, other]).unwrap();
    let cache = Arc::new(helios_thumbs::ThumbCache::open(&dir.join("thumbs"), 1 << 30).unwrap());
    let thumb = helios_thumbs::Rgb8 {
        width: 64,
        height: 48,
        pixels: vec![120; 64 * 48 * 3],
    };
    cache
        .put(
            &helios_thumbs::Key::preview(id.hex()),
            helios_thumbs::SMALL,
            &thumb,
        )
        .unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_import_recorded(helios_gui::import_scan::recorded_of(vault));
    shell.set_import_thumbnail_cache(cache);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    run(
        &mut h,
        "library.importWindow",
        json!({ "source": gone.display().to_string() }),
    );
    read_through(&mut h);
    assert_eq!(listed(&h), ["A.JPG", "B.JPG"]);
    // Both offline, the one in the Library said so; neither imported.
    let _ = h.get_by_label("A.JPG · offline · in your Library");
    let _ = h.get_by_label("B.JPG · offline");
    assert_eq!(h.state().import_counts().selected, 0);
    // The Library's photo shows its cached thumbnail.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !h.state().import_thumbnail_ready(&gone.join("A.JPG")) {
        assert!(Instant::now() < deadline, "no cached thumbnail in 10 s");
        std::thread::sleep(Duration::from_millis(20));
        h.step();
    }
    assert!(!h.state().import_thumbnail_ready(&gone.join("B.JPG")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Writes `n` JPEGs of `width`×`height` in `folder`, each its own
/// content, without an Exif thumbnail: each glance decodes one whole.
fn photos(folder: &Path, n: usize, width: u32, height: u32) {
    std::fs::create_dir_all(folder).unwrap();
    for i in 0..n {
        let img = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x as usize + i) as u8, (y * 3) as u8, (i * 4) as u8])
        });
        img.save(folder.join(format!("IMG_{i:04}.JPG"))).unwrap();
    }
}

/// Steps frames (`pause` between them) until every cell in view has its
/// thumbnail: the frames and the time it took.
fn fill_in_view(
    h: &mut Harness<'static, Shell>,
    pause: Duration,
    within: Duration,
) -> (usize, Duration) {
    let started = Instant::now();
    let mut frames = 0;
    loop {
        let on_screen = h.state().import_thumbnails_on_screen();
        if !on_screen.is_empty()
            && on_screen
                .iter()
                .all(|p| h.state().import_thumbnail_ready(p))
        {
            return (frames, started.elapsed());
        }
        assert!(
            started.elapsed() < within,
            "the cells in view not filled in {within:?} ({frames} frames)"
        );
        std::thread::sleep(pause);
        h.step();
        frames += 1;
    }
}

#[test]
fn a_cold_folder_fills_the_cells_in_view_on_several_threads_and_opened_again_reads_nothing() {
    const N: usize = 96;
    let dir = temp("cold");
    let folder = dir.join("Shoot");
    photos(&folder, N, 1600, 1200);
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let source = json!({ "source": folder.display().to_string() });
    h.state_mut()
        .run("library.importWindow", source.clone(), Origin::User)
        .unwrap();
    let (frames, took) = fill_in_view(&mut h, Duration::from_millis(5), Duration::from_mins(1));
    let in_view = h.state().import_thumbnails_on_screen().len();
    let glances = h.state().import_glances();
    eprintln!(
        "import thumbnails, cold: {in_view} cells in view of {N} filled in {frames} frames, {} ms; {glances:?}",
        took.as_millis()
    );
    // Read on several threads at once, the cells in view before the rest
    // of the folder.
    assert!(glances.peak >= 2, "the glances were serial: {glances:?}");
    assert!(
        glances.read < N,
        "the whole folder read before the cells in view: {glances:?}"
    );
    read_through(&mut h);
    // Opened again: every glance from the cache, nothing read.
    h.state_mut()
        .run("library.importWindow", source, Origin::User)
        .unwrap();
    // The command runs on the next frame: the window cleared, then filled.
    h.step();
    let (frames, took) = fill_in_view(&mut h, Duration::from_millis(5), Duration::from_secs(5));
    eprintln!(
        "import thumbnails, opened again: filled in {frames} frames, {} ms",
        took.as_millis()
    );
    read_through(&mut h);
    let glances = h.state().import_glances();
    assert_eq!(
        glances.read, 0,
        "opened again, files were read: {glances:?}"
    );
    assert_eq!(glances.cached, N);
    assert!(
        frames <= 20,
        "opened again, {frames} frames to fill the cells in view"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Steps frames until the sources are read through, a few frames more,
/// and asserts on each that every cell in view among `cached` already has
/// its thumbnail: a cached thumbnail is drawn with its cell, on the frame
/// that first draws it. Gives the cells checked in view.
fn painted_with_their_cells(
    h: &mut Harness<'static, Shell>,
    cached: impl Fn(&Path) -> bool,
) -> std::collections::HashSet<PathBuf> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = std::collections::HashSet::new();
    let mut after = 0;
    while after < 5 {
        assert!(Instant::now() < deadline, "the sources not read in 20 s");
        h.step();
        for p in h.state().import_thumbnails_on_screen() {
            if cached(&p) {
                assert!(
                    h.state().import_thumbnail_ready(&p),
                    "{} drawn without its cached thumbnail",
                    p.display()
                );
                seen.insert(p);
            }
        }
        if h.state().import_read() && !seen.is_empty() {
            after += 1;
        } else {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    seen
}

#[test]
fn cached_glances_paint_with_their_cells_when_opened_again_and_scrolled() {
    const N: usize = 160;
    let dir = temp("painted");
    let folder = dir.join("Shoot");
    photos(&folder, N, 320, 240);
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let source = json!({ "source": folder.display().to_string() });
    h.state_mut()
        .run("library.importWindow", source.clone(), Origin::User)
        .unwrap();
    read_through(&mut h);
    // Closed, then opened again: every glance is in the RAM cache.
    h.get_by_label("Cancel").click();
    h.step();
    h.step();
    assert!(!h.state().import_window_open());
    h.state_mut()
        .run("library.importWindow", source, Origin::User)
        .unwrap();
    let seen = painted_with_their_cells(&mut h, |_| true);
    assert_eq!(h.state().import_glances().read, 0, "files read again");
    // Scrolled down to cells not drawn yet: theirs show with them too.
    h.get_by_label("IMG_0000.JPG").click();
    h.step();
    for _ in 0..6 {
        h.key_press(Key::PageDown);
        h.step();
        for p in h.state().import_thumbnails_on_screen() {
            assert!(
                h.state().import_thumbnail_ready(&p),
                "scrolled to, {} drawn without its cached thumbnail",
                p.display()
            );
        }
    }
    let now: std::collections::HashSet<PathBuf> = h
        .state()
        .import_thumbnails_on_screen()
        .into_iter()
        .collect();
    assert!(
        !now.is_subset(&seen),
        "the scroll did not reach new cells: {} of {N}",
        seen.len()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_disk_cached_thumbnail_paints_with_its_cell() {
    let dir = temp("painted-disk");
    let library = dir.join("library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::copy(jpeg("a"), library.join("a.jpg")).unwrap();
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let id = vault.import_file(&library.join("a.jpg"), 0).unwrap();
    let gone = dir.join("Unplugged/DCIM");
    let mut imported = helios_vault::Sighting::seen(gone.join("A.JPG"), 1_000, 50, 60, "import");
    imported.image = Some(id);
    vault.record_listed(&[imported]).unwrap();
    let cache = Arc::new(helios_thumbs::ThumbCache::open(&dir.join("thumbs"), 1 << 30).unwrap());
    let thumb = helios_thumbs::Rgb8 {
        width: 64,
        height: 48,
        pixels: vec![120; 64 * 48 * 3],
    };
    cache
        .put(
            &helios_thumbs::Key::preview(id.hex()),
            helios_thumbs::SMALL,
            &thumb,
        )
        .unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_import_recorded(helios_gui::import_scan::recorded_of(vault));
    shell.set_import_thumbnail_cache(cache);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h.state_mut()
        .run(
            "library.importWindow",
            json!({ "source": gone.display().to_string() }),
            Origin::User,
        )
        .unwrap();
    let seen = painted_with_their_cells(&mut h, |_| true);
    assert!(seen.contains(&gone.join("A.JPG")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The import window's status bar names what is read while its sources
/// are read.
#[test]
fn the_status_bar_names_what_is_read() {
    let dir = temp("reading");
    let folder = dir.join("Shoot");
    photos(&folder, 48, 1600, 1200);
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    h.state_mut()
        .run(
            "library.importWindow",
            json!({ "source": folder.display().to_string() }),
            Origin::User,
        )
        .unwrap();
    // The command runs on the next frame: until then, nothing is read.
    h.step();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut named = Vec::new();
    while !h.state().import_read() {
        assert!(Instant::now() < deadline, "the sources not read in 20 s");
        h.step();
        if let Some(node) = h.query_by_label_contains("Reading") {
            let n = node.accesskit_node();
            let label = n.label().or_else(|| n.value()).unwrap_or_default();
            named.push(label);
        }
    }
    assert!(!named.is_empty(), "no status bar line while reading");
    for label in &named {
        assert!(
            label.starts_with("Reading ") && (label.contains("Shoot") || label.contains(".JPG")),
            "the status bar does not name what it reads: {label:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The text a pass painted, each run of text apart.
fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for s in shapes {
        walk(&s.shape, &mut out);
    }
    out
}

/// The application windows the main window's last pass declared, egui's
/// deferred viewports, as eframe would open them.
fn declared(h: &Harness<'static, Shell>) -> Vec<egui::ViewportId> {
    h.output()
        .viewport_output
        .iter()
        .filter(|(v, o)| **v != egui::ViewportId::ROOT && o.viewport_ui_cb.is_some())
        .map(|(v, _)| *v)
        .collect()
}

/// A deferred window's own pass, as eframe runs it apart from the main
/// window's (`close`: its close button pressed): the text it painted.
fn window_pass(
    h: &mut Harness<'static, Shell>,
    window: egui::ViewportId,
    close: bool,
) -> Vec<String> {
    use egui::{Pos2, RawInput, Rect, ViewportEvent, ViewportId, ViewportInfo};
    let cb = h.output().viewport_output[&window]
        .viewport_ui_cb
        .clone()
        .expect("the window is declared by the main window's last pass");
    let rect = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
    let mut info = ViewportInfo {
        inner_rect: Some(rect),
        focused: Some(true),
        ..Default::default()
    };
    if close {
        info.events.push(ViewportEvent::Close);
    }
    let mut input = RawInput {
        viewport_id: window,
        screen_rect: Some(rect),
        time: Some(h.ctx.input(|i| i.time) + 1.0 / 60.0),
        focused: true,
        ..Default::default()
    };
    input.viewports.insert(window, info);
    input
        .viewports
        .insert(ViewportId::ROOT, ViewportInfo::default());
    let mut out = h.ctx.run_ui(input, |ui| cb(ui));
    // No painter for it: its textures are uploaded nowhere (as egui-pop's
    // `MultiWindow` does).
    out.textures_delta.clear();
    painted_text(&out.shapes)
}

/// The import focus in an OS window of its own (docs/design/windows.md,
/// slice 4), with real viewports as on the desktop: File > Import… opens
/// it as an application window (egui's deferred viewport, drawn in its
/// own pass), not inside the main window; its status bar says what is
/// read; agents name it (`window: "import"`); its close button closes the
/// import focus and the main window stays.
#[test]
fn import_opens_in_its_own_window_reads_in_its_status_bar_and_closes_alone() {
    let dir = temp("own-window");
    let folder = dir.join("Shoot");
    photos(&folder, 48, 1600, 1200);
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    h.run();
    assert!(declared(&h).is_empty(), "no window but the main one");
    h.state_mut()
        .run(
            "library.importWindow",
            json!({ "source": folder.display().to_string() }),
            Origin::User,
        )
        .unwrap();
    h.run_steps(3);
    assert!(h.state().import_window_open());
    // Its own window, declared by the main window's pass.
    let windows = declared(&h);
    assert_eq!(windows.len(), 1, "the import window's own: {windows:?}");
    let window = windows[0];
    let app = h.state().app().handle().clone();
    let listed: Vec<_> = app
        .windows()
        .into_iter()
        .map(|w| (w.id.to_string(), w.title))
        .collect();
    assert_eq!(
        listed,
        [
            ("main".to_owned(), app.windows()[0].title.clone()),
            ("import".to_owned(), "Import photos".to_owned())
        ]
    );
    assert_eq!(
        app.window_at(window).map(|w| w.to_string()).as_deref(),
        Some("import")
    );
    // Nothing of it inside the main window.
    assert!(headless::window_rect(&h, "Import photos").is_none());
    assert!(h.query_by_label("Choose a folder…").is_none());
    // Agents name it: a snapshot of its window, refused once it is gone.
    assert!(app.debug_snapshot_in(Some("import")).is_ok());
    // Its pass draws it; its status bar names what is read, while read.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut reading = Vec::new();
    let mut drawn = Vec::new();
    while !h.state().import_read() {
        assert!(Instant::now() < deadline, "the sources not read in 20 s");
        drawn = window_pass(&mut h, window, false);
        reading.extend(drawn.iter().filter(|t| t.starts_with("Reading ")).cloned());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        h.state().import_window_viewport(),
        Some(window),
        "drawn in its own window"
    );
    assert!(drawn.iter().any(|t| t == "Choose a folder…"), "{drawn:?}");
    assert!(!reading.is_empty(), "no status bar line while reading");
    for line in &reading {
        assert!(
            line.contains("Shoot") || line.contains(".JPG"),
            "the status bar does not name what it reads: {line:?}"
        );
    }
    // Read through: the status bar says nothing more.
    let drawn = window_pass(&mut h, window, false);
    assert!(
        !drawn.iter().any(|t| t.starts_with("Reading ")),
        "{drawn:?}"
    );
    // Its close button: the import focus closes, the main window stays.
    window_pass(&mut h, window, true);
    h.run_steps(3);
    assert!(!h.state().import_window_open());
    assert!(declared(&h).is_empty(), "its window is gone");
    let listed: Vec<_> = app
        .windows()
        .into_iter()
        .map(|w| w.id.to_string())
        .collect();
    assert_eq!(listed, ["main"]);
    assert!(app.debug_snapshot_in(Some("import")).is_err());
    // The main window drawn as before: the Library panel's and the
    // Filmstrip's "Import photos…".
    assert_eq!(h.query_all_by_label("Import photos…").count(), 2);
    // File > Import… opens it again, in a window of its own.
    h.state_mut()
        .run("library.importWindow", json!(null), Origin::User)
        .unwrap();
    h.run_steps(3);
    assert_eq!(declared(&h), [window]);
    let _ = std::fs::remove_dir_all(&dir);
}
