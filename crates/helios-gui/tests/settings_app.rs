//! Helios's own settings (`helios-config`) in the Settings app: placed
//! once in Helios's scopes, each with the value scopes of its concern,
//! read from their layers (the library's as `library`, `--set` as the
//! command line's) and written back to the layer they belong to.

use std::sync::{Arc, Mutex};

use egui::vec2;
use egui_kittest::kittest::Queryable;
use egui_pop::api::admin::ADMIN;
use egui_pop::api::admin::model::ValueScope;
use egui_pop::model::ids::SettingId;
use helios_gui::config::{self, Settings, Sources};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

type Written = Arc<Mutex<Vec<(String, String)>>>;

#[test]
fn helios_settings_are_placed_read_and_written_to_their_layer() {
    let shell = Shell::new(helios_panels(), None);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let dir = std::env::temp_dir().join(format!("helios-settings-app-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let written: Written = Arc::default();
    let sink = written.clone();
    let settings = Settings::resolve(&Sources {
        global: Some("[system.ai]\ncalls = 4\n".into()),
        project: Some((
            "project//photos".into(),
            Some("[project.decisions]\nmode = \"human\"\n".into()),
        )),
        user: Some("[interface.graph]\nzoom = \"overview\"\n".into()),
        run: vec![("system.scheduler.workers".into(), "8".into())],
    });
    let app = h.state().app().handle().clone();
    config::install(
        &app,
        settings,
        Some(Box::new(move |name: &str, text: &str| {
            sink.lock().unwrap().push((name.into(), text.into()));
            Ok(())
        })),
        &dir,
    );
    h.run();
    let admin = app.service(&ADMIN).unwrap();

    // Placed once, in Helios's scopes, Developer still last.
    let tree = admin.tree(&app);
    let places = |id: &str| -> Vec<String> {
        tree.fields()
            .into_iter()
            .filter(|(_, f)| f.setting_id() == Some(id))
            .map(|(p, _)| p)
            .collect()
    };
    assert_eq!(places("system.cache.thumbnails"), ["storage/caches"]);
    assert_eq!(places("project.decisions.mode"), ["library/decisions"]);
    assert_eq!(places("display.wide-gamut-preview"), ["hardware/display"]);
    assert_eq!(places("catalogue.snapshotEvery"), ["storage/catalogue"]);
    let scopes: Vec<&str> = tree
        .shown_scopes()
        .iter()
        .map(|s| s.meta.id.as_str())
        .collect();
    assert_eq!(scopes.last(), Some(&"developer"), "{scopes:?}");
    let library = scopes.iter().position(|s| *s == "library").unwrap();
    assert!(library < scopes.len() - 1);
    assert!(
        admin
            .diagnostics()
            .all()
            .iter()
            .all(|d| d.code != "admin-no-setting"),
        "every placed setting exists"
    );

    // Read from their layers.
    let id = |k: &str| SettingId::new(k).unwrap();
    let handle = h.state().app().handle().clone();
    assert_eq!(handle.setting::<i64>(&id("system.ai.calls")), Some(4));
    assert_eq!(
        handle
            .setting::<String>(&id("interface.graph.zoom"))
            .as_deref(),
        Some("overview")
    );
    assert_eq!(
        handle
            .setting::<String>(&id("project.decisions.mode"))
            .as_deref(),
        Some("human")
    );
    assert_eq!(
        handle.setting::<i64>(&id("system.scheduler.workers")),
        Some(8)
    );
    let layers = admin.layers();
    assert_eq!(
        layers.origin("settings/project.decisions.mode"),
        ValueScope::Library
    );
    assert_eq!(
        layers.origin("settings/system.scheduler.workers"),
        ValueScope::Run
    );
    assert!(admin.offered().contains(&ValueScope::Library));
    assert!(
        written.lock().unwrap().is_empty(),
        "nothing written by reading"
    );

    // Edits go back to the layer of their concern.
    let mut set = |key: &str, scope: &str, value: serde_json::Value| {
        h.state_mut()
            .run(
                "admin.set",
                json!({ "key": key, "scope": scope, "value": value }),
                Origin::User,
            )
            .unwrap();
        h.run();
        h.run();
    };
    set("system.ai.calls", "app", json!(6));
    let global = std::fs::read_to_string(dir.join(config::GLOBAL_FILE)).unwrap();
    assert!(global.contains("calls = 6"), "{global}");
    set("project.decisions.mode", "library", json!("full_ai"));
    set("interface.graph.zoom", "app", json!("detail"));
    let written = written.lock().unwrap().clone();
    let last = |name: &str| {
        written
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
    };
    assert!(last("project//photos").unwrap().contains("full_ai"));
    // Back to its default: the person's layer holds nothing any more.
    assert!(!last("user").unwrap().contains("zoom"));
    // No workspace is open: its value scope is refused.
    let refused = h.state_mut().run(
        "admin.set",
        json!({ "key": "system.ai.calls", "scope": "workspace", "value": 2 }),
        Origin::User,
    );
    assert!(refused.is_err());

    // The page renders from the manifest.
    h.state_mut()
        .run(
            "admin.open",
            json!({ "path": "library/image-map" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    h.run();
    h.get_by_role_and_label(egui::accesskit::Role::Group, "Image map records");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The search reaches inside the shortcuts editor: a shortcut found by
/// its key lands on the shortcuts page, marked; and every setting shows
/// once, whether a field binds it or a page's widget edits it.
#[test]
fn the_search_lands_on_a_shortcut_and_every_setting_shows_once() {
    use egui::accesskit::{Role, Toggled};
    use egui_kittest::kittest::{NodeT as _, by};
    use egui_pop::api::admin::model::Item;

    let shell = Shell::new(helios_panels(), None);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let app = h.state().app().handle().clone();
    let admin = app.service(&ADMIN).unwrap();

    let tree = admin.tree(&app);
    let mut placed: Vec<&str> = tree
        .sections
        .iter()
        .flat_map(|s| s.groups.iter().flat_map(|g| &g.items))
        .flat_map(|i| -> Vec<&str> {
            match i {
                Item::Field(f) => f.setting_id().into_iter().collect(),
                Item::Widget(w) => w.settings().collect(),
                _ => Vec::new(),
            }
        })
        .collect();
    let all = placed.len();
    placed.sort_unstable();
    placed.dedup();
    assert_eq!(placed.len(), all, "a setting placed twice");

    h.state_mut()
        .run("admin.open", json!({ "path": "library" }), Origin::User)
        .unwrap();
    h.run();
    h.run();
    let hint = "Search every setting";
    h.get_by_role_and_label(Role::SearchInput, hint).click();
    h.run();
    h.get_by_role_and_label(Role::SearchInput, hint)
        .type_text("ctrl+shift+i");
    h.run();
    h.run();
    // The import window's shortcut, chosen in the left panel.
    h.get(by().role(Role::Button).label_contains("(Ctrl+Shift+I)"))
        .click();
    h.run();
    h.run();
    assert_eq!(admin.shown().1.as_deref(), Some("shortcuts"));
    assert_eq!(admin.marked().as_deref(), Some("library.importWindow"));
    let marked = h
        .get_all_by_role(Role::Label)
        .filter(|n| n.accesskit_node().toggled() == Some(Toggled::True))
        .count();
    assert_eq!(marked, 1, "the shortcut's row is marked");
}
