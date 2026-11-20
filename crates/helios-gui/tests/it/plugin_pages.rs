//! A plugin's declared page (`[ui] pages`) in Settings > Plugins: the
//! official Google Photos plugin's settings page, read from its folder
//! before anything of it runs, shows its field, status and action, found
//! by their accessibility labels; the action runs the plugin's own command
//! in its process. A page reaching beyond its plugin is refused item by
//! item, said on the page, and the rest still shows.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::api::admin::ADMIN;
use helios_gui::{Shell, headless, helios_panels};
use helios_plugin::{Broker, Policy, Scope};
use helios_shell::commands::Origin;
use serde_json::json;

const GOOGLE: &str = "io.github.quazardous.helios.google-photos";
const SECTION: &str = "plugins/io-github-quazardous-helios-google-photos";

/// Stands in for the plugin's program: answers its check, and writes a
/// line to `calls` for each command it runs.
const STUB: &str = r#"
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        if m.get("method") == "exit":
            break
        continue
    r = {"jsonrpc": "2.0", "id": m["id"]}
    method = m.get("method")
    if method == "initialize":
        r["result"] = {"name": "stub"}
    elif method == "command/execute":
        with open("calls", "a") as f:
            f.write(m["params"]["command"] + "\n")
        r["result"] = {"answers": True, "status": {"text": "https://stub.example/v1", "tone": "success"}}
    else:
        r["result"] = None
    sys.stdout.write(json.dumps(r) + "\n")
    sys.stdout.flush()
"#;

fn temp(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("helios-plugin-pages-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The Google Photos plugin's folder as it ships (manifest, page,
/// catalogues), its program swapped for [`STUB`].
fn google_photos(dir: &Path) -> PathBuf {
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/sources/google-photos");
    let to = dir.join("google-photos");
    for file in [
        "ui/settings.json",
        "i18n/en/plugin.ftl",
        "i18n/fr/plugin.ftl",
    ] {
        std::fs::create_dir_all(to.join(file).parent().unwrap()).unwrap();
        std::fs::copy(from.join(file), to.join(file)).unwrap();
    }
    let manifest = std::fs::read_to_string(from.join("helios-plugin.toml"))
        .unwrap()
        .replace(
            r#"command = ["helios-google-photos"]"#,
            r#"command = ["python3", "stub.py"]"#,
        );
    std::fs::write(to.join("helios-plugin.toml"), manifest).unwrap();
    std::fs::write(to.join("stub.py"), STUB).unwrap();
    to
}

fn shell(broker: Broker) -> Harness<'static, Shell> {
    let broker = Arc::new(broker);
    let mut shell = Shell::builder(helios_panels(), None)
        .plugins(&broker)
        .build();
    shell.set_plugins(broker);
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    h.run();
    h
}

fn open(h: &mut Harness<'_, Shell>, path: &str) {
    h.state_mut()
        .run("admin.open", json!({ "path": path }), Origin::User)
        .unwrap();
    h.run();
    h.run();
}

/// Runs passes until `done` holds, at most 30 s.
fn until(h: &mut Harness<'_, Shell>, what: &str, done: impl Fn(&Harness<'_, Shell>) -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        h.run();
        if done(h) {
            return;
        }
        assert!(
            Instant::now() < end,
            "{what} not seen in 30 s\n{:#?}",
            h.root()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn calls(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("calls")).map_or(0, |t| t.lines().count())
}

#[test]
fn google_photos_declares_its_settings_page_and_its_action_runs_its_command() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 is not installed: skipped");
        return;
    }
    let dir = temp("google");
    let plugin = google_photos(&dir);
    let mut broker = Broker::new(Policy::default());
    broker.add(&plugin, Scope::User).unwrap();
    let mut h = shell(broker);
    h.state_mut()
        .run(
            "plugin.grant",
            json!({"plugin": GOOGLE, "capability": "network"}),
            Origin::User,
        )
        .unwrap();

    // The page's field is the plugin's own setting, placed once.
    let app = h.state().app().handle().clone();
    let admin = app.service(&ADMIN).unwrap();
    let placed: Vec<String> = admin
        .tree(&app)
        .fields()
        .into_iter()
        .filter(|(p, _)| p == SECTION)
        .filter_map(|(_, f)| f.setting_id().map(String::from))
        .collect();
    assert_eq!(placed, [format!("{GOOGLE}.api")]);

    open(&mut h, SECTION);
    for shown in ["Connection", "Picker API address", "Picker API"] {
        assert!(
            h.query_all_by_label_contains(shown).next().is_some(),
            "{shown} on the page\n{:#?}",
            h.root()
        );
    }
    assert!(
        h.query_all_by_label_contains("not shown").next().is_none(),
        "nothing refused"
    );
    // Its status reads the plugin's `read_only` check.
    until(&mut h, "the status", |h| {
        h.query_all_by_label_contains("https://stub.example/v1")
            .next()
            .is_some()
    });
    let before = calls(&plugin);
    h.get_by_role_and_label(Role::Button, "Test connection")
        .click();
    until(&mut h, "the command run", |_| calls(&plugin) > before);
    let run = std::fs::read_to_string(plugin.join("calls")).unwrap();
    assert!(run.lines().all(|c| c == "google-photos.api-check"), "{run}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_page_reaching_beyond_its_plugin_is_refused_item_by_item() {
    let dir = temp("rogue");
    let plugin = dir.join("rogue");
    std::fs::create_dir_all(plugin.join("ui")).unwrap();
    std::fs::write(
        plugin.join("helios-plugin.toml"),
        r#"
id = "org.example.rogue"
name = "Rogue page"
version = "1.0.0"
license = "GPL-3.0-or-later"
command = ["false"]

[compat]
helios = ">=0.0.1"
interfaces = { protocol = 1, ui = 1 }

[settings.mode]
type = "choice"
choices = ["fast", "best"]
default = "fast"
label = "Mode"

[ui]
pages = ["ui/page.json", "ui/missing.json"]
"#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("ui/page.json"),
        json!({
            "version": 1,
            "sections": [{"id": "main", "scope": "plugins", "groups": [{
                "id": "mine",
                "title": "org-example-rogue-mine",
                "items": [
                    {"node": "field", "id": "theirs", "bind": {"to": "setting", "setting": "helios.import.folder"}},
                    {"node": "action", "id": "forget", "command": "library.forget"},
                    {"node": "hologram", "id": "future"},
                    {"node": "field", "id": "mode", "bind": {"to": "setting", "setting": "org.example.rogue.mode"}},
                ],
            }]}],
        })
        .to_string(),
    )
    .unwrap();
    let mut broker = Broker::new(Policy::default());
    broker.add(&plugin, Scope::User).unwrap();
    let mut h = shell(broker);
    open(&mut h, "plugins/org-example-rogue");
    // Its own field still shows; what reached beyond it is said.
    assert!(h.query_all_by_label_contains("Mode").next().is_some());
    let page = format!("{:#?}", h.root());
    for said in [
        "not shown",
        "helios.import.folder",
        "library.forget",
        "unknown brick `hologram`",
        "ui/missing.json: not read",
    ] {
        assert!(page.contains(said), "{said}\n{page}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
