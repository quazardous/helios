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
use egui_pop::api::windows::WindowId;
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
        [json!({ "path": "/photos/trip", "origin": "user" })]
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
            "duplicates": "skip",
            "origin": "user"
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
            "duplicates": "skip",
            "origin": "user"
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
            "darktable": false,
            "origin": "user"
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
            "duplicates": "skip",
            "origin": "user"
        })]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A library that keeps a camera's default look.
struct WithCameraLooks;

impl helios_gui::shell::LibraryFolder for WithCameraLooks {
    fn get(&self) -> Result<Option<PathBuf>, String> {
        Ok(None)
    }

    fn set(&self, _: &Path) -> Result<(), String> {
        Ok(())
    }

    fn has_camera_looks(&self) -> bool {
        true
    }
}

#[test]
fn the_camera_s_default_look_is_a_choice_on_by_default_when_the_library_keeps_one() {
    let dir = temp("camera-look");
    let (dcim, _) = fake_card(&dir);
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_import_library_folder(Arc::new(WithCameraLooks));
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    drop_on(&mut h, &dcim.join("IMG_0002.JPG"));
    read_through(&mut h);
    // On by default; unticked, the import leaves the looks off.
    assert!(h.state().import_choices().camera_look);
    h.get_by_role_and_label(
        egui::accesskit::Role::CheckBox,
        "Apply the camera's default look",
    )
    .click();
    h.run_steps(3);
    assert!(!h.state().import_choices().camera_look);
    h.get_by_label("Import 1").click();
    h.run_steps(3);
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({
            "paths": [dcim.join("IMG_0002.JPG")],
            "duplicates": "skip",
            "camera_look": false,
            "origin": "user"
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
    assert_eq!(
        h.state_mut().take_imports(),
        [json!({ "paths": [dir], "origin": "user" })]
    );
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
        [json!({
            "path": "/photos/trip",
            "copy": true,
            "duplicates": "skip",
            "origin": "user"
        })]
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
            "darktable": false,
            "origin": "user"
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

/// Where the backend has no windows of its own (kittest, the web), the
/// import window is a window of the main one, centred and closable, as
/// before it had an OS window of its own; not egui's embedding of an
/// application window, which sits at the top left over the menus with a
/// collapse arrow and no close button.
#[test]
fn the_embedded_import_window_is_centred_and_closes() {
    const TITLE: &str = "Import photos";
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    h.state_mut()
        .run("library.importWindow", json!(null), Origin::User)
        .unwrap();
    h.run_steps(3);
    let rect = headless::window_rect(&h, TITLE).expect("drawn in the main window");
    let centre = h.ctx.content_rect().center();
    assert!(
        (rect.center() - centre).length() < 2.0,
        "centred: {rect:?} around {centre:?}"
    );
    // A window of the main one, not an application window egui embeds.
    let app = h.state().app().handle().clone();
    assert_eq!(app.windows().len(), 1, "the main window only");
    // Its close button closes the import focus.
    h.get_by_role_and_label(egui::accesskit::Role::Window, TITLE)
        .get_by_role_and_label(egui::accesskit::Role::Button, "Close window")
        .click();
    h.run_steps(3);
    assert!(!h.state().import_window_open());
    assert!(headless::window_rect(&h, TITLE).is_none());
    // File > Import… opens it again, centred.
    h.state_mut()
        .run("library.importWindow", json!(null), Origin::User)
        .unwrap();
    h.run_steps(3);
    let again = headless::window_rect(&h, TITLE).expect("open again");
    assert!((again.center() - centre).length() < 2.0, "{again:?}");
}

/// The import window's body fits the window at any size: only the grid
/// scrolls (the Choices column would on its own, taller than the window),
/// never the body as a whole; the grid takes the width it has, a column
/// more as the window widens; its last row, scrolled to, ends above the
/// footer; the backup line, last of the Choices column, is in view. Opened
/// in main windows of two sizes, on a folder of 120 photos.
#[test]
fn the_import_body_fits_the_window_and_only_its_grid_scrolls() {
    use egui::accesskit::Role;
    use egui_kittest::kittest::by;
    const TITLE: &str = "Import photos";
    const N: usize = 120;
    let dir = temp("fits");
    let folder = dir.join("Shoot");
    photos(&folder, N, 32, 24);
    let mut columns = Vec::new();
    for size in [vec2(1280.0, 800.0), vec2(1920.0, 1200.0)] {
        let mut h = headless::harness(Shell::new(helios_panels(), None), size);
        h.run();
        h.state_mut()
            .run(
                "library.importWindow",
                json!({ "source": folder.display().to_string() }),
                Origin::User,
            )
            .unwrap();
        read_through(&mut h);
        h.run_steps(5);
        let window = headless::window_rect(&h, TITLE).expect("the import window");
        let entries = h
            .get_by_role_and_label(Role::ListBox, "Entries, thumbnails")
            .rect();
        let footer = h.get_by_label("Cancel").rect();
        // One scroll bar in the window: the grid's, at its right edge.
        let bars: Vec<egui::Rect> = h
            .query_all(by().role(Role::ScrollBar))
            .map(|n| n.rect())
            .filter(|r| window.contains_rect(*r))
            .collect();
        assert_eq!(
            bars.len(),
            1,
            "{size:?}: only the grid scrolls; bars {bars:?} in {window:?}, the grid {entries:?}"
        );
        assert!(
            (bars[0].right() - entries.right()).abs() < 12.0
                && bars[0].top() >= entries.top() - 1.0
                && bars[0].bottom() <= entries.bottom() + 1.0,
            "{size:?}: the bar {:?} is not the grid's {entries:?}",
            bars[0]
        );
        // The grid ends above the footer.
        assert!(
            entries.bottom() <= footer.top(),
            "{size:?}: the grid {entries:?} runs under the footer {footer:?}"
        );
        // The backup line, last of the Choices column, shows above it.
        let storage = h
            .query_all_by_label("Storage…")
            .map(|n| n.rect())
            .find(|r| window.contains_rect(*r))
            .expect("the backup line's Storage… button");
        assert!(
            storage.bottom() <= footer.top() && storage.left() > entries.right(),
            "{size:?}: the backup line {storage:?} is not shown above the footer {footer:?}"
        );
        // The cells of the first row: the grid's columns.
        let cell = |h: &Harness<'static, Shell>, name: &str| {
            h.query_all_by_label(name)
                .map(|n| n.rect())
                .find(|r| entries.intersects(*r))
        };
        let first = cell(&h, "IMG_0000.JPG").expect("the first cell");
        let row = (0..N)
            .filter_map(|i| cell(&h, &format!("IMG_{i:04}.JPG")))
            .filter(|r| (r.top() - first.top()).abs() < 1.0)
            .count();
        columns.push(row);
        // The last row, scrolled to, shows whole above the footer.
        h.get_by_label("IMG_0000.JPG").click();
        h.run_steps(2);
        h.key_press(Key::End);
        h.run_steps(5);
        let last = cell(&h, &format!("IMG_{:04}.JPG", N - 1)).expect("the last cell in view");
        let footer = h.get_by_label("Cancel").rect();
        assert!(
            last.bottom() <= entries.bottom() + 1.0 && last.bottom() <= footer.top(),
            "{size:?}: the last cell {last:?} is cut, the grid {entries:?}, the footer {footer:?}"
        );
        // Still no other scroll bar, scrolled.
        let bars = h
            .query_all(by().role(Role::ScrollBar))
            .filter(|n| window.contains_rect(n.rect()))
            .count();
        assert_eq!(bars, 1, "{size:?}: scrolled, only the grid scrolls");
    }
    assert!(
        columns[1] > columns[0],
        "the grid gains columns as the window widens: {columns:?}"
    );
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
            "duplicates": "skip",
            "origin": "user"
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

/// Steps frames until every cell in view has its thumbnail: the frames
/// it took. Paced by the glances, not by the clock: it fails only when no
/// glance came for `stalled` while cells in view still wait for theirs.
fn fill_in_view(h: &mut Harness<'static, Shell>, stalled: Duration) -> usize {
    let mut frames = 0;
    let mut seen = 0;
    let mut since = Instant::now();
    loop {
        let on_screen = h.state().import_thumbnails_on_screen();
        if !on_screen.is_empty()
            && on_screen
                .iter()
                .all(|p| h.state().import_thumbnail_ready(p))
        {
            return frames;
        }
        let glances = h.state().import_glances();
        let progress = glances.read + glances.cached + h.state().import_counts().photos;
        if progress != seen {
            seen = progress;
            since = Instant::now();
        }
        assert!(
            since.elapsed() < stalled,
            "no glance in {stalled:?} and the cells in view not filled ({frames} frames, {glances:?})"
        );
        std::thread::sleep(Duration::from_millis(2));
        h.step();
        frames += 1;
    }
}

#[test]
fn a_cold_folder_fills_the_cells_in_view_on_several_threads_and_opened_again_reads_nothing() {
    // More photos than the window shows, small enough to write and read
    // quickly on a loaded machine; each one decoded whole (no Exif
    // thumbnail), so the threads overlap.
    const N: usize = 64;
    let dir = temp("cold");
    let folder = dir.join("Shoot");
    photos(&folder, N, 640, 480);
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let source = json!({ "source": folder.display().to_string() });
    h.state_mut()
        .run("library.importWindow", source.clone(), Origin::User)
        .unwrap();
    let frames = fill_in_view(&mut h, Duration::from_secs(30));
    let in_view = h.state().import_thumbnails_on_screen();
    eprintln!(
        "import thumbnails, cold: {} cells in view of {N} filled in {frames} frames; {:?}",
        in_view.len(),
        h.state().import_glances()
    );
    assert!(in_view.len() < N, "the whole folder is in view");
    read_through(&mut h);
    let glances = h.state().import_glances();
    // Read on several threads at once.
    assert!(glances.peak >= 2, "the glances were serial: {glances:?}");
    // Every file glanced once (a scan the window started again finds the
    // first ones in the glance cache).
    assert_eq!(glances.read + glances.cached, N, "{glances:?}");
    // The cells in view before the rest of the folder, whatever the
    // machine's speed: by the order the glances came, give or take the
    // ones the other threads were reading.
    let order = h.state().import_glanced_in_order();
    let first = in_view.len() + helios_gui::import_scan::glancers();
    let late: Vec<_> = in_view
        .iter()
        .filter(|p| !order.iter().take(first).any(|o| o == *p))
        .collect();
    assert!(
        late.is_empty(),
        "cells in view glanced after the first {first}: {late:?}"
    );
    // Opened again: every glance from the cache, nothing read.
    h.state_mut()
        .run("library.importWindow", source, Origin::User)
        .unwrap();
    // The command runs on the next frame: the window cleared, then filled.
    h.step();
    let frames = fill_in_view(&mut h, Duration::from_secs(30));
    eprintln!("import thumbnails, opened again: filled in {frames} frames");
    read_through(&mut h);
    let glances = h.state().import_glances();
    assert_eq!(
        glances.read, 0,
        "opened again, files were read: {glances:?}"
    );
    assert_eq!(glances.cached, N);
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

/// Where a pass painted a text (its bidirectional isolation marks left
/// out), if it did.
fn painted_rect(shapes: &[egui::epaint::ClippedShape], text: &str) -> Option<egui::Rect> {
    fn walk(shape: &egui::Shape, text: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(t) => {
                let shown: String = t
                    .galley
                    .text()
                    .chars()
                    .filter(|c| !matches!(c, '\u{2068}' | '\u{2069}'))
                    .collect();
                (shown == text).then(|| t.galley.rect.translate(t.pos.to_vec2()))
            }
            egui::Shape::Vec(v) => v.iter().find_map(|s| walk(s, text)),
            _ => None,
        }
    }
    shapes.iter().find_map(|s| walk(&s.shape, text))
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
    window_pass_at(h, window, close, vec2(1000.0, 700.0), None)
}

/// [`window_pass`] with the window's size and, where the platform tells
/// it, its outer position on the screen.
fn window_pass_at(
    h: &mut Harness<'static, Shell>,
    window: egui::ViewportId,
    close: bool,
    size: egui::Vec2,
    at: Option<egui::Pos2>,
) -> Vec<String> {
    painted_text(&window_run(h, window, close, size, at).shapes)
}

/// A deferred window's pass, as eframe runs it: its whole output.
fn window_run(
    h: &mut Harness<'static, Shell>,
    window: egui::ViewportId,
    close: bool,
    size: egui::Vec2,
    at: Option<egui::Pos2>,
) -> egui::FullOutput {
    window_run_with(h, window, close, size, at, Vec::new())
}

/// A deferred window's own pass, given input events (a click).
fn window_run_with(
    h: &mut Harness<'static, Shell>,
    window: egui::ViewportId,
    close: bool,
    size: egui::Vec2,
    at: Option<egui::Pos2>,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    use egui::{Pos2, RawInput, Rect, ViewportEvent, ViewportId, ViewportInfo};
    let cb = h.output().viewport_output[&window]
        .viewport_ui_cb
        .clone()
        .expect("the window is declared by the main window's last pass");
    let rect = Rect::from_min_size(Pos2::ZERO, size);
    let mut info = ViewportInfo {
        inner_rect: Some(rect),
        outer_rect: at.map(|p| Rect::from_min_size(p, size)),
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
        events,
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
    out
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

#[test]
fn a_card_plugged_in_again_shows_its_glances_from_disk_and_reads_only_a_changed_file() {
    const N: usize = 12;
    let dir = temp("card-disk");
    let dcim = dir.join("EOS_DIGITAL/DCIM/100CANON");
    photos(&dcim, N, 320, 240);
    let glances = helios_gui::import_scan::GlanceCache::global();
    let cache = dir.join("cache");
    glances.open_disk(&cache).unwrap();
    // First seen: every file read, its glance kept on disk.
    let h = card_window(&dir, None);
    let first = h.state().import_glances();
    assert_eq!((first.read, first.from_disk), (N, 0), "{first:?}");
    assert_eq!(glances.disk().unwrap().usage().1, N);
    drop(h);
    // Unmounted, then mounted again elsewhere after a restart: RAM is
    // empty, the disk tier listed anew. One file changed meanwhile.
    let elsewhere = dir.join("media");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::rename(dir.join("EOS_DIGITAL"), elsewhere.join("EOS_DIGITAL")).unwrap();
    glances.forget_ram();
    glances.open_disk(&cache).unwrap();
    let changed = elsewhere.join("EOS_DIGITAL/DCIM/100CANON/IMG_0003.JPG");
    image::RgbImage::from_pixel(320, 240, image::Rgb([200, 10, 10]))
        .save(&changed)
        .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&changed)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_mins(1))
        .unwrap();
    let h = card_window(&elsewhere, None);
    let again = h.state().import_glances();
    // Nothing read from the card but the file changed.
    assert_eq!(again.read, 1, "{again:?}");
    assert_eq!((again.cached, again.from_disk), (N - 1, N - 1), "{again:?}");
    for p in h.state().import_thumbnails_on_screen() {
        assert!(
            h.state().import_thumbnail_ready(&p),
            "{} without its glance",
            p.display()
        );
    }
    assert_eq!(h.state().import_cells().len(), N);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_opened_again_after_a_restart_reads_its_glances_from_disk_and_nothing_else() {
    const N: usize = 12;
    let dir = temp("folder-disk");
    let folder = dir.join("Shoot");
    photos(&folder, N, 320, 240);
    let glances = helios_gui::import_scan::GlanceCache::global();
    let cache = dir.join("cache");
    glances.open_disk(&cache).unwrap();
    let open = || {
        let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
        h.run();
        let source = json!({ "source": folder.display().to_string() });
        h.state_mut()
            .run("library.importWindow", source, Origin::User)
            .unwrap();
        read_through(&mut h);
        h
    };
    // First opened: every file read, its glance kept on disk under the
    // folder's source.
    let h = open();
    let first = h.state().import_glances();
    assert_eq!((first.read, first.from_disk), (N, 0), "{first:?}");
    assert_eq!(glances.disk().unwrap().usage().1, N);
    drop(h);
    // A restart: RAM empty, the disk tier listed anew.
    glances.forget_ram();
    glances.open_disk(&cache).unwrap();
    let h = open();
    let again = h.state().import_glances();
    assert_eq!(again.read, 0, "opened again, files were read: {again:?}");
    assert_eq!((again.cached, again.from_disk), (N, N), "{again:?}");
    assert_eq!(h.state().import_cells().len(), N);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Closed from its own pass (its close button, Cancel), the import window
/// hides at once, brings the main window forward, and nothing wakes it
/// again: its OS window goes only at the main window's next pass, which a
/// compositor holds back while the main window is hidden (behind a
/// browser opened for a picking), and meanwhile it would stay on screen
/// drawn empty (black).
#[test]
fn import_window_closed_from_its_own_pass_hides_at_once_and_is_woken_no_more() {
    let (mut h, window, dir) = import_in_its_own_window("close-hides");
    assert_eq!(h.state().import_window_viewport(), Some(window));
    let out = window_run(&mut h, window, true, vec2(1000.0, 700.0), None);
    assert!(!h.state().import_window_open());
    let commands = &out.viewport_output[&window].commands;
    assert!(
        commands.contains(&egui::ViewportCommand::Visible(false)),
        "the closing window is not hidden: {commands:?}"
    );
    let main = &out.viewport_output[&egui::ViewportId::ROOT].commands;
    assert!(
        main.contains(&egui::ViewportCommand::Focus),
        "the main window is not brought forward: {main:?}"
    );
    // Before any pass of the main window: what still answers (a picking
    // ending) asks the main window for a frame, not the window going.
    assert_eq!(h.state().import_window_viewport(), None);
    h.run_steps(3);
    assert!(declared(&h).is_empty(), "its window is gone");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A toast raised by what the import window asked (Import without a
/// library, a forget or Storage… failing) shows in the import window,
/// not in the main window, though the main window's pass answers it and
/// the main window is the active one by then.
#[test]
fn a_toast_from_the_import_window_s_request_shows_in_the_import_window() {
    let (mut h, window, dir) = import_in_its_own_window("toast-in-window");
    // The library goes while the window shows Import still on: its
    // words follow at the main window's next pass, after the click.
    h.state_mut()
        .set_no_library(Some("the disk went away".into()));
    let size = vec2(1000.0, 700.0);
    let out = window_run(&mut h, window, false, size, None);
    let selected = h.state().import_counts().selected;
    assert!(selected > 0, "nothing ticked to import");
    let at = painted_rect(&out.shapes, &format!("Import {selected}"))
        .expect("the Import button")
        .center();
    let press = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    let click = vec![egui::Event::PointerMoved(at), press(true)];
    window_run_with(&mut h, window, false, size, None, click);
    window_run_with(&mut h, window, false, size, None, vec![press(false)]);
    // The main window's pass answers it; the user is back in the main
    // window already, which draws again: the toast stays the import
    // window's all the same.
    for _ in 0..2 {
        h.input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(true);
        h.run();
    }
    assert!(h.state().import_window_open(), "the window stays open");
    let app = h.state().app().handle().clone();
    let toasts = app.service(&egui_pop::api::toasts::TOASTS).unwrap();
    let toast = || {
        toasts
            .shown()
            .into_iter()
            .find(|t| t.toast.text.contains("the disk went away"))
            .expect("no toast says why nothing was imported")
    };
    let said = toast();
    assert_eq!(said.window.as_deref(), Some("import"), "{said:?}");
    // Not drawn by the main window's passes (its status bar says the
    // notice, as for every one).
    assert!(said.since.is_none(), "drawn in the main window: {said:?}");
    // The import window's pass draws it.
    window_pass(&mut h, window, false);
    let said = toast();
    assert!(
        said.since.is_some(),
        "not drawn in the import window: {said:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Storage… asked from the import window, the Settings popped in over the
/// main window: the main window comes to the front, else the page would
/// open behind the import window. In their own window (the default), the
/// Settings window shows and the main window stays where it is.
#[test]
fn storage_asked_from_the_import_window_brings_the_main_window_forward() {
    let (mut h, window, dir) = import_in_its_own_window("storage-forward");
    let focus_asked = |h: &mut Harness<'static, Shell>| {
        // The import window worked in, then the main window's pass in the
        // back applies the command.
        window_pass(h, window, false);
        h.state_mut()
            .run("library.storage", Value::Null, Origin::User)
            .unwrap();
        h.input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(false);
        h.step();
        h.output().viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::Focus)
    };
    assert!(
        !focus_asked(&mut h),
        "the Settings in their own window: the main window stays behind"
    );
    h.state_mut()
        .run("admin.close", Value::Null, Origin::User)
        .unwrap();
    main_passes_in_the_back(&mut h, 2);
    // Popped in over the main window.
    let popped = h
        .state_mut()
        .run("admin.popOut", Value::Null, Origin::User)
        .unwrap();
    assert_eq!(popped["window"], false, "{popped}");
    main_passes_in_the_back(&mut h, 2);
    assert!(
        focus_asked(&mut h),
        "Storage… from the import window leaves the main window behind"
    );
    assert!(h.state().storage_page_open());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Opens the import window in an OS window of its own (real viewports, as
/// on the desktop), its sources read: the harness and the window's
/// viewport.
fn import_in_its_own_window(name: &str) -> (Harness<'static, Shell>, egui::ViewportId, PathBuf) {
    import_in_its_own_window_framed(name, false)
}

/// [`import_in_its_own_window`], Helios drawing the windows' frames
/// (`own_frame`, as on the desktop) or the system drawing them.
fn import_in_its_own_window_framed(
    name: &str,
    own_frame: bool,
) -> (Harness<'static, Shell>, egui::ViewportId, PathBuf) {
    let dir = temp(name);
    let folder = dir.join("Shoot");
    photos(&folder, 4, 400, 300);
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    if own_frame {
        h.state_mut().draw_frame(true);
    }
    h.run();
    h.state_mut()
        .run(
            "library.importWindow",
            json!({ "source": folder.display().to_string() }),
            Origin::User,
        )
        .unwrap();
    h.run_steps(3);
    let windows = declared(&h);
    assert_eq!(windows.len(), 1, "the import window's own: {windows:?}");
    let window = windows[0];
    let deadline = Instant::now() + Duration::from_secs(20);
    while !h.state().import_read() {
        assert!(Instant::now() < deadline, "the sources not read in 20 s");
        window_pass(&mut h, window, false);
        std::thread::sleep(Duration::from_millis(5));
    }
    (h, window, dir)
}

/// A key pressed in a deferred window's own pass, as eframe runs it.
fn key_in_window(
    h: &mut Harness<'static, Shell>,
    window: egui::ViewportId,
    key: Key,
    modifiers: Modifiers,
) {
    use egui::{Pos2, RawInput, Rect, ViewportId, ViewportInfo};
    let cb = h.output().viewport_output[&window]
        .viewport_ui_cb
        .clone()
        .expect("the window is declared by the main window's last pass");
    let rect = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
    let info = ViewportInfo {
        inner_rect: Some(rect),
        focused: Some(true),
        ..Default::default()
    };
    let mut input = RawInput {
        viewport_id: window,
        screen_rect: Some(rect),
        time: Some(h.ctx.input(|i| i.time) + 1.0 / 60.0),
        focused: true,
        ..Default::default()
    };
    for pressed in [true, false] {
        input.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        });
    }
    input.viewports.insert(window, info);
    input.viewports.insert(
        ViewportId::ROOT,
        ViewportInfo {
            focused: Some(false),
            ..Default::default()
        },
    );
    let mut out = h.ctx.run_ui(input, |ui| cb(ui));
    out.textures_delta.clear();
}

/// The main window's passes, the import window in front (as when the
/// person works there): the main window is not focused.
fn main_passes_in_the_back(h: &mut Harness<'static, Shell>, steps: usize) {
    for _ in 0..steps {
        h.input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(false);
        h.run();
    }
}

/// The main window's close requested (its close button, Alt+F4, the window
/// manager), as the desktop's host meets it: the shell says what it does,
/// and the pass that follows runs with it.
fn close_main_window(h: &mut Harness<'static, Shell>) -> helios_gui::shell::Closing {
    let ctx = h.ctx.clone();
    let closing = h.state_mut().main_window_closing(&ctx);
    if closing == helios_gui::shell::Closing::Moved {
        h.input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        h.run();
    }
    closing
}

/// Ctrl+Q pressed in the import window quits Helios, not that window
/// alone (docs/design/windows.md, section 7, D1): the main window is asked
/// to close, and its close then ends the process.
#[test]
fn ctrl_q_in_the_import_window_quits_helios() {
    let (mut h, window, dir) = import_in_its_own_window("ctrl-q");
    assert!(!h.state().quitting());
    key_in_window(&mut h, window, Key::Q, Modifiers::COMMAND);
    main_passes_in_the_back(&mut h, 2);
    assert!(h.state().quitting(), "Ctrl+Q in the import window quits");
    let ctx = h.ctx.clone();
    assert_eq!(
        h.state_mut().main_window_closing(&ctx),
        helios_gui::shell::Closing::Quit
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ctrl+W pressed in the import window closes that window alone: the
/// import focus closes, the main window stays, Helios goes on.
#[test]
fn ctrl_w_in_the_import_window_closes_it_alone() {
    let (mut h, window, dir) = import_in_its_own_window("ctrl-w");
    key_in_window(&mut h, window, Key::W, Modifiers::COMMAND);
    main_passes_in_the_back(&mut h, 3);
    assert!(!h.state().import_window_open(), "the import window closed");
    assert!(declared(&h).is_empty(), "its OS window is gone");
    assert!(!h.state().quitting());
    let app = h.state().app().handle().clone();
    let listed: Vec<_> = app
        .windows()
        .into_iter()
        .map(|w| w.id.to_string())
        .collect();
    assert_eq!(listed, ["main"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The main window closed while the import window is open: Helios goes on,
/// the import window moving into the main one (egui-pop's rule), its scan
/// and choices kept; its Window menu lists the main window. Window > Main
/// window brings the shell back, the import window in a window of its own
/// again. Closing that one, then the main window, the last, quits.
#[test]
fn closing_the_main_window_keeps_the_import_window_and_window_main_brings_it_back() {
    let (mut h, _window, dir) = import_in_its_own_window("close-main");
    let import_title = h.state().app().handle().windows()[1].title.clone();
    let closing = close_main_window(&mut h);
    assert_eq!(closing, helios_gui::shell::Closing::Moved);
    h.run_steps(2);
    assert!(!h.state().quitting());
    assert!(h.state().import_window_open(), "the import window stays");
    let app = h.state().app().handle().clone();
    let listed: Vec<_> = app
        .windows()
        .into_iter()
        .map(|w| (w.id.to_string(), w.main))
        .collect();
    assert_eq!(
        listed,
        [("import".to_owned(), true)],
        "it took the main window's place"
    );
    // Drawn in the main window now, with the main menus.
    assert!(h.query_by_label("Choose a folder…").is_some());
    h.get_by_label("Window").click();
    h.run_steps(2);
    assert!(
        h.query_by_label("Main window").is_some(),
        "Window lists the main window"
    );
    // Window > Main window: the shell is back in the main window, the
    // import window in its own again, with its title.
    h.get_by_label("Main window").click();
    h.run_steps(3);
    assert!(!h.state().quitting());
    assert!(h.state().import_window_open(), "the import window stays");
    let listed: Vec<_> = app
        .windows()
        .into_iter()
        .map(|w| (w.id.to_string(), w.main, w.title))
        .collect();
    assert_eq!(app.windows()[0].id, WindowId::main(), "the shell is back");
    assert_eq!(
        listed[1..],
        [("import".to_owned(), false, import_title)],
        "the import window is its own again"
    );
    let windows = declared(&h);
    assert_eq!(windows.len(), 1, "its OS window again: {windows:?}");
    // Its close button: it closes alone, Helios goes on.
    window_pass(&mut h, windows[0], true);
    main_passes_in_the_back(&mut h, 3);
    assert!(!h.state().import_window_open(), "the import window closed");
    assert!(!h.state().quitting());
    // The last window closed: Helios quits.
    assert_eq!(close_main_window(&mut h), helios_gui::shell::Closing::Quit);
    assert!(h.state().quitting());
    let _ = std::fs::remove_dir_all(&dir);
}

/// With Helios's own frame (as on the desktop), the import window opens
/// without the system's decorations and draws Helios's title bar. Closing
/// the main window moves it into the main window with its size and place
/// (not the main window's), and its title bar comes along: its title and
/// the window's buttons.
#[test]
fn closing_the_main_window_moves_the_import_window_with_its_geometry_and_title_bar() {
    use egui::ViewportCommand;
    let (mut h, window, dir) = import_in_its_own_window_framed("moved-chrome", true);
    let decorations = h.output().viewport_output[&window].builder.decorations;
    assert_eq!(decorations, Some(false), "no system title bar");
    let import_title = h.state().app().handle().windows()[1].title.clone();
    // Worked in at its own size and place.
    let drawn = window_pass_at(
        &mut h,
        window,
        false,
        vec2(900.0, 640.0),
        Some(egui::pos2(300.0, 120.0)),
    );
    assert!(
        drawn.iter().any(|t| t.starts_with(&import_title)),
        "its title bar names it: {drawn:?}"
    );
    let ctx = h.ctx.clone();
    assert_eq!(
        h.state_mut().main_window_closing(&ctx),
        helios_gui::shell::Closing::Moved
    );
    h.input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    h.step();
    let sent = h.output().viewport_output[&egui::ViewportId::ROOT]
        .commands
        .clone();
    assert!(sent.contains(&ViewportCommand::CancelClose), "{sent:?}");
    assert!(
        sent.contains(&ViewportCommand::InnerSize(vec2(900.0, 640.0))),
        "{sent:?}"
    );
    assert!(
        sent.contains(&ViewportCommand::OuterPosition(egui::pos2(300.0, 120.0))),
        "{sent:?}"
    );
    h.run_steps(2);
    assert!(h.state().import_window_open(), "the import window stays");
    // The main window's title bar is the import window's now.
    // The main window's accessible names (as `MultiWindow::names` reads
    // them): a button's label, a text's words.
    let names: Vec<String> = h
        .root()
        .children_recursive()
        .filter_map(|n| {
            let node = n.accesskit_node();
            node.label().or_else(|| node.value())
        })
        .collect();
    for name in ["Minimize", "Maximize", "Close"] {
        assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
    }
    assert!(
        names.iter().any(|n| n.starts_with(&import_title)),
        "{import_title} in {names:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Helios does not offer egui-pop's window switcher: Switch to window…
/// is neither in the Window menu nor in the palette, nor is
/// `window.switch`; the Window menu keeps Helios's windows and the list
/// of those open.
#[test]
fn the_window_switcher_is_neither_in_the_window_menu_nor_in_the_palette() {
    use egui_pop::api::windows::{CMD_PICK, CMD_SWITCH, MENU_WINDOW};
    use egui_pop::model::menus::Item;
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let app = h.state().app().handle().clone();
    // Its menu as every window shows it (`menus::show` reads it).
    let items = app.menu(&MENU_WINDOW);
    let commands: Vec<String> = items
        .iter()
        .filter_map(|i| match &i.item {
            Item::Command { id, .. } => Some(id.to_string()),
            _ => None,
        })
        .collect();
    assert!(!commands.contains(&CMD_PICK.to_string()), "{commands:?}");
    assert!(!commands.contains(&CMD_SWITCH.to_string()), "{commands:?}");
    for kept in ["window.close", "library.importWindow"] {
        assert!(commands.iter().any(|c| c == kept), "{kept} in {commands:?}");
    }
    assert!(
        items.iter().any(|i| matches!(&i.item, Item::List { .. })),
        "the open windows' list stays"
    );
    let palette: Vec<String> = app
        .palette()
        .into_iter()
        .map(|e| e.command.to_string())
        .collect();
    assert!(palette.iter().any(|c| c == "window.close"), "{palette:?}");
    for gone in [CMD_PICK, CMD_SWITCH] {
        assert!(
            !palette.contains(&gone.to_string()),
            "{gone} in the palette"
        );
    }
}

/// The main window alone, closed: Helios quits.
#[test]
fn closing_the_last_window_quits() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    h.state_mut()
        .set_last_window(helios_gui::shell::LastWindow::Quit);
    h.run();
    assert_eq!(close_main_window(&mut h), helios_gui::shell::Closing::Quit);
    assert!(h.state().quitting());
}

/// macOS's habit, on any platform: the last window closed is hidden and
/// Helios goes on; Window > Main window brings it back; Quit still quits.
#[test]
fn the_last_window_closed_stays_open_on_macos() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    h.state_mut()
        .set_last_window(helios_gui::shell::LastWindow::StayOpen);
    h.run();
    assert_eq!(
        close_main_window(&mut h),
        helios_gui::shell::Closing::Hidden
    );
    assert!(h.state().window_hidden());
    assert!(!h.state().quitting());
    h.state_mut()
        .run("helios.window.main", json!(null), Origin::User)
        .unwrap();
    h.run();
    assert!(!h.state().window_hidden(), "shown again");
    h.state_mut()
        .run("workbench.quit", json!(null), Origin::User)
        .unwrap();
    h.run();
    assert!(h.state().quitting());
}

/// The painted bounds of every shape a pass drew, nested ones apart, each
/// cut to its clip rect; shapes clipped away entirely are left out.
fn painted_rects(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
    fn walk(shape: &egui::Shape, clip: egui::Rect, out: &mut Vec<egui::Rect>) {
        if let egui::Shape::Vec(v) = shape {
            for s in v {
                walk(s, clip, out);
            }
            return;
        }
        let r = shape.visual_bounding_rect().intersect(clip);
        if r.is_positive() {
            out.push(r);
        }
    }
    let mut out = Vec::new();
    for s in shapes {
        walk(&s.shape, s.clip_rect, &mut out);
    }
    out
}

/// Whether `a` and `b` share more than a hairline.
fn overlap(a: egui::Rect, b: egui::Rect) -> bool {
    let i = a.intersect(b);
    i.width() > 0.5 && i.height() > 0.5
}

#[test]
fn the_carousel_cells_never_overlap_whatever_the_window_and_the_thumbnail_size() {
    let dir = temp("carousel");
    let folder = dir.join("Shoot");
    photos(&folder, 9, 320, 240);
    for window in [vec2(1280.0, 800.0), vec2(900.0, 640.0)] {
        // The middle cell's width at each thumbnail size.
        let mut middles = Vec::new();
        for side in [64.0, 256.0] {
            let at = format!("window {window:?}, thumbnails of {side}");
            let mut h = headless::harness(Shell::new(helios_panels(), None), window);
            h.run();
            h.state_mut()
                .run(
                    "library.importWindow",
                    json!({ "source": folder.display().to_string() }),
                    Origin::User,
                )
                .unwrap();
            read_through(&mut h);
            {
                let mut b = h.state_mut().import_browser_mut().unwrap();
                assert!(b.set_mode("carousel"));
                b.set_thumbnail_size(side);
                // The middle one has neighbours on both sides.
                b.focus_on(&folder.join("IMG_0004.JPG"), false);
            }
            h.run_steps(3);
            fill_in_view(&mut h, Duration::from_secs(30));
            h.run_steps(3);
            let cells: Vec<(String, egui::Rect)> = h
                .query_all_by_role(egui::accesskit::Role::ListBoxOption)
                .filter_map(|n| {
                    let label = n.accesskit_node().label()?;
                    label.starts_with("IMG_").then(|| (label, n.rect()))
                })
                .collect();
            assert!(cells.len() >= 3, "{at}: a middle and neighbours: {cells:?}");
            // The cells themselves are apart.
            for (i, (a, ra)) in cells.iter().enumerate() {
                for (b, rb) in &cells[i + 1..] {
                    assert!(!overlap(*ra, *rb), "{at}: {a} {ra:?} over {b} {rb:?}");
                }
            }
            // Nothing the import window paints spills from one cell over
            // another: a shape over two cells or more is one that holds
            // them all (the browser's background). The main window's panels
            // behind the import window are painted before its frame and
            // left out: what counts is painted from the frame on.
            let window = h
                .get_by_role_and_label(egui::accesskit::Role::Window, "Import photos")
                .rect();
            let shapes = &h.output().shapes;
            let frame = shapes
                .iter()
                .position(|s| {
                    painted_rects(std::slice::from_ref(s)).iter().any(|r| {
                        (r.min - window.min).length() < 1.0 && (r.max - window.max).length() < 1.0
                    })
                })
                .unwrap_or_else(|| panic!("{at}: the import window's frame is painted"));
            for r in painted_rects(&shapes[frame..]) {
                let over: Vec<&(String, egui::Rect)> =
                    cells.iter().filter(|(_, c)| overlap(r, *c)).collect();
                if over.len() >= 2 {
                    assert!(
                        over.iter().all(|(_, c)| r.expand(0.5).contains_rect(*c)),
                        "{at}: a shape painted at {r:?} spills over {:?}",
                        over.iter().map(|(n, c)| (n, c)).collect::<Vec<_>>()
                    );
                }
            }
            let middle = cells
                .iter()
                .find(|(label, _)| label.starts_with("IMG_0004"))
                .unwrap_or_else(|| panic!("{at}: the middle cell is shown: {cells:?}"));
            middles.push(middle.1.width());
            eprintln!("{at}: {cells:?}");
        }
        // The size control sizes the middle cell.
        assert!(
            middles[1] > middles[0] + 1.0,
            "window {window:?}: the middle cell at 64 and 256: {middles:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The vault is the local node's, not a window's: the main window closed
/// while the import window remains leaves the node running and its vault
/// open to writes; the last window closed asks the node to quit, and it
/// closes the vault once the GUI client is gone.
#[test]
fn a_window_closing_while_another_remains_leaves_the_vault_open() {
    use helios_gui::node::LocalNode;
    use helios_vault_sqlite::SqliteStore;

    let (mut h, _window, dir) = import_in_its_own_window("node-vault");
    let store = SqliteStore::open(&dir.join("vault.db")).unwrap();
    let node = LocalNode::open(
        store,
        Arc::new(helios_core::Bus::new()),
        Duration::from_hours(1),
    );
    let link = node.link();
    let client = node.connect();
    h.state_mut().attach_node(link.clone());
    assert_eq!(close_main_window(&mut h), helios_gui::shell::Closing::Moved);
    h.run_steps(2);
    assert!(!link.quitting(), "the node goes on");
    node.vault()
        .set_config_layer("user", Some("kept = 1"))
        .expect("the vault is open to writes");
    // The last window closed: Helios quits, the node is asked to.
    assert_eq!(close_main_window(&mut h), helios_gui::shell::Closing::Quit);
    assert!(link.quitting());
    assert!(!link.stopped(), "the GUI is still connected");
    drop(client);
    assert!(
        link.wait_stopped(Duration::from_secs(10)),
        "the node stopped"
    );
    node.stop();
    let again = SqliteStore::open(&dir.join("vault.db")).unwrap();
    assert!(!again.report().unclean_shutdown, "closed cleanly");
    drop(again);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An export is the node's job, not its window's: started from the main
/// window, it goes on once that window closed (the import window moving
/// into its place) and the window's handle on the exporter is gone; it
/// finishes, its file written, its batch done in the jobs centre the
/// remaining window shows.
#[test]
fn an_export_keeps_running_when_its_window_closes() {
    use egui_pop::api::jobs::JOBS;
    use egui_pop::model::jobs::JobState;
    use helios_gui::export::Exporter;
    use helios_gui::node::LocalNode;
    use helios_vault_sqlite::SqliteStore;

    let (mut h, _window, dir) = import_in_its_own_window("node-export");
    let bus = Arc::new(helios_core::Bus::new());
    let node = LocalNode::open(
        SqliteStore::open(&dir.join("vault.db")).unwrap(),
        bus.clone(),
        Duration::from_hours(1),
    );
    let link = node.link();
    let client = node.connect();
    h.state_mut().attach_node(link.clone());
    let persist = helios_gui::persist::Persist::on(node.vault().clone(), bus);
    persist
        .vault()
        .import_file(&dir.join("Shoot").join("IMG_0000.JPG"), 1)
        .unwrap();
    persist.fill_library(h.state_mut()).unwrap();
    h.run();
    // The window's handle; the node holds the exporter too. Held until
    // the window closed.
    let exporter = Exporter::with_engine(
        node.vault().clone(),
        h.state().jobs(),
        h.state().export_sink(),
        || {},
        1,
        h.state().viewport().engine().clone(),
    )
    .adopted_by(&link);
    exporter.pause();
    let out = dir.join("out");
    h.state_mut()
        .run(
            "library.export",
            json!({ "scope": "shown", "format": "png", "folder": out }),
            Origin::User,
        )
        .unwrap();
    h.run();
    let bundle = h
        .state()
        .workspace_bundle()
        .unwrap_or_else(helios_template::workspace::blank);
    let asks = h.state_mut().take_exports();
    assert_eq!(asks.len(), 1, "one export asked");
    let batch = exporter.run(&bundle, &asks[0]).unwrap();
    assert_eq!(
        link.running_jobs(),
        std::slice::from_ref(&batch),
        "the node's job"
    );

    // Its window closes; the window's handle goes with it.
    assert_eq!(close_main_window(&mut h), helios_gui::shell::Closing::Moved);
    h.run_steps(2);
    exporter.resume();
    drop(exporter);

    let deadline = Instant::now() + Duration::from_mins(1);
    while !link.running_jobs().is_empty() {
        assert!(Instant::now() < deadline, "the export did not end in 60 s");
        h.run();
        std::thread::sleep(Duration::from_millis(10));
    }
    h.run_steps(3);
    let written: Vec<_> = std::fs::read_dir(&out)
        .expect("the export's folder")
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(written, ["IMG_0000.png"], "the file exported");
    let app = h.state().app().handle().clone();
    let centre = app.service(&JOBS).expect("the jobs plugin");
    assert_eq!(centre.job(&batch).expect("listed").state, JobState::Done);

    assert_eq!(close_main_window(&mut h), helios_gui::shell::Closing::Quit);
    drop(persist);
    drop(h);
    drop(client);
    assert!(
        link.wait_stopped(Duration::from_secs(10)),
        "the node stopped"
    );
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

type Kept = helios_node_core::settings::NodeSettings<helios_vault_sqlite::SqliteStore>;

/// A node over the vault at `file`, and its settings, as the desktop
/// keeps them.
fn node_at(
    file: &Path,
) -> (
    helios_node_core::node::LocalNode<helios_vault_sqlite::SqliteStore>,
    Kept,
) {
    let store = helios_vault_sqlite::SqliteStore::open(file).unwrap();
    let bus = Arc::new(helios_core::Bus::new());
    let node = helios_node_core::node::LocalNode::open(store, bus, Duration::from_hours(1));
    let kept = node.settings();
    (node, kept)
}

/// A shell keeping its import sources through the node's settings.
fn shell_keeping(kept: &Kept) -> Harness<'static, Shell> {
    let doc = helios_gui::shell::IMPORT_SOURCES;
    let (read, write) = (kept.clone(), kept.clone());
    let mut shell = Shell::new(helios_panels(), None);
    shell.keep_import_sources_with(
        Box::new(move || read.read(doc)),
        Box::new(move |text| write.write(doc, text)),
    );
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

/// The import window opened on `source`, read through (a spinner turns
/// meanwhile: frames are stepped, not run to rest).
fn open_on(h: &mut Harness<'static, Shell>, source: &Path) {
    h.state_mut()
        .run(
            "library.importWindow",
            json!({ "source": source }),
            Origin::User,
        )
        .unwrap();
    read_through(h);
}

#[test]
fn a_sources_choices_are_saved_on_it_follow_the_selection_and_come_back_after_a_restart() {
    let dir = temp("sources");
    let (trips, family) = (dir.join("trips"), dir.join("family"));
    for (folder, name) in [(&trips, "a"), (&family, "b")] {
        std::fs::create_dir_all(folder).unwrap();
        put(&folder.join(format!("{name}.jpg")), Ok(&jpeg(name)), 2);
    }
    let vault = dir.join("vault.db");
    {
        let (node, kept) = node_at(&vault);
        let mut h = shell_keeping(&kept);
        open_on(&mut h, &trips);
        let _ = h.get_by_label("trips: the default choices");
        // Changing a choice saves it on the selected source.
        h.get_by_label("Copy into the Library").click();
        h.run_steps(3);
        h.get_by_label("By month").click();
        h.run_steps(3);
        h.get_by_label("Keep").click();
        h.run_steps(3);
        let _ = h.get_by_label("trips: its own choices");
        let text = kept
            .read(helios_gui::shell::IMPORT_SOURCES)
            .unwrap()
            .expect("the sources kept by the node");
        assert!(
            text.contains("trips") && text.contains("keep") && text.contains("month"),
            "{text}"
        );
        // Another folder shows the default choices.
        open_on(&mut h, &family);
        let _ = h.get_by_label("family: the default choices");
        assert_eq!(h.state().import_choices().duplicates.as_str(), "skip");
        assert!(!h.state().import_copies());
        // Back on the first: its own; Reset gives the defaults back, Undo
        // takes the reset back.
        open_on(&mut h, &trips);
        assert_eq!(h.state().import_choices().duplicates.as_str(), "keep");
        assert!(h.state().import_copies());
        h.get_by_label("Reset to defaults").click();
        h.run_steps(3);
        assert_eq!(h.state().import_choices().duplicates.as_str(), "skip");
        let _ = h.get_by_label("trips: the default choices");
        h.get_by_label("Undo").click();
        h.run_steps(3);
        assert_eq!(h.state().import_choices().duplicates.as_str(), "keep");
        // Import runs each folder with its own choices.
        h.get_by_label("Import 1").click();
        h.run_steps(3);
        let imports = h.state_mut().take_imports();
        assert_eq!(imports.len(), 1, "{imports:?}");
        assert_eq!(
            (
                &imports[0]["copy"],
                &imports[0]["layout"],
                &imports[0]["duplicates"]
            ),
            (&json!(true), &json!("month"), &json!("keep"))
        );
        drop(h);
        node.stop();
    }
    // After a restart, the source comes back with its choices.
    let (node, kept) = node_at(&vault);
    let mut h = shell_keeping(&kept);
    assert_eq!(h.state().import_sources().len(), 1);
    open_on(&mut h, &trips);
    let choices = h.state().import_choices();
    assert_eq!(
        (choices.duplicates.as_str(), choices.layout.as_str()),
        ("keep", "month")
    );
    let _ = h.get_by_label("trips: its own choices");
    drop(h);
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The focused text field: the wizard's step gives it the focus.
fn type_in_focused(h: &mut Harness<'static, Shell>, text: &str) {
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .find(egui_kittest::Node::is_focused)
        .expect("the step's field focused")
        .type_text(text);
    h.run_steps(3);
}

/// Clicks the first node of this label and lets the frames follow.
fn click(h: &mut Harness<'static, Shell>, label: &str) {
    h.query_all_by_label(label)
        .next()
        .unwrap_or_else(|| panic!("no {label}"))
        .click();
    h.run_steps(3);
}

/// The wizard opened in the import window and taken to `step` (0: the
/// kinds, 1: the folder, 2: the choices, 3: the name) for `folder`.
fn wizard_at(h: &mut Harness<'static, Shell>, step: usize, folder: &Path) {
    h.state_mut()
        .run("library.importWindow", Value::Null, Origin::User)
        .unwrap();
    h.run_steps(3);
    click(h, "Add a source…");
    assert!(h.state().import_wizard_open());
    // Each kind says what it does.
    let _ = h.get_by_label("A folder on this computer or a share it mounts, with its subfolders");
    if step == 0 {
        return;
    }
    click(h, "Folder");
    click(h, "Next");
    if step == 1 {
        return;
    }
    type_in_focused(h, &folder.display().to_string());
    click(h, "Next");
    let _ = h.get_by_label("Add where they are");
    if step == 2 {
        return;
    }
    click(h, "Next");
}

#[test]
fn the_add_a_source_wizard_saves_a_folder_with_its_choices_selected_and_kept_after_a_restart() {
    let dir = temp("wizard");
    let trips = dir.join("trips");
    std::fs::create_dir_all(&trips).unwrap();
    put(&trips.join("a.jpg"), Ok(&jpeg("a")), 2);
    let vault = dir.join("vault.db");
    {
        let (node, kept) = node_at(&vault);
        let mut h = shell_keeping(&kept);
        wizard_at(&mut h, 2, &trips);
        // The choices start from the window's, every one of them offered
        // for a folder.
        let _ = h.get_by_label("Read darktable edits");
        click(&mut h, "Copy into the Library");
        click(&mut h, "By month");
        click(&mut h, "Keep");
        // Back keeps them.
        click(&mut h, "Back");
        click(&mut h, "Next");
        click(&mut h, "Next");
        // The name starts as the folder's.
        click(&mut h, "Add the source");
        assert!(!h.state().import_wizard_open());
        let sources = h.state().import_sources();
        assert_eq!(sources.len(), 1, "{sources:?}");
        let source = &sources[0];
        assert_eq!(
            (
                source.connector.as_str(),
                source.name.as_deref(),
                source.path.as_deref()
            ),
            ("folder", Some("trips"), Some(trips.as_path()))
        );
        let choices = source.choices.clone().expect("its own choices");
        assert_eq!(
            (
                choices.copy_folders,
                choices.layout.as_str(),
                choices.duplicates.as_str()
            ),
            (true, "month", "keep")
        );
        // Selected: ticked, its choices in the column, its row under its
        // name.
        assert_eq!(h.state().import_ticked(), std::slice::from_ref(&trips));
        let _ = h.get_by_label("trips: its own choices");
        let _ = h.get_by_label("Saved sources");
        let text = kept
            .read(helios_gui::shell::IMPORT_SOURCES)
            .unwrap()
            .expect("the source kept by the node");
        assert!(text.contains("name = \"trips\""), "{text}");
        drop(h);
        node.stop();
    }
    // After a restart, its row is back under its name, with its choices.
    let (node, kept) = node_at(&vault);
    let mut h = shell_keeping(&kept);
    h.state_mut()
        .run("library.importWindow", Value::Null, Origin::User)
        .unwrap();
    h.run_steps(3);
    let _ = h.get_by_label("Saved sources");
    click(&mut h, "trips");
    let _ = h.get_by_label("trips: its own choices");
    assert_eq!(h.state().import_choices().layout.as_str(), "month");
    drop(h);
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancelling_the_add_a_source_wizard_at_any_step_leaves_no_source() {
    let dir = temp("wizard-cancel");
    let trips = dir.join("trips");
    std::fs::create_dir_all(&trips).unwrap();
    let (node, kept) = node_at(&dir.join("vault.db"));
    let mut h = shell_keeping(&kept);
    for step in 0..4 {
        wizard_at(&mut h, step, &trips);
        click(&mut h, "Cancel");
        assert!(!h.state().import_wizard_open(), "step {step}");
        assert!(h.state().import_sources().is_empty(), "step {step}");
        assert!(h.state().import_ticked().is_empty(), "step {step}");
        // The window's own columns are back.
        let _ = h.get_by_label("Choose a folder…");
    }
    assert_eq!(kept.read(helios_gui::shell::IMPORT_SOURCES).unwrap(), None);
    drop(h);
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The menu of the folder row labelled `label` in the Sources column.
fn row_menu(h: &mut Harness<'static, Shell>, label: &str) {
    h.get(
        egui_kittest::kittest::By::new()
            .role(egui::accesskit::Role::CheckBox)
            .label(label),
    )
    .click_secondary();
    h.run_steps(3);
}

#[test]
fn edit_source_saves_a_recent_folder_with_its_copy_or_add_and_name_kept_after_a_restart() {
    let dir = temp("editor");
    let trips = dir.join("trips");
    std::fs::create_dir_all(&trips).unwrap();
    put(&trips.join("a.jpg"), Ok(&jpeg("a")), 2);
    let vault = dir.join("vault.db");
    {
        let (node, kept) = node_at(&vault);
        let mut h = shell_keeping(&kept);
        open_on(&mut h, &trips);
        // A recent folder, remembered but not saved.
        assert!(h.state().import_sources().is_empty());
        row_menu(&mut h, "trips");
        click(&mut h, "Edit source…");
        assert!(h.state().import_wizard_open());
        let _ = h.get_by_label("Edit source");
        // Its parameter, the folder, is there; its kind is not asked.
        assert!(
            h.query_by_label("What kind of source is it?").is_none(),
            "the editor starts at the setup"
        );
        click(&mut h, "Next");
        // A folder honours both: copy or add is offered.
        let _ = h.get_by_label("Add where they are");
        click(&mut h, "Copy into the Library");
        click(&mut h, "By month");
        click(&mut h, "Next");
        type_in_focused(&mut h, " raw");
        click(&mut h, "Save the source");
        assert!(!h.state().import_wizard_open());
        let sources = h.state().import_sources();
        assert_eq!(sources.len(), 1, "{sources:?}");
        let source = &sources[0];
        assert_eq!(
            (source.connector.as_str(), source.path.as_deref()),
            ("folder", Some(trips.as_path()))
        );
        let name = source.name.clone().unwrap_or_default();
        assert!(name.contains("trips") && name.contains("raw"), "{name}");
        let choices = source.choices.clone().expect("its own choices");
        assert!(choices.copy_folders, "{choices:?}");
        assert_eq!(choices.layout.as_str(), "month");
        // Selected, under its name among the saved sources.
        assert_eq!(h.state().import_ticked(), std::slice::from_ref(&trips));
        let _ = h.get_by_label("Saved sources");
        // Edited again: switched back to add where they are.
        row_menu(&mut h, &name);
        click(&mut h, "Edit source…");
        click(&mut h, "Next");
        click(&mut h, "Add where they are");
        click(&mut h, "Next");
        click(&mut h, "Save the source");
        let sources = h.state().import_sources();
        assert_eq!(sources.len(), 1, "{sources:?}");
        assert!(!sources[0].choices.as_ref().unwrap().copy_folders);
        // Undo takes the edit back, as the column's other changes.
        h.state_mut().undo_import_source_choice();
        h.run_steps(3);
        assert!(
            h.state().import_sources()[0]
                .choices
                .as_ref()
                .unwrap()
                .copy_folders
        );
        drop(h);
        node.stop();
    }
    // After a restart, it is back with its name and choices.
    let (node, kept) = node_at(&vault);
    let h = shell_keeping(&kept);
    let sources = h.state().import_sources();
    assert_eq!(sources.len(), 1, "{sources:?}");
    assert!(
        sources[0]
            .name
            .as_deref()
            .unwrap_or_default()
            .contains("raw")
    );
    let choices = sources[0].choices.clone().unwrap();
    assert!(choices.copy_folders && choices.layout.as_str() == "month");
    drop(h);
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn forget_this_source_takes_a_folder_off_the_column_and_undo_brings_it_back() {
    let dir = temp("forget-source");
    let (trips, family) = (dir.join("trips"), dir.join("family"));
    for (folder, name) in [(&trips, "a"), (&family, "b")] {
        std::fs::create_dir_all(folder).unwrap();
        put(&folder.join(format!("{name}.jpg")), Ok(&jpeg(name)), 2);
    }
    let (node, kept) = node_at(&dir.join("vault.db"));
    let mut h = shell_keeping(&kept);
    // A recent folder, only remembered: forgotten, then back.
    open_on(&mut h, &family);
    open_on(&mut h, &trips);
    assert!(h.state().import_recent().contains(&family));
    row_menu(&mut h, "family");
    click(&mut h, "Forget this source");
    assert!(!h.state().import_recent().contains(&family));
    assert!(h.query_by_label("family").is_none());
    h.state_mut().undo_import_source_choice();
    h.run_steps(3);
    assert!(h.state().import_recent().contains(&family));
    // A folder with its own choices: forgotten with them.
    click(&mut h, "Keep");
    assert_eq!(h.state().import_sources().len(), 1);
    row_menu(&mut h, "trips");
    click(&mut h, "Forget this source");
    assert!(h.state().import_sources().is_empty());
    assert!(!h.state().import_recent().contains(&trips));
    assert!(!h.state().import_ticked().contains(&trips));
    let text = kept
        .read(helios_gui::shell::IMPORT_SOURCES)
        .unwrap()
        .unwrap_or_default();
    assert!(!text.contains("trips"), "{text}");
    h.state_mut().undo_import_source_choice();
    h.run_steps(3);
    let sources = h.state().import_sources();
    assert_eq!(sources.len(), 1, "{sources:?}");
    assert_eq!(
        sources[0].choices.as_ref().unwrap().duplicates.as_str(),
        "keep"
    );
    assert!(h.state().import_recent().contains(&trips));
    drop(h);
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_offers_three_browsers_the_grid_follows_the_choice_and_it_comes_back_after_a_restart() {
    use helios_connect::Browser;
    let dir = temp("browsers");
    let shoot = dir.join("shoot");
    let day2 = shoot.join("day2");
    std::fs::create_dir_all(&day2).unwrap();
    put(&shoot.join("a.jpg"), Ok(&jpeg("a")), 2);
    put(&day2.join("b.jpg"), Ok(&jpeg("b")), 3);
    let vault = dir.join("vault.db");
    {
        let (node, kept) = node_at(&vault);
        let mut h = shell_keeping(&kept);
        open_on(&mut h, &shoot);
        // Three ways to browse a folder, all its photos to start with.
        let (offered, current) = h.state().import_source_browsers();
        assert_eq!(offered, [Browser::Folder, Browser::Tree, Browser::Timeline]);
        assert_eq!(current, Some(Browser::Folder));
        for label in ["Browse", "All photos", "Folder tree", "By date"] {
            let _ = h.get_by_label(label);
        }
        assert!(h.query_by_label("Albums").is_none());
        assert_eq!(h.state().import_counts().photos, 2);
        // The tree: a subfolder clicked shows its photos alone.
        click(&mut h, "Folder tree");
        let _ = h.get_by_label("Every folder");
        click(&mut h, "day2");
        assert_eq!(h.state().import_counts().photos, 1);
        assert_eq!(h.state().import_counts().selected, 1);
        click(&mut h, "Every folder");
        assert_eq!(h.state().import_counts().photos, 2);
        click(&mut h, "day2");
        // The timeline: every photo again, on the grid's agenda.
        let mode = |h: &Harness<'static, Shell>| {
            h.state()
                .import_browser()
                .map(|b| b.mode().to_owned())
                .unwrap_or_default()
        };
        let before = mode(&h);
        assert_ne!(before, "agenda");
        click(&mut h, "By date");
        assert_eq!(h.state().import_counts().photos, 2);
        assert_eq!(mode(&h), "agenda");
        // Back to all photos: the grid's mode before.
        click(&mut h, "All photos");
        assert_eq!(mode(&h), before);
        click(&mut h, "By date");
        let sources = h.state().import_sources();
        assert_eq!(sources.len(), 1, "{sources:?}");
        assert_eq!(sources[0].browser, Some(Browser::Timeline));
        let text = kept
            .read(helios_gui::shell::IMPORT_SOURCES)
            .unwrap()
            .expect("the sources kept by the node");
        assert!(text.contains("browser = \"timeline\""), "{text}");
        drop(h);
        node.stop();
    }
    // After a restart, the folder opens by date.
    let (node, kept) = node_at(&vault);
    let mut h = shell_keeping(&kept);
    open_on(&mut h, &shoot);
    h.run_steps(3);
    assert_eq!(
        h.state().import_source_browsers().1,
        Some(Browser::Timeline)
    );
    assert_eq!(
        h.state().import_browser().map(|b| b.mode().to_owned()),
        Some("agenda".to_owned())
    );
    drop(h);
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A deferred window's pass with input events, as eframe runs it: the
/// accessible nodes it drew, each a name, a rectangle (in points) and
/// whether it is disabled.
fn window_nodes(
    h: &mut Harness<'static, Shell>,
    window: egui::ViewportId,
    events: Vec<egui::Event>,
) -> Vec<(String, egui::Rect, bool)> {
    use egui::{Pos2, RawInput, Rect, ViewportId, ViewportInfo};
    let cb = h.output().viewport_output[&window]
        .viewport_ui_cb
        .clone()
        .expect("the window is declared by the main window's last pass");
    let rect = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
    let info = ViewportInfo {
        inner_rect: Some(rect),
        focused: Some(true),
        ..Default::default()
    };
    // The window's own clock, a frame on per pass (the context's input is
    // the main window's between passes): a tooltip's delay goes by.
    thread_local!(static CLOCK: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) });
    let time = CLOCK.with(|c| {
        let t = c.get().max(h.ctx.input(|i| i.time)) + 1.0 / 60.0;
        c.set(t);
        t
    });
    let mut input = RawInput {
        viewport_id: window,
        screen_rect: Some(rect),
        time: Some(time),
        focused: true,
        events,
        ..Default::default()
    };
    input.viewports.insert(window, info);
    input.viewports.insert(
        ViewportId::ROOT,
        ViewportInfo {
            focused: Some(false),
            ..Default::default()
        },
    );
    let mut out = h.ctx.run_ui(input, |ui| cb(ui));
    out.textures_delta.clear();
    let ppp = h.ctx.pixels_per_point();
    let Some(update) = out.platform_output.accesskit_update else {
        return Vec::new();
    };
    update
        .nodes
        .iter()
        .filter_map(|(_, n)| {
            let name = n.label().or_else(|| n.value())?.to_string();
            let b = n.bounds()?;
            let rect = Rect::from_min_max(
                Pos2::new(b.x0 as f32 / ppp, b.y0 as f32 / ppp),
                Pos2::new(b.x1 as f32 / ppp, b.y1 as f32 / ppp),
            );
            Some((name, rect, n.is_disabled()))
        })
        .collect()
}

/// Clicks the node `name` (or `name` and its key) a window's last pass drew: the pointer pressed
/// in one pass, released in the next; the nodes of the pass after.
fn click_in_window(
    h: &mut Harness<'static, Shell>,
    window: egui::ViewportId,
    nodes: &[(String, egui::Rect, bool)],
    name: &str,
) -> Vec<(String, egui::Rect, bool)> {
    let names: Vec<&String> = nodes.iter().map(|(n, _, _)| n).collect();
    let at = nodes
        .iter()
        // A menu entry's name ends with its key ("Close window Ctrl+W").
        .find(|(n, _, _)| n == name || n.starts_with(&format!("{name} ")))
        .unwrap_or_else(|| panic!("no `{name}` in {names:?}"))
        .1
        .center();
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    window_nodes(h, window, vec![egui::Event::PointerMoved(at), button(true)]);
    window_nodes(h, window, vec![button(false)]);
    window_nodes(h, window, Vec::new())
}

/// The menus a bar of `app.main` showed last, in the bar or in "…".
fn top_menus(app: &egui_pop::api::app::AppHandle) -> Vec<String> {
    let layout = egui_pop::api::menus::bar_layout(app, &egui_pop::api::app::MENU_MAIN)
        .expect("the main menu is drawn as a bar");
    layout
        .bar
        .iter()
        .chain(&layout.overflow)
        .map(ToString::to_string)
        .collect()
}

/// Every Helios window has the main window's title bar and menus
/// (docs/design/windows.md, section 7), as VS Code's windows do: the
/// import window's bar has the menu button, the logo and the name, and
/// the same menus as the main window, evaluated in the import window. A
/// command of the main window (Export…) is greyed there and says where
/// it runs; a command run from the import window's menu acts on it:
/// File > Close window closes the import window, not the main one.
#[test]
fn the_import_window_has_the_main_windows_title_bar_and_menus() {
    let (mut h, window, dir) = import_in_its_own_window_framed("chrome", true);
    let app = h.state().app().handle().clone();
    h.run();
    let main_menus = top_menus(&app);
    assert!(main_menus.len() >= 4, "{main_menus:?}");
    let nodes = window_nodes(&mut h, window, Vec::new());
    let names: Vec<&String> = nodes.iter().map(|(n, _, _)| n).collect();
    // The title bar's start: the menu button, the logo, the name.
    for name in ["Window menu", "Helios"] {
        assert!(names.iter().any(|n| *n == name), "{name} in {names:?}");
    }
    let named = h.ctx.data(|d| {
        d.get_temp::<std::collections::BTreeMap<String, (egui::Rect, egui::ViewportId)>>(
            egui::Id::unique("helios.named"),
        )
    });
    assert_eq!(
        named
            .unwrap_or_default()
            .get("helios.logo")
            .map(|(_, v)| *v),
        Some(window),
        "the logo is drawn in the import window's title bar"
    );
    // The same menus as the main window.
    assert_eq!(top_menus(&app), main_menus);
    for name in ["File", "Edit", "View", "Window"] {
        assert!(names.iter().any(|n| *n == name), "{name} in {names:?}");
    }
    // File > Export…: the main window's, greyed here, saying where it runs.
    let nodes = click_in_window(&mut h, window, &nodes, "File");
    let (export, _, disabled) = nodes
        .iter()
        .find(|(n, _, _)| n.starts_with("Export"))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "Export in {:?}",
                nodes.iter().map(|n| &n.0).collect::<Vec<_>>()
            )
        });
    assert!(disabled, "{export} is greyed in the import window");
    let at = nodes
        .iter()
        .find(|(n, _, _)| *n == export)
        .unwrap()
        .1
        .center();
    let mut said = None;
    let mut events = vec![egui::Event::PointerMoved(at)];
    for _ in 0..120 {
        let nodes = window_nodes(&mut h, window, std::mem::take(&mut events));
        said = nodes
            .into_iter()
            .map(|(n, _, _)| n)
            .find(|n| n.starts_with("Only in the "));
        if said.is_some() {
            break;
        }
    }
    let said = said.expect("the greyed entry says where it runs");
    assert!(said.ends_with(" window"), "{said}");
    // File > Close window, run from the import window: it closes.
    let nodes = window_nodes(
        &mut h,
        window,
        vec![egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
    let nodes = click_in_window(&mut h, window, &nodes, "File");
    click_in_window(&mut h, window, &nodes, "Close window");
    h.run_steps(3);
    assert!(!h.state().import_window_open(), "the import window closed");
    let open: Vec<String> = app
        .windows()
        .into_iter()
        .map(|w| w.id.to_string())
        .collect();
    assert_eq!(open, ["main"], "the main window stays");
    let _ = std::fs::remove_dir_all(&dir);
}
