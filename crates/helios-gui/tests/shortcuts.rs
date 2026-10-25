//! The default keymap (see `docs/gui.md`, "Keyboard shortcuts"): it loads
//! with no conflict and no single global letter; each culling key runs
//! its command in the library and does nothing in the darkroom; the
//! darktable package loads over it and comes off again.

use std::path::{Path, PathBuf};

use egui::vec2;
use egui_kittest::Harness;
use egui_pop::model::commands::Origin as PopOrigin;
use egui_pop::model::ids::CommandId;
use egui_pop::model::keymap::{Keys, Resolution};
use helios_gui::contexts::{self, Opened};
use helios_gui::library::Entry;
use helios_gui::{Shell, headless, helios_panels, keymaps, keys};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-shortcuts-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("photos")).unwrap();
    dir
}

/// A window on the workspace `photos` of `dir`, three images in its
/// library, the first selected.
fn window(dir: &Path) -> Harness<'static, Shell> {
    let mut shell = Shell::builder(helios_panels(), Some(dir.join("ui")))
        .workspace(&dir.join("photos"))
        .build();
    shell.load_layout().unwrap();
    let entries: Vec<Entry> = ["a", "b", "c"]
        .iter()
        .map(|id| Entry {
            id: id.to_string(),
            name: format!("{id}.raf"),
            ..Entry::default()
        })
        .collect();
    shell.set_library(entries);
    shell.library_mut().select(0);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

fn key(h: &Harness<'_, Shell>, k: &str) -> String {
    h.state().app().handle().context().get(k).to_string()
}

fn run(h: &mut Harness<'_, Shell>, command: &str, args: Value) {
    h.state_mut()
        .run(command, args, Origin::User)
        .unwrap_or_else(|e| panic!("{command}: {e}"));
    h.run();
}

/// The culling keys of the default map: those bound in the library.
fn culling_keys() -> Vec<(String, String)> {
    let library = keys::library();
    keys::defaults()
        .into_iter()
        .filter(|(_, _, when)| *when == library)
        .map(|(k, command, _)| (k, command))
        .collect()
}

/// What the keymap does with `keys` here, the vault's grid having the
/// focus.
fn resolve_in_vault(h: &Harness<'_, Shell>, keys: &str) -> Resolution {
    let mut cx = h.state().app().handle().when_context();
    cx.set("focusedPanel", "helios.vault");
    h.state()
        .app()
        .keymap()
        .resolve(&Keys::parse(keys).unwrap(), &cx)
}

/// Presses `keys` (one chord) and returns how the keymap resolved them,
/// when it did.
fn press(h: &mut Harness<'_, Shell>, keys: &str) -> Option<Resolution> {
    let parsed = Keys::parse(keys).unwrap();
    let (modifiers, k) = egui_pop::core::keys::key_of(&parsed.0[0]).unwrap();
    let before = h.state().app().handle().key_traces().last().cloned();
    h.key_press_modifiers(modifiers, k);
    h.run();
    let last = h.state().app().handle().key_traces().last().cloned();
    last.filter(|t| Some(t) != before.as_ref() && t.keys == parsed.to_string())
        .map(|t| t.resolution)
}

#[test]
fn the_default_map_has_no_conflict_and_no_global_letter() {
    let shell = Shell::new(helios_panels(), None);
    egui_pop_test::assert_keymap_clean(shell.app());
    let app = shell.app().handle();
    for (k, command, when) in keys::defaults() {
        let parsed = Keys::parse(&k).unwrap();
        // Every key of the map is live on its command.
        let id = CommandId::new(&command).unwrap();
        assert!(
            app.keys_for(&id).contains(&parsed.to_string()),
            "`{k}` on {command}: {:?}",
            app.keys_for(&id)
        );
        // A key without a clause has a modifier, or is a chord sequence.
        let first = &parsed.0[0];
        assert!(
            !when.is_empty() || parsed.0.len() > 1 || first.ctrl || first.alt || first.meta,
            "`{k}` ({command}) is global without a modifier"
        );
    }
}

