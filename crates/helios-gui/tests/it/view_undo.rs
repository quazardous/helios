//! Ctrl+Z follows the focused view (#5258 R3, `docs/design/undo.md`,
//! "Ctrl+Z follows the focused view"): the image's views undo the photo's
//! edit (`edit`), the graph undoes the workspace's (`workspace`), whichever
//! step is newer. Inside a line, an open step takes every change made to it
//! until it is sealed, and a gesture across several modules is one step.
//! Through the real keys (Ctrl+Z, Ctrl+Shift+Z) and the real focus
//! (`<panel>.focus`, what a click in a panel does).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use helios_core::{Bus, Image, Region, Value};
use helios_gui::persist::Persist;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use helios_template::Source;
use helios_values::HistoryScope;
use helios_vault::ImageId;
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value as Json, json};

pub(crate) const EV: &str = "node/exposure/exposure";

pub(crate) fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-view-undo-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A window on a blank workspace, its photo open.
pub(crate) fn window(root: &Path) -> (Harness<'static, Shell>, Persist<SqliteStore>) {
    let config = root.join("config");
    let dir = root.join("photos");
    std::fs::create_dir_all(&dir).unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&dir)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &dir).unwrap();
    crate::host::listen_network(&shell);
    let mut persist = Persist::new(
        SqliteStore::open(&root.join("vault.db")).unwrap(),
        Arc::new(Bus::new()),
    );
    let file = dir.join("a.png");
    image::RgbImage::from_fn(12, 8, |x, y| {
        image::Rgb([(x * 20) as u8, (y * 30) as u8, 90])
    })
    .save(&file)
    .unwrap();
    let photo = persist.vault().import_file(&file, 1).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    open(&mut h, &mut persist, photo);
    (h, persist)
}

fn open(h: &mut Harness<'static, Shell>, persist: &mut Persist<SqliteStore>, photo: ImageId) {
    let picture = Image::from_fn(
        Region::sized(16, 12),
        4,
        |_, _, c| {
            if c == 3 { 1.0 } else { 0.3 }
        },
    );
    let built = h
        .state()
        .workspace_bundle()
        .unwrap()
        .build_with(&templates::modules(), Source::picture(&picture), None)
        .unwrap();
    persist
        .open_image(
            h.state_mut(),
            photo,
            built.graph,
            built.view.0,
            &built.view.1,
        )
        .unwrap();
    persist.settle(h.state_mut());
    follow(h);
}

/// The frames a step takes, then the image kept open through a network
/// that changed (what the host does): [`crate::host::follow`], whose runs
/// wait for the panels' work on their threads (more frames on a slower
/// machine) within a deadline.
pub(crate) fn follow(h: &mut Harness<'static, Shell>) {
    crate::host::follow(h);
}

pub(crate) fn run(h: &mut Harness<'static, Shell>, id: &str, args: Json) {
    h.state_mut().run(id, args, Origin::User).unwrap();
    follow(h);
}

/// Focuses `panel` (`<panel>.focus`, what a click in it does).
pub(crate) fn focus(h: &mut Harness<'static, Shell>, panel: &str) {
    run(h, &format!("{panel}.focus"), json!(null));
}

pub(crate) fn ctrl_z(h: &mut Harness<'static, Shell>) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    follow(h);
}

fn ctrl_shift_z(h: &mut Harness<'static, Shell>) {
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    follow(h);
}

pub(crate) fn set(
    h: &Harness<'static, Shell>,
    path: &str,
    v: f64,
    gesture: Option<helios_values::GestureId>,
) {
    let mut set = helios_values::Set::new(
        path,
        helios_values::Change::To(Value::Float(v)),
        Origin::User,
    );
    if let Some(g) = gesture {
        set = set.gesture(g);
    }
    h.state().values().lock().unwrap().set(set).unwrap();
}

pub(crate) fn value(h: &Harness<'static, Shell>, path: &str) -> Option<f64> {
    h.state()
        .values()
        .lock()
        .unwrap()
        .get(path)
        .and_then(|e| e.value.as_f64())
}

pub(crate) fn steps(h: &Harness<'static, Shell>, scope: HistoryScope) -> usize {
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo_steps_of(scope)
        .len()
}

/// Adds a node to the workspace's network: a step of its line.
pub(crate) fn add_node(h: &mut Harness<'static, Shell>) {
    run(
        h,
        "network.add",
        json!({ "nodes": [{ "module": "sigmoid" }] }),
    );
    assert!(in_network(h, "sigmoid"), "the node added");
}

pub(crate) fn in_network(h: &Harness<'static, Shell>, node: &str) -> bool {
    h.state()
        .viewport()
        .graph()
        .and_then(|g| g.find(node))
        .is_some()
}

/// A float value of a module other than the exposure, and its value.
fn other_module(h: &Harness<'static, Shell>) -> (String, f64) {
    let values = h.state().values().lock().unwrap();
    values
        .list("node/")
        .into_iter()
        .find_map(|(p, e)| {
            let module = p.strip_prefix("node/")?.split('/').next()?;
            (module != "exposure")
                .then(|| e.value.as_f64().map(|v| (p.to_string(), v)))
                .flatten()
        })
        .expect("a second module with a float value")
}

