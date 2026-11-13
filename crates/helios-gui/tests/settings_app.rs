//! Helios's own settings (`helios-config`) in the Settings app: placed
//! once in Helios's scopes, each with the value scopes of its concern,
//! read from their layers (the library's as `library`, `--set` as the
//! command line's) and saved through the store Helios gives the Settings
//! app to the layer they belong to; a layer another client of the node
//! changes is read again. A setting is
//! app-wide unless it opts into Helios's library or workspace scope, and
//! a value stored at a scope it no longer allows moves to the app's.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::kittest::Queryable;
use egui_pop::api::admin::ADMIN;
use egui_pop::api::admin::model::ValueScope;
use egui_pop::model::ids::SettingId;
use helios_gui::config::LIBRARY;
use helios_gui::config::{self, NodeLayers, Settings, Sources};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

/// The node's layers in memory, every write recorded.
#[derive(Default)]
struct Written(Mutex<Vec<(String, String)>>);

impl NodeLayers for Written {
    fn read(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self.last(name))
    }

    fn write(&self, name: &str, text: &str) -> Result<(), String> {
        self.0.lock().unwrap().push((name.into(), text.into()));
        Ok(())
    }
}

impl Written {
    fn last(&self, name: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
    }

    fn is_empty(&self) -> bool {
        self.0.lock().unwrap().is_empty()
    }
}

/// Runs passes until the Settings app's saves have reached the store.
fn saved(h: &mut egui_kittest::Harness<'_, Shell>) {
    let admin = h.state().app().handle().service(&ADMIN).unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    h.run();
    while admin.saving() {
        assert!(Instant::now() < end, "not saved in 30 s");
        std::thread::sleep(Duration::from_millis(10));
        h.run();
    }
    assert!(
        admin.store_errors().is_empty(),
        "{:?}",
        admin.store_errors()
    );
}

#[test]
fn helios_settings_are_placed_read_and_written_to_their_layer() {
    let shell = Shell::new(helios_panels(), None);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let dir = std::env::temp_dir().join(format!("helios-settings-app-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let written = Arc::new(Written::default());
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
    config::install(&app, settings, Some(written.clone()), &dir);
    h.run();
    let admin = app.service(&ADMIN).unwrap();
    assert!(admin.has_store());

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
        LIBRARY.scope
    );
    assert_eq!(
        layers.origin("settings/system.scheduler.workers"),
        ValueScope::Run
    );
    assert!(admin.offered().contains(&LIBRARY.scope));
    saved(&mut h);
    assert!(written.is_empty(), "nothing written by reading");

    // Edits go back to the layer of their concern.
    let mut set = |key: &str, scope: &str, value: serde_json::Value| {
        h.state_mut()
            .run(
                "admin.set",
                json!({ "key": key, "scope": scope, "value": value }),
                Origin::User,
            )
            .unwrap();
        saved(&mut h);
    };
    set("system.ai.calls", "app", json!(6));
    let global = std::fs::read_to_string(dir.join(config::GLOBAL_FILE)).unwrap();
    assert!(global.contains("calls = 6"), "{global}");
    set("project.decisions.mode", "library", json!("full_ai"));
    set("interface.graph.zoom", "app", json!("detail"));
    assert!(written.last("project//photos").unwrap().contains("full_ai"));
    // Back to its default: the person's layer holds nothing any more.
    assert!(!written.last("user").unwrap().contains("zoom"));
    // The interface's own settings go to their file in the folder given.
    set("display.wide-gamut-preview", "app", json!(true));
    let interface: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(config::INTERFACE_FILE)).unwrap())
            .unwrap();
    assert_eq!(interface["display.wide-gamut-preview"], json!(true));
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
    // The import window's shortcut, chosen in the left panel (the
    // results page lists it too).
    h.get_by_role_and_label(Role::Tree, "Sections")
        .get(by().role(Role::Button).label_contains("(Ctrl+Shift+I)"))
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

