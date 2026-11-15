//! Each workspace keeps its own layout: egui-pop keeps it in the
//! document the host names (`workspaces/<key>/layout`), saved when the
//! shell leaves a workspace and loaded when it comes back.

use std::path::Path;

use helios_gui::{Shell, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

fn detached(shell: &Shell) -> Vec<String> {
    shell
        .layout()
        .auxiliary
        .iter()
        .map(|a| a.panel.to_string())
        .collect()
}

#[test]
fn switching_workspace_restores_each_one_s_layout() {
    let dir = std::env::temp_dir().join(format!("helios-workspaces-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (photos, prints) = (Path::new("/work/photos"), Path::new("/work/prints"));
    let shell = Shell::builder(helios_panels(), Some(dir.clone()))
        .workspace(photos)
        .build();
    assert_eq!(shell.workspace().as_deref(), Some("_work_photos"));
    let mut h = helios_gui::headless::harness(shell, egui::vec2(800.0, 500.0));
    h.run();
    assert!(detached(h.state()).is_empty());
    // In photos, the log goes to a window of its own.
    h.state_mut()
        .run("helios.log.detach", json!(null), Origin::User)
        .unwrap();
    h.run();
    assert_eq!(detached(h.state()), ["helios.log"]);
    // Prints has no layout yet: it keeps the one shown; there the log
    // comes back.
    h.state_mut().switch_workspace(prints).unwrap();
    h.run();
    assert!(dir.join("workspaces/_work_photos/layout.json").is_file());
    h.state_mut()
        .run("helios.log.dock", json!(null), Origin::User)
        .unwrap();
    h.run();
    assert!(detached(h.state()).is_empty());
    // Back and forth: each workspace's own layout.
    h.state_mut().switch_workspace(photos).unwrap();
    h.run();
    assert_eq!(detached(h.state()), ["helios.log"]);
    h.state_mut().switch_workspace(prints).unwrap();
    h.run();
    assert!(detached(h.state()).is_empty());
    // The interface's one layout is not written while a workspace is open.
    assert!(!dir.join("layout.json").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
