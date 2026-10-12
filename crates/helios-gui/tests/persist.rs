//! The library and the vault behind it: what the user changes is written
//! as field commands, a failed write puts the stored value back on screen,
//! and other writers' changes show.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use helios_core::{Bus, ContentHash};
use helios_gui::persist::Persist;
use helios_gui::{Shell, helios_panels};
use helios_vault::fields::{Command, LABELS, RATING, REJECTED};
use helios_vault::{Image, ImageId, Store, Vault};
use helios_vault_sqlite::{Options, SqliteStore};
use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-persist-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn id(n: u8) -> ImageId {
    ImageId(ContentHash([n; 32]))
}

/// A vault file with images 1 and 2.
fn vault_file(dir: &Path) -> PathBuf {
    let path = dir.join("vault.db");
    let mut store = SqliteStore::open(&path).unwrap();
    for n in [1, 2] {
        store
            .add_image(&Image {
                id: id(n),
                size: 10,
                partial: ContentHash([n; 32]),
            })
            .unwrap();
    }
    path
}

/// A shell whose library is filled from the vault at `path`, the first
/// image selected.
fn open(path: &Path, options: &Options) -> (Shell, Persist<SqliteStore>) {
    let mut shell = Shell::new(helios_panels(), None);
    let persist = Persist::new(
        SqliteStore::open_with(path, options).unwrap(),
        Arc::new(Bus::new()),
    );
    persist.fill_library(&mut shell).unwrap();
    shell.library_mut().select(0);
    (shell, persist)
}

fn stored(path: &Path, field: &str) -> Option<Value> {
    let v = Vault::new(SqliteStore::open(path).unwrap());
    v.fields_of(&[id(1)], &[field]).unwrap()[0]
        .get(field)
        .cloned()
}

