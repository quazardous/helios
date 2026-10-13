//! egui-pop's command palette holds every Helios command; quick open
//! (`Ctrl+P`) goes to a module, an image or a workspace by name.

use std::path::PathBuf;

use egui::{Key, Modifiers, vec2};
use egui_pop::model::ids::CommandId;
use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};
use helios_gui::library::Entry;
use helios_gui::platform::Open;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

#[test]
fn every_helios_command_is_in_the_palette_with_its_title_category_and_keys() {
    let shell = Shell::new(helios_panels(), None);
    let app = shell.app().handle().clone();
    let entries = app.palette();
    let mut shown = 0;
    for spec in shell.specs() {
        let id = CommandId::new(spec.id.as_str()).unwrap();
        let declared = app.command(&id).expect("declared to egui-pop");
        if !declared.palette {
            continue;
        }
        let entry = entries
            .iter()
            .find(|e| e.command == id)
            .unwrap_or_else(|| panic!("`{}` is not in the palette", spec.id));
        assert_eq!(entry.title, spec.title, "{}", spec.id);
        assert_eq!(entry.category, spec.category, "{}", spec.id);
        // Its keys, where it has some.
        assert_eq!(
            entry.keys.is_empty(),
            app.keys_for(&id).is_empty(),
            "{}",
            spec.id
        );
        shown += 1;
    }
    assert!(shown > 50, "{shown} commands in the palette");
    // Quick open shows its key there.
    let quick = entries
        .iter()
        .find(|e| e.command.as_str() == "workbench.quickOpen")
        .unwrap();
    assert!(
        quick.keys.iter().any(|k| k.contains('P')),
        "{:?}",
        quick.keys
    );
}

#[test]
fn the_palette_runs_a_helios_command() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1024.0, 700.0));
    h.run();
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::P);
    h.run();
    assert!(h.state().palette_open());
    // Typed into the focused field (the Vault has a search field too).
    h.event(egui::Event::Text("before and after side".into()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    let history = h.state().history();
    let last = history.last().unwrap();
    assert_eq!(last.id, "viewport.compare");
    assert_eq!(last.origin, Origin::User);
    let store = h.state().values.lock().unwrap();
    let on = store.get(helios_gui::viewport::COMPARE).unwrap();
    assert_eq!(on.value, Value::Bool(true));
}

/// A shell showing a small network, its library holding two images.
fn shell() -> (Shell, PathBuf) {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(
            Region::sized(64, 48),
            4,
            |_, _, c| {
                if c == 3 { 1.0 } else { 0.2 }
            },
        ),
    );
    let exp = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, exp, "output").unwrap();
    shell.set_library(
        [("a1", "harbour.jpg"), ("b2", "beach.raf")]
            .iter()
            .map(|(id, name)| Entry {
                id: (*id).into(),
                name: (*name).into(),
                ..Entry::default()
            })
            .collect(),
    );
    let beach = PathBuf::from("/photos/beach.raf");
    shell.set_image_files([
        ("a1".to_string(), PathBuf::from("/photos/harbour.jpg")),
        ("b2".to_string(), beach.clone()),
    ]);
    (shell, beach)
}

#[test]
fn quick_open_opens_an_image_by_its_name() {
    let (shell, beach) = shell();
    let mut h = headless::harness(shell, vec2(1024.0, 700.0));
    h.run();
    h.key_press_modifiers(Modifiers::CTRL, Key::P);
    h.run();
    let current = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap();
    assert!(
        current.to_string().contains("beach.raf"),
        "quick open lists the library: {current}"
    );
    // The picker's filter has the keyboard (the library has a search
    // field of its own).
    h.input_mut().events.push(egui::Event::Text("beach".into()));
    h.run();
    let current = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap();
    assert_eq!(current["filter"], json!("beach"), "{current}");
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(h.state().take_opens(), vec![Open::Image(beach)]);
}

#[test]
fn quick_open_selects_a_module_and_agents_go_there_too() {
    let (shell, _) = shell();
    let mut h = headless::harness(shell, vec2(1024.0, 700.0));
    h.run();
    let agent = Origin::Agent("a1".into());
    h.state_mut()
        .run(
            "workbench.quickOpen.go",
            json!({ "item": "module:exposure" }),
            agent.clone(),
        )
        .unwrap();
    h.run();
    h.run();
    {
        let store = h.state().values.lock().unwrap();
        let selected = store.get(helios_gui::params::SELECTION).unwrap();
        assert_eq!(selected.value, Value::Text("exposure".into()));
    }
    assert_eq!(h.state().focused().as_deref(), Some("helios.params"));
    // What is not there is said, and nothing moves.
    h.state_mut()
        .run(
            "workbench.quickOpen.go",
            json!({ "item": "image:nope" }),
            agent,
        )
        .unwrap();
    h.run();
    assert!(h.state().take_opens().is_empty());
    assert!(
        h.state().notes.iter().any(|n| n.contains("nope")),
        "{:?}",
        h.state().notes
    );
}
