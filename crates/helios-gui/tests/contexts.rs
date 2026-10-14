//! Places and modes (egui-pop's context stack) on the desktop: the
//! library, the darkroom on an image and the workspace's network, each
//! with its arrangement; culling, compare and mask editing; the image's
//! own states kept with their modes; the stack beside the workspace's
//! layout, back when it opens again, cut where its image is gone.

use std::path::{Path, PathBuf};

use egui::vec2;
use egui_kittest::Harness;
use egui_pop::model::commands::Origin as PopOrigin;
use egui_pop::model::ids::PanelId;
use egui_pop::model::panels::Area;
use helios_gui::contexts::{self, Opened};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

fn temp(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("helios-gui-contexts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("photos")).unwrap();
    dir
}

/// A window on the workspace `photos` of `dir`, its settings beside it.
fn window(dir: &Path) -> Harness<'static, Shell> {
    let mut shell = Shell::builder(helios_panels(), Some(dir.join("ui")))
        .workspace(&dir.join("photos"))
        .build();
    shell.load_layout().unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

fn key(h: &Harness<'_, Shell>, k: &str) -> String {
    h.state().app().handle().context().get(k).to_string()
}

fn run(h: &mut Harness<'_, Shell>, command: &str, args: serde_json::Value) {
    h.state_mut().run(command, args, Origin::User).unwrap();
    h.run();
}

fn panel(id: &str) -> PanelId {
    PanelId::new(id).unwrap()
}

#[test]
fn places_have_their_arrangements_and_the_breadcrumb_says_where() {
    let dir = temp("places");
    let mut h = window(&dir);
    assert_eq!(key(&h, "place"), "root");
    let root = h.state().layout();
    // The library: the vault's grid in the centre.
    run(&mut h, "view.library", json!(null));
    assert_eq!(key(&h, "place"), "helios.library");
    assert_eq!(
        h.state().layout().area_of(&panel("helios.vault")),
        Some(Area::Center)
    );
    // An image opened from it: the darkroom, under the library.
    let photo = dir.join("photo.png");
    std::fs::write(&photo, b"not read").unwrap();
    contexts::open(
        h.state().app().handle(),
        &Opened::File(photo.clone()),
        &PopOrigin::User,
    );
    h.run();
    assert_eq!(key(&h, "place"), "helios.darkroom");
    assert_eq!(key(&h, "placeDepth"), "2");
    assert_eq!(
        contexts::darkroom(h.state().app().handle()),
        Some(Opened::File(photo))
    );
    // Back: the library as it was, then the root.
    run(&mut h, "nav.back", json!(null));
    assert_eq!(key(&h, "place"), "helios.library");
    run(&mut h, "nav.back", json!(null));
    assert_eq!(h.state().layout(), root);
    // The workspace's network: its own, nobody else's.
    run(&mut h, "view.network", json!(null));
    assert_eq!(key(&h, "place"), "helios.workspace");
    assert_eq!(
        h.state().layout().area_of(&panel("helios.graph")),
        Some(Area::Center)
    );
    egui_pop_test::assert_keymap_clean(h.state().app());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn culling_is_an_orientation_with_its_keys_and_the_image_first() {
    let dir = temp("culling");
    let mut h = window(&dir);
    run(&mut h, "view.culling", json!({ "on": true }));
    assert_eq!(key(&h, "mode.orientation"), "helios.culling");
    let layout = h.state().layout();
    assert_eq!(
        layout.area_of(&panel("helios.viewport")),
        Some(Area::Center)
    );
    assert!(layout.hidden.contains(&panel("helios.params")));
    // Its keys reach the vault from anywhere: P picks, not a mask path.
    let keys = h
        .state()
        .app()
        .handle()
        .keys_for(&"vault.flag.pick".parse().unwrap());
    assert!(keys.iter().any(|k| k == "P"), "{keys:?}");
    egui_pop_test::assert_keymap_clean(h.state().app());
    // Toggled off: the layout as it was before.
    run(&mut h, "view.culling", json!(null));
    assert_eq!(key(&h, "mode.orientation"), "");
    assert!(!h.state().layout().hidden.contains(&panel("helios.params")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compare_and_mask_editing_follow_the_image_and_the_stack() {
    let dir = temp("follow");
    let mut h = window(&dir);
    let compare = |h: &Harness<'_, Shell>| {
        h.state()
            .values
            .lock()
            .unwrap()
            .get(helios_gui::viewport::COMPARE)
            .and_then(|e| e.value.as_bool())
            .unwrap_or(false)
    };
    // The command turns compare on: its mode is pushed.
    run(&mut h, "viewport.compare", json!(null));
    h.run();
    assert!(compare(&h));
    assert_eq!(key(&h, "mode.compare"), "helios.compare");
    assert_eq!(key(&h, "modePushed"), "true");
    // Back (Escape, a chip, an agent) pops it: compare goes off.
    run(&mut h, "nav.back", json!(null));
    h.run();
    assert_eq!(key(&h, "mode.compare"), "");
    assert!(!compare(&h));
    // An agent pushes mask editing: the path tool is on, the Masks panel
    // shown; popped, the tool is off.
    h.state_mut()
        .run(
            "context.set_mode",
            json!({ "mode": "helios.mask-editing", "push": true }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    h.run();
    h.run();
    let tool = |h: &Harness<'_, Shell>| helios_gui::masks::tool(&h.state().values.lock().unwrap());
    assert_eq!(tool(&h), "path");
    assert!(
        h.state()
            .app()
            .handle()
            .panel_visible(&panel("helios.masks"))
    );
    run(&mut h, "nav.back", json!(null));
    h.run();
    assert_eq!(tool(&h), "none");
    assert_eq!(key(&h, "mode.tool"), "");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_workspace_comes_back_where_it_was_left_or_falls_back() {
    let dir = temp("restore");
    let photo = dir.join("photo.png");
    std::fs::write(&photo, b"not read").unwrap();
    {
        let mut h = window(&dir);
        run(&mut h, "view.library", json!(null));
        contexts::open(
            h.state().app().handle(),
            &Opened::File(photo.clone()),
            &PopOrigin::User,
        );
        run(&mut h, "view.culling", json!({ "on": true }));
        run(&mut h, "viewport.compare", json!(null));
        h.run();
        assert_eq!(key(&h, "mode.compare"), "helios.compare");
        // Written beside the workspace's layout, inside each transition.
        let key = helios_gui::host::workspace_key(&dir.join("photos"));
        assert!(
            dir.join("ui/workspaces")
                .join(key)
                .join("context.json")
                .exists()
        );
    }
    // Opened again: the same places and modes, compare's flag with them.
    let mut h = window(&dir);
    assert_eq!(key(&h, "place"), "helios.darkroom");
    assert_eq!(key(&h, "mode.orientation"), "helios.culling");
    assert_eq!(key(&h, "mode.compare"), "helios.compare");
    h.run();
    assert!(
        h.state()
            .values
            .lock()
            .unwrap()
            .get(helios_gui::viewport::COMPARE)
            .and_then(|e| e.value.as_bool())
            .unwrap_or(false)
    );
    drop(h);
    // The image is gone since: the darkroom is cut, the library and the
    // modes stay, and a toast says why.
    std::fs::remove_file(&photo).unwrap();
    let h = window(&dir);
    assert_eq!(key(&h, "place"), "helios.library");
    assert_eq!(key(&h, "mode.orientation"), "helios.culling");
    let toasts = h
        .state()
        .app()
        .handle()
        .service(&egui_pop::api::toasts::TOASTS)
        .unwrap();
    assert!(
        toasts
            .shown()
            .iter()
            .any(|t| t.toast.text.contains("is gone")),
        "{:?}",
        toasts.shown()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
