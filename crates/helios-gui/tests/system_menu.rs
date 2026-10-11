//! The logo and the system menu: declared menus whose entries are
//! commands, with their keys, ticks and lists filled when they open.

use egui::{Key, Modifiers, vec2};
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
fn the_logo_opens_the_system_menu_and_alt_space_too() {
    let mut h = headless::harness(framed(), vec2(1280.0, 800.0));
    h.run();
    let logo = h.state().target_rect("helios.logo").unwrap();
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
    shell.set_theme("helios-light", Default::default());
    let items = shell.menu_items("system");
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
    let keys = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap();
    assert_eq!(keys["kind"], json!("picker"));
    let palette = keys["choices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["value"] == "workbench.palette")
        .unwrap();
    let keys = palette["detail"].as_str().unwrap();
    assert!(
        keys.contains("Ctrl+Shift+P") && keys.contains("F1"),
        "{keys}"
    );
    // Choosing one runs it.
    h.state_mut()
        .run(
            "dialog.answer",
            json!({ "choice": "workbench.palette" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    assert!(
        h.state()
            .registry
            .history()
            .iter()
            .any(|r| r.id == "workbench.palette")
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
}
