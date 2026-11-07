//! Importer plugins on the desktop, end to end with the test plugin
//! (`helios-plugin`'s `tests/fixtures/ratings-folder`, Python with its
//! standard library alone): File > Import from lists it, choosing it asks
//! for the folder, the import runs as the host runs it, the photos land
//! in the library with their ratings, and the Log panel says what the
//! plugin found and warned about.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use egui::vec2;
use egui_kittest::Harness;
use helios_gui::dialogs::Level;
use helios_gui::{Shell, headless, helios_panels, importers};
use helios_plugin::{Broker, Policy, Scope};
use helios_shell::commands::Origin;
use helios_vault::{ImageId, MemoryStore, Vault};
use serde_json::{Value, json};

const PLUGIN: &str = "org.example.ratings-folder";
const IMPORTER: &str = "ratings-folder";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../helios-plugin/tests/fixtures")
}

fn broker() -> Broker {
    let mut b = Broker::new(Policy::default());
    b.add(&fixtures().join("ratings-folder"), Scope::Builtin)
        .unwrap();
    b
}

/// A copy of the rated photos: three JPEGs and their `ratings.json`.
fn photos() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-gui-importer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(fixtures().join("rated-photos")).unwrap() {
        let path = entry.unwrap().path();
        std::fs::copy(&path, dir.join(path.file_name().unwrap())).unwrap();
    }
    dir
}

fn run(h: &mut Harness<'static, Shell>, id: &str, args: Value) {
    h.state_mut().run(id, args, Origin::User).unwrap();
    for _ in 0..3 {
        h.run();
    }
}

/// File > Import from's entries, or none when the submenu is not shown.
fn import_from(shell: &mut Shell) -> Option<Vec<Value>> {
    let items = shell.menu_items(egui_pop::api::app::MENU_FILE.as_str());
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "submenu" && i["label"] == "Import from")
        .map(|s| s["items"].as_array().unwrap().clone())
}

#[test]
fn import_from_shows_only_when_an_importer_is_installed() {
    let mut bare = Shell::new(helios_panels(), None);
    assert!(import_from(&mut bare).is_none());
    let mut shell = Shell::builder(helios_panels(), None)
        .plugins(&broker())
        .build();
    let entries = import_from(&mut shell).expect("File > Import from");
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["id"], importers::COMMAND);
    assert_eq!(entries[0]["title"], "Folder with ratings.json");
    assert_eq!(
        entries[0]["args"],
        json!({ "plugin": PLUGIN, "importer": IMPORTER })
    );
}

#[test]
fn an_importer_asks_for_its_folder_and_its_photos_land_with_their_ratings() {
    let python = Command::new("python3").arg("--version").output().is_ok();
    if !python {
        eprintln!("python3 is not installed: skipped");
        return;
    }
    let broker = Arc::new(broker());
    let dir = photos();
    let shell = Shell::builder(helios_panels(), None)
        .plugins(&broker)
        .build();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    // The menu's entry asks for the folder, and where the photos go.
    run(
        &mut h,
        importers::COMMAND,
        json!({ "plugin": PLUGIN, "importer": IMPORTER }),
    );
    let form = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap();
    assert_eq!(form["command"], json!(importers::COMMAND), "{form}");
    assert!(
        form["title"]
            .as_str()
            .unwrap()
            .contains("Folder with ratings.json"),
        "{form}"
    );
    let keys: Vec<&str> = form["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["source", "copy", "darktable"]);
    run(
        &mut h,
        "dialog.answer",
        json!({ "values": { "source": dir, "copy": false } }),
    );
    let imports = h.state_mut().take_imports();
    let expected = json!({
        "importer": { "plugin": PLUGIN, "importer": IMPORTER, "source": dir },
    });
    assert_eq!(imports, [expected]);
    // As the desktop's `library.import` runs it: the plugin lists, Helios
    // imports, each line to the shell, then the end.
    let vault = Vault::new(MemoryStore::default());
    let jobs = helios_core::jobs::Jobs::new(Arc::new(helios_core::Bus::new()));
    let sink = h.state().import_sink();
    let args = imports[0].clone();
    let mut lines = Vec::new();
    let report = importers::run(&broker, &vault, &jobs, &args, 1, &mut |line: &Value| {
        lines.push(line.clone());
        sink.send((args.clone(), line.clone())).unwrap();
    })
    .unwrap();
    sink.send((args.clone(), json!({ "ended": report })))
        .unwrap();
    assert_eq!((report.added, report.metadata), (3, 2), "{lines:?}");
    // The photos, with the ratings, labels and tags ratings.json gives.
    let id = |name: &str| {
        let line = lines
            .iter()
            .find(|l| {
                l["path"]
                    .as_str()
                    .is_some_and(|p| p.ends_with(&format!("/{name}")))
            })
            .unwrap_or_else(|| panic!("no {name}: {lines:?}"));
        ImageId::from_hex(line["id"].as_str().unwrap()).unwrap()
    };
    let fields = vault
        .fields_of(
            &[id("a.jpg"), id("b.jpg"), id("c.jpg")],
            &["rating", "rejected", "labels", "tags", "title"],
        )
        .unwrap();
    assert_eq!(fields[0]["rating"], json!(5));
    assert_eq!(fields[0]["labels"], json!(["red"]));
    assert_eq!(fields[0]["tags"], json!(["beach", "trip"]));
    assert_eq!(fields[0]["title"], json!("Low tide"));
    assert_eq!(
        (&fields[1]["rating"], &fields[1]["rejected"]),
        (&json!(2), &json!(true))
    );
    assert!(fields[2].get("rating").is_none_or(Value::is_null));
    // The Log panel: what the plugin found and warned about, then the end.
    h.run();
    let log = h.state().log();
    let said: Vec<(Level, String)> = log
        .entries()
        .iter()
        .map(|e| (e.level, e.message.english.clone()))
        .collect();
    assert!(
        said.iter()
            .any(|(l, m)| *l == Level::Info
                && m.starts_with("Folder with ratings.json: 3 photos found")),
        "{said:?}"
    );
    assert!(
        said.iter()
            .any(|(l, m)| *l == Level::Warning && m.contains("gone.jpg")),
        "{said:?}"
    );
    assert!(
        said.iter()
            .any(|(_, m)| m.starts_with("2 photos took their ratings")),
        "{said:?}"
    );
    assert!(
        said.iter().any(|(_, m)| m.starts_with("3 photos imported")),
        "{said:?}"
    );
    drop(log);
    let _ = std::fs::remove_dir_all(&dir);
}
