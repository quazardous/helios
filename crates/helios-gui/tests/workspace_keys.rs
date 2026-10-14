//! Keys changed for a workspace in the shortcuts editor: kept as the
//! workspace's own (its `tools.keymap`), one undo step written to its
//! files; the user's keys stay the user's; each key says whether it
//! comes from the workspace's template, the workspace or the user.

use std::path::{Path, PathBuf};

use egui::vec2;
use egui_kittest::Harness;
use egui_pop::model::ids::CommandId;
use egui_pop::model::keymap::Layer;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use helios_template::workspace::Files;

const CIRCLE: CommandId = CommandId::from_static("mask.add.circle");

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-wskeys-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn window(root: &Path) -> (Harness<'static, Shell>, Files) {
    let config = root.join("config");
    let shell = Shell::new(helios_panels(), None);
    let catalogue = templates::catalogue(shell.panels(), shell.specs());
    let created = templates::create_workspace(
        &templates::find("helios.portrait-retouch").unwrap(),
        &catalogue,
        &root.join("work"),
        &config,
    )
    .unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&created.workspace)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &created.workspace).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    (h, Files::new(&config, &created.workspace))
}

/// The circle's keys, each with where the shortcuts editor says it
/// comes from.
fn circle(h: &Harness<'static, Shell>) -> Vec<(String, Option<String>)> {
    let handle = h.state().app().handle().clone();
    handle
        .key_sources(&CIRCLE)
        .iter()
        .map(|b| (b.keys.to_string(), handle.key_source_name(&h.ctx, b)))
        .collect()
}

fn steps(h: &Harness<'static, Shell>) -> usize {
    h.state().values.lock().unwrap().undo_steps().len()
}

#[test]
fn keys_changed_for_the_workspace_are_its_own_and_say_where_they_come_from() {
    let root = temp("own");
    let (mut h, files) = window(&root);
    let handle = h.state().app().handle().clone();
    assert!(handle.workspace_keys_offered());
    assert_eq!(
        circle(&h),
        [("C".to_string(), Some("template".to_string()))]
    );
    // Changed for this workspace: one undo step, in its files.
    let before = steps(&h);
    handle.rebind_in(Layer::Workspace, CIRCLE, Some("B".into()));
    h.run();
    assert_eq!(steps(&h), before + 1, "one undo step");
    assert_eq!(
        circle(&h),
        [("B".to_string(), Some("workspace".to_string()))]
    );
    let kept = files.load().unwrap().bundle.template.tools.keymap;
    assert!(
        kept.iter()
            .any(|b| b.keys == "B" && b.command == "mask.add.circle"),
        "{kept:?}"
    );
    assert!(
        kept.iter()
            .any(|b| b.keys.is_empty() && b.command == "-mask.add.circle"),
        "the template's key taken away: {kept:?}"
    );
    assert!(kept.iter().any(|b| b.command == "mask.add.ellipse"));
    // The user's own keys stay the user's: no workspace step.
    let before = steps(&h);
    handle.rebind(CIRCLE, Some("K".into()));
    h.run();
    assert_eq!(steps(&h), before);
    assert_eq!(circle(&h), [("K".to_string(), Some("user".to_string()))]);
    handle.reset_keys(CIRCLE);
    h.run();
    // Undone: the template's key is back, and so is its name.
    h.state()
        .values
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    h.run();
    assert_eq!(
        circle(&h),
        [("C".to_string(), Some("template".to_string()))]
    );
    assert!(
        !files
            .load()
            .unwrap()
            .bundle
            .template
            .tools
            .keymap
            .iter()
            .any(|b| b.keys == "B")
    );
    drop(h);
    std::fs::remove_dir_all(&root).unwrap();
}