#[test]
fn ctrl_z_in_the_image_undoes_the_edit_and_not_a_later_workspace_change() {
    let root = temp("edit");
    let (mut h, _persist) = window(&root);
    let ev0 = value(&h, EV);
    set(&h, EV, 1.0, None);
    follow(&mut h);
    // The workspace's step is the newer one.
    add_node(&mut h);
    focus(&mut h, "helios.viewport");
    ctrl_z(&mut h);
    assert_eq!(value(&h, EV), ev0, "the exposure undone");
    assert!(in_network(&h, "sigmoid"), "the newer graph edit left alone");
    // Redo goes to the same line.
    ctrl_shift_z(&mut h);
    assert_eq!(value(&h, EV), Some(1.0), "the exposure redone");
    assert!(in_network(&h, "sigmoid"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ctrl_z_in_the_graph_undoes_the_workspace_change_and_leaves_the_edit() {
    let root = temp("workspace");
    let (mut h, _persist) = window(&root);
    add_node(&mut h);
    // The photo's step is the newer one.
    set(&h, EV, 1.0, None);
    follow(&mut h);
    focus(&mut h, "helios.graph");
    ctrl_z(&mut h);
    assert!(!in_network(&h, "sigmoid"), "the graph edit undone");
    assert_eq!(value(&h, EV), Some(1.0), "the newer edit left alone");
    assert_eq!(steps(&h, HistoryScope::Image), 1, "still undoable");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn two_changes_to_one_module_in_the_open_step_are_one_step() {
    let root = temp("amend");
    let (mut h, _persist) = window(&root);
    let ev0 = value(&h, EV);
    let before = steps(&h, HistoryScope::Image);
    let g = h
        .state()
        .values()
        .lock()
        .unwrap()
        .begin_gesture(Origin::User, "drag exposure");
    set(&h, EV, 0.5, Some(g));
    set(&h, EV, 1.0, Some(g));
    // Sealed when the gesture ends.
    h.state().values().lock().unwrap().end(g);
    follow(&mut h);
    assert_eq!(steps(&h, HistoryScope::Image), before + 1, "one step");
    focus(&mut h, "helios.viewport");
    ctrl_z(&mut h);
    assert_eq!(value(&h, EV), ev0, "both changes undone at once");
    assert_eq!(steps(&h, HistoryScope::Image), before);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_sealed_gesture_across_modules_undoes_as_one_step() {
    let root = temp("sealed");
    let (mut h, _persist) = window(&root);
    let ev0 = value(&h, EV);
    let (other, other0) = other_module(&h);
    let before = steps(&h, HistoryScope::Image);
    // A preset: two modules in one group.
    let g = h
        .state()
        .values()
        .lock()
        .unwrap()
        .begin_group(Origin::User, "preset");
    set(&h, EV, 1.0, Some(g));
    set(&h, &other, other0 + 0.25, Some(g));
    h.state().values().lock().unwrap().end(g);
    follow(&mut h);
    assert_eq!(
        steps(&h, HistoryScope::Image),
        before + 1,
        "one sealed step"
    );
    // A workspace change after it: the image's focus still reaches it.
    add_node(&mut h);
    focus(&mut h, "helios.viewport");
    ctrl_z(&mut h);
    assert_eq!(value(&h, EV), ev0, "the exposure undone");
    assert_eq!(value(&h, &other), Some(other0), "the other module too");
    assert!(in_network(&h, "sigmoid"), "the workspace's step stays");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn writes_to_one_module_amend_the_open_step_until_commit() {
    // #5258 R7: darktable's model, separate writes to the same module
    // amend the open step; Commit seals it.
    let root = temp("amend-commit");
    let (mut h, _persist) = window(&root);
    let ev0 = value(&h, EV);
    let (other, other0) = other_module(&h);
    let before = steps(&h, HistoryScope::Image);
    set(&h, EV, 0.5, None);
    set(&h, EV, 1.0, None);
    follow(&mut h);
    assert_eq!(steps(&h, HistoryScope::Image), before + 1, "amended");
    // Commit seals it: the next exposure is a step of its own.
    let sealed = h
        .state_mut()
        .run("photo.commit", json!(null), Origin::User)
        .unwrap();
    assert_eq!(sealed["sealed"], true);
    set(&h, EV, 1.5, None);
    // Another module seals in turn.
    set(&h, &other, other0 + 0.25, None);
    set(&h, EV, 2.0, None);
    follow(&mut h);
    assert_eq!(steps(&h, HistoryScope::Image), before + 4);
    focus(&mut h, "helios.viewport");
    for _ in 0..3 {
        ctrl_z(&mut h);
    }
    assert_eq!(value(&h, EV), Some(1.0), "back to the committed step");
    ctrl_z(&mut h);
    assert_eq!(value(&h, EV), ev0, "both amended writes undone at once");
    let _ = std::fs::remove_dir_all(&root);
}
