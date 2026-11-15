//! The first-run setup (`shell::init_wizard`): a fresh profile opens it,
//! a required step blocks Finish until a writable folder is given, the
//! import window asks for the library folder in a partial run and opens
//! after it, and the health warning's fix opens the same step.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

/// The library folder, kept in memory for the test.
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

/// The setup's state, `None` while it is not open.
fn setup(h: &mut Harness<'_, Shell>) -> Option<Value> {
    let s = h
        .state_mut()
        .run("admin.wizard.set", json!({ "values": {} }), Origin::User)
        .ok()?;
    (s["id"] == "init.setup" || s["wizard"] == "init.setup").then_some(s)
}

/// A fresh shell whose library has no folder chosen.
fn fresh(first_run: bool) -> (Shell, Arc<Mutex<Option<PathBuf>>>) {
    let folder = Arc::new(Mutex::new(None));
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_import_library_folder(Arc::new(Folder(folder.clone())));
    shell.set_first_run(first_run);
    (shell, folder)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-init-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A fresh profile: the setup opens on its own, on the library folder; a
/// required step blocks Finish while empty or not a full path; a folder
/// given is made, set, and the setup closes.
#[test]
fn a_fresh_profile_opens_the_setup_and_a_folder_finishes_it() {
    let (shell, folder) = fresh(true);
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    until(&mut h, "the setup opened on the first run", |h| {
        setup(h).is_some()
    });
    until(&mut h, "the folder step drawn", |h| {
        h.query_by_label("Choose…").is_some()
    });
    let s = setup(&mut h).unwrap();
    assert_eq!(s["step"], "library.folder", "{s}");

    // Required: empty, Finish is refused; a relative path too.
    run(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "library.folder", "value": "" }),
    )
    .unwrap();
    assert!(run(&mut h, "admin.wizard.finish", json!({})).is_err());
    run(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "library.folder", "value": "Pictures/Helios" }),
    )
    .unwrap();
    let refused = run(&mut h, "admin.wizard.finish", json!({})).unwrap_err();
    assert!(refused.contains("full path"), "{refused}");
    assert!(folder.lock().unwrap().is_none());

    // A folder that does not exist yet: made, set, the setup closed.
    let chosen = scratch("first").join("Pictures").join("Helios");
    run(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "library.folder", "value": chosen.display().to_string() }),
    )
    .unwrap();
    run(&mut h, "admin.wizard.finish", json!({})).unwrap();
    until(&mut h, "the folder set", |_| {
        folder.lock().unwrap().as_deref() == Some(chosen.as_path())
    });
    assert!(chosen.is_dir(), "the folder is made");
    until(&mut h, "the setup closed", |h| setup(h).is_none());
    let _ = std::fs::remove_dir_all(scratch("first"));
}

/// The import window with no library folder: the setup on that step
/// alone; left, the window opens and says why it cannot import; done,
/// the window opens without it.
#[test]
fn the_import_window_asks_for_the_library_folder_first() {
    let (shell, folder) = fresh(false);
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    h.run();
    assert!(setup(&mut h).is_none(), "no first run asked");

    run(&mut h, "library.importWindow", Value::Null).unwrap();
    until(&mut h, "the folder step", |h| {
        setup(h).is_some_and(|s| s["step"] == "library.folder")
    });
    assert!(!h.state().import_window_open(), "not before the folder");
    run(&mut h, "admin.wizard.cancel", json!({ "force": true })).unwrap();
    let why = "Choose a library folder first: imports copy photos into it (Helios menu › Set up Helios…).";
    until(&mut h, "the window saying why", |h| {
        h.state().import_window_open() && h.query_by_label(why).is_some()
    });
    run(&mut h, "window.close", json!({ "window": "import" })).ok();
    h.run();

    // Again, the folder given: back to the import window, nothing to say.
    let chosen = scratch("import");
    run(&mut h, "library.importWindow", Value::Null).unwrap();
    until(&mut h, "the folder step again", |h| setup(h).is_some());
    run(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "library.folder", "value": chosen.display().to_string() }),
    )
    .unwrap();
    run(&mut h, "admin.wizard.finish", json!({})).unwrap();
    until(&mut h, "the import window", |h| {
        h.state().import_window_open()
    });
    assert_eq!(folder.lock().unwrap().as_deref(), Some(chosen.as_path()));
    until(&mut h, "no reason left", |h| {
        h.query_by_label(why).is_none()
    });
    let _ = std::fs::remove_dir_all(&chosen);
}

/// The health warning's fix and "Set up Helios…" open the setup, even
/// with a folder chosen; an agent's `path` sets the folder at once.
#[test]
fn the_fix_and_the_menu_open_the_setup_and_a_path_sets_the_folder() {
    let (shell, folder) = fresh(false);
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    h.run();
    let chosen = scratch("agent");
    run(
        &mut h,
        helios_health::checks::CHOOSE_LIBRARY_FOLDER,
        json!({ "path": chosen.display().to_string() }),
    )
    .unwrap();
    assert_eq!(folder.lock().unwrap().as_deref(), Some(chosen.as_path()));
    assert!(setup(&mut h).is_none());

    run(
        &mut h,
        helios_health::checks::CHOOSE_LIBRARY_FOLDER,
        json!({}),
    )
    .unwrap();
    until(&mut h, "the folder step", |h| {
        setup(h).is_some_and(|s| s["step"] == "library.folder")
    });
    run(&mut h, "admin.wizard.cancel", json!({ "force": true })).unwrap();
    until(&mut h, "closed", |h| setup(h).is_none());

    run(&mut h, "helios.setUp", json!({})).unwrap();
    until(&mut h, "the setup from the menu", |h| setup(h).is_some());
    let _ = std::fs::remove_dir_all(&chosen);
}

/// The setup is a focus screen: the main window's whole page while it
/// runs, whatever the Settings app's window setting (Helios opens it in a
/// window of its own), nothing of the library under it; finished, the
/// library is back.
#[test]
fn the_setup_is_a_focus_screen_and_the_library_comes_back_after_it() {
    let (shell, folder) = fresh(false);
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    h.run();
    let app = h.state().app().handle().clone();
    assert!(
        egui_pop::api::admin::in_window(&app),
        "Helios opens the Settings app in a window of its own"
    );
    assert!(h.query_all_by_label("Library").count() > 0, "the library");

    run(&mut h, "helios.setUp", json!({})).unwrap();
    until(&mut h, "the setup", |h| setup(h).is_some());
    let s = setup(&mut h).unwrap();
    assert_eq!(s["focus"], true, "{s}");
    assert_eq!(s["count"], 1, "the run's steps alone: {s}");
    assert!(!egui_pop::api::admin::in_window(&app), "in the main window");
    until(&mut h, "the folder step drawn", |h| {
        h.query_by_label("Choose…").is_some()
    });
    assert_eq!(
        h.query_all_by_label("Library").count(),
        0,
        "nothing of the library under the setup"
    );

    let chosen = scratch("focus");
    run(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "library.folder", "value": chosen.display().to_string() }),
    )
    .unwrap();
    run(&mut h, "admin.wizard.finish", json!({})).unwrap();
    until(&mut h, "the setup closed", |h| setup(h).is_none());
    assert_eq!(folder.lock().unwrap().as_deref(), Some(chosen.as_path()));
    until(&mut h, "the library back", |h| {
        h.query_all_by_label("Library").count() > 0
    });
    assert!(egui_pop::api::admin::in_window(&app), "the setting holds");
    let _ = std::fs::remove_dir_all(&chosen);
}