#[test]
fn a_label_toggled_from_a_stale_view_keeps_the_label_another_writer_added() {
    let dir = scratch("stale-label");
    let path = vault_file(&dir);
    let (mut shell, mut persist) = open(&path, &Options::default());
    assert!(shell.library().entries[0].labels.is_empty());
    // Another process labels the image red; this library still shows no
    // label.
    let other = Vault::new(SqliteStore::open(&path).unwrap());
    other
        .apply(&[Command::add(id(1), LABELS, "red")], "agent", None, 1)
        .unwrap();
    assert!(shell.library().entries[0].labels.is_empty(), "stale view");
    // The user toggles blue: one label added, red kept.
    shell.library_mut().toggle_label("blue").unwrap();
    persist.frame(&mut shell, 2.0);
    assert_eq!(stored(&path, LABELS), Some(json!(["blue", "red"])));
    // And the library now shows both, read back from the vault.
    assert_eq!(shell.library().entries[0].labels, ["red", "blue"]);
    // Toggling blue again takes off blue only.
    shell.library_mut().toggle_label("blue").unwrap();
    persist.frame(&mut shell, 3.0);
    assert_eq!(stored(&path, LABELS), Some(json!(["red"])));
    drop((shell, persist, other));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn taking_a_reject_back_restores_the_stars() {
    let dir = scratch("reject");
    let path = vault_file(&dir);
    let (mut shell, mut persist) = open(&path, &Options::default());
    shell.library_mut().rate(4).unwrap();
    persist.frame(&mut shell, 1.0);
    shell.library_mut().toggle_reject().unwrap();
    persist.frame(&mut shell, 2.0);
    assert_eq!(stored(&path, REJECTED), Some(json!(true)));
    assert_eq!(stored(&path, RATING), Some(json!(4)), "the stars stay");
    shell.library_mut().toggle_reject().unwrap();
    persist.frame(&mut shell, 3.0);
    assert_eq!(stored(&path, REJECTED), None);
    assert_eq!(stored(&path, RATING), Some(json!(4)));
    let entry = shell.library().entries[0].clone();
    assert_eq!((entry.rating, entry.rejected), (4, false));
    // Reopened, the library shows what was saved.
    drop((shell, persist));
    let (shell, _persist) = open(&path, &Options::default());
    assert_eq!(shell.library().entries[0].rating, 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_write_puts_the_stored_value_back_and_says_why() {
    let dir = scratch("failed");
    let path = vault_file(&dir);
    // A short busy timeout, so the blocked write gives up quickly.
    let options = Options {
        busy_timeout: Duration::from_millis(20),
        ..Options::default()
    };
    let (mut shell, mut persist) = open(&path, &options);
    shell.library_mut().rate(2).unwrap();
    persist.frame(&mut shell, 1.0);
    assert_eq!(stored(&path, RATING), Some(json!(2)));
    // Another process holds the write lock: the next write fails.
    let mut blocker = SqliteStore::open(&path).unwrap();
    blocker.begin().unwrap();
    shell.library_mut().rate(5).unwrap();
    assert_eq!(shell.library().entries[0].rating, 5, "shown at once");
    persist.frame(&mut shell, 2.0);
    let notes = shell.notices();
    let last = notes.last().expect("a notice says why");
    assert!(last.text.contains("Not saved"), "{}", last.text);
    assert!(
        last.text.contains("locked") || last.text.contains("busy"),
        "{}",
        last.text
    );
    assert_eq!(
        shell.library().entries[0].rating,
        2,
        "the grid shows the stored value again"
    );
    blocker.rollback().unwrap();
    assert_eq!(stored(&path, RATING), Some(json!(2)));
    drop((shell, persist, blocker));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_change_by_another_writer_on_the_bus_shows_in_the_library() {
    let dir = scratch("events");
    let path = vault_file(&dir);
    let (mut shell, mut persist) = open(&path, &Options::default());
    // An agent in this process, writing through its own handle on the
    // same bus.
    let agent = Vault::new(SqliteStore::open(&path).unwrap()).with_bus(persist.bus().clone());
    agent
        .apply(
            &[
                Command::set(id(2), RATING, json!(3)),
                Command::add(id(2), LABELS, "green"),
            ],
            "agent",
            None,
            1,
        )
        .unwrap();
    persist.frame(&mut shell, 2.0);
    let entry = shell
        .library()
        .entries
        .iter()
        .find(|e| e.id == id(2).hex())
        .cloned()
        .unwrap();
    assert_eq!(entry.rating, 3);
    assert_eq!(entry.labels, ["green"]);
    drop((shell, persist, agent));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A source and an exposure node: the network the viewport shows.
fn network() -> (helios_core::Graph, helios_core::NodeId) {
    let mut g = helios_core::Graph::new();
    let src = g.add_source(
        "source",
        helios_core::Image::from_fn(helios_core::Region::sized(16, 8), 4, |_, _, _| 0.25),
    );
    let exp = g
        .add_module(
            "exposure",
            helios_core::ModuleRef::new(helios_module_exposure::Exposure),
            &helios_core::Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    (g, exp)
}

const EV: &str = "node/exposure/exposure";

fn set(shell: &Shell, ev: f64, gesture: Option<helios_values::GestureId>) {
    let mut set = helios_values::Set::new(
        EV,
        helios_values::Change::To(helios_core::Value::Float(ev)),
        helios_shell::commands::Origin::User,
    );
    if let Some(g) = gesture {
        set = set.gesture(g);
    }
    shell.values.lock().unwrap().set(set).unwrap();
}

fn shown(shell: &Shell) -> f64 {
    shell
        .values
        .lock()
        .unwrap()
        .get(EV)
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

fn saved(bus: &Bus) -> Vec<(u64, String)> {
    bus.since(0)
        .into_iter()
        .filter(|e| e.kind == "edit.saved")
        .map(|e| {
            (
                e.payload["step"].as_u64().unwrap(),
                e.payload["label"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn each_edit_step_is_a_version_and_reopening_loads_the_head() {
    use helios_shell::commands::Origin;
    let dir = scratch("edit");
    let path = vault_file(&dir);
    let mut shell = Shell::new(helios_panels(), None);
    let mut persist = Persist::new(SqliteStore::open(&path).unwrap(), Arc::new(Bus::new()));
    let (graph, exp) = network();
    assert!(
        persist
            .open_image(&mut shell, id(1), graph, exp, "output")
            .unwrap()
            .is_none()
    );
    // One write, one step.
    set(&shell, 1.0, None);
    persist.frame(&mut shell, 10.0);
    // A drag: saved while it goes, every FLUSH_SECONDS, then at its end.
    let drag = shell
        .values
        .lock()
        .unwrap()
        .begin_gesture(Origin::User, "drag exposure");
    set(&shell, 1.2, Some(drag));
    persist.frame(&mut shell, 10.2);
    set(&shell, 1.4, Some(drag));
    persist.frame(&mut shell, 10.5);
    assert_eq!(saved(persist.bus()).len(), 1, "not yet a second");
    persist.frame(&mut shell, 11.3);
    set(&shell, 1.6, Some(drag));
    shell.values.lock().unwrap().end(drag);
    persist.frame(&mut shell, 11.4);
    // Undo and redo are steps too.
    shell.values.lock().unwrap().undo(&Origin::User).unwrap();
    persist.frame(&mut shell, 12.0);
    assert!((shown(&shell) - 1.0).abs() < 1e-9);
    shell.values.lock().unwrap().redo(&Origin::User).unwrap();
    persist.frame(&mut shell, 13.0);
    // A frame with no step saves nothing.
    persist.frame(&mut shell, 14.0);
    assert_eq!(
        saved(persist.bus()),
        [
            (1, format!("set {EV}")),
            (2, "drag exposure (in progress)".into()),
            (3, "drag exposure".into()),
            (4, "undo drag exposure".into()),
            (5, "redo drag exposure".into()),
        ]
    );
    let vault = persist.vault().clone();
    let history: Vec<_> = vault
        .versions(&id(1))
        .unwrap()
        .into_iter()
        .map(|v| v.label.unwrap())
        .collect();
    assert_eq!(history.len(), 5);
    assert_eq!(vault.heads(&id(1)).unwrap().len(), 1, "one line of history");
    drop((shell, persist, vault));

    // Reopened: the viewport's network has the head's values.
    let mut shell = Shell::new(helios_panels(), None);
    let mut persist = Persist::new(SqliteStore::open(&path).unwrap(), Arc::new(Bus::new()));
    let (graph, exp) = network();
    let head = persist
        .open_image(&mut shell, id(1), graph, exp, "output")
        .unwrap()
        .unwrap();
    assert_eq!(head.label.as_deref(), Some("redo drag exposure"));
    assert!((shown(&shell) - 1.6).abs() < 1e-9, "{}", shown(&shell));
    // And the next step continues it.
    set(&shell, 0.5, None);
    persist.frame(&mut shell, 20.0);
    let next = persist.vault().heads(&id(1)).unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].parents, [head.id]);
    drop((shell, persist));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ctrl_z_undoes_a_drag_as_a_version_redo_replays_it_and_a_drag_going_on_stops_it() {
    use egui::{Key, Modifiers};
    use helios_shell::commands::Origin;
    let dir = scratch("undo");
    let path = vault_file(&dir);
    let mut shell = Shell::new(helios_panels(), None);
    let mut persist = Persist::new(SqliteStore::open(&path).unwrap(), Arc::new(Bus::new()));
    let (graph, exp) = network();
    persist
        .open_image(&mut shell, id(1), graph, exp, "output")
        .unwrap();
    let start = shown(&shell);
    let drag = shell
        .values
        .lock()
        .unwrap()
        .begin_gesture(Origin::User, "drag exposure");
    set(&shell, start + 1.5, Some(drag));
    shell.values.lock().unwrap().end(drag);
    persist.frame(&mut shell, 10.0);
    let mut h = helios_gui::headless::harness(shell, egui::vec2(1280.0, 800.0));
    h.run();
    // Edit > Undo names the step it would take back.
    let undo = h
        .state()
        .app()
        .handle()
        .command(&egui_pop::api::app::CMD_UNDO)
        .unwrap();
    assert_eq!(
        h.state().app().handle().command_title(&undo),
        "Undo drag exposure"
    );
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert!((shown(h.state()) - start).abs() < 1e-9, "the drag undone");
    persist.frame(h.state_mut(), 11.0);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    assert!((shown(h.state()) - (start + 1.5)).abs() < 1e-9, "redone");
    persist.frame(h.state_mut(), 12.0);
    assert_eq!(
        saved(persist.bus()),
        [
            (1, "drag exposure".into()),
            (2, "undo drag exposure".into()),
            (3, "redo drag exposure".into()),
        ]
    );
    // A drag going on: the undo is vetoed, the value stays, and the
    // journal says why.
    let before = h.state().app().handle().last_event();
    let drag = h
        .state()
        .values
        .lock()
        .unwrap()
        .begin_gesture(Origin::User, "drag exposure");
    set(h.state(), start + 2.0, Some(drag));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert!((shown(h.state()) - (start + 2.0)).abs() < 1e-9);
    let failed = h
        .state()
        .app()
        .handle()
        .events_since(before)
        .into_iter()
        .find(|e| e.name == "edit.undo_failed")
        .expect("the veto is on the journal");
    assert_eq!(failed.data["reason"], "drag exposure is in progress");
    h.state().values.lock().unwrap().end(drag);
    drop((h, persist));
    let _ = std::fs::remove_dir_all(&dir);
}
