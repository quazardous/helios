//! Automatic imports, without the import window: a card source set to
//! "when plugged in" imports its photos with its own choices when the
//! card is mounted, a folder source set to "when new files appear"
//! imports a file dropped into it once it settled, once; what the
//! Library holds is not imported again, and turning it off stops it. The
//! sources follow another client's change through the node.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use helios_gui::node_volumes::VolumeWatch;
use helios_gui::shell::{IMPORT_SOURCES, ImportTrigger};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_storage::{Card, Volume, Volumes};
use helios_vault::{MemoryStore, Vault};
use serde_json::{Value, json};

type Kept = helios_node_core::settings::NodeSettings<helios_vault_sqlite::SqliteStore>;
type Node = helios_node_core::node::LocalNode<helios_vault_sqlite::SqliteStore>;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-gui-auto-{name}-{}", std::process::id()));
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

/// The cards plugged in, as a test plugs and unplugs them.
#[derive(Default)]
struct Slot(Mutex<Vec<Card>>);

impl Volumes for Slot {
    fn volumes(&self) -> Vec<Volume> {
        self.0.lock().unwrap().iter().map(Card::volume).collect()
    }

    fn cards(&self) -> Vec<Card> {
        self.0.lock().unwrap().clone()
    }

    fn cards_of(&self, _: &[Volume]) -> Vec<Card> {
        self.cards()
    }
}

/// A node over a vault in `dir`, its bus and its settings.
fn node_in(dir: &Path) -> (Node, Arc<helios_core::Bus>, Kept) {
    let store = helios_vault_sqlite::SqliteStore::open(&dir.join("vault.db")).unwrap();
    let bus = Arc::new(helios_core::Bus::new());
    let node = Node::open(store, bus.clone(), Duration::from_hours(1));
    let kept = node.settings();
    (node, bus, kept)
}

/// A shell keeping its import sources through the node, following its
/// changes, set up by `setup` first; folders rescanned every 50 ms and
/// a file settled after 300 ms.
fn shell_on(
    kept: &Kept,
    bus: &helios_core::Bus,
    setup: impl FnOnce(&mut Shell),
) -> Harness<'static, Shell> {
    let (read, write, again) = (kept.clone(), kept.clone(), kept.clone());
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_automatic_import_timing(Duration::from_millis(50), Duration::from_millis(300));
    shell.keep_import_sources_with(
        Box::new(move || read.read(IMPORT_SOURCES)),
        Box::new(move |text| write.write(IMPORT_SOURCES, text)),
    );
    shell.follow_import_sources(bus.subscribe(), move || again.read(IMPORT_SOURCES));
    setup(&mut shell);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
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

/// Frames stepped for `long`, the imports asked meanwhile.
fn imports_during(h: &mut Harness<'static, Shell>, long: Duration) -> Vec<Value> {
    let end = Instant::now() + long;
    let mut imports = Vec::new();
    while Instant::now() < end {
        h.step();
        imports.extend(h.state_mut().take_imports());
        std::thread::sleep(Duration::from_millis(20));
    }
    imports
}

/// The host's `library.import`, as `main.rs` registers it, into `vault`;
/// `done` once each import ended.
fn host_import(shell: &mut Shell, vault: &Arc<Vault<MemoryStore>>, done: &Arc<AtomicBool>) {
    let (v, done, lines, jobs) = (
        vault.clone(),
        done.clone(),
        shell.import_sink(),
        shell.jobs(),
    );
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
                let (v, jobs, done, lines, asked) = (
                    v.clone(),
                    jobs.clone(),
                    done.clone(),
                    lines.clone(),
                    args.clone(),
                );
                std::thread::spawn(move || {
                    let mut emit = |line: &Value| {
                        let _ = lines.send((asked.clone(), line.clone()));
                    };
                    let end = match helios_library::import::import(
                        &v, &jobs, &sources, &options, 1, &mut emit,
                    ) {
                        Ok(report) => json!({ "ended": report }),
                        Err(e) => json!({ "failed": e }),
                    };
                    let _ = lines.send((asked, end));
                    done.store(true, Ordering::Relaxed);
                });
                Ok(json!({ "queued": true }))
            },
        )
        .unwrap();
}

