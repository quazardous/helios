//! Settings > Plugins: an installed plugin's page rendered from its
//! configuration contract, its values saved through the store Helios
//! gives the Settings app (the app's to their file, the library's to the
//! node), a secret masked on screen and never logged, and a library value
//! another client of the node changes shown live.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::api::admin::ADMIN;
use egui_pop::model::ids::SettingId;
use helios_connect::account::{MemorySecrets, Secrets as _};
use helios_gui::config::{self, LIBRARY, Settings};
use helios_gui::{Shell, headless, helios_panels};
use helios_node_core::node::LocalNode;
use helios_plugin::{Broker, Policy, Scope};
use helios_shell::commands::Origin;
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

const PLUGIN: &str = "org.example.fake-settings";
const PROJECT: &str = "project//photos";
const SECRET: &str = "hunter2-very-secret";

/// Every log line, to check a secret never reaches one.
struct Capture(Mutex<String>);

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        let mut all = self.0.lock().unwrap();
        let _ = writeln!(all, "{}", record.args());
    }

    fn flush(&self) {}
}

fn logs() -> &'static Capture {
    static LOGS: OnceLock<&'static Capture> = OnceLock::new();
    LOGS.get_or_init(|| {
        let c: &'static Capture = Box::leak(Box::new(Capture(Mutex::new(String::new()))));
        let _ = log::set_logger(c);
        log::set_max_level(log::LevelFilter::Trace);
        c
    })
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-settings-plugins-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A plugin that never runs, with settings of every kind.
fn plugin_dir(dir: &std::path::Path) -> PathBuf {
    let dir = dir.join("plugin");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("helios-plugin.toml"),
        format!(
            r#"
id = "{PLUGIN}"
name = "Fake settings"
description = "A plugin to try settings with"
version = "1.0.0"
license = "GPL-3.0-or-later"
command = ["false"]

[compat]
helios = ">=0.0.1"
interfaces = {{ protocol = 1 }}

[settings.quality]
type = "int"
default = 90
min = 1
max = 100
label = "Preview quality"
per_library = true

[settings.mode]
type = "choice"
choices = ["fast", "best"]
default = "fast"
label = "Mode"

[settings.token]
type = "secret"
label = "Access token"

[[capabilities]]
name = "network"
reason = "Talks to the fake service"
"#
        ),
    )
    .unwrap();
    dir
}

struct World {
    h: Harness<'static, Shell>,
    node: LocalNode<SqliteStore>,
    secrets: Arc<MemorySecrets>,
    dir: PathBuf,
}

fn world(name: &str) -> World {
    let dir = temp(name);
    let mut broker = Broker::new(Policy::default());
    broker.add(&plugin_dir(&dir), Scope::User).unwrap();
    let broker = Arc::new(broker);
    let mut shell = Shell::builder(helios_panels(), None)
        .plugins(&broker)
        .build();
    shell.set_plugins(broker);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let store = SqliteStore::open(&dir.join("vault.db")).unwrap();
    let bus = Arc::new(helios_core::Bus::new());
    let node = LocalNode::open(store, bus.clone(), Duration::from_hours(1));
    let app = h.state().app().handle().clone();
    let secrets = Arc::new(MemorySecrets::default());
    config::use_secrets(&app, secrets.clone());
    let heard = bus.subscribe();
    let mine = node.settings();
    let (sources, problems) = config::read(&dir, &|n| mine.read(n), Some(PROJECT.into()), &[]);
    assert!(problems.is_empty(), "{problems:?}");
    let kept = config::install(
        &app,
        Settings::resolve(&sources),
        Some(Arc::new(node.settings())),
        &dir,
    );
    kept.follow(heard);
    let mut w = World {
        h,
        node,
        secrets,
        dir,
    };
    saved(&mut w.h);
    w
}

/// Runs passes until the Settings app's saves have reached the store.
fn saved(h: &mut Harness<'_, Shell>) {
    let admin = h.state().app().handle().service(&ADMIN).unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    h.run();
    while admin.saving() {
        assert!(Instant::now() < end, "not saved in 30 s");
        std::thread::sleep(Duration::from_millis(10));
        h.run();
    }
}

