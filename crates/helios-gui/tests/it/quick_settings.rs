//! Quick settings in Helios (egui-pop's `admin.quick`): the settings
//! Helios marks quick, "Set for" only on one that has a library level,
//! and a change saved through the node, which another of its clients
//! reads, then listed among those changed lately.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::kittest::Queryable;
use egui_pop::api::admin::ADMIN;
use egui_pop::api::admin::model::ValueScope;
use egui_pop::api::admin::quick::now;
use egui_pop::model::ids::SettingId;
use helios_gui::config::{self, Settings};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

#[test]
fn quick_settings_list_helios_marks_and_save_through_the_node() {
    use helios_node_core::node::LocalNode;
    use helios_vault_sqlite::SqliteStore;

    let dir = std::env::temp_dir().join(format!("helios-quick-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = SqliteStore::open(&dir.join("vault.db")).unwrap();
    let bus = Arc::new(helios_core::Bus::new());
    let node = LocalNode::open(store, bus.clone(), Duration::from_hours(1));
    let project = "project//photos";

    let shell = Shell::new(helios_panels(), None);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let app = h.state().app().handle().clone();
    let mine = node.settings();
    let (sources, problems) = config::read(&dir, &|n| mine.read(n), Some(project.into()), &[]);
    assert!(problems.is_empty(), "{problems:?}");
    config::install(
        &app,
        Settings::resolve(&sources),
        Some(Arc::new(node.settings())),
        &dir,
    );
    h.run();
    h.run();
    let admin = app.service(&ADMIN).unwrap();
    let quick = admin.quick_settings(&app);
    for id in [
        "appearance.theme_preference",
        "appearance.interface_zoom",
        "interface.graph.zoom",
        "project.decisions.mode",
    ] {
        assert!(quick.iter().any(|q| q == id), "{id} in {quick:?}");
    }

    h.state_mut()
        .run("admin.quick", json!(null), Origin::User)
        .unwrap();
    h.run_steps(3);
    h.get_by_label("Often changed");
    // "Set for" only on a setting that has a library level.
    h.get_by_label("Where Decision nodes is set");
    assert!(
        h.query_by_label("Where Graph view opens on is set")
            .is_none()
    );

    // A change saved through the node, which another client reads.
    let zoom = SettingId::new("interface.graph.zoom").unwrap();
    admin
        .set_in(&app, ValueScope::App, &zoom, json!("overview"))
        .unwrap();
    let other = node.settings();
    let end = Instant::now() + Duration::from_secs(30);
    while !other
        .read("user")
        .unwrap()
        .unwrap_or_default()
        .contains("overview")
    {
        assert!(Instant::now() < end, "not saved through the node in 30 s");
        std::thread::sleep(Duration::from_millis(10));
        h.run();
    }
    // A setting not marked quick, changed: listed among those changed
    // lately; the quick one is not listed twice.
    let kept = SettingId::new("system.workflow.keep_days").unwrap();
    let before = app.setting::<i64>(&kept).unwrap_or(0);
    admin
        .set_in(&app, ValueScope::App, &kept, json!(before + 3))
        .unwrap();
    h.run();
    h.run();
    let recent = admin.recent_settings(&app, now());
    assert!(
        recent.iter().any(|r| r == "system.workflow.keep_days"),
        "{recent:?}"
    );
    assert!(
        !recent.iter().any(|r| r == "interface.graph.zoom"),
        "{recent:?}"
    );
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
