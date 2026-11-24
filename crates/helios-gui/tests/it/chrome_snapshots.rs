//! The shell's chrome in a suite of its own: the top bars (the title bar
//! of a workspace's window, the menu bar) and the status bar, pop-dark at
//! 1x and 2x, the status bar with a job running, and one picture of the
//! whole main window (`tests/snapshots/chrome-*.png`, thresholds in the
//! workspace's `kittest.toml`).
//!
//! Every other suite of the shell cuts its pictures to what it tests
//! (`shot`), so a change to a bar moves these pictures and no others.
//! Regenerate with `UPDATE_SNAPSHOTS=1 cargo test -p helios-gui --test it
//! chrome_snapshots::` (the box's `gui-snapshots` job with
//! `test=chrome_snapshots`).

use std::path::PathBuf;

use egui::vec2;
use egui_kittest::{Harness, SnapshotResults};
use helios_gui::{Shell, headless, helios_panels};
use helios_vault::promises::{Promise, State};
use serde_json::json;

use crate::shot::{Shots as _, bars};

const SIZE: egui::Vec2 = vec2(1280.0, 800.0);

/// The main window as it opens, settled, at `scale`.
fn main_window(scale: f32) -> Harness<'static, Shell> {
    let mut h = headless::harness_at(Shell::new(helios_panels(), None), SIZE, scale);
    h.run();
    h
}

/// A workspace's window (with a title bar of its own), in a folder of the
/// same name on every run.
fn workspace_window() -> (Harness<'static, Shell>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("helios-gui-chrome-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("photos")).unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(dir.join("ui")))
        .workspace(&dir.join("photos"))
        .build();
    shell.load_layout().unwrap();
    let mut h = headless::harness_at(shell, SIZE, 1.0);
    h.run();
    (h, dir)
}

/// A running import of known counts and times.
fn import_running() -> Promise {
    let at = 1_794_650_000;
    Promise {
        id: "promise-1".into(),
        owner: "import-promise-1".into(),
        kind: "import".into(),
        label: "Import 412 photos".into(),
        request: json!({ "origin": "user", "source": "EOS_DIGITAL" }),
        state: State::Running {
            done: 120,
            total: Some(412),
            note: Some("IMG_0120.JPG".into()),
        },
        cancel_asked: false,
        attempts: 1,
        paused: None,
        created: at - 95,
        updated: at,
    }
}

#[test]
fn the_chrome_has_its_own_snapshots() {
    let mut results = SnapshotResults::new();
    for scale in [1.0, 2.0] {
        let mut h = main_window(scale);
        let b = bars(&h);
        assert!(b.top.height() > 0.0, "a menu bar: {b:?}");
        assert!(b.status.height() > 0.0, "a status bar: {b:?}");
        results.add(h.try_shot_of(format!("chrome-top-bar-pop-dark-{scale}x"), b.top));
        results.add(h.try_shot_of(format!("chrome-status-bar-pop-dark-{scale}x"), b.status));
        if scale == 1.0 {
            // The one picture of the whole main window.
            results.add(h.try_whole_shot("chrome-window-pop-dark-1x"));
            // A job running: the status bar's activity indicator.
            h.state_mut()
                .show_activity_at(vec![import_running()], 1_794_650_000);
            // The spinner repaints for ever: a few frames, not a settled
            // run.
            h.run_steps(4);
            let b = bars(&h);
            results.add(h.try_shot_of("chrome-status-bar-activity-pop-dark-1x", b.status));
        }
    }
    // A workspace's window: the title bar over the menu bar.
    let (mut h, dir) = workspace_window();
    let b = bars(&h);
    results.add(h.try_shot_of("chrome-top-bar-workspace-pop-dark-1x", b.top));
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
