//! The import window's snapshots, pop-dark at 1x and 2x: empty (no
//! source), a card ticked with its photos in the file browser's
//! thumbnails, and a folder added where it is; at 1x, the card's Contents
//! in the browser's other modes (details, filmstrip, agenda), and the
//! "Add…" wizard's steps (the folder step with a folder that is not
//! there)
//! (`tests/snapshots/`, thresholds in the workspace's `kittest.toml`).
//! Each picture is the import window alone, not the main window behind
//! it (`shot::Shots::try_window_shot`).
//! Regenerate with `UPDATE_SNAPSHOTS=1 cargo test -p helios-gui --test
//! it import_snapshots::` (the box's `gui-snapshots` job).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::kittest::Queryable as _;
use egui_kittest::{Harness, SnapshotResults};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_storage::Card as Volume;
use helios_vault::{MemoryStore, Vault};
use serde_json::{Value, json};

use crate::shot::Shots as _;

/// The import window's title, as its picture is cut to it.
const IMPORT_PHOTOS: &str = "Import photos";

/// The same folder on every run: the window shows folder names.
fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-import-shots-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A tiny JPEG of the fixtures (`a`, `b` or `c`).
fn jpeg(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../helios-plugin/tests/fixtures/rated-photos")
        .join(format!("{name}.jpg"))
}

/// Copies a fixture to `to`, last modified at noon of `day` of November
/// 2026, local time.
fn put(to: &Path, from: &Path, day: u32) {
    use chrono::TimeZone as _;
    std::fs::copy(from, to).unwrap();
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

/// Three photos over two days under `dir/DCIM/100CANON`.
fn photos(dir: &Path) -> PathBuf {
    let dcim = dir.join("DCIM/100CANON");
    std::fs::create_dir_all(&dcim).unwrap();
    put(&dcim.join("IMG_0001.JPG"), &jpeg("a"), 2);
    put(&dcim.join("IMG_0002.JPG"), &jpeg("b"), 2);
    put(&dcim.join("IMG_0003.JPG"), &jpeg("c"), 3);
    dcim
}

/// The shell watching `cards`, the import window opened with `args` and
/// its sources read through, at `scale`.
fn window(cards: Vec<Volume>, args: Value, scale: f32) -> Harness<'static, Shell> {
    window_with(cards, args, scale, |_| {})
}

/// [`window`], the shell set up by `setup` first.
fn window_with(
    cards: Vec<Volume>,
    args: Value,
    scale: f32,
    setup: impl FnOnce(&mut Shell),
) -> Harness<'static, Shell> {
    let mut shell = Shell::new(helios_panels(), None);
    setup(&mut shell);
    let any = !cards.is_empty();
    shell.watch_cards(Arc::new(cards));
    let mut h = headless::harness_at(shell, vec2(1280.0, 800.0), scale);
    h.run();
    let deadline = Instant::now() + Duration::from_secs(10);
    while any && h.state().import_cards().is_empty() {
        assert!(Instant::now() < deadline, "no card listed in 10 s");
        std::thread::sleep(Duration::from_millis(20));
        h.run();
    }
    h.state_mut()
        .run("library.importWindow", args, Origin::User)
        .unwrap();
    h.step();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !h.state().import_read() {
        assert!(Instant::now() < deadline, "the sources not read in 20 s");
        std::thread::sleep(Duration::from_millis(20));
        h.step();
    }
    for _ in 0..4 {
        h.run();
    }
    h
}

