//! After a reset (`helios_gui::reset`, `helios_gui::forget`), on fixture
//! folders and an in-memory keyring only, never the person's:
//!
//! - an "everything" reset records the photo folders, and the first-run
//!   setup offers them back: Finish adds the photos in place through the
//!   import, every photo byte the same, the offer made once;
//! - "Forget accounts" removes Helios's keyring entries and no other
//!   application's, only with the word typed;
//! - an agent can neither forget the accounts nor re-import.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_gui::forget::{self, Keyring, MemoryKeyring};
use helios_gui::reset::{self, Pending, Photos, Reset};
use helios_gui::shell::init_wizard::{REIMPORT, REIMPORT_FOLDERS, REIMPORT_STEP};
use helios_gui::{Shell, headless, helios_panels};
use helios_node_core::reset::{Places, Scope};
use helios_shell::commands::Origin;
use helios_vault::{MemoryStore, Vault};
use serde_json::{Value, json};

fn until(h: &mut Harness<'_, Shell>, what: &str, done: impl Fn(&mut Harness<'_, Shell>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("never got there: {what}");
}

fn run(h: &mut Harness<'_, Shell>, cmd: &str, args: Value) -> Result<Value, String> {
    let out = h.state_mut().run(cmd, args, Origin::User);
    h.run();
    out
}

/// The open wizard's state when it is `id`.
fn wizard(h: &mut Harness<'_, Shell>, id: &str) -> Option<Value> {
    let s = h
        .state_mut()
        .run("admin.wizard.set", json!({ "values": {} }), Origin::User)
        .ok()?;
    (s["id"] == id || s["wizard"] == id).then_some(s)
}

/// A scratch folder, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("helios-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    /// Helios's four folders, each with something in it.
    fn places(&self) -> Places {
        let dir = |p: &str| {
            let d = self.0.join(p).join("helios");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("x"), b"x").unwrap();
            d
        };
        Places {
            config: dir("config"),
            data: dir("data"),
            state: dir("state"),
            cache: dir("cache"),
            library: None,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Every file under `dir` and its bytes.
fn hashes(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.insert(p.clone(), std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, &mut out);
    out
}

/// The library folder, kept in memory.
struct Folder(Arc<Mutex<Option<PathBuf>>>);

impl helios_gui::shell::LibraryFolder for Folder {
    fn get(&self) -> Result<Option<PathBuf>, String> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn set(&self, dir: &Path) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(dir.to_path_buf());
        Ok(())
    }
}

fn jpeg(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../helios-plugin/tests/fixtures/rated-photos")
        .join(format!("{name}.jpg"))
}

/// A shell with the reset's commands over `places`, a confirmed plan
/// kept in `pending`, the quit asked doing nothing.
fn shell_with_reset(places: Places, pending: Pending) -> Shell {
    let mut shell = Shell::new(helios_panels(), None);
    reset::register(
        &mut shell,
        Reset {
            places,
            photos: Arc::new(Photos::default),
            pending,
            go: Arc::new(|| Ok(())),
        },
    )
    .unwrap();
    shell
}

#[test]
fn after_a_reset_the_setup_adds_the_photos_back_where_they_are() {
    let home = Scratch::new("reimport");
    let places = home.places();
    let photos = home.0.join("Pictures");
    std::fs::create_dir_all(photos.join("2026")).unwrap();
    std::fs::copy(jpeg("a"), photos.join("2026/a.jpg")).unwrap();
    std::fs::copy(jpeg("c"), photos.join("c.jpg")).unwrap();
    std::fs::write(photos.join("c.jpg.xmp"), b"<x:xmpmeta/>").unwrap();
    let before = hashes(&photos);

    // The reset of everything, as the desktop runs it after its quit.
    let plan = helios_node_core::reset::plan(
        &places,
        Scope::Everything,
        std::slice::from_ref(&photos),
        "1",
    )
    .unwrap();
    assert_eq!(helios_node_core::reset::run(&plan).failed, None);
    let offer = helios_node_core::reset::pending_reimport(&places.data)
        .expect("the reset recorded the photo folders");
    assert_eq!(offer.folders, [photos.clone()]);

    // The next launch: a fresh library, the first-run setup.
    let folder = Arc::new(Mutex::new(None));
    let vault = Arc::new(Vault::new(MemoryStore::default()));
    let done = Arc::new(AtomicBool::new(false));
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_import_library_folder(Arc::new(Folder(folder.clone())));
    shell.set_first_run(true);
    shell.offer_reimport(Some(offer));
    super::import::host_import(&mut shell, &vault, &done);
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    until(&mut h, "the setup open", |h| wizard(h, "init.setup").is_some());
    run(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "library.folder", "value": home.0.join("Library").display().to_string() }),
    )
    .unwrap();
    for _ in 0..5 {
        if wizard(&mut h, "init.setup").unwrap()["step"] == REIMPORT_STEP {
            break;
        }
        run(&mut h, "admin.wizard.next", json!({})).unwrap();
    }
    assert_eq!(wizard(&mut h, "init.setup").unwrap()["step"], REIMPORT_STEP);
    // The folders the reset kept, listed; the toggle on.
    let listed = run(&mut h, REIMPORT_FOLDERS, json!({})).unwrap();
    assert_eq!(
        listed["folders"][0]["path"],
        json!(photos.display().to_string())
    );
    until(&mut h, "the folder listed", |h| {
        h.query_by_label_contains(&photos.display().to_string())
            .is_some()
    });
    assert!(
        h.query_by_label_contains("Add these photos back, where they are")
            .is_some()
    );

    run(&mut h, "admin.wizard.finish", json!({})).unwrap();
    until(&mut h, "the setup closed", |h| {
        wizard(h, "init.setup").is_none()
    });
    // Added in place: the import asked for the folder, nothing copied.
    let imports = h.state_mut().take_imports();
    assert_eq!(imports.len(), 1, "{imports:?}");
    assert_eq!(imports[0]["paths"], json!([photos]));
    assert_eq!(imports[0]["copy"], json!(false));
    assert!(imports[0].get("into").is_none());
    for args in imports {
        h.state_mut()
            .run("library.import", args, Origin::User)
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done.load(Ordering::Relaxed) {
        assert!(Instant::now() < deadline, "the import not done in 30 s");
        std::thread::sleep(Duration::from_millis(20));
    }
    let images = vault.images().unwrap();
    assert_eq!(images.len(), 2, "both photos added");
    assert_eq!(hashes(&photos), before, "a photo changed");
    assert!(
        !home.0.join("Library").read_dir().is_ok_and(|mut d| d.next().is_some()),
        "nothing copied into the library folder"
    );
    // Offered once.
    assert!(h.state().reimport_offer().is_none());
    assert_eq!(
        helios_node_core::reset::pending_reimport(&places.data),
        None
    );
    assert!(run(&mut h, REIMPORT, json!({})).is_err(), "nothing left");
}