/// The host runs the imports asked, one at a time, each to its end.
fn run_imports(h: &mut Harness<'static, Shell>, imports: Vec<Value>, done: &AtomicBool) {
    for args in imports {
        done.store(false, Ordering::Relaxed);
        h.state_mut()
            .run("library.import", args, Origin::User)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done.load(Ordering::Relaxed) {
            assert!(Instant::now() < deadline, "the import not done in 30 s");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn a_card_set_to_import_when_plugged_in_imports_its_new_photos_with_its_choices() {
    let dir = temp("card");
    let card = dir.join("EOS_DIGITAL");
    let dcim = card.join("DCIM/100CANON");
    std::fs::create_dir_all(&dcim).unwrap();
    std::fs::copy(jpeg("a"), dcim.join("IMG_0001.JPG")).unwrap();
    std::fs::copy(jpeg("b"), dcim.join("IMG_0002.JPG")).unwrap();
    let into = dir.join("photos");
    let (node, bus, kept) = node_in(&dir);
    // The card saved with its own choices, set to import when plugged in.
    let doc = format!(
        "[[source]]\nconnector = \"card\"\nname = \"Camera\"\npath = {card:?}\nauto = \"plugged\"\nconfirmed = \"2026-11-13\"\n\n\
         [source.choices]\ncopy_folders = false\ninto = {into:?}\nlayout = \"month\"\n\
         rename = \"{{stem}}\"\nduplicates = \"keep\"\ndarktable = true\n"
    );
    kept.write(IMPORT_SOURCES, &doc).unwrap();
    let slot = Arc::new(Slot::default());
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let done = Arc::new(AtomicBool::new(false));
    let mut h = shell_on(&kept, &bus, |shell| {
        host_import(shell, &vault, &done);
        shell.follow_volumes(VolumeWatch::start(
            slot.clone(),
            Arc::new(helios_core::Bus::new()),
            Duration::from_millis(50),
        ));
    });
    assert_eq!(
        h.state().import_sources()[0].auto,
        Some(ImportTrigger::Plugged)
    );
    let plug = |slot: &Slot| {
        slot.0.lock().unwrap().push(Card {
            label: "EOS_DIGITAL".into(),
            root: card.clone(),
            capacity: Some(64_000_000_000),
        });
    };
    // Plugged in: its photos imported with its own choices, copied, the
    // duplicates skipped, without the window.
    plug(&slot);
    let mut imports = Vec::new();
    until(&mut h, "the card imported", |s| {
        imports.extend(s.take_imports());
        !imports.is_empty()
    });
    assert!(!h.state().import_window_open());
    assert_eq!(imports.len(), 1, "{imports:?}");
    assert_eq!(
        imports[0],
        json!({
            "paths": [dcim.join("IMG_0001.JPG"), dcim.join("IMG_0002.JPG")],
            "copy": true,
            "into": into,
            "layout": "month",
            "rename": "{stem}",
            "duplicates": "skip",
            // Who asked, for the import history: the automatic import
            // of the source named Camera.
            "origin": "auto:Camera",
        })
    );
    run_imports(&mut h, imports, &done);
    assert_eq!(vault.images().unwrap().len(), 2);
    assert!(into.is_dir(), "copied into the source's folder");
    assert!(
        h.state()
            .notes
            .iter()
            .any(|n| n.contains("Automatic import from Camera: 2 files")),
        "{:?}",
        h.state().notes
    );
    // Unplugged, a new photo taken, plugged in again: only the new photo
    // comes in, the two already in the Library skipped.
    slot.0.lock().unwrap().clear();
    until(&mut h, "the card unplugged", |s| {
        s.import_cards().is_empty()
    });
    std::fs::copy(jpeg("c"), dcim.join("IMG_0003.JPG")).unwrap();
    plug(&slot);
    let mut imports = Vec::new();
    until(&mut h, "the card imported again", |s| {
        imports.extend(s.take_imports());
        !imports.is_empty()
    });
    run_imports(&mut h, imports, &done);
    assert_eq!(vault.images().unwrap().len(), 3, "only the new photo added");
    // Turned off: plugged in again, nothing is imported.
    h.state_mut().set_import_auto(&card, None);
    h.step();
    assert_eq!(h.state().import_sources()[0].auto, None);
    slot.0.lock().unwrap().clear();
    until(&mut h, "the card unplugged", |s| {
        s.import_cards().is_empty()
    });
    plug(&slot);
    until(&mut h, "the card listed", |s| !s.import_cards().is_empty());
    assert!(imports_during(&mut h, Duration::from_millis(500)).is_empty());
    drop(h);
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_set_to_import_new_files_imports_a_file_dropped_in_once_settled_and_once() {
    let dir = temp("folder");
    let hot = dir.join("hot");
    std::fs::create_dir_all(&hot).unwrap();
    let (node, bus, kept) = node_in(&dir);
    let mut h = shell_on(&kept, &bus, |_| {});
    // Another client saves the folder, set to import new files: the
    // shell follows it through the node.
    let doc = format!(
        "[[source]]\nconnector = \"folder\"\nname = \"Hot folder\"\npath = {hot:?}\nauto = \"new-files\"\nconfirmed = \"2026-11-13\"\n"
    );
    kept.write(IMPORT_SOURCES, &doc).unwrap();
    until(&mut h, "the source followed", |s| {
        s.import_sources()
            .first()
            .is_some_and(|s| s.auto == Some(ImportTrigger::NewFiles))
    });
    // A file dropped in: imported once it settled, added where it is,
    // with the window's choices.
    let dropped = hot.join("a.jpg");
    std::fs::copy(jpeg("a"), &dropped).unwrap();
    let started = Instant::now();
    let mut imports = Vec::new();
    until(&mut h, "the file imported", |s| {
        imports.extend(s.take_imports());
        !imports.is_empty()
    });
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "imported before it settled"
    );
    assert_eq!(imports.len(), 1, "{imports:?}");
    assert_eq!(imports[0]["paths"], json!([dropped]));
    assert_ne!(imports[0]["copy"], json!(true), "added where it is");
    assert_eq!(imports[0]["duplicates"], json!("skip"));
    assert_eq!(imports[0]["origin"], json!("auto:Hot folder"));
    // Once: the next scans leave it alone.
    assert!(imports_during(&mut h, Duration::from_millis(800)).is_empty());
    // A second file: only it.
    let second = hot.join("b.jpg");
    std::fs::copy(jpeg("b"), &second).unwrap();
    let mut imports = Vec::new();
    until(&mut h, "the second file imported", |s| {
        imports.extend(s.take_imports());
        !imports.is_empty()
    });
    assert_eq!(imports[0]["paths"], json!([second]));
    // Turned off: a file dropped in stays where it is.
    h.state_mut().set_import_auto(&hot, None);
    h.step();
    let text = kept.read(IMPORT_SOURCES).unwrap().unwrap_or_default();
    assert!(!text.contains("auto ="), "{text}");
    std::fs::copy(jpeg("c"), hot.join("c.jpg")).unwrap();
    assert!(imports_during(&mut h, Duration::from_millis(800)).is_empty());
    drop(h);
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A card `EOS_DIGITAL` holding two photos in `dir`, saved as a source
/// imported when plugged in (`confirmed` or not), and a shell with a host
/// importing into a vault, the card not plugged in yet.
struct CardSetup {
    dir: PathBuf,
    card: PathBuf,
    node: Node,
    slot: Arc<Slot>,
    vault: Arc<Vault<MemoryStore>>,
    done: Arc<AtomicBool>,
    h: Harness<'static, Shell>,
}

fn card_setup(name: &str, confirmed: bool) -> CardSetup {
    let dir = temp(name);
    let card = dir.join("EOS_DIGITAL");
    let dcim = card.join("DCIM/100CANON");
    std::fs::create_dir_all(&dcim).unwrap();
    std::fs::copy(jpeg("a"), dcim.join("IMG_0001.JPG")).unwrap();
    std::fs::copy(jpeg("b"), dcim.join("IMG_0002.JPG")).unwrap();
    let (node, bus, kept) = node_in(&dir);
    let confirmed = if confirmed {
        "confirmed = \"2026-11-13\"\n"
    } else {
        ""
    };
    // the path quoted as a TOML string
    let path = card.display().to_string();
    // Its own choices: copied into a folder of the test's.
    let into = dir.join("photos").display().to_string();
    let doc = format!(
        "[[source]]\nconnector = \"card\"\nname = \"Camera\"\npath = {path:?}\nauto = \"plugged\"\n{confirmed}\n\
         [source.choices]\ncopy_folders = false\ninto = {into:?}\nlayout = \"month\"\n\
         rename = \"{{stem}}\"\nduplicates = \"keep\"\ndarktable = true\n"
    );
    kept.write(IMPORT_SOURCES, &doc).unwrap();
    let slot = Arc::new(Slot::default());
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let done = Arc::new(AtomicBool::new(false));
    let h = shell_on(&kept, &bus, |shell| {
        host_import(shell, &vault, &done);
        shell.follow_volumes(VolumeWatch::start(
            slot.clone(),
            Arc::new(helios_core::Bus::new()),
            Duration::from_millis(50),
        ));
    });
    CardSetup {
        dir,
        card,
        node,
        slot,
        vault,
        done,
        h,
    }
}

impl CardSetup {
    fn plug(&mut self) {
        self.slot.0.lock().unwrap().push(Card {
            label: "EOS_DIGITAL".into(),
            root: self.card.clone(),
            capacity: Some(64_000_000_000),
        });
        until(&mut self.h, "the card listed", |s| {
            !s.import_cards().is_empty()
        });
    }

    fn unplug(&mut self) {
        self.slot.0.lock().unwrap().clear();
        until(&mut self.h, "the card unplugged", |s| {
            s.import_cards().is_empty()
        });
    }

    fn said(&self, text: &str) -> bool {
        self.h.state().notes.iter().any(|n| n.contains(text))
    }

    fn end(self) {
        drop(self.h);
        self.node.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// One of the settings, an integer, as the Settings window writes it.
fn setting_int(h: &mut Harness<'static, Shell>, id: &str, value: i64) {
    use egui_pop::model::values::{Change, Set, Value as PopValue};
    let store = h.state().app().handle().store();
    store
        .lock()
        .unwrap()
        .set(
            Set::new(
                &format!("settings/{id}"),
                Change::To(PopValue::Int(value)),
                egui_pop::model::commands::Origin::User,
            )
            .untracked(),
        )
        .unwrap();
    h.step();
}

#[test]
fn a_source_s_first_automatic_import_opens_the_import_window_where_import_confirms_it() {
    let mut s = card_setup("first-run", false);
    assert_eq!(s.h.state().import_sources()[0].confirmed, None);
    // Plugged in for the first time: nothing imported, the import window
    // opened on the card, saying why.
    s.plug();
    until(&mut s.h, "the import window opened", |s| {
        s.import_window_open()
    });
    assert!(
        s.said("First automatic import from Camera"),
        "{:?}",
        s.h.state().notes
    );
    until(&mut s.h, "the card read", |s| s.import_read());
    assert!(s.h.state_mut().take_imports().is_empty());
    // Import pressed there: imported, and confirmed.
    s.h.get_by_label("Import 2").click();
    let mut imports = Vec::new();
    until(&mut s.h, "the window's import", |s| {
        imports.extend(s.take_imports());
        !imports.is_empty()
    });
    run_imports(&mut s.h, imports, &s.done);
    assert_eq!(s.vault.images().unwrap().len(), 2);
    until(&mut s.h, "the source confirmed", |s| {
        s.import_sources()[0].confirmed.is_some()
    });
    // Plugged in again with a new photo: imported on its own, no window.
    s.unplug();
    std::fs::copy(jpeg("c"), s.card.join("DCIM/100CANON/IMG_0003.JPG")).unwrap();
    s.plug();
    let mut imports = Vec::new();
    until(&mut s.h, "the card imported on its own", |s| {
        imports.extend(s.take_imports());
        !imports.is_empty()
    });
    assert!(!s.h.state().import_window_open());
    assert_eq!(imports[0]["duplicates"], json!("skip"));
    // Turned off then on again: to be confirmed again.
    let card = s.card.clone();
    s.h.state_mut().set_import_auto(&card, None);
    s.h.state_mut()
        .set_import_auto(&card, Some(ImportTrigger::Plugged));
    s.h.step();
    assert_eq!(s.h.state().import_sources()[0].confirmed, None);
    s.end();
}

#[test]
fn an_automatic_import_above_the_limit_asks_first_and_pausing_holds_them_all() {
    let mut s = card_setup("ask", true);
    // More photos than the limit: offered in a toast, not imported.
    setting_int(&mut s.h, "import.automatic.ask_files", 1);
    s.plug();
    until(&mut s.h, "the question", |h| {
        h.notes.iter().any(|n| n.contains("2 new photos"))
    });
    assert!(imports_during(&mut s.h, Duration::from_millis(300)).is_empty());
    assert!(!s.h.state().import_window_open());
    // Paused: nothing imported, said with Resume.
    setting_int(&mut s.h, "import.automatic.ask_files", 0);
    s.unplug();
    s.h.state_mut()
        .run(
            "library.importAutomatic.pause",
            json!({ "on": true }),
            Origin::User,
        )
        .unwrap();
    s.plug();
    until(&mut s.h, "said paused", |h| {
        h.notes
            .iter()
            .any(|n| n.contains("Automatic imports are paused: Camera not imported"))
    });
    assert!(imports_during(&mut s.h, Duration::from_millis(300)).is_empty());
    // Resumed (the command toggles): plugged in again, imported.
    s.unplug();
    s.h.state_mut()
        .run("library.importAutomatic.pause", Value::Null, Origin::User)
        .unwrap();
    s.plug();
    let mut imports = Vec::new();
    until(&mut s.h, "imported once resumed", |h| {
        imports.extend(h.take_imports());
        !imports.is_empty()
    });
    run_imports(&mut s.h, imports, &s.done);
    assert_eq!(s.vault.images().unwrap().len(), 2);
    s.end();
}
