//! Closing: egui-pop's exit runs Helios's `on_exit` (the workspace's
//! layout) before the `app.exit` step, whose default saves the
//! interface's settings, keys and layout.

use std::cell::RefCell;
use std::rc::Rc;

use egui_pop::model::layout::Layout;
use helios_gui::{Shell, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

#[test]
fn exit_runs_on_exit_then_the_exit_step_saves() {
    let dir = std::env::temp_dir().join(format!("helios-exit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seen: Rc<RefCell<Option<Layout>>> = Rc::default();
    let at_exit = seen.clone();
    let mut shell = Shell::builder(helios_panels(), Some(dir.clone()))
        .on_exit(move |app| *at_exit.borrow_mut() = Some(app.layout()))
        .build();
    shell
        .run("helios.log.detach", json!(null), Origin::User)
        .unwrap();
    shell
        .bind_user_keys("ctrl+shift+e", "vault.reject")
        .unwrap();
    // The queued detach applies at the next frame; a harness draws one.
    let mut h = helios_gui::headless::harness(shell, egui::vec2(800.0, 500.0));
    h.run();
    h.state_mut().exit().unwrap();
    // The workspace's layout, as the window closed.
    let layout = seen.borrow().clone().expect("on_exit ran");
    assert_eq!(layout.auxiliary[0].panel.as_str(), "helios.log");
    // Then egui-pop's step saved the interface's documents.
    for doc in ["settings", "keymap", "layout"] {
        assert!(dir.join(format!("{doc}.json")).is_file(), "{doc}.json");
    }
    let keymap = std::fs::read_to_string(dir.join("keymap.json")).unwrap();
    assert!(keymap.contains("vault.reject"), "{keymap}");
    // The run is journaled as egui-pop's step.
    let app = h.state().app().handle().clone();
    assert!(
        app.step_traces()
            .iter()
            .any(|t| t.step.as_str() == "app.exit")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
