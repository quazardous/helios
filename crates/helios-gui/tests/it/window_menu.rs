//! The View and Window menus as a user opens them: View holds what is
//! shown, Window where things are (Panels ▸, a checklist of every panel;
//! Studio ▸, the saved arrangements of panels), then the OS windows. A
//! closed panel comes back from Window ▸ Panels where it was.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::model::ids::PanelId;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

fn shell() -> Harness<'static, Shell> {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    h
}

/// Clicks the last node named `label` (an open menu is drawn last).
fn click(h: &mut Harness<'_, Shell>, label: &str) {
    h.query_all_by_label(label)
        .last()
        .unwrap_or_else(|| panic!("no `{label}` on screen"))
        .click();
    h.run_steps(3);
}

/// Whether a node's name holds `label` (a menu row may add its key).
fn shows(h: &Harness<'_, Shell>, label: &str) -> bool {
    h.query_all_by_label_contains(label).next().is_some()
}

/// Opens a submenu of the open menu (a click on its row, the pointer
/// left on it).
fn open_submenu(h: &mut Harness<'_, Shell>, label: &str) {
    let node = h
        .query_all_by_label(label)
        .last()
        .unwrap_or_else(|| panic!("no `{label}` on screen"));
    node.hover();
    node.click();
    h.run_steps(4);
}

fn close_menus(h: &mut Harness<'_, Shell>) {
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
}

#[test]
fn view_holds_what_is_shown_and_window_where_things_are() {
    let mut h = shell();
    click(&mut h, "View");
    for item in ["Activity…", "Zoom in the interface", "Full screen"] {
        assert!(shows(&h, item), "View has {item}");
    }
    // The layout entries left View for Window ▸ Studio.
    for gone in [
        "Customize layout",
        "Toggle primary side bar",
        "Save the arrangement as...",
        "Switch arrangement",
        "Reset the arrangement",
    ] {
        assert!(!shows(&h, gone), "View has no {gone}");
    }
    close_menus(&mut h);
    click(&mut h, "Window");
    for item in ["Panels", "Studio", "Main window", "Close window"] {
        assert!(shows(&h, item), "Window has {item}");
    }
    open_submenu(&mut h, "Panels");
    // Every panel, egui-pop's History among Helios's.
    for panel in ["Library", "Filters", "Modules", "Masks", "History"] {
        assert!(shows(&h, panel), "Window ▸ Panels has {panel}");
    }
    close_menus(&mut h);
    click(&mut h, "Window");
    open_submenu(&mut h, "Studio");
    // egui-pop's layout menu: the areas, saving the current studio.
    assert!(shows(&h, "Toggle primary side bar"), "Studio has the areas");
    egui_pop_test::assert_keymap_clean(h.state().app());
}

#[test]
fn the_panels_list_ticks_shown_panels_and_reopens_a_closed_one_in_place() {
    let mut h = shell();
    let app = h.state().app().handle().clone();
    let modules = PanelId::new("helios.modules").unwrap();
    // Opened first, so it sits among its tab group.
    h.state_mut()
        .run(
            "workbench.panel.open",
            json!({ "panel": modules, "place": "last" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    let layout = app.layout();
    let area = layout.area_of(&modules).expect("Modules is docked");
    let (tabs, _) = layout.group_of(&modules).expect("in a tab group");
    let index = tabs.iter().position(|p| *p == modules).unwrap();
    let cx = egui_pop::model::when::WhenContext::default();
    let ticked = |app: &egui_pop::api::app::AppHandle| {
        app.list_items("helios.panels.controls", &cx)
            .into_iter()
            .find(|e| e.label == "Modules")
            .map(|e| e.checked)
    };
    assert_eq!(ticked(&app), Some(true), "a shown panel is ticked");
    h.state_mut()
        .run(
            "workbench.panel.close",
            json!({ "panel": modules }),
            Origin::User,
        )
        .unwrap();
    h.run();
    assert!(!app.panel_visible(&modules));
    assert_eq!(ticked(&app), Some(false), "a closed panel is not");
    // Window ▸ Panels ▸ Modules: back where it was.
    click(&mut h, "Window");
    open_submenu(&mut h, "Panels");
    click(&mut h, "Modules");
    assert!(app.panel_visible(&modules), "Modules is back");
    let layout = app.layout();
    assert_eq!(layout.area_of(&modules), Some(area), "in its area");
    let (back, _) = layout.group_of(&modules).expect("in a tab group");
    assert_eq!(
        back.iter().position(|p| *p == modules),
        Some(index),
        "at its place among the tabs: {back:?}"
    );
}

#[test]
fn view_lists_the_photos_zoom_and_the_scopes() {
    let mut h = shell();
    let app = h.state().app().handle().clone();
    let scopes = PanelId::new("helios.scopes").unwrap();
    click(&mut h, "View");
    for item in [
        "Image: zoom in",
        "Image: zoom out",
        "Image: fit to the view",
        "Image: actual pixels, 100 %",
        "Show or hide Scopes",
    ] {
        assert!(shows(&h, item), "View has {item}");
    }
    // The scopes' entry shows or closes the panel.
    let before = app.panel_visible(&scopes);
    h.query_all_by_label_contains("Show or hide Scopes")
        .last()
        .expect("the scopes' entry")
        .click();
    h.run_steps(3);
    assert_ne!(app.panel_visible(&scopes), before, "the scopes toggled");
}