#[test]
fn forget_accounts_removes_helios_entries_only_and_never_for_an_agent() {
    let keyring = Arc::new(MemoryKeyring::default());
    keyring.put("helios-import", "google-1/refresh_token");
    keyring.put("helios-ai", "openai");
    keyring.put("other-app", "token");
    let all = keyring.entries();
    let home = Scratch::new("forget");
    let mut shell = shell_with_reset(home.places(), Pending::default());
    forget::register(&mut shell, keyring.clone() as Arc<dyn Keyring>).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    h.run();

    // Agents: refused, even with the word; the re-import too.
    for (command, args) in [
        (forget::COMMAND, json!({ "confirm": forget::WORD })),
        (REIMPORT, json!({})),
    ] {
        let refused = h
            .state_mut()
            .run(command, args, Origin::Agent("helper".into()))
            .unwrap_err();
        assert!(refused.contains("not offered to agents"), "{command}: {refused}");
    }
    // Without the word: refused.
    assert!(run(&mut h, forget::COMMAND, json!({})).is_err());
    assert_eq!(keyring.entries(), all, "nothing forgotten yet");

    // The Danger zone's row, then the word.
    run(&mut h, "admin.open", json!({ "path": reset::SECTION })).unwrap();
    until(&mut h, "the Danger zone drawn", |h| {
        h.query_by_role_and_label(Role::Button, "Forget accounts…")
            .is_some()
    });
    h.get_by_role_and_label(Role::Button, "Forget accounts…")
        .click();
    until(&mut h, "the wizard open", |h| {
        wizard(h, forget::WIZARD).is_some()
    });
    run(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "confirm", "value": "forget" }),
    )
    .unwrap();
    assert!(run(&mut h, "admin.wizard.finish", json!({})).is_err());
    assert_eq!(keyring.entries(), all);
    run(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "confirm", "value": forget::WORD }),
    )
    .unwrap();
    run(&mut h, "admin.wizard.finish", json!({})).unwrap();
    until(&mut h, "the wizard closed", |h| {
        wizard(h, forget::WIZARD).is_none()
    });
    assert_eq!(
        keyring.entries(),
        [("other-app".to_owned(), "token".to_owned())],
        "only Helios's entries go"
    );
}

/// An "everything" reset asked with `forget_accounts` keeps that for its
/// quit; another scope refuses it.
#[test]
fn a_reset_of_everything_may_forget_the_accounts_with_it() {
    let home = Scratch::new("reset-accounts");
    let places = home.places();
    let pending = Pending::default();
    let shell = shell_with_reset(places, pending.clone());
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    h.run();
    let refused = run(
        &mut h,
        reset::COMMAND,
        json!({ "scope": "caches", "confirm": reset::WORD, "forget_accounts": true }),
    );
    assert!(refused.is_err());
    assert!(!pending.is_set());
    run(
        &mut h,
        reset::COMMAND,
        json!({ "scope": "everything", "confirm": reset::WORD, "forget_accounts": true }),
    )
    .unwrap();
    let (plan, accounts) = pending.take_with_accounts().unwrap();
    assert_eq!(plan.scope, Scope::Everything);
    assert!(accounts, "the accounts go with it");
    // As the desktop does after its quit, on an in-memory keyring.
    let keyring = MemoryKeyring::default();
    keyring.put("helios-ai", "openai");
    keyring.put("other-app", "token");
    forget::forget(&keyring).unwrap();
    assert_eq!(
        keyring.entries(),
        [("other-app".to_owned(), "token".to_owned())]
    );
}
