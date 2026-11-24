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
    /// Its value, a slider's as it shows it.
    value: Option<String>,
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
                value: n.value().map(str::to_owned),
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

/// The main window's passes Helios asks for, as an idle event loop runs
/// them with no input: each repaint request heard (eframe's rule: current
/// while the main window ran at most the pass it was asked in). With the
/// main window `covered` (GNOME under Wayland sends no frame callback to
/// a window it does not show), the system sends it no redraw: the
/// request is only owed, and the loop's wake (`main_wake`, as Helios's
/// event loop runs it) paints it after the grace. The passes run.
fn main_passes_asked(
    h: &mut Harness<'static, Shell>,
    asked: &std::sync::Mutex<Vec<egui::RequestRepaintInfo>>,
    wake: &mut egui_pop::api::main_wake::MainWake<ViewportId>,
    covered: bool,
) -> usize {
    let mut clock = std::time::Instant::now();
    let mut ran = 0;
    while ran < 10 {
        let now = h.ctx.cumulative_pass_nr_for(ViewportId::ROOT);
        let due = {
            let mut a = asked.lock().unwrap();
            let due = a.iter().any(|i| {
                i.viewport_id == ViewportId::ROOT && now <= i.current_cumulative_pass_nr + 1
            });
            a.retain(|i| i.viewport_id != ViewportId::ROOT || i.current_cumulative_pass_nr > now);
            due
        };
        if due && covered {
            wake.asked(clock);
            continue;
        }
        if !due {
            // Nothing asked: the loop sleeps until the owed frame.
            let Some(at) = wake.next() else { break };
            clock = clock.max(at);
            if wake.due(clock).is_none() {
                break;
            }
        }
        h.input_mut()
            .viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .focused = Some(false);
        h.step();
        ran += 1;
    }
    ran
}

/// Closed from its own window, the Settings window goes with only the
/// passes Helios asks for itself: no click, no Alt+Tab, no other window
/// brought forward by the system. Its window goes at a pass of the main
/// window, which the system does not redraw while it does not show it
/// (covered by the Settings window, on GNOME under Wayland): the event
/// loop paints it when its frame is owed too long. Without that, the
/// window stayed on screen, frozen.
#[test]
fn a_closed_settings_window_goes_without_an_outside_event() {
    use egui_pop::api::main_wake::{MainWake, PassCounter};
    use std::sync::{Arc, Mutex};
    for (covered, loop_wakes) in [(false, true), (true, true), (true, false)] {
        let (mut h, window) = settings_in_their_own_window();
        main_passes_in_the_back(&mut h, 2);
        let asked: Arc<Mutex<Vec<egui::RequestRepaintInfo>>> = Arc::default();
        let heard = asked.clone();
        h.ctx.set_request_repaint_callback(move |info| {
            heard.lock().unwrap().push(info);
        });
        let mut wake = MainWake::new();
        wake.set_main(ViewportId::ROOT);
        wake.set_counter(PassCounter::install(&h.ctx));
        wake.set_enabled(loop_wakes);
        window_pass(
            &mut h,
            window,
            Pass {
                close: true,
                ..Pass::default()
            },
        );
        let ran = main_passes_asked(&mut h, &asked, &mut wake, covered);
        assert!(!admin_open(&h), "the Settings closed");
        let gone = declared(&h).is_empty();
        assert_eq!(
            gone, loop_wakes,
            "covered: {covered}, the loop paints the main window: {loop_wakes}; after the {ran} main passes asked for, the closed window is gone: {gone}"
        );
    }
}

