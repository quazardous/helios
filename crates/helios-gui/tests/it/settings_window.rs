//! Settings in an OS window of their own (docs/design/windows.md, slice 5):
//! egui-pop's Settings app in its window mode, Helios's default, an
//! application window as the import window is. It opens from its usual
//! commands and from the Window menu, draws Helios's title bar, its form
//! works there, closing it leaves the main window, and closing the main
//! window while it is open moves it into the main window with its size,
//! place and title bar.
//!
//! kittest embeds windows in the main one (where the Settings app keeps
//! its overlay): these tests run real viewports, as eframe does, the main
//! window's pass then the Settings window's own.

use egui::accesskit::Role;
use egui::{Event, Pos2, RawInput, Rect, ViewportCommand, ViewportEvent, ViewportId, ViewportInfo};
use egui::{Vec2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::NodeT as _;
use egui_pop::api::admin::ADMIN;
use egui_pop::api::windows::{MENU_WINDOW, WindowId};
use egui_pop::model::menus::Item;
use helios_gui::shell::{Closing, SETTINGS_WINDOW};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

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

/// What a window's pass is given: its size, where the platform tells it
/// its place, its close requested, the events.
#[derive(Default)]
struct Pass {
    size: Option<Vec2>,
    at: Option<Pos2>,
    close: bool,
    events: Vec<Event>,
}

/// A node of a window's accessibility tree: its role, its name, its
/// rectangle in points.
struct Node {
    role: Role,
    name: String,
    rect: Rect,
    disabled: bool,
}

/// A deferred window's own pass, as eframe runs it apart from the main
/// window's: the nodes of its accessibility tree.
fn window_pass(h: &mut Harness<'static, Shell>, window: ViewportId, pass: Pass) -> Vec<Node> {
    let cb = h.output().viewport_output[&window]
        .viewport_ui_cb
        .clone()
        .expect("the window is declared by the main window's last pass");
    let size = pass.size.unwrap_or(vec2(1000.0, 700.0));
    let rect = Rect::from_min_size(Pos2::ZERO, size);
    let mut info = ViewportInfo {
        inner_rect: Some(rect),
        outer_rect: pass.at.map(|p| Rect::from_min_size(p, size)),
        focused: Some(true),
        ..Default::default()
    };
    if pass.close {
        info.events.push(ViewportEvent::Close);
    }
    let mut input = RawInput {
        viewport_id: window,
        screen_rect: Some(rect),
        time: Some(h.ctx.input(|i| i.time) + 1.0 / 60.0),
        focused: true,
        events: pass.events,
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
    // No painter for it: its textures are uploaded nowhere.
    out.textures_delta.clear();
    let ppp = out.pixels_per_point;
    let Some(update) = out.platform_output.accesskit_update else {
        return Vec::new();
    };
    update
        .nodes
        .iter()
        .filter_map(|(_, n)| {
            let b = n.bounds()?;
            let name = n.label().or_else(|| n.value())?.to_owned();
            Some(Node {
                role: n.role(),
                name,
                disabled: n.is_disabled(),
                rect: Rect::from_min_max(
                    Pos2::new(b.x0 as f32 / ppp, b.y0 as f32 / ppp),
                    Pos2::new(b.x1 as f32 / ppp, b.y1 as f32 / ppp),
                ),
            })
        })
        .collect()
}

/// A click in a window: the pointer pressed in one pass, released in the
/// next.
fn click_in(h: &mut Harness<'static, Shell>, window: ViewportId, at: Pos2) {
    let button = |pressed| Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    for events in [
        vec![Event::PointerMoved(at), button(true)],
        vec![button(false)],
    ] {
        window_pass(
            h,
            window,
            Pass {
                events,
                ..Pass::default()
            },
        );
    }
}

/// The main window's passes, the Settings window in front.
fn main_passes_in_the_back(h: &mut Harness<'static, Shell>, steps: usize) {
    for _ in 0..steps {
        h.input_mut()
            .viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .focused = Some(false);
        h.run();
    }
}

/// Helios with real viewports and its own frame (as on the desktop), the
/// Settings opened by their command: the harness and their window.
fn settings_in_their_own_window() -> (Harness<'static, Shell>, ViewportId) {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    h.state_mut().draw_frame(true);
    h.run();
    assert!(declared(&h).is_empty(), "no window but the main one");
    h.state_mut()
        .run("admin.open", json!(null), Origin::User)
        .unwrap();
    h.run_steps(3);
    let windows = declared(&h);
    assert_eq!(windows.len(), 1, "the Settings window's own: {windows:?}");
    (h, windows[0])
}

fn admin_open(h: &Harness<'static, Shell>) -> bool {
    egui_pop::api::admin::is_open(h.state().app().handle())
}

#[test]
fn settings_open_in_their_own_window_with_helios_title_bar_and_work_there() {
    let (mut h, window) = settings_in_their_own_window();
    let app = h.state().app().handle().clone();
    let listed: Vec<_> = app
        .windows()
        .into_iter()
        .map(|w| (w.id.to_string(), w.title))
        .collect();
    assert_eq!(
        listed[1],
        (SETTINGS_WINDOW.to_owned(), "Settings".to_owned())
    );
    assert_eq!(
        app.window_at(window).map(|w| w.to_string()).as_deref(),
        Some(SETTINGS_WINDOW)
    );
    // Helios's frame: no system title bar, Helios's own drawn.
    let decorations = h.output().viewport_output[&window].builder.decorations;
    assert_eq!(decorations, Some(false), "no system title bar");
    // Agents name it.
    assert!(app.debug_snapshot_in(Some(SETTINGS_WINDOW)).is_ok());
    let nodes = window_pass(&mut h, window, Pass::default());
    for name in ["Minimize", "Maximize", "Close"] {
        assert!(
            nodes.iter().any(|n| n.name == name),
            "{name} in its title bar"
        );
    }
    assert!(nodes.iter().any(|n| n.name.starts_with("Settings")));
    // The form works in it: a section chosen in its tree.
    let admin = app.service(&ADMIN).unwrap();
    let before = admin.shown().1;
    let fonts = nodes
        .iter()
        .find(|n| n.name == "Fonts")
        .expect("Appearance › Fonts in the tree");
    let at = fonts.rect.center();
    click_in(&mut h, window, at);
    let after = admin.shown().1;
    assert!(
        after.is_some() && after != before,
        "{before:?} then {after:?}"
    );
    // The search typed narrows it.
    let nodes = window_pass(&mut h, window, Pass::default());
    let search = nodes
        .iter()
        .find(|n| n.role == Role::SearchInput)
        .expect("the search field");
    let at = search.rect.center();
    click_in(&mut h, window, at);
    window_pass(
        &mut h,
        window,
        Pass {
            events: vec![Event::Text("theme".into())],
            ..Pass::default()
        },
    );
    assert_eq!(admin.shown().2, "theme", "the search typed in its window");
    // Nothing of it over the main window.
    main_passes_in_the_back(&mut h, 2);
    assert!(headless::window_rect(&h, "Settings").is_none());
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
/// (docs/design/windows.md, section 7): the Settings window's bar has the
/// menu button, the logo and the name, and the same menus as the main
/// window, evaluated in the Settings window, where a command of the main
/// window (Export…) is greyed.
#[test]
fn the_settings_window_has_the_main_windows_title_bar_and_menus() {
    let (mut h, window) = settings_in_their_own_window();
    let app = h.state().app().handle().clone();
    h.run();
    let main_menus = top_menus(&app);
    assert!(main_menus.len() >= 4, "{main_menus:?}");
    let nodes = window_pass(&mut h, window, Pass::default());
    let names: Vec<&String> = nodes.iter().map(|n| &n.name).collect();
    for name in ["Window menu", "Helios", "File", "Edit", "View", "Window"] {
        assert!(names.iter().any(|n| *n == name), "{name} in {names:?}");
    }
    // The same menus as the main window, laid out in the Settings window.
    assert_eq!(top_menus(&app), main_menus);
    // File > Export…: the main window's, greyed here.
    let file = nodes
        .iter()
        .find(|n| n.name == "File")
        .map(|n| n.rect.center())
        .unwrap();
    click_in(&mut h, window, file);
    let nodes = window_pass(&mut h, window, Pass::default());
    let export = nodes
        .iter()
        .find(|n| n.name.starts_with("Export"))
        .unwrap_or_else(|| {
            panic!(
                "Export in {:?}",
                nodes.iter().map(|n| &n.name).collect::<Vec<_>>()
            )
        });
    assert!(
        export.disabled,
        "{} is greyed in the Settings window",
        export.name
    );
}

#[test]
fn closing_the_settings_window_leaves_the_main_window() {
    let (mut h, window) = settings_in_their_own_window();
    window_pass(
        &mut h,
        window,
        Pass {
            close: true,
            ..Pass::default()
        },
    );
    main_passes_in_the_back(&mut h, 3);
    assert!(!admin_open(&h), "the Settings closed");
    assert!(declared(&h).is_empty(), "its window is gone");
    assert!(!h.state().quitting());
    let app = h.state().app().handle().clone();
    let listed: Vec<_> = app
        .windows()
        .into_iter()
        .map(|w| w.id.to_string())
        .collect();
    assert_eq!(listed, ["main"]);
    // Opened again: in a window of its own again.
    h.state_mut()
        .run("admin.open", json!(null), Origin::User)
        .unwrap();
    h.run_steps(3);
    assert_eq!(declared(&h), [window]);
}

#[test]
fn the_window_menu_opens_the_settings() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let items = h.state().app().handle().menu(&MENU_WINDOW);
    let commands: Vec<String> = items
        .iter()
        .filter_map(|i| match &i.item {
            Item::Command { id, .. } => Some(id.to_string()),
            _ => None,
        })
        .collect();
    assert!(
        commands.iter().any(|c| c == "admin.open"),
        "Window › Settings: {commands:?}"
    );
}

/// The main window closed while the Settings window is open: Helios goes
/// on, the Settings moving into the main window with their size, place and
/// title bar (as the import window does).
#[test]
fn closing_the_main_window_moves_the_settings_with_their_geometry_and_title_bar() {
    let (mut h, window) = settings_in_their_own_window();
    window_pass(
        &mut h,
        window,
        Pass {
            size: Some(vec2(900.0, 640.0)),
            at: Some(egui::pos2(300.0, 120.0)),
            ..Pass::default()
        },
    );
    let ctx = h.ctx.clone();
    assert_eq!(h.state_mut().main_window_closing(&ctx), Closing::Moved);
    h.input_mut()
        .viewports
        .entry(ViewportId::ROOT)
        .or_default()
        .events
        .push(ViewportEvent::Close);
    h.step();
    let sent = h.output().viewport_output[&ViewportId::ROOT]
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
    assert!(!h.state().quitting());
    assert!(admin_open(&h), "the Settings stay open");
    let app = h.state().app().handle().clone();
    let listed: Vec<_> = app
        .windows()
        .into_iter()
        .map(|w| (w.id.to_string(), w.main))
        .collect();
    assert_eq!(
        listed,
        [(SETTINGS_WINDOW.to_owned(), true)],
        "they took the main window's place"
    );
    // The main window's title bar is the Settings window's now.
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
    assert!(names.iter().any(|n| n.starts_with("Settings")), "{names:?}");
    assert!(
        app.window_viewport(&WindowId::new(SETTINGS_WINDOW)) == Some(ViewportId::ROOT),
        "drawn in the main window"
    );
}

/// Quick settings asked in the import window (`Ctrl+Shift+Q`) open
/// there, over the import window, and not in the main one.
#[test]
fn quick_settings_asked_in_the_import_window_open_there() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    h.state_mut().draw_frame(true);
    h.run();
    h.state_mut()
        .run("library.importWindow", json!(null), Origin::User)
        .unwrap();
    h.run_steps(3);
    let windows = declared(&h);
    assert_eq!(windows.len(), 1, "the import window's own: {windows:?}");
    let import = windows[0];
    window_pass(&mut h, import, Pass::default());
    window_pass(
        &mut h,
        import,
        Pass {
            events: vec![Event::Key {
                key: egui::Key::Q,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            }],
            ..Pass::default()
        },
    );
    main_passes_in_the_back(&mut h, 2);
    // Its overlay comes in over a few of the window's passes.
    let mut nodes = Vec::new();
    for _ in 0..4 {
        nodes = window_pass(&mut h, import, Pass::default());
    }
    let admin = h.state().app().handle().service(&ADMIN).unwrap();
    assert!(admin.quick_open_in(import), "open in the import window");
    assert!(
        !admin.quick_open_in(ViewportId::ROOT),
        "not in the main window"
    );
    assert!(
        nodes.iter().any(|n| n.name == "Often changed"),
        "drawn in the import window: {:?}",
        nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>()
    );
}

/// Helios with real viewports, a library open through an in-process node
/// and a workspace's settings file, and the import window open: two foci.
fn two_foci(name: &str) -> (Harness<'static, Shell>, ViewportId, std::path::PathBuf) {
    use helios_gui::config::{self, Settings};
    use helios_node_core::node::LocalNode;
    use helios_vault_sqlite::SqliteStore;
    use std::sync::Arc;
    use std::time::Duration;

    let dir = std::env::temp_dir().join(format!("helios-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = SqliteStore::open(&dir.join("vault.db")).unwrap();
    let node = LocalNode::open(
        store,
        Arc::new(helios_core::Bus::new()),
        Duration::from_hours(1),
    );
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    h.state_mut().draw_frame(true);
    h.run();
    let app = h.state().app().handle().clone();
    let layers = node.settings();
    let (sources, _) = config::read(
        &dir,
        &|n| layers.read(n),
        Some("project//photos".into()),
        &[],
    );
    config::install(
        &app,
        Settings::resolve(&sources),
        Some(Arc::new(node.settings())),
        &dir,
    );
    config::open_workspace(&app, Some(dir.join("workspace.json")));
    h.state_mut().set_title("Wedding");
    h.run();
    h.state_mut()
        .run("library.importWindow", json!(null), Origin::User)
        .unwrap();
    h.run_steps(3);
    let windows = declared(&h);
    assert_eq!(windows.len(), 1, "the import window's own: {windows:?}");
    (h, windows[0], dir)
}

#[test]
fn settings_opened_from_the_main_window_edit_its_library_and_workspace_named_in_their_header() {
    let (mut h, import, dir) = two_foci("settings-focus-main");
    let app = h.state().app().handle().clone();
    let admin = app.service(&ADMIN).unwrap();
    h.state_mut()
        .run("admin.open", json!(null), Origin::User)
        .unwrap();
    h.run_steps(3);
    assert_eq!(admin.edited_focus().as_deref(), Some("main"));
    let foci = admin.foci(&app);
    // Named by the workspace and the library (Fluent may isolate the
    // names it places).
    let main = &foci[0].label;
    assert!(main.starts_with("Main window"), "{main}");
    assert!(
        main.contains("Wedding") && main.contains("photos"),
        "{main}"
    );
    let import_label = &foci[1].label;
    assert!(import_label.starts_with("Import window"), "{import_label}");
    assert!(
        import_label.contains("photos") && !import_label.contains("Wedding"),
        "{import_label}"
    );
    let offered: Vec<_> = admin.offered().iter().map(|s| s.name()).collect();
    assert!(
        offered.contains(&"library") && offered.contains(&"workspace"),
        "{offered:?}"
    );
    // The header, in the Settings window: the focus edited, its switch.
    let settings = declared(&h)
        .into_iter()
        .find(|v| *v != import)
        .expect("the Settings window");
    let nodes = window_pass(&mut h, settings, Pass::default());
    let combo = nodes
        .iter()
        .find(|n| n.role == Role::ComboBox && n.name == "Settings for")
        .expect("the header's switch");
    assert!(combo.rect.width() > 0.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn settings_asked_in_the_import_window_edit_its_focus_and_the_switch_changes_it() {
    let (mut h, import, dir) = two_foci("settings-focus-import");
    let app = h.state().app().handle().clone();
    let admin = app.service(&ADMIN).unwrap();
    let zoom = egui_pop::model::ids::SettingId::new("interface.graph.zoom").unwrap();
    let decisions = egui_pop::model::ids::SettingId::new("project.decisions.mode").unwrap();
    let before = (
        app.setting::<String>(&zoom),
        app.setting::<String>(&decisions),
    );
    window_pass(&mut h, import, Pass::default());
    window_pass(
        &mut h,
        import,
        Pass {
            events: vec![Event::Key {
                key: egui::Key::Comma,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
            ..Pass::default()
        },
    );
    main_passes_in_the_back(&mut h, 3);
    // It lands in the Settings window, the import window left open.
    assert!(admin_open(&h));
    let windows = declared(&h);
    assert_eq!(windows.len(), 2, "import and Settings: {windows:?}");
    assert!(
        app.window_viewport(&WindowId::new(SETTINGS_WINDOW))
            .is_some()
    );
    // The import window's focus: the library it imports into, no
    // workspace.
    assert_eq!(admin.edited_focus().as_deref(), Some("import"));
    let offered: Vec<_> = admin.offered().iter().map(|s| s.name()).collect();
    assert!(offered.contains(&"library"), "{offered:?}");
    assert!(!offered.contains(&"workspace"), "{offered:?}");
    // The switch: the main window's focus, its workspace offered again.
    h.state_mut()
        .run("admin.focus", json!({ "id": "main" }), Origin::User)
        .unwrap();
    h.run_steps(3);
    assert_eq!(admin.edited_focus().as_deref(), Some("main"));
    let offered: Vec<_> = admin.offered().iter().map(|s| s.name()).collect();
    assert!(offered.contains(&"workspace"), "{offered:?}");
    // A setting with no workspace level, and the library's (the same
    // library in both foci), keep their values through the switch.
    assert_eq!(
        (
            app.setting::<String>(&zoom),
            app.setting::<String>(&decisions)
        ),
        before
    );
    let _ = std::fs::remove_dir_all(&dir);
}
