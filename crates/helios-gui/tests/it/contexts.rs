//! Places and modes (egui-pop's context stack) on the desktop: the
//! library, the darkroom on an image and the workspace's network, each
//! with its arrangement; culling, compare and mask editing; the image's
//! own states kept with their modes; the stack beside the workspace's
//! layout, back when it opens again, cut where its image is gone.

use std::path::{Path, PathBuf};

use crate::host::Settle as _;
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
    h.settle();
    h
}

fn key(h: &Harness<'_, Shell>, k: &str) -> String {
    h.state().app().handle().context().get(k).to_string()
}

fn run(h: &mut Harness<'_, Shell>, command: &str, args: serde_json::Value) {
    h.state_mut().run(command, args, Origin::User).unwrap();
    h.settle();
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
    h.settle();
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

fn items(h: &Harness<'_, Shell>) -> Vec<(String, String)> {
    egui_pop::api::strips::visible(h.state().app().handle())
        .into_iter()
        .filter(|i| i.id.starts_with("helios.status."))
        .map(|i| (i.id, i.text))
        .collect()
}

fn shows(h: &Harness<'_, Shell>, id: &str) -> bool {
    items(h).iter().any(|(i, _)| i == id)
}

/// egui-pop's status bar starts with the breadcrumb (the root named
/// Helios, the darkroom on a file by the file's name) and a chip per mode
/// on, in its tone; Helios's items follow the place and the mode.
#[test]
fn the_status_bar_says_where_and_its_items_follow_the_place() {
    use helios_gui::status_items::{CULLED, EDIT, HISTORY, IMAGES, ZOOM};
    let dir = temp("status");
    let mut h = window(&dir);
    let crumbs = |h: &Harness<'_, Shell>| egui_pop::api::contexts::crumbs(h.state().app().handle());
    assert_eq!(crumbs(&h), ["Helios"]);
    assert!(items(&h).is_empty(), "{:?}", items(&h));
    // The library: its images counted.
    run(&mut h, "view.library", json!(null));
    assert_eq!(crumbs(&h), ["Helios", "Library"]);
    assert_eq!(
        items(&h),
        [(IMAGES.to_string(), "0 images".to_string())],
        "{:?}",
        egui_pop::api::strips::status(h.state().app().handle())
    );
    // The darkroom on a file: the zoom, the history and the edit, not the count.
    let photo = dir.join("photo.png");
    std::fs::write(&photo, b"not read").unwrap();
    contexts::open(
        h.state().app().handle(),
        &Opened::File(photo),
        &PopOrigin::User,
    );
    h.settle();
    assert_eq!(crumbs(&h), ["Helios", "Library", "Darkroom · photo.png"]);
    assert!(shows(&h, ZOOM) && shows(&h, HISTORY), "{:?}", items(&h));
    assert!(!shows(&h, IMAGES), "{:?}", items(&h));
    // The photo as it came: "Original" until a module changes.
    assert!(
        items(&h).contains(&(EDIT.to_string(), "Original".to_string())),
        "{:?}",
        items(&h)
    );
    // Culling: a chip in the accent tone, and the picks and rejects.
    run(&mut h, "view.culling", json!({ "on": true }));
    let chips = egui_pop::api::contexts::chips(h.state().app().handle());
    let culling = chips
        .iter()
        .find(|c| c.frame == contexts::CULLING)
        .expect("culling has its chip");
    assert_eq!(culling.tone.as_deref(), Some("accent"));
    assert!(shows(&h, CULLED), "{:?}", items(&h));
    run(&mut h, "view.culling", json!({ "on": false }));
    assert!(!shows(&h, CULLED), "{:?}", items(&h));
    let _ = std::fs::remove_dir_all(&dir);
}

/// darktable's pointer profile, once chosen: a double click on the
/// image leaves the darkroom, back where it was opened from.
#[test]
fn darktable_s_double_click_on_the_image_leaves_the_darkroom() {
    let dir = temp("leave");
    let mut h = window(&dir);
    egui_pop::api::pointer::set_profile(h.state().app().handle(), "darktable").unwrap();
    run(&mut h, "view.library", json!(null));
    let photo = dir.join("photo.png");
    std::fs::write(&photo, b"not read").unwrap();
    contexts::open(
        h.state().app().handle(),
        &Opened::File(photo),
        &PopOrigin::User,
    );
    // The desktop loads the file (main.rs); here the viewport is given an
    // image of its own: the pointer's gestures need one shown.
    let mut g = helios_core::Graph::new();
    let src = g.add_source(
        "source",
        helios_core::Image::from_fn(helios_core::Region::sized(64, 48), 4, |_, _, _| 0.5),
    );
    h.state_mut().set_view_source(g, src, "image").unwrap();
    for _ in 0..8 {
        h.step();
    }
    h.settle();
    assert_eq!(key(&h, "place"), "helios.darkroom");
    let at = h
        .state()
        .target_rect("viewport.display")
        .expect("the darkroom shows the image")
        .center();
    h.hover_at(at);
    h.step();
    let mut t = h.ctx.input(|i| i.time) + 1.0;
    for pressed in [true, false, true, false] {
        t += 0.05;
        h.input_mut().time = Some(t);
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
    }
    h.settle();
    assert_eq!(key(&h, "place"), "helios.library");
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
            .values()
            .lock()
            .unwrap()
            .get(helios_gui::viewport::COMPARE)
            .and_then(|e| e.value.as_bool())
            .unwrap_or(false)
    };
    // The command turns compare on: its mode is pushed.
    run(&mut h, "viewport.compare", json!(null));
    h.settle();
    assert!(compare(&h));
    assert_eq!(key(&h, "mode.compare"), "helios.compare");
    assert_eq!(key(&h, "modePushed"), "true");
    // Back (Escape, a chip, an agent) pops it: compare goes off.
    run(&mut h, "nav.back", json!(null));
    h.settle();
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
    h.settle();
    h.settle();
    let tool =
        |h: &Harness<'_, Shell>| helios_gui::masks::tool(&h.state().values().lock().unwrap());
    assert_eq!(tool(&h), "path");
    assert!(
        h.state()
            .app()
            .handle()
            .panel_visible(&panel("helios.masks"))
    );
    run(&mut h, "nav.back", json!(null));
    h.settle();
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
        h.settle();
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
    h.settle();
    assert!(
        h.state()
            .values()
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

/// `gui.snapshot` by name, asked while its target is not drawn yet (the
/// photo opened, the darkroom entered a frame later, as in the first-user
/// journey), is taken from the first frame that draws it, round after
/// round.
#[test]
fn a_picture_by_name_waits_for_its_target_to_be_drawn() {
    let dir = temp("picture");
    let mut h = window(&dir);
    let photo = dir.join("photo.png");
    std::fs::write(&photo, b"not read").unwrap();
    for round in 0..3 {
        run(&mut h, "view.library", json!(null));
        assert!(
            h.state().target_rect("viewport.display").is_err(),
            "round {round}: the library does not show the image"
        );
        let path = dir.join(format!("viewport-{round}.png"));
        h.state_mut()
            .run(
                "gui.snapshot",
                json!({ "target": "viewport.display", "path": path }),
                Origin::Agent("a1".into()),
            )
            .unwrap();
        // Asked while the library shows: nothing pictured yet.
        h.step();
        h.step();
        assert!(!path.exists(), "round {round}: pictured before it is shown");
        contexts::open(
            h.state().app().handle(),
            &Opened::File(photo.clone()),
            &PopOrigin::User,
        );
        for _ in 0..8 {
            h.step();
            if path.exists() {
                break;
            }
        }
        assert!(
            path.exists(),
            "round {round}: not pictured; the status line: {:?}",
            h.state().status_text()
        );
        assert_eq!(key(&h, "place"), "helios.darkroom");
        let rect = h.state().target_rect("viewport.display").unwrap();
        let png = image::open(&path).unwrap();
        let ppp = h.state().pixels_per_point();
        let near = |got: u32, points: f32| got.abs_diff((points * ppp).round() as u32) <= 1;
        assert!(
            near(png.width(), rect.width()) && near(png.height(), rect.height()),
            "round {round}: {}x{} pictured, the image's rectangle is {rect:?}",
            png.width(),
            png.height()
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A name no frame draws fails as not shown, once its wait is out, and
/// not before.
#[test]
fn a_picture_of_a_name_never_drawn_fails_after_its_wait() {
    let dir = temp("picture-absent");
    let mut h = window(&dir);
    let path = dir.join("absent.png");
    h.state_mut()
        .run(
            "gui.snapshot",
            json!({ "target": "nothing.here", "path": path }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    let start = h.ctx.input(|i| i.time);
    let failed = |h: &Harness<'_, Shell>| {
        h.state()
            .status_text()
            .is_some_and(|t| t.contains("nothing named `nothing.here` is shown"))
    };
    while h.ctx.input(|i| i.time) - start < helios_gui::shell::PICTURE_WAIT - 0.5 {
        h.step();
        assert!(!failed(&h), "failed before its wait is out");
    }
    for _ in 0..8 {
        h.step();
    }
    assert!(failed(&h), "the status line: {:?}", h.state().status_text());
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A panel detached into a window of its own in the darkroom stays in
/// its window when another photo opens: the darkroom on another photo is
/// the same place for the layout, not one starting from the darkroom's
/// arrangement (which docks the panel back as a tab).
#[test]
fn a_detached_panel_stays_in_its_window_when_another_photo_opens() {
    let dir = temp("detached");
    let shell = Shell::builder(helios_panels(), Some(dir.join("ui")))
        .workspace(&dir.join("photos"))
        .build();
    let (mut h, windows) = headless::harness_with_windows(shell, vec2(1280.0, 800.0), 1.0);
    h.state_mut().load_layout().unwrap();
    h.settle();
    run(&mut h, "view.library", json!(null));
    let open = |h: &mut Harness<'_, Shell>, name: &str| {
        let photo = dir.join(name);
        std::fs::write(&photo, b"not read").unwrap();
        contexts::open(
            h.state().app().handle(),
            &Opened::File(photo),
            &PopOrigin::User,
        );
        for _ in 0..4 {
            h.step();
        }
        h.settle();
        assert_eq!(key(h, "place"), "helios.darkroom");
    };
    let detached = |h: &Harness<'_, Shell>| -> Vec<String> {
        h.state()
            .layout()
            .auxiliary
            .iter()
            .map(|a| a.panel.to_string())
            .collect()
    };
    open(&mut h, "a.png");
    run(&mut h, "helios.scopes.detach", json!(null));
    h.step();
    assert_eq!(detached(&h), ["helios.scopes"]);
    assert!(
        windows.drawn("helios.scopes").is_some(),
        "a window of its own"
    );
    // Another photo: the window stays, the panel in it.
    open(&mut h, "b.png");
    assert_eq!(detached(&h), ["helios.scopes"], "{:?}", h.state().layout());
    // And back on the first one.
    open(&mut h, "a.png");
    assert_eq!(detached(&h), ["helios.scopes"], "{:?}", h.state().layout());
    let _ = std::fs::remove_dir_all(&dir);
}
