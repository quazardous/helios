//! The import window's snapshots, pop-dark at 1x and 2x: empty (no
//! source), a card ticked with its photos in the file browser's
//! thumbnails, and a folder added where it is; at 1x, the card's Contents
//! in the browser's other modes (details, filmstrip, agenda)
//! (`tests/snapshots/`, thresholds in the workspace's `kittest.toml`).
//! Regenerate with `UPDATE_SNAPSHOTS=1 cargo test -p helios-gui --test
//! import_snapshots` (the box's `gui-snapshots` job).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::{Harness, SnapshotResults};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_storage::Card as Volume;
use serde_json::{Value, json};

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
    let mut shell = Shell::new(helios_panels(), None);
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
        results.add(h.try_snapshot(name("empty", scale)));
        // A card, ticked first, with its grid; copied.
        let cards = vec![Volume {
            label: "EOS_DIGITAL".into(),
            root: card.clone(),
            capacity: Some(64_000_000_000),
        }];
        let mut h = window(cards, json!(null), scale);
        assert_eq!(h.state().import_counts().selected, 3);
        assert!(h.state().import_copies());
        results.add(h.try_snapshot(name("card", scale)));
        // The Contents in the browser's other modes.
        if scale == 1.0 {
            for mode in ["details", "filmstrip", "agenda"] {
                assert!(h.state_mut().import_browser_mut().unwrap().set_mode(mode));
                for _ in 0..4 {
                    h.run();
                }
                results.add(h.try_snapshot(name(&format!("card-{mode}"), scale)));
            }
        }
        // A folder: added where it is.
        let mut h = window(Vec::new(), json!({ "paths": [folder] }), scale);
        assert!(!h.state().import_copies());
        results.add(h.try_snapshot(name("folder", scale)));
        // The import profile editor, on the folder's profile.
        h.state_mut().open_import_profiles();
        for _ in 0..4 {
            h.run();
        }
        assert!(h.state().import_profiles_open());
        results.add(h.try_snapshot(name("profiles", scale)));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
