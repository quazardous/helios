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

/// An exposure through a mask holding one circle, `spot`: the blend is
/// what the viewport shows.
fn masked_network() -> (helios_core::Graph, helios_core::NodeId) {
    use helios_core::{ModuleRef, Params, Value as V};
    use helios_module_masks::{MaskBlend, MaskCircle, MaskCombine};
    let (mut g, exp) = network();
    let src = g.find("source").unwrap();
    let mask = g
        .add_module("mask", ModuleRef::new(MaskCombine), &Params::new())
        .unwrap();
    let spot = g
        .add_module(
            "spot",
            ModuleRef::new(MaskCircle),
            &Params::new()
                .with("x", V::Float(8.0))
                .with("y", V::Float(4.0))
                .with("radius", V::Float(3.0)),
        )
        .unwrap();
    g.connect(spot, "mask", mask, "in0").unwrap();
    let blend = g
        .add_module("blend", ModuleRef::new(MaskBlend), &Params::new())
        .unwrap();
    g.connect(src, "image", blend, "input").unwrap();
    g.connect(exp, "output", blend, "processed").unwrap();
    g.connect(mask, "mask", blend, "mask").unwrap();
    (g, blend)
}

#[test]
fn mask_labels_and_locks_are_saved_with_the_edit() {
    use helios_gui::mask_tree;
    use helios_shell::commands::Origin;
    let dir = scratch("marks");
    let path = vault_file(&dir);
    let mut shell = Shell::new(helios_panels(), None);
    let mut persist = Persist::new(SqliteStore::open(&path).unwrap(), Arc::new(Bus::new()));
    let (graph, blend) = masked_network();
    persist
        .open_image(&mut shell, id(1), graph, blend, "output")
        .unwrap();
    // The spot named and locked: two steps, saved with the edit.
    {
        let v = shell.viewport();
        let mut store = shell.values.lock().unwrap();
        let net = mask_tree::network(v.graph().unwrap(), &store, None);
        mask_tree::rename(&mut store, &net, "spot", "Face", &Origin::User).unwrap();
        mask_tree::set_locked(&mut store, &net, "spot", Some(true), &Origin::User).unwrap();
    }
    persist.frame(&mut shell, 1.0);
    let heads = persist.vault().heads(&id(1)).unwrap();
    assert_eq!(
        heads[0].edit["marks"],
        json!({ "spot": { "label": "Face", "locked": true } })
    );
    drop((shell, persist));

    // Reopened: the spot has its label and its lock, not as undo steps.
    let mut shell = Shell::new(helios_panels(), None);
    let mut persist = Persist::new(SqliteStore::open(&path).unwrap(), Arc::new(Bus::new()));
    let (graph, blend) = masked_network();
    persist
        .open_image(&mut shell, id(1), graph, blend, "output")
        .unwrap();
    {
        let store = shell.values.lock().unwrap();
        assert_eq!(mask_tree::label_of(&store, "spot").as_deref(), Some("Face"));
        assert!(mask_tree::is_locked(&store, "spot"));
        assert!(store.undo_steps().is_empty());
    }
    // Another image opened in this shell does not take them.
    let (graph, blend) = masked_network();
    persist
        .open_image(&mut shell, id(2), graph, blend, "output")
        .unwrap();
    {
        let store = shell.values.lock().unwrap();
        assert_eq!(mask_tree::label_of(&store, "spot"), None);
        assert!(!mask_tree::is_locked(&store, "spot"));
    }
    drop((shell, persist));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_library_read_on_another_thread_shows_as_one_filled_in_place() {
    let dir = scratch("read-elsewhere");
    let path = vault_file(&dir);
    let (filled, _) = open(&path, &Options::default());
    let mut shell = Shell::new(helios_panels(), None);
    let persist = Persist::new(SqliteStore::open(&path).unwrap(), Arc::new(Bus::new()));
    let vault = persist.vault().clone();
    // Read as the desktop reads it while its window opens.
    let rows = std::thread::spawn(move || helios_gui::persist::read_library(&vault))
        .join()
        .unwrap()
        .unwrap();
    assert_eq!(rows.len(), 2);
    persist.show_library(&mut shell, rows).unwrap();
    assert_eq!(shell.library().entries, filled.library().entries);
    drop((filled, shell, persist));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Opens image `n` of the vault through the exposure network.
fn open_exposure(shell: &mut Shell, persist: &mut Persist<SqliteStore>, n: u8) {
    let (graph, exp) = network();
    persist
        .open_image(shell, id(n), graph, exp, "output")
        .unwrap();
}

#[test]
fn opening_another_image_leaves_the_undo_steps_of_the_first_behind() {
    use helios_shell::commands::Origin;
    let dir = scratch("switch");
    let path = vault_file(&dir);
    let mut shell = Shell::new(helios_panels(), None);
    let mut persist = Persist::new(SqliteStore::open(&path).unwrap(), Arc::new(Bus::new()));
    open_exposure(&mut shell, &mut persist, 1);
    let start = shown(&shell);
    set(&shell, start + 1.0, None);
    persist.frame(&mut shell, 10.0);
    set(&shell, start + 2.0, None);
    persist.frame(&mut shell, 11.0);
    // The second image goes through the same network: the same paths.
    open_exposure(&mut shell, &mut persist, 2);
    assert!((shown(&shell) - start).abs() < 1e-9);
    {
        let mut store = shell.values.lock().unwrap();
        assert!(store.undo_steps().is_empty(), "the first image's steps");
        assert_eq!(store.undo(&Origin::User).unwrap(), None, "nothing to undo");
    }
    assert!(
        (shown(&shell) - start).abs() < 1e-9,
        "the first image's value written into the second: {}",
        shown(&shell)
    );
    persist.frame(&mut shell, 12.0);
    assert!(persist.vault().versions(&id(2)).unwrap().is_empty());
    drop((shell, persist));
    let _ = std::fs::remove_dir_all(&dir);
}

fn set_by(shell: &Shell, ev: f64, origin: helios_shell::commands::Origin) {
    shell
        .values
        .lock()
        .unwrap()
        .set(helios_values::Set::new(
            EV,
            helios_values::Change::To(helios_core::Value::Float(ev)),
            origin,
        ))
        .unwrap();
}

#[test]
fn each_version_names_who_made_its_step() {
    use helios_shell::commands::Origin;
    let dir = scratch("origin");
    let path = vault_file(&dir);
    let mut shell = Shell::new(helios_panels(), None);
    let mut persist = Persist::new(SqliteStore::open(&path).unwrap(), Arc::new(Bus::new()));
    open_exposure(&mut shell, &mut persist, 1);
    let start = shown(&shell);
    // An agent's write, then the user's undo of it.
    set_by(&shell, start + 1.0, Origin::Agent("retouch".into()));
    persist.frame(&mut shell, 10.0);
    shell.values.lock().unwrap().undo(&Origin::User).unwrap();
    persist.frame(&mut shell, 11.0);
    // A script's drag, saved while it goes and at its end.
    let drag = shell
        .values
        .lock()
        .unwrap()
        .begin_gesture(Origin::Script("ramp".into()), "drag exposure");
    set(&shell, start + 0.5, Some(drag));
    persist.frame(&mut shell, 12.0);
    persist.frame(&mut shell, 13.5);
    shell.values.lock().unwrap().end(drag);
    persist.frame(&mut shell, 14.0);
    let vault = persist.vault().clone();
    let head = vault.heads(&id(1)).unwrap().remove(0);
    let history: Vec<(String, String)> = vault
        .history(&head.id)
        .unwrap()
        .into_iter()
        .map(|v| (v.label.unwrap_or_default(), v.origin))
        .collect();
    assert_eq!(
        history,
        [
            (format!("set {EV}"), "agent:retouch".to_string()),
            (format!("undo set {EV}"), "user".to_string()),
            ("drag exposure (in progress)".into(), "script:ramp".into()),
            ("drag exposure".into(), "script:ramp".into()),
        ]
    );
    // And the bus says the same.
    let origins: Vec<String> = persist
        .bus()
        .since(0)
        .into_iter()
        .filter(|e| e.kind == "edit.saved")
        .map(|e| e.payload["origin"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        origins,
        ["agent:retouch", "user", "script:ramp", "script:ramp"]
    );
    drop((shell, persist, vault));
    let _ = std::fs::remove_dir_all(&dir);
}