fn set(h: &mut Harness<'_, Shell>, key: &str, scope: &str, value: Value) {
    h.state_mut()
        .run(
            "admin.set",
            json!({ "key": format!("{PLUGIN}.{key}"), "scope": scope, "value": value }),
            Origin::User,
        )
        .unwrap();
    saved(h);
}

fn id(key: &str) -> SettingId {
    SettingId::new(format!("{PLUGIN}.{key}")).unwrap()
}

#[test]
fn a_plugin_s_page_renders_its_contract_and_saves_through_the_store() {
    logs();
    let mut w = world("page");
    let app = w.h.state().app().handle().clone();
    let admin = app.service(&ADMIN).unwrap();

    // Placed on the plugin's page, with the contract's defaults.
    let tree = admin.tree(&app);
    let placed: Vec<String> = tree
        .fields()
        .into_iter()
        .filter(|(p, _)| p == "plugins/org-example-fake-settings")
        .filter_map(|(_, f)| f.setting_id().map(String::from))
        .collect();
    assert_eq!(
        placed,
        [
            format!("{PLUGIN}.mode"),
            format!("{PLUGIN}.quality"),
            format!("{PLUGIN}.token")
        ]
    );
    assert_eq!(app.setting::<i64>(&id("quality")), Some(90));
    assert_eq!(app.setting::<String>(&id("mode")).as_deref(), Some("fast"));
    assert!(admin.allows(&app, &id("quality"), LIBRARY.scope));
    assert!(!admin.allows(&app, &id("mode"), LIBRARY.scope));

    // The app's value lands in its file, the library's in the node.
    set(&mut w.h, "mode", "app", json!("best"));
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(w.dir.join(config::plugins::PLUGINS_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(file[PLUGIN]["mode"], "best");
    set(&mut w.h, "quality", "library", json!(40));
    let layer = w.node.settings().read(PROJECT).unwrap().unwrap();
    assert!(layer.contains("quality = 40"), "{layer}");
    assert!(
        admin.store_errors().is_empty(),
        "{:?}",
        admin.store_errors()
    );

    // A secret: kept in the keyring, typed masked, never shown or
    // logged.
    set(&mut w.h, "token", "app", json!(SECRET));
    assert_eq!(
        w.secrets
            .get(&format!("plugin:{PLUGIN}"), "token")
            .unwrap()
            .as_deref(),
        Some(SECRET)
    );
    w.h.state_mut()
        .run(
            "admin.open",
            json!({ "path": "plugins/org-example-fake-settings" }),
            Origin::User,
        )
        .unwrap();
    w.h.run();
    w.h.run();
    for shown in [
        "Preview quality",
        "Access token",
        "Talks to the fake service",
    ] {
        assert!(
            w.h.query_all_by_label_contains(shown).next().is_some(),
            "{shown} on the page\n{:#?}",
            w.h.root()
        );
    }
    let tree = format!("{:#?}", w.h.root());
    assert!(!tree.contains(SECRET), "the secret is masked on screen");
    let file = std::fs::read_to_string(w.dir.join(config::plugins::PLUGINS_FILE)).unwrap();
    assert!(!file.contains(SECRET), "never in the file");
    assert!(!logs().0.lock().unwrap().contains(SECRET), "never logged");
    assert!(
        admin.store_errors().iter().all(|e| !e.1.contains(SECRET)),
        "never in an error"
    );
    w.node.stop();
    let _ = std::fs::remove_dir_all(&w.dir);
}

#[test]
fn a_library_value_another_client_changes_appears() {
    let mut w = world("live");
    let app = w.h.state().app().handle().clone();
    let other = w.node.settings();
    other
        .write(
            PROJECT,
            &format!("[plugin-settings.\"{PLUGIN}\"]\nquality = 33\n"),
        )
        .unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    while app.setting::<i64>(&id("quality")) != Some(33) {
        assert!(
            Instant::now() < end,
            "not read again in 30 s: {:?}",
            app.setting::<i64>(&id("quality"))
        );
        std::thread::sleep(Duration::from_millis(10));
        w.h.run();
    }
    let admin = app.service(&ADMIN).unwrap();
    assert_eq!(
        admin.layers().origin(&format!("settings/{PLUGIN}.quality")),
        LIBRARY.scope
    );
    w.node.stop();
    let _ = std::fs::remove_dir_all(&w.dir);
}