#[test]
fn culling_keys_run_their_commands_in_the_library_and_nothing_in_the_darkroom() {
    let dir = temp("culling");
    let mut h = window(&dir);
    let keys = culling_keys();
    assert!(keys.len() > 20, "{keys:?}");
    // In the library, the grid having the focus.
    run(&mut h, "view.library", json!(null));
    run(&mut h, "helios.vault.focus", json!(null));
    assert_eq!(key(&h, "place"), "helios.library");
    assert_eq!(key(&h, "focusedPanel"), "helios.vault");
    for (k, command) in &keys {
        let want = Resolution::Run(CommandId::new(command).unwrap());
        assert_eq!(resolve_in_vault(&h, k), want, "`{k}` in the library");
        assert_eq!(press(&mut h, k), Some(want), "`{k}` pressed in the library");
    }
    assert!(
        !h.state_mut().take_library_changes().is_empty(),
        "the keys marked the images"
    );
    // While culling, Space and Backspace step through the images too.
    run(&mut h, "view.culling", json!({ "on": true }));
    let at = h.state().library().selected;
    assert_eq!(
        press(&mut h, "Space"),
        Some(Resolution::Run(CommandId::from_static("vault.next")))
    );
    assert_ne!(h.state().library().selected, at);
    assert_eq!(
        press(&mut h, "Backspace"),
        Some(Resolution::Run(CommandId::from_static("vault.previous")))
    );
    assert_eq!(h.state().library().selected, at);
    run(&mut h, "view.culling", json!({ "on": false }));
    // In the darkroom, on an image of the library: none of them.
    let photo = dir.join("photo.png");
    std::fs::write(&photo, b"not read").unwrap();
    contexts::open(
        h.state().app().handle(),
        &Opened::File(photo),
        &PopOrigin::User,
    );
    h.run();
    assert_eq!(key(&h, "place"), "helios.darkroom");
    let _ = h.state_mut().take_library_changes();
    let at = h.state().library().selected;
    for (k, command) in &keys {
        let culling = Resolution::Run(CommandId::new(command).unwrap());
        // Not even with the vault's grid focused there.
        assert_ne!(resolve_in_vault(&h, k), culling, "`{k}` in the darkroom");
        assert_ne!(
            press(&mut h, k),
            Some(culling),
            "`{k}` pressed in the darkroom"
        );
    }
    assert!(h.state_mut().take_library_changes().is_empty());
    assert_eq!(h.state().library().selected, at);
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_darktable_package_loads_over_the_defaults_and_comes_off_again() {
    let shell = Shell::new(helios_panels(), None);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let app = h.state().app().handle().clone();
    let keys_of = |id: &'static str| app.keys_for(&CommandId::from_static(id));
    run(
        &mut h,
        "keymap.package",
        json!({ "name": "darktable-classic" }),
    );
    // Clean: F1 is taken from the palette before it labels red.
    egui_pop_test::assert_keymap_clean(h.state().app());
    assert!(keys_of("vault.label.red").contains(&"F1".to_string()));
    let palette = keys_of("app.palette");
    assert!(!palette.contains(&"F1".to_string()), "{palette:?}");
    assert!(palette.contains(&"Ctrl+Shift+P".to_string()), "{palette:?}");
    assert!(keys_of("view.library").contains(&"L".to_string()));
    assert!(keys_of("view.side_bar.toggle").contains(&"Ctrl+Shift+L".to_string()));
    // What it leaves out of darktable's keys, it does not bind.
    let package = keymaps::package("darktable-classic").unwrap();
    assert!(!package.left_out.is_empty());
    for left in &package.left_out {
        for k in left.keys.split(", ") {
            let k = Keys::parse(k).unwrap();
            assert!(
                !package
                    .keymap
                    .iter()
                    .any(|b| b.command.is_some() && Keys::parse(&b.keys).ok() == Some(k.clone())),
                "`{k}` is left out and bound"
            );
        }
    }
    // Twice is once.
    run(
        &mut h,
        "keymap.package",
        json!({ "name": "darktable-classic" }),
    );
    egui_pop_test::assert_keymap_clean(h.state().app());
    // Taken away: the defaults again.
    run(
        &mut h,
        "keymap.package.remove",
        json!({ "name": "darktable-classic" }),
    );
    assert!(keys_of("app.palette").contains(&"F1".to_string()));
    assert!(!keys_of("vault.label.red").contains(&"F1".to_string()));
    assert!(keys_of("view.library").is_empty());
    assert!(
        h.state()
            .app()
            .handle()
            .export_keymap(egui_pop::model::keymap::Layer::User)
            .bindings
            .is_empty()
    );
}
