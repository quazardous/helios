//! The frame's menu button and the system menu: declared menus
//! (egui-pop's) whose entries are commands, with their keys, ticks and
//! lists read again when they open.

use egui::{Key, Modifiers, vec2};
use egui_kittest::kittest::Queryable;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

fn framed() -> Shell {
    let mut shell = Shell::new(helios_panels(), None);
    shell.draw_frame(true);
    shell.set_title("photos");
    shell
}

fn titles(items: &Value) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|i| match i["kind"].as_str().unwrap() {
            "separator" => "-".into(),
            "submenu" => format!("{} >", i["label"].as_str().unwrap()),
            _ => i["title"].as_str().unwrap().to_string(),
        })
        .collect()
}

#[test]
fn the_frame_s_menu_button_opens_the_system_menu_and_alt_space_too() {
    let mut h = headless::harness(framed(), vec2(1280.0, 800.0));
    h.run();
    // egui-pop's frame draws the title bar; its menu button opens
    // Helios's system menu.
    let logo = h.get_by_label("Window menu").rect();
    h.hover_at(logo.center());
    h.run();
    h.event(egui::Event::PointerButton {
        pos: logo.center(),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.event(egui::Event::PointerButton {
        pos: logo.center(),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
    let menu = h.state().target_rect("helios.menu").unwrap();
    assert!(menu.top() >= logo.bottom() - 1.0, "{menu:?} under {logo:?}");
    h.key_press(Key::Escape);
    h.run();
    assert!(h.state().target_rect("helios.menu").is_err());
    h.key_press_modifiers(Modifiers::ALT, Key::Space);
    h.run();
    assert!(h.state().target_rect("helios.menu").is_ok());
}

#[test]
fn the_system_menu_is_declared_with_keys_ticks_and_lists() {
    let mut shell = framed();
    shell.set_theme("helios-light", std::collections::BTreeMap::default());
    let items = shell.menu_items("helios.system");
    assert_eq!(
        titles(&items),
        [
            "Open a workspace in a new window",
            "Recent workspaces >",
            "-",
            "Profiles >",
            "Themes >",
            "Keyboard shortcuts",
            "-",
            "About Helios",
            "-",
            "Quit Helios"
        ]
    );
    // The themes list is filled when it opens, the one in use ticked.
    let themes = &items[4]["items"];
    let light = themes
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == "helios-light")
        .unwrap();
    assert_eq!(light["checked"], json!(true));
    assert_eq!(light["args"], json!({ "name": "helios-light" }));
    // Keys come from the keymap.
    let shortcuts = items
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "workbench.keybindings")
        .unwrap();
    assert_eq!(shortcuts["enabled"], json!(true));
}

#[test]
fn about_and_keyboard_shortcuts_open_shell_dialogs() {
    let mut h = headless::harness(framed(), vec2(1280.0, 800.0));
    h.state_mut()
        .run("workbench.about", json!(null), Origin::User)
        .unwrap();
    h.run();
    let about = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap();
    assert_eq!(about["kind"], json!("info"));
    assert!(about["title"].as_str().unwrap().starts_with("Helios "));
    h.key_press(Key::Escape);
    h.run();
    h.state_mut()
        .run("workbench.keybindings", json!(null), Origin::User)
        .unwrap();
    h.run();
    // egui-pop's settings window, on its shortcuts editor: every command
    // with the keys the keymap binds, which a click there changes.
    let app = h.state().app().handle().clone();
    assert!(egui_pop::api::settings_window::is_open(&app));
    let keys = app.keys_for(&egui_pop::api::app::CMD_PALETTE);
    assert!(
        keys.iter().any(|k| k == "Ctrl+Shift+P") && keys.iter().any(|k| k == "F1"),
        "{keys:?}"
    );
    assert!(
        h.query_by_label("Change the keys of Window: Show the Helios menu")
            .is_some(),
        "Helios's commands are listed"
    );
}

#[test]
fn opening_a_workspace_without_a_path_asks_for_one() {
    let mut h = headless::harness(framed(), vec2(1280.0, 800.0));
    h.state_mut()
        .run("workspace.open", json!(null), Origin::User)
        .unwrap();
    h.run();
    let form = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap();
    assert_eq!(form["kind"], json!("form"));
    assert_eq!(form["command"], json!("workspace.open"));
    // A directory field, with Browse.
    let field = &form["fields"][0];
    assert_eq!(field["key"], json!("path"));
    assert_eq!(field["descriptor"]["path"], json!({ "kind": "dir" }));
    h.get_by_label("Browse…");
}