/// A card's photos with marks: one the Library holds (`IMG_0002`, from
/// another folder) and a RAW with its JPEG (`IMG_0003`); what the
/// Library holds.
fn marked(dir: &Path) -> helios_gui::import_scan::Known {
    let dcim = dir.join("MARKED/DCIM/100CANON");
    std::fs::create_dir_all(&dcim).unwrap();
    put(&dcim.join("IMG_0001.JPG"), &jpeg("a"), 2);
    put(&dcim.join("IMG_0002.JPG"), &jpeg("b"), 2);
    put(&dcim.join("IMG_0003.CR2"), &jpeg("c"), 3);
    put(&dcim.join("IMG_0003.JPG"), &jpeg("c"), 3);
    let library = dir.join("library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::copy(jpeg("b"), library.join("b.jpg")).unwrap();
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    vault.import_file(&library.join("b.jpg"), 0).unwrap();
    let client = helios_gui::persist::Persist::on(vault, Arc::new(helios_core::Bus::new()))
        .node()
        .clone();
    helios_gui::import_scan::known_of(client)
}

fn name(state: &str, scale: f32) -> String {
    format!("import-window-{state}-pop-dark-{scale}x")
}

#[test]
fn the_import_window_snapshots() {
    let mut results = SnapshotResults::new();
    let dir = temp("states");
    let card = dir.join("EOS_DIGITAL");
    photos(&card);
    let folder = photos(&dir.join("Trip"));
    for scale in [1.0, 2.0] {
        // No source: nothing to import yet.
        let mut h = window(Vec::new(), json!(null), scale);
        assert!(h.state().import_ticked().is_empty());
        results.add(h.try_window_shot(name("empty", scale), IMPORT_PHOTOS));
        // A card, ticked first, with its grid; copied.
        let cards = vec![Volume {
            label: "EOS_DIGITAL".into(),
            root: card.clone(),
            capacity: Some(64_000_000_000),
        }];
        let mut h = window(cards, json!(null), scale);
        assert_eq!(h.state().import_counts().selected, 3);
        assert!(h.state().import_copies());
        results.add(h.try_window_shot(name("card", scale), IMPORT_PHOTOS));
        // The Contents in the browser's other modes.
        if scale == 1.0 {
            for mode in ["details", "filmstrip", "agenda"] {
                assert!(h.state_mut().import_browser_mut().unwrap().set_mode(mode));
                for _ in 0..4 {
                    h.run();
                }
                results.add(h.try_window_shot(name(&format!("card-{mode}"), scale), IMPORT_PHOTOS));
            }
        }
        // A card whose photos carry marks: in your Library (an icon, top
        // right) and RAW+JPG (an "R+J" chip, bottom right).
        let cards = vec![Volume {
            label: "MARKED".into(),
            root: dir.join("MARKED"),
            capacity: Some(64_000_000_000),
        }];
        let known = marked(&dir);
        let mut h = window_with(cards, json!(null), scale, |s| s.set_import_known(known));
        let _ = h.get_by_label("IMG_0002.JPG · in your Library");
        let _ = h.get_by_label("IMG_0003.CR2 + IMG_0003.JPG · RAW+JPG");
        results.add(h.try_window_shot(name("card-marks", scale), IMPORT_PHOTOS));
        // A folder: added where it is.
        let mut h = window(Vec::new(), json!({ "paths": [folder] }), scale);
        assert!(!h.state().import_copies());
        results.add(h.try_window_shot(name("folder", scale), IMPORT_PHOTOS));
        // The import profile editor, on the folder's profile.
        h.state_mut().open_import_profiles();
        for _ in 0..4 {
            h.run();
        }
        assert!(h.state().import_profiles_open());
        results.add(h.try_window_shot(name("profiles", scale), IMPORT_PHOTOS));
    }
    // The "Add…" wizard's steps (1x), on the card connector: its kind,
    // the volume, how to import and the name.
    let cards = vec![Volume {
        label: "EOS_DIGITAL".into(),
        root: card.clone(),
        capacity: Some(64_000_000_000),
    }];
    let mut h = window(cards, json!(null), 1.0);
    for (click, state) in [
        ("Add…", None),
        ("Cards and cameras", Some("wizard-kind")),
        ("Next", None),
        ("EOS_DIGITAL", Some("wizard-volume")),
        ("Next", Some("wizard-how")),
        ("Next", Some("wizard-name")),
    ] {
        h.query_all_by_label(click)
            .next()
            .unwrap_or_else(|| panic!("no {click}"))
            .click();
        for _ in 0..4 {
            h.run();
        }
        if let Some(state) = state {
            assert!(h.state().import_wizard_open(), "{state}");
            results.add(h.try_window_shot(name(state, 1.0), IMPORT_PHOTOS));
        }
    }
    h.query_all_by_label("Add the source")
        .next()
        .unwrap()
        .click();
    for _ in 0..4 {
        h.run();
    }
    let sources = h.state().import_sources();
    assert_eq!(sources.len(), 1, "{sources:?}");
    assert_eq!(sources[0].connector, "card");
    assert_eq!(sources[0].name.as_deref(), Some("EOS_DIGITAL"));
    // The folder step, a folder typed that is not there: the field, its
    // Choose… button and the warning.
    let mut h = window(Vec::new(), json!(null), 1.0);
    for click in ["Add…", "Folder", "Next"] {
        h.query_all_by_label(click)
            .next()
            .unwrap_or_else(|| panic!("no {click}"))
            .click();
        for _ in 0..4 {
            h.run();
        }
    }
    h.query_all_by_role(egui::accesskit::Role::TextInput)
        .find(egui_kittest::Node::is_focused)
        .expect("the folder field focused")
        .type_text("/photos/Trip");
    for _ in 0..4 {
        h.run();
    }
    let _ = h.get_by_label("No folder there");
    results.add(h.try_window_shot(name("wizard-folder", 1.0), IMPORT_PHOTOS));
    let _ = std::fs::remove_dir_all(&dir);
}
