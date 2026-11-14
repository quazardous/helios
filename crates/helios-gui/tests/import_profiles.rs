//! Filter profiles and tags on a saved import: a filter saved under a
//! name on one source is picked again on another, kept on it with what
//! its photos are given (keywords, a rating, a label); its automatic
//! import takes only the files the filter takes, gives each of them the
//! keywords, rating and label, and the import history records both.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use helios_gui::shell::{IMPORT_SOURCES, ImportTrigger};
use helios_gui::{Shell, headless, helios_panels};
use helios_library::import_profile::{Apply, Filter, Kind};
use helios_shell::commands::Origin;
use helios_vault::{MemoryStore, Vault};
use serde_json::{Value, json};

type Kept = helios_node_core::settings::NodeSettings<helios_vault_sqlite::SqliteStore>;
type Node = helios_node_core::node::LocalNode<helios_vault_sqlite::SqliteStore>;

fn temp(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("helios-gui-profiles-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A tiny JPEG of the fixtures (`a`, `b` or `c`, each its own content).
fn jpeg(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../helios-plugin/tests/fixtures/rated-photos")
        .join(format!("{name}.jpg"))
}

fn node_in(dir: &Path) -> (Node, Arc<helios_core::Bus>, Kept) {
    let store = helios_vault_sqlite::SqliteStore::open(&dir.join("vault.db")).unwrap();
    let bus = Arc::new(helios_core::Bus::new());
    let node = Node::open(store, bus.clone(), Duration::from_hours(1));
    let kept = node.settings();
    (node, bus, kept)
}

/// Frames stepped until `done` (at most 20 s).
fn until(h: &mut Harness<'static, Shell>, what: &str, mut done: impl FnMut(&mut Shell) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        h.step();
        if done(h.state_mut()) {
            return;
        }
        assert!(Instant::now() < deadline, "never got there in 20 s: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The host's `library.import`, as `main.rs` registers it, into `vault`;
/// each import's report kept in `ended`.
fn host_import(shell: &mut Shell, vault: &Arc<Vault<MemoryStore>>, ended: &Arc<Mutex<Vec<Value>>>) {
    let (v, ended, jobs) = (vault.clone(), ended.clone(), shell.jobs());
    shell
        .register(
            helios_shell::commands::CommandSpec::new("library.import", "Import", "Library"),
            move |args, _| {
                let options = helios_gui::importers::options(args)?;
                let sources: Vec<PathBuf> = args["paths"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect();
                let (v, jobs, ended) = (v.clone(), jobs.clone(), ended.clone());
                std::thread::spawn(move || {
                    let end = match helios_library::import::import(
                        &v,
                        &jobs,
                        &sources,
                        &options,
                        1,
                        &mut |_| {},
                    ) {
                        Ok(report) => json!({ "ended": report }),
                        Err(e) => json!({ "failed": e }),
                    };
                    ended.lock().unwrap().push(end);
                });
                Ok(json!({ "queued": true }))
            },
        )
        .unwrap();
}

#[test]
fn a_named_filter_and_tags_on_a_saved_source_shape_its_automatic_import_and_its_history() {
    let dir = temp("auto");
    let first = dir.join("first");
    let hot = dir.join("hot");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&hot).unwrap();
    let (node, bus, kept) = node_in(&dir);
    let doc = format!(
        "[[source]]\nconnector = \"folder\"\nname = \"First\"\npath = {first:?}\n\n\
         [[source]]\nconnector = \"folder\"\nname = \"Hot folder\"\npath = {hot:?}\nauto = \"new-files\"\nconfirmed = \"2026-11-13\"\n"
    );
    kept.write(IMPORT_SOURCES, &doc).unwrap();
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let ended = Arc::new(Mutex::new(Vec::new()));
    let (read, write, again) = (kept.clone(), kept.clone(), kept.clone());
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_automatic_import_timing(Duration::from_millis(50), Duration::from_millis(300));
    shell.keep_import_sources_with(
        Box::new(move || read.read(IMPORT_SOURCES)),
        Box::new(move |text| write.write(IMPORT_SOURCES, text)),
    );
    shell.follow_import_sources(bus.subscribe(), move || again.read(IMPORT_SOURCES));
    shell.remember_import_choices_in(dir.join("library/import-choices.json"));
    host_import(&mut shell, &vault, &ended);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    until(&mut h, "the sources read", |s| {
        s.import_sources().len() == 2
    });
    assert_eq!(
        h.state().import_sources()[1].auto,
        Some(ImportTrigger::NewFiles)
    );

    // A filter saved under a name on the first source, then picked again
    // on the hot folder, with the keywords, rating and label its photos
    // are given.
    let keepers = Filter {
        kinds: vec![Kind::Jpeg],
        names: vec!["keep_*".into()],
        ..Filter::default()
    };
    h.state_mut().save_import_filter("Keepers", keepers.clone());
    h.state_mut()
        .use_import_filter_on(&first, "Keepers")
        .unwrap();
    h.state_mut().use_import_filter_on(&hot, "Keepers").unwrap();
    assert!(h.state_mut().use_import_filter_on(&hot, "Nope").is_err());
    let apply = Apply {
        keywords: vec!["trip".into(), "family".into()],
        rating: Some(3),
        label: Some("red".into()),
    };
    h.state_mut().set_import_apply_on(&hot, apply.clone());
    h.step();
    let sources = h.state().import_sources();
    let choices = |path: &Path| {
        sources
            .iter()
            .find(|s| s.path.as_deref() == Some(path))
            .and_then(|s| s.choices.clone())
            .expect("the source's own choices")
    };
    assert_eq!(choices(&first).filter, keepers);
    assert_eq!(choices(&hot).filter, keepers);
    assert_eq!(choices(&hot).apply, apply);
    // Kept: the named filter as a filter preset with the import
    // profiles, the source's in the sources' document, each source
    // pointing at a profile made of its choices.
    assert!(sources.iter().all(|s| s.profile.is_some()), "{sources:?}");
    let filters = std::fs::read_to_string(dir.join("library/import-profiles.json")).unwrap();
    assert!(filters.contains("Keepers"), "{filters}");
    assert_eq!(
        h.state().import_filters(),
        vec![("Keepers".into(), keepers)]
    );
    let until_kept = Instant::now() + Duration::from_secs(10);
    loop {
        let text = kept.read(IMPORT_SOURCES).unwrap().unwrap_or_default();
        if text.contains("keep_*") && text.contains("trip") {
            break;
        }
        assert!(Instant::now() < until_kept, "never kept: {text}");
        std::thread::sleep(Duration::from_millis(20));
    }

    // Two files dropped in: the automatic import asks for both with the
    // filter and the tags, and only the one the filter takes comes in.
    std::fs::copy(jpeg("a"), hot.join("keep_1.jpg")).unwrap();
    std::fs::copy(jpeg("b"), hot.join("other_2.jpg")).unwrap();
    let mut imports = Vec::new();
    until(&mut h, "the files imported", |s| {
        imports.extend(s.take_imports());
        !imports.is_empty()
    });
    let args = imports.pop().unwrap();
    assert_eq!(args["filter"]["names"], json!(["keep_*"]), "{args}");
    assert_eq!(args["apply"]["keywords"], json!(["trip", "family"]));
    assert_eq!(args["origin"], json!("auto:Hot folder"));
    h.state_mut()
        .run("library.import", args, Origin::User)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while ended.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the import not done in 30 s");
        std::thread::sleep(Duration::from_millis(20));
    }
    let end = ended.lock().unwrap()[0].clone();
    assert_eq!(end["ended"]["added"], 1, "{end}");
    let images = vault.images().unwrap();
    assert_eq!(images.len(), 1, "only keep_1.jpg");
    let fields = vault
        .fields_of(&[images[0].id], &["rating", "labels", "tags"])
        .unwrap();
    assert_eq!(fields[0]["rating"], json!(3));
    assert_eq!(fields[0]["labels"], json!(["red"]));
    let mut tags: Vec<&str> = fields[0]["tags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    tags.sort_unstable();
    assert_eq!(tags, ["family", "trip"]);
    // The history records the filter and the tags with the choices.
    let records = helios_library::import_history::list(&vault, None).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].choices["filter"]["names"], json!(["keep_*"]));
    assert_eq!(records[0].choices["apply"]["rating"], json!(3));
    assert_eq!(records[0].origin, "auto:Hot folder");
    drop(h);
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_manual_import_with_a_filter_and_tags_takes_only_matching_files_and_tags_them() {
    let dir = temp("manual");
    let folder = dir.join("card");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::copy(jpeg("a"), folder.join("IMG_0001.JPG")).unwrap();
    std::fs::copy(jpeg("b"), folder.join("IMG_0002.JPG")).unwrap();
    std::fs::copy(jpeg("c"), folder.join("DSC_0003.JPG")).unwrap();
    let vault = Vault::new(MemoryStore::default());
    let jobs = helios_core::jobs::Jobs::new(Arc::new(helios_core::Bus::new()));
    // As the import window asks it: the source's choices as arguments.
    let args = json!({
        "paths": [folder],
        "filter": { "names": ["IMG_*"], "new_only": true },
        "apply": { "keywords": ["holiday"], "label": "green" },
        "origin": "user",
    });
    let options = helios_gui::importers::options(&args).unwrap();
    let report = helios_library::import::import(
        &vault,
        &jobs,
        std::slice::from_ref(&folder),
        &options,
        1,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(report.added, 2, "the DSC file left out");
    for image in vault.images().unwrap() {
        let fields = vault.fields_of(&[image.id], &["labels", "tags"]).unwrap();
        assert_eq!(fields[0]["tags"], json!(["holiday"]));
        assert_eq!(fields[0]["labels"], json!(["green"]));
    }
    // New only: a second run finds nothing new at those places.
    let again = helios_library::import::import(
        &vault,
        &jobs,
        std::slice::from_ref(&folder),
        &options,
        2,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(again.files, 0, "{again:?}");
    let records = helios_library::import_history::list(&vault, None).unwrap();
    assert!(
        records
            .iter()
            .all(|r| r.choices["filter"]["names"] == json!(["IMG_*"])),
        "{records:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
