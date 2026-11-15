//! Helios's settings through the Settings app's export and import: an
//! exported file holds Helios's own settings and reads back into them,
//! the import's plan flags what it cannot apply, and the import is undone
//! in one step.

use egui::vec2;
use egui_pop::model::ids::SettingId;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::{Value as Json, json};

fn run(h: &mut egui_kittest::Harness<'_, Shell>, cmd: &str, args: Json) -> Result<Json, String> {
    let out = h.state_mut().run(cmd, args, Origin::User);
    h.run();
    h.run();
    out
}

#[test]
fn helios_settings_export_and_import_back_in_one_undoable_step() {
    let shell = Shell::new(helios_panels(), None);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let zoom = SettingId::new("interface.graph.zoom").unwrap();
    let calls = SettingId::new("system.ai.calls").unwrap();
    run(
        &mut h,
        "admin.set",
        json!({ "key": "interface.graph.zoom", "value": "overview" }),
    )
    .unwrap();
    run(
        &mut h,
        "admin.set",
        json!({ "key": "system.ai.calls", "value": 5 }),
    )
    .unwrap();
    let out = run(&mut h, "settings.export", json!({ "dryRun": true })).unwrap();
    let values = &out["document"]["settings"]["values"];
    assert_eq!(values["interface.graph.zoom"], "overview", "{out}");
    assert_eq!(values["system.ai.calls"], 5);
    assert_eq!(out["written"], false);

    // Back to the defaults, then the file read back, with a value out of
    // range and a setting Helios does not have.
    let mut document = out["document"].clone();
    document["settings"]["values"]["system.scheduler.workers"] = json!(100_000);
    document["settings"]["values"]["helios.nowhere"] = json!(true);
    run(
        &mut h,
        "admin.reset",
        json!({ "key": "interface.graph.zoom" }),
    )
    .unwrap();
    run(&mut h, "admin.reset", json!({ "key": "system.ai.calls" })).unwrap();
    let handle = h.state().app().handle().clone();
    let before = handle.setting::<String>(&zoom);
    let plan = run(
        &mut h,
        "settings.import",
        json!({ "document": document, "dryRun": true }),
    )
    .unwrap();
    let verdict = |key: &str| {
        plan["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["key"] == key)
            .map(|r| r["verdict"].as_str().unwrap().to_string())
    };
    assert_eq!(verdict("interface.graph.zoom").as_deref(), Some("change"));
    assert_eq!(
        verdict("system.scheduler.workers").as_deref(),
        Some("invalid")
    );
    assert_eq!(verdict("helios.nowhere").as_deref(), Some("unknown"));
    let done = run(&mut h, "settings.import", json!({ "document": document })).unwrap();
    assert!(
        done["applied"]
            .as_array()
            .unwrap()
            .contains(&json!("interface.graph.zoom")),
        "{done}"
    );
    assert_eq!(handle.setting::<String>(&zoom).as_deref(), Some("overview"));
    assert_eq!(handle.setting::<i64>(&calls), Some(5));
    run(&mut h, "settings.undoImport", json!({})).unwrap();
    assert_eq!(handle.setting::<String>(&zoom), before);
    assert_ne!(handle.setting::<i64>(&calls), Some(5));
}
