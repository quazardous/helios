//! The History panel where a user looks for it: a tab beside the Modules
//! in the darkroom, an entry of Window > Panels and of the palette, and
//! Ctrl+Shift+H. The edit's steps read as edits ("Exposure +0.30 EV"),
//! listed under Start; a click on one goes back there and Ctrl+Z moves
//! the mark.

use std::path::{Path, PathBuf};
use std::time::Duration;

use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_pop::api::history_view::PANEL_HISTORY;
use egui_pop::model::commands::Origin as PopOrigin;
use egui_pop::model::ids::PanelId;
use egui_pop::model::panels::Area;
use helios_core::{Graph, Image, ModuleRef, NodeId, Params, Region, Value};
use helios_gui::contexts::{self, Opened};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_values::{Change, Set};
use serde_json::json;

const EV: &str = "node/exposure/exposure";

/// A scratch folder of the test's own, with a workspace in it.
fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-history-panel-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("photos")).unwrap();
    dir
}

/// A grey ramp through an exposure: the photo's network.
fn network() -> (Graph, NodeId) {
    let (w, h) = (64u32, 48u32);
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(w, h), 4, |x, _, c| match c {
            0..=2 => 0.02 + 0.3 * x as f32 / w as f32,
            _ => 1.0,
        }),
    );
    let exp = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_modules::exposure::Exposure),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    (g, exp)
}

/// Frames until the image's preview has landed and is drawn.
fn settle(h: &mut Harness<'_, Shell>) {
    for _ in 0..3 {
        h.step();
        assert!(
            h.state().preview().wait(Duration::from_secs(30)),
            "the preview renders"
        );
    }
    h.step();
    h.step();
}

/// A window on the workspace of `dir`, a photo open in the darkroom.
fn darkroom(dir: &Path) -> Harness<'static, Shell> {
    let mut shell = Shell::builder(helios_panels(), Some(dir.join("ui")))
        .workspace(&dir.join("photos"))
        .build();
    shell.load_layout().unwrap();
    let (graph, exp) = network();
    shell.set_view_source(graph, exp, "output").unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.step();
    let photo = dir.join("photos").join("photo.png");
    std::fs::write(&photo, b"a test fixture, not read").unwrap();
    contexts::open(
        h.state().app().handle(),
        &Opened::File(photo),
        &PopOrigin::User,
    );
    settle(&mut h);
    h
}

fn set(h: &Harness<'_, Shell>, ev: f64) {
    h.state()
        .values()
        .lock()
        .unwrap()
        .set(Set::new(EV, Change::To(Value::Float(ev)), Origin::User))
        .unwrap();
}

fn ev(h: &Harness<'_, Shell>) -> f64 {
    h.state()
        .values()
        .lock()
        .unwrap()
        .get(EV)
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

/// Whether the panel's row saying `label` is the selected one (the
/// history's mark): a selectable row says so as toggled.
fn marked(h: &Harness<'_, Shell>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

#[test]
fn the_darkroom_docks_the_history_beside_the_modules_and_its_rows_walk_the_edit() {
    let dir = temp("darkroom");
    let mut h = darkroom(&dir);
    let app = h.state().app().handle().clone();
    assert_eq!(app.context().get("place").to_string(), "helios.darkroom");
    // The darkroom's layout: History is a tab of the start side, beside
    // the Modules.
    let layout = h.state().layout();
    let modules = PanelId::new("helios.modules").unwrap();
    assert_eq!(layout.area_of(&PANEL_HISTORY), Some(Area::StartSide));
    assert_eq!(layout.area_of(&modules), Some(Area::StartSide));
    assert!(app.panel_visible(&PANEL_HISTORY));
    // Two edits of the photo.
    set(&h, 0.3);
    set(&h, 0.7);
    settle(&mut h);
    // They read as edits, not as the values' paths.
    let read = h
        .state_mut()
        .run("history.read", json!({ "scope": "edit" }), Origin::User)
        .unwrap();
    let texts: Vec<&str> = read["undo"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["Exposure +0.30 EV", "Exposure +0.70 EV"], "{read}");
    // The image's view was focused, as a click on the photo does: the
    // History panel and Ctrl+Z walk the history of the view focused last
    // (egui-pop's focus routing), not the newest step of any history.
    h.state_mut()
        .run("helios.viewport.focus", json!(null), Origin::User)
        .unwrap();
    h.step();
    // The panel in front: both steps under Start, the last one marked.
    h.state_mut()
        .run("egui_pop.history.focus", json!(null), Origin::User)
        .unwrap();
    h.step();
    h.step();
    let start = h.get_by_label("Start").rect();
    let first = h.get_by_label("Exposure +0.30 EV").rect();
    let second = h.get_by_label("Exposure +0.70 EV").rect();
    assert!(start.top() < first.top() && first.top() < second.top());
    assert!(marked(&h, "Exposure +0.70 EV"));
    // A click on the first goes back there.
    h.get_by_label("Exposure +0.30 EV").click();
    h.step();
    h.step();
    assert!((ev(&h) - 0.3).abs() < 1e-9, "{}", ev(&h));
    assert!(marked(&h, "Exposure +0.30 EV"));
    // Ctrl+Z moves the mark to Start.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.step();
    h.step();
    assert_eq!(ev(&h), 0.0);
    assert!(marked(&h, "Start"));
    // Edit > Redo names the edit too.
    let redo = app.command(&egui_pop::api::app::CMD_REDO).unwrap();
    assert_eq!(app.command_title(&redo), "Redo Exposure +0.30 EV");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_window_menu_the_palette_and_ctrl_shift_h_open_the_history() {
    use egui_pop::model::menus::Item;
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let app = h.state().app().handle().clone();
    let toggle = "egui_pop.history.toggle";
    // Outside the darkroom, closed until asked for.
    assert!(!app.panel_visible(&PANEL_HISTORY));
    // Window > Panels lists it with every panel.
    let window = app.menu(&egui_pop::api::windows::MENU_WINDOW);
    assert!(
        window.iter().any(|i| matches!(&i.item,
            Item::Submenu { menu, .. } if menu.as_str() == "helios.window.panels")),
        "{window:?}"
    );
    let cx = egui_pop::model::when::WhenContext::default();
    let listed = app.list_items("helios.panels.views", &cx);
    assert!(
        listed
            .iter()
            .any(|e| e.args["panel"] == PANEL_HISTORY.as_str()),
        "History is in Window > Panels"
    );
    assert!(
        app.palette().iter().any(|e| e.command.as_str() == toggle),
        "History is in the palette"
    );
    // Its command opens it.
    h.state_mut()
        .run(toggle, json!(null), Origin::User)
        .unwrap();
    h.run();
    assert!(app.panel_visible(&PANEL_HISTORY));
    // Ctrl+Shift+H closes it and opens it again.
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::H);
    h.run();
    assert!(!app.panel_visible(&PANEL_HISTORY));
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::H);
    h.run();
    assert!(app.panel_visible(&PANEL_HISTORY));
    egui_pop_test::assert_keymap_clean(h.state().app());
}