/// The Settings opened at their Caches section, in their own window, with
/// Helios's settings installed: the harness, the window, and the
/// accessibility tree of the window's settled pass.
fn caches_in_their_own_window(name: &str) -> (Harness<'static, Shell>, ViewportId, Vec<Node>) {
    let dir = std::env::temp_dir().join(format!("helios-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    h.state_mut().draw_frame(true);
    h.run();
    let app = h.state().app().handle().clone();
    helios_gui::config::install(
        &app,
        helios_gui::config::Settings::resolve(&helios_gui::config::Sources::default()),
        None,
        &dir,
    );
    h.run();
    h.state_mut()
        .run(
            "admin.open",
            json!({ "path": "storage/caches" }),
            Origin::User,
        )
        .unwrap();
    h.run_steps(3);
    let windows = declared(&h);
    assert_eq!(windows.len(), 1, "the Settings window's own: {windows:?}");
    let window = windows[0];
    let mut nodes = Vec::new();
    for _ in 0..3 {
        nodes = window_pass(&mut h, window, Pass::default());
    }
    (h, window, nodes)
}

/// The Render cache share's slider in a window's pass.
fn share(nodes: &[Node]) -> &Node {
    nodes
        .iter()
        .find(|n| n.role == Role::Slider && n.name == "Render cache share")
        .unwrap_or_else(|| {
            let sliders: Vec<_> = nodes
                .iter()
                .filter(|n| n.role == Role::Slider)
                .map(|n| &n.name)
                .collect();
            panic!("no Render cache share slider; sliders: {sliders:?}")
        })
}

/// Escape pressed in a window's pass.
fn escape_in(h: &mut Harness<'static, Shell>, window: ViewportId) -> Vec<Node> {
    let key = |pressed| Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let nodes = window_pass(
        h,
        window,
        Pass {
            events: vec![key(true), key(false)],
            ..Pass::default()
        },
    );
    main_passes_in_the_back(h, 1);
    nodes
}

/// Escape goes to the innermost gesture first (egui-pop's focus order):
/// pressed while a slider of the Settings window is dragged, it cancels
/// the drag, the slider's value goes back, and the window stays open;
/// once the pointer is let go, the next Escape closes the window.
#[test]
fn escape_during_a_drag_in_the_settings_window_cancels_the_drag_and_keeps_it_open() {
    let (mut h, window, nodes) = caches_in_their_own_window("escape-drag");
    let slider = share(&nodes);
    let before = slider.value.clone();
    let at = slider.rect.center();
    let button = |pressed| Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    window_pass(
        &mut h,
        window,
        Pass {
            events: vec![Event::PointerMoved(at), button(true)],
            ..Pass::default()
        },
    );
    let mut nodes = Vec::new();
    for i in 1..=4 {
        nodes = window_pass(
            &mut h,
            window,
            Pass {
                events: vec![Event::PointerMoved(at + vec2(30.0 * i as f32, 0.0))],
                ..Pass::default()
            },
        );
    }
    assert_ne!(share(&nodes).value, before, "the drag moves the value");

    escape_in(&mut h, window);
    let nodes = window_pass(&mut h, window, Pass::default());
    assert!(admin_open(&h), "the Settings stay open");
    assert_eq!(declared(&h), [window], "their window too");
    assert_eq!(share(&nodes).value, before, "the value goes back");

    // Let go: nothing written; the next Escape is the window's.
    let nodes = window_pass(
        &mut h,
        window,
        Pass {
            events: vec![button(false)],
            ..Pass::default()
        },
    );
    assert_eq!(share(&nodes).value, before, "letting go writes nothing");
    escape_in(&mut h, window);
    main_passes_in_the_back(&mut h, 3);
    assert!(!admin_open(&h), "the second Escape closes the Settings");
    assert!(declared(&h).is_empty(), "and their window");
}

/// With no gesture under way, Escape closes the Settings window as
/// before, and leaves the main window.
#[test]
fn escape_with_no_gesture_closes_the_settings_window() {
    let (mut h, window, _) = caches_in_their_own_window("escape-close");
    escape_in(&mut h, window);
    main_passes_in_the_back(&mut h, 3);
    assert!(!admin_open(&h), "the Settings closed");
    assert!(declared(&h).is_empty(), "their window is gone");
    assert!(!h.state().quitting());
}
