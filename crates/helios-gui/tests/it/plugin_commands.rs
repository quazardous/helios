//! The plugins' service commands in the desktop: the registry the shell
//! builds over the broker (as `main` hands it the plugins it read) lists
//! `metadata.read`, the `device.*` commands and the commands a plugin
//! contributes, and runs them in their plugin, its capabilities checked.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use egui::vec2;
use helios_gui::{Shell, headless, helios_panels};
use helios_plugin::{Broker, Policy, Scope};
use helios_shell::commands::Origin;
use serde_json::json;

const PLUGIN: &str = "org.example.testee";
const CONTRIBUTED: &str = "testee.hello";

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-plugin-commands-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The broker's test plugin (Python, its standard library alone), with a
/// command of its own.
fn plugin() -> PathBuf {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../helios-plugin/tests/fixtures/testee");
    let dir = scratch("plugin");
    std::fs::copy(fixture.join("testee.py"), dir.join("testee.py")).unwrap();
    let manifest = std::fs::read_to_string(fixture.join("helios-plugin.toml"))
        .unwrap()
        .replace(
            r#"activation = ["onService:metadata.read"]"#,
            &format!(r#"activation = ["onService:metadata.read", "onCommand:{CONTRIBUTED}"]"#),
        )
        + &format!(
            "\n[[commands]]\nid = \"{CONTRIBUTED}\"\ntitle = \"Say hello\"\ncategory = \"Testee\"\n"
        );
    std::fs::write(dir.join("helios-plugin.toml"), manifest).unwrap();
    dir
}

#[test]
fn the_desktop_runs_the_plugins_service_and_contributed_commands() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 is not installed: skipped");
        return;
    }
    let mut broker = Broker::new(Policy::default());
    broker.add(&plugin(), Scope::User).unwrap();
    let broker = Arc::new(broker);
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_plugins(broker.clone());
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();

    let ids: Vec<String> = h.state().specs().into_iter().map(|s| s.id).collect();
    for id in [
        "plugin.list",
        "metadata.read",
        "device.list",
        "device.info",
        "device.capture",
        "device.download",
        CONTRIBUTED,
    ] {
        assert!(ids.iter().any(|i| i == id), "`{id}` is not declared");
    }

    let said = h
        .state_mut()
        .run(CONTRIBUTED, json!({"to": "you"}), Origin::User)
        .unwrap();
    assert_eq!(said["command"], CONTRIBUTED);
    assert_eq!(said["args"]["to"], "you");
    assert_eq!(
        said["pid"],
        broker.pid(PLUGIN).unwrap(),
        "run in its plugin"
    );

    // `metadata.read` holds the plugin to its grant.
    let image = scratch("image").join("a.png");
    std::fs::write(&image, b"x").unwrap();
    let image = image.to_string_lossy().to_string();
    let refused = h
        .state_mut()
        .run("metadata.read", json!({"image": image}), Origin::User)
        .unwrap_err();
    assert!(refused.contains("images.read"), "{refused}");
    h.state_mut()
        .run(
            "plugin.grant",
            json!({"plugin": PLUGIN, "capability": "images.read"}),
            Origin::User,
        )
        .unwrap();
    let read = h
        .state_mut()
        .run("metadata.read", json!({"image": image}), Origin::User)
        .unwrap();
    assert_eq!(read["fields"][0]["source"], "testee");
    assert_eq!(
        h.state_mut()
            .run("device.list", json!({}), Origin::User)
            .unwrap()["devices"],
        json!([])
    );
}
