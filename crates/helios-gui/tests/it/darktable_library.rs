//! The darktable library importer Helios ships, on the desktop: a Helios
//! with its built-in plugins (written out with the importer's program, as
//! the desktop starts) offers File > Import from → darktable library, and a darktable
//! folder (built by the importer's `fixture` module) imports with its
//! stars, rejects, labels, tags and texts landing in the library.
//!
//! The importer's program must be built beside these tests
//! (`target/<profile>/helios-darktable-library`, as `cargo nextest run
//! --workspace` does); without it they say so and pass.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::vec2;
use egui_kittest::Harness;
use helios_gui::{Shell, headless, helios_panels, importers};
use helios_plugin::{Broker, Policy, bundled};
use helios_plugin_darktable_library::fixture::{self, Options, Schema};
use helios_shell::commands::Origin;
use helios_vault::{ImageId, MemoryStore, Vault};
use serde_json::{Value, json};

const PLUGIN: &str = "io.github.quazardous.helios.darktable-library";
const IMPORTER: &str = "darktable-library";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../helios-plugin/tests/fixtures")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-gui-dt-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Where the workspace's programs are built: the test's own profile
/// folder (`target/<profile>/deps/<test>`).
fn programs() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().to_path_buf()
}

/// A broker with Helios's built-in plugins, as the desktop finds them;
/// none when the importer's program is not built.
fn builtin(dir: &Path) -> Option<Broker> {
    let mut dirs = Vec::new();
    let added = bundled::add_to(&mut dirs, &dir.join("plugins"), &programs()).unwrap();
    if added.dir.is_none() {
        eprintln!(
            "helios-darktable-library is not built in {}: skipped",
            programs().display()
        );
        return None;
    }
    let mut b = Broker::new(Policy::default());
    b.discover(&dirs);
    assert!(b.rejected().is_empty(), "{:?}", b.rejected());
    Some(b)
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

fn run(h: &mut Harness<'static, Shell>, id: &str, args: Value) {
    h.state_mut().run(id, args, Origin::User).unwrap();
    for _ in 0..3 {
        h.run();
    }
}

#[test]
fn import_from_offers_the_darktable_library_with_the_built_in_plugins() {
    let dir = scratch("menu");
    let Some(broker) = builtin(&dir) else {
        return;
    };
    let shell = Shell::builder(helios_panels(), None)
        .plugins(&broker)
        .build();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let entries = import_from(h.state_mut()).expect("File > Import from");
    let entry = entries
        .iter()
        .find(|e| e["title"] == "darktable library")
        .unwrap_or_else(|| panic!("no darktable library: {entries:?}"));
    assert_eq!(entry["id"], importers::COMMAND);
    assert_eq!(
        entry["args"],
        json!({ "plugin": PLUGIN, "importer": IMPORTER })
    );
    // Chosen, it asks for darktable's folder.
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
            .contains("darktable library"),
        "{form}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_darktable_library_imports_with_its_marks() {
    let dir = scratch("import");
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        std::fs::copy(
            fixtures().join("rated-photos").join(name),
            photos.join(name),
        )
        .unwrap();
    }
    let source = dir.join("darktable");
    fixture::build(&source, Schema::V5_2, &photos, &Options::default()).unwrap();
    let Some(broker) = builtin(&dir) else {
        return;
    };
    let broker = Arc::new(broker);
    let vault = Vault::new(MemoryStore::default());
    let jobs = helios_core::jobs::Jobs::new(Arc::new(helios_core::Bus::new()));
    let args = json!({
        "importer": { "plugin": PLUGIN, "importer": IMPORTER, "source": source },
        "copy": false,
    });
    let mut lines = Vec::new();
    let report = importers::run(
        &broker,
        &jobs,
        &args,
        &mut |line: &Value| {
            lines.push(line.clone());
        },
        |files, options, emit| {
            helios_library::import::import(&vault, &jobs, files, options, 1, emit)
        },
    )
    .unwrap();
    assert_eq!((report.added, report.metadata), (3, 3), "{lines:?}");
    let scanned = &lines[0]["scanned"];
    assert!(
        scanned["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("drive-not-plugged-in")),
        "{scanned}"
    );
    let id = |name: &str| {
        let line = lines
            .iter()
            .find(|l| {
                l["path"]
                    .as_str()
                    .is_some_and(|p| std::path::Path::new(p).ends_with(name))
            })
            .unwrap_or_else(|| panic!("no {name}: {lines:?}"));
        ImageId::from_hex(line["id"].as_str().unwrap()).unwrap()
    };
    let fields = vault
        .fields_of(
            &[id("a.jpg"), id("b.jpg"), id("c.jpg")],
            &[
                "rating",
                "rejected",
                "labels",
                "tags",
                "title",
                "description",
            ],
        )
        .unwrap();
    let sorted = |v: &Value| {
        let mut s: Vec<String> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect();
        s.sort();
        s
    };
    assert_eq!(fields[0]["rating"], json!(3));
    assert_eq!(fields[0]["rejected"], json!(false));
    assert_eq!(sorted(&fields[0]["labels"]), ["blue", "red"]);
    assert_eq!(
        sorted(&fields[0]["tags"]),
        ["people|Ann", "places|France|Lyon"]
    );
    assert_eq!(fields[0]["title"], json!("Low tide"));
    assert_eq!(
        (&fields[1]["rating"], &fields[1]["rejected"]),
        (&json!(2), &json!(true))
    );
    assert_eq!(sorted(&fields[1]["labels"]), ["green"]);
    assert_eq!(fields[1]["description"], json!("Rocks"));
    assert_eq!(fields[2]["rating"], json!(0));
    let _ = std::fs::remove_dir_all(&dir);
}
