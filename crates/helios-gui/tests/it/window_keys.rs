//! Keys stay in the window they are pressed in: an arrow pressed in the
//! import window (an application window, egui's deferred viewport drawn
//! in its own pass) never steps the main window's library, though the
//! library's grid was the panel focused last in the main window.
//!
//! kittest embeds windows in the main one: these tests run real
//! viewports, as eframe does, the main window's pass then the import
//! window's own.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, ViewportId, ViewportInfo, vec2};
use egui_kittest::Harness;
use helios_core::{Bus, ContentHash};
use helios_gui::persist::Persist;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_vault::{Image, ImageId, Store};
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-window-keys-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A vault file with images 1 to 3.
fn vault_file(dir: &Path) -> PathBuf {
    let path = dir.join("vault.db");
    let mut store = SqliteStore::open(&path).unwrap();
    for n in 1..=3 {
        store
            .add_to_library(&Image {
                id: ImageId(ContentHash([n; 32])),
                size: 10,
                partial: ContentHash([n; 32]),
            })
            .unwrap();
    }
    path
}

fn run(h: &mut Harness<'_, Shell>, command: &str, args: Value) {
    h.state_mut()
        .run(command, args, Origin::User)
        .unwrap_or_else(|e| panic!("{command}: {e}"));
}

/// The windows the main window's last pass declared, egui's deferred
/// viewports, as eframe would open them.
fn declared(h: &Harness<'static, Shell>) -> Vec<ViewportId> {
    h.output()
        .viewport_output
        .iter()
        .filter(|(v, o)| **v != ViewportId::ROOT && o.viewport_ui_cb.is_some())
        .map(|(v, _)| *v)
        .collect()
}

/// A deferred window's own pass, focused, the main window not, given
/// these events.
fn window_pass(h: &mut Harness<'static, Shell>, window: ViewportId, events: Vec<Event>) {
    let cb = h.output().viewport_output[&window]
        .viewport_ui_cb
        .clone()
        .expect("the window is declared by the main window's last pass");
    let rect = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
    let mut input = RawInput {
        viewport_id: window,
        screen_rect: Some(rect),
        time: Some(h.ctx.input(|i| i.time) + 1.0 / 60.0),
        focused: true,
        events,
        ..Default::default()
    };
    input.viewports.insert(
        window,
        ViewportInfo {
            inner_rect: Some(rect),
            focused: Some(true),
            ..Default::default()
        },
    );
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

fn key(key: Key, pressed: bool) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn an_arrow_pressed_in_the_import_window_does_not_step_the_main_window_s_library() {
    let dir = scratch("import-arrows");
    let path = vault_file(&dir);
    let mut shell = Shell::new(helios_panels(), None);
    let persist = Persist::new(SqliteStore::open(&path).unwrap(), Arc::new(Bus::new()));
    persist.fill_library(&mut shell).unwrap();
    shell.library_mut().select(0);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    h.run();
    // The library's grid focused in the main window: there, Right steps
    // to the next photo (the control: the binding is live).
    run(&mut h, "helios.vault.focus", json!(null));
    h.run();
    h.key_press(Key::ArrowRight);
    h.run();
    assert_eq!(
        h.state().library().selected(),
        Some(1),
        "Right in the main window steps the library"
    );
    // The import window opens in its own OS window and is worked in.
    run(&mut h, "library.importWindow", Value::Null);
    h.run_steps(3);
    assert!(h.state().import_window_open());
    let windows = declared(&h);
    assert_eq!(windows.len(), 1, "the import window's own: {windows:?}");
    let import = windows[0];
    window_pass(&mut h, import, Vec::new());
    // Right, then Left twice, pressed in the import window.
    for k in [Key::ArrowRight, Key::ArrowLeft, Key::ArrowLeft] {
        window_pass(&mut h, import, vec![key(k, true)]);
        window_pass(&mut h, import, vec![key(k, false)]);
        // The commands a window's keys asked run in the main window's
        // next pass.
        h.run_steps(2);
        assert_eq!(
            h.state().library().selected(),
            Some(1),
            "{k:?} pressed in the import window stepped the main window's library"
        );
    }
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&dir);
}