/// A layer the node keeps that another of its clients changes is read
/// again: the Settings app shows the new values without a restart, and
/// the desktop writes nothing back.
#[test]
fn a_layer_another_client_of_the_node_changes_is_read_again() {
    use helios_node_core::node::LocalNode;
    use helios_vault_sqlite::SqliteStore;

    let dir = std::env::temp_dir().join(format!("helios-settings-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = SqliteStore::open(&dir.join("vault.db")).unwrap();
    let bus = Arc::new(helios_core::Bus::new());
    let node = LocalNode::open(store, bus.clone(), Duration::from_hours(1));
    let project = "project//photos";
    let other = node.settings();
    other
        .write("user", "[interface.graph]\nzoom = \"overview\"\n")
        .unwrap();

    // The desktop: its settings read through the node, followed live.
    let shell = Shell::new(helios_panels(), None);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let app = h.state().app().handle().clone();
    let heard = bus.subscribe();
    let mine = node.settings();
    let (sources, problems) = config::read(&dir, &|n| mine.read(n), Some(project.into()), &[]);
    assert!(problems.is_empty(), "{problems:?}");
    let kept = config::install(
        &app,
        Settings::resolve(&sources),
        Some(Arc::new(node.settings())),
        &dir,
    );
    kept.follow(heard);
    saved(&mut h);
    let user_before = mine.read("user").unwrap();
    let id = |k: &str| SettingId::new(k).unwrap();
    let zoom = |app: &egui_pop::api::AppHandle| app.setting::<String>(&id("interface.graph.zoom"));
    assert_eq!(zoom(&app).as_deref(), Some("overview"));

    // Another client of the node changes the person's layer and the
    // library's.
    other
        .write("user", "[interface.graph]\nzoom = \"detail\"\n")
        .unwrap();
    other
        .write(project, "[project.decisions]\nmode = \"human\"\n")
        .unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    let mode =
        |app: &egui_pop::api::AppHandle| app.setting::<String>(&id("project.decisions.mode"));
    while zoom(&app).as_deref() != Some("detail") || mode(&app).as_deref() != Some("human") {
        assert!(
            Instant::now() < end,
            "not read again in 30 s: zoom {:?}, mode {:?}",
            zoom(&app),
            mode(&app)
        );
        h.run();
    }
    h.run();
    h.run();
    let admin = app.service(&ADMIN).unwrap();
    assert_eq!(
        admin.layers().origin("settings/project.decisions.mode"),
        LIBRARY.scope
    );
    saved(&mut h);
    assert!(
        mine.read("user").unwrap().unwrap().contains("detail"),
        "the other client's change stays"
    );
    assert_ne!(mine.read("user").unwrap(), user_before);
    assert!(
        !dir.join(config::GLOBAL_FILE).exists(),
        "nothing written back by reading"
    );
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_a_setting_that_opts_into_a_scope_offers_set_for_and_a_stray_value_moves_to_the_app() {
    let shell = Shell::new(helios_panels(), None);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let dir = std::env::temp_dir().join(format!("helios-settings-scopes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A library value of an app-wide setting, left by an older program:
    // read as the app's; the library's own defaults stay in its layer.
    let settings = Settings::resolve(&Sources {
        project: Some((
            "project//photos".into(),
            Some("[system.ai]\ncalls = 5\n[project.decisions]\nmode = \"full_ai\"\n".into()),
        )),
        ..Sources::default()
    });
    let app = h.state().app().handle().clone();
    config::install(&app, settings, None, &dir);
    // A workspace saved when every setting could be set per workspace.
    let file = dir.join("workspace-settings.json");
    std::fs::write(
        &file,
        json!({ "system.cache.share": 12.5, "project.decisions.mode": "human" }).to_string(),
    )
    .unwrap();
    config::open_workspace(&app, Some(file.clone()));
    h.run();
    h.run();
    let admin = app.service(&ADMIN).unwrap();
    assert_eq!(admin.extra_scopes(), config::SCOPES.to_vec());
    let id = |k: &str| SettingId::new(k).unwrap();
    assert!(admin.allows(&app, &id("project.decisions.mode"), LIBRARY.scope));
    assert!(!admin.allows(&app, &id("project.decisions.mode"), config::WORKSPACE.scope));
    assert!(!admin.allows(&app, &id("system.ai.calls"), LIBRARY.scope));

    // The stray workspace values moved to the app's, out of the file.
    saved(&mut h);
    assert_eq!(app.setting::<f64>(&id("system.cache.share")), Some(12.5));
    assert_eq!(
        admin.layers().origin("settings/project.decisions.mode"),
        LIBRARY.scope
    );
    assert_eq!(
        admin.layers().origin("settings/system.cache.share"),
        ValueScope::App
    );
    // The project setting's workspace value is now its app value, in
    // the machine's file.
    let global = std::fs::read_to_string(dir.join(config::GLOBAL_FILE)).unwrap();
    assert!(global.contains("human"), "{global}");
    let left: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(left, json!({}), "the workspace saved without them");

    assert_eq!(app.setting::<i64>(&id("system.ai.calls")), Some(5));
    assert_eq!(
        admin.layers().origin("settings/system.ai.calls"),
        ValueScope::App
    );
    assert_eq!(
        admin.layers().origin("settings/project.decisions.mode"),
        LIBRARY.scope
    );
    assert!(
        admin
            .set_in(&app, LIBRARY.scope, &id("system.ai.calls"), json!(3))
            .is_err(),
        "an app-wide setting is refused a library value"
    );

    // An app-wide setting: its editor, no "Set for".
    h.state_mut()
        .run(
            "admin.open",
            json!({ "key": "system.ai.calls" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    h.run();
    h.get_by_role_and_label(egui::accesskit::Role::Slider, "Model calls at once");
    h.get_by_label("Reset Model calls at once");
    assert!(
        h.query_by_label("Where Model calls at once is set")
            .is_none()
    );
    // A library default: "Set for", with Helios's label for the library
    // and none for the workspace it did not opt into.
    h.state_mut()
        .run(
            "admin.open",
            json!({ "key": "project.decisions.mode" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    h.run();
    h.get_by_label("Where Decision nodes is set").click();
    h.run();
    h.run();
    // The library holds its value: the choice shows it, and the list
    // offers it.
    assert!(h.query_all_by_label("This library").count() >= 2);
    assert!(h.query_by_label("This workspace").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
