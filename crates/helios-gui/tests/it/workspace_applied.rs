//! Applying a workspace's state (an edit, an undo, a redo, the
//! workspace opened) is said once on the bus: `workspace.applied`, with
//! what changed and the listeners that followed it, in their declared
//! order, in the same call that applied it. A listener runs only for the
//! parts it reads: a change of the name runs none of the network's.

use std::path::{Path, PathBuf};

use egui::vec2;
use egui_kittest::Harness;
use egui_pop::model::ids::CommandId;
use egui_pop::model::keymap::Layer;
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

const APPLIED: &str = "helios.workspace.applied";
const CIRCLE: CommandId = CommandId::from_static("mask.add.circle");

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-wsapplied-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A window on a workspace made from the portrait template (it has
/// workshops and keys of its own).
fn window(root: &Path) -> Harness<'static, Shell> {
    let config = root.join("config");
    let shell = Shell::new(helios_panels(), None);
    let catalogue = templates::catalogue(shell.panels(), shell.specs());
    let created = templates::create_workspace(
        &templates::find("helios.portrait-retouch").unwrap(),
        &catalogue,
        &root.join("work"),
        &config,
    )
    .unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&created.workspace)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &created.workspace).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

/// The `workspace.applied` events said since `since`.
fn applied_since(h: &Harness<'static, Shell>, since: u64) -> Vec<Value> {
    h.state()
        .app()
        .handle()
        .events_since(since)
        .into_iter()
        .filter(|e| e.name == APPLIED)
        .map(|e| e.data)
        .collect()
}

fn strings(v: &Value) -> Vec<&str> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn steps(h: &Harness<'static, Shell>) -> usize {
    h.state().values().lock().unwrap().all_undo_steps().len()
}

/// An undo of a change of the workspace's keys: applied once, its
/// tools only; the keys' listener follows it (no new undo step taken
/// for the keys put back), the network's listeners do not run.
#[test]
fn an_undo_of_a_tools_change_is_applied_once() {
    let root = temp("tools");
    let mut h = window(&root);
    let handle = h.state().app().handle().clone();
    handle.rebind_in(Layer::Workspace, CIRCLE, Some("B".into()));
    h.run();
    let before = steps(&h);
    let since = handle.last_event();
    h.state()
        .values()
        .lock()
        .unwrap()
        .in_history(helios_values::HistoryScope::Workspace, |s| {
            s.undo(&Origin::User)
        })
        .unwrap();
    h.run();
    let said = applied_since(&h, since);
    assert_eq!(said.len(), 1, "said once: {said:?}");
    assert_eq!(strings(&said[0]["changed"]), ["tools"], "{said:?}");
    assert_eq!(
        strings(&said[0]["ran"]),
        ["helios.workspace.keys"],
        "{said:?}"
    );
    assert_eq!(steps(&h), before - 1, "the undo only, no step for the keys");
    drop(h);
    std::fs::remove_dir_all(&root).unwrap();
}

/// A change of the name alone runs no listener of the network, the
/// keys or the places; it is said in the call that made it, before any
/// frame.
#[test]
fn a_name_change_runs_none_of_the_networks_listeners() {
    let root = temp("name");
    let mut h = window(&root);
    let since = h.state().app().handle().last_event();
    h.state_mut()
        .edit_workspace(&json!({ "name": "Renamed" }), &Origin::User)
        .unwrap();
    let said = applied_since(&h, since);
    assert_eq!(said.len(), 1, "said in the same call: {said:?}");
    assert_eq!(strings(&said[0]["changed"]), ["name"], "{said:?}");
    assert!(strings(&said[0]["ran"]).is_empty(), "{said:?}");
    h.run();
    assert_eq!(applied_since(&h, since).len(), 1, "and once only");
    drop(h);
    std::fs::remove_dir_all(&root).unwrap();
}

/// A network change (a node moved in the graph view, then moved back
/// by an undo) reaches the graph, then the Workshops panel, each once;
/// the keys' and the places' listeners do not run.
#[test]
fn a_network_change_runs_the_graph_then_the_workshops() {
    let root = temp("network");
    let mut h = window(&root);
    let state = h.state().workspace_state().unwrap();
    let node = state.bundle.template.network.nodes[0].name.clone();
    let since = h.state().app().handle().last_event();
    h.state_mut()
        .run(
            "network.move",
            json!({ "moves": [{ "node": node, "to": [512.0, 384.0] }] }),
            Origin::User,
        )
        .unwrap();
    h.run();
    let said = applied_since(&h, since);
    assert_eq!(said.len(), 1, "{said:?}");
    assert_network(&said[0]);
    let since = h.state().app().handle().last_event();
    h.state()
        .values()
        .lock()
        .unwrap()
        .in_history(helios_values::HistoryScope::Workspace, |s| {
            s.undo(&Origin::User)
        })
        .unwrap();
    h.run();
    let said = applied_since(&h, since);
    assert_eq!(said.len(), 1, "the undo applied once: {said:?}");
    assert_network(&said[0]);
    drop(h);
    std::fs::remove_dir_all(&root).unwrap();
}

/// A node added changes what an image goes through: the library's
/// renders, then the host's image (`darkroom.network`), each once per
/// change, the undo included; a node moved (above) runs neither.
#[test]
fn a_nodes_change_reaches_the_renders_then_the_host_once() {
    let root = temp("nodes");
    let mut h = window(&root);
    crate::host::listen_network(h.state());
    let since = h.state().app().handle().last_event();
    h.state_mut()
        .run(
            "network.add",
            json!({ "nodes": [{ "module": "sigmoid" }] }),
            Origin::User,
        )
        .unwrap();
    h.run();
    let said = applied_since(&h, since);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(strings(&said[0]["changed"]).contains(&"nodes"), "{said:?}");
    let ran = strings(&said[0]["ran"]);
    assert_eq!(
        ran[ran.len() - 2..],
        ["helios.library.heads", "helios.darkroom.network"],
        "{said:?}"
    );
    assert_eq!(crate::host::network_changed(), 1, "once");
    h.state()
        .values()
        .lock()
        .unwrap()
        .in_history(helios_values::HistoryScope::Workspace, |s| {
            s.undo(&Origin::User)
        })
        .unwrap();
    h.run();
    assert_eq!(crate::host::network_changed(), 1, "the undo's, once");
    drop(h);
    std::fs::remove_dir_all(&root).unwrap();
}

fn assert_network(said: &Value) {
    assert!(strings(&said["changed"]).contains(&"network"), "{said}");
    assert_eq!(
        strings(&said["ran"]),
        ["helios.graph.template", "helios.workshops.composites"],
        "{said}"
    );
}

/// The listeners of `workspace.applied` as the desktop registers them,
/// in the order they run: a reviewed dump (`tests/bus/workspace.txt`).
/// The keys first, then the places (the list before the scope that
/// reads it), then the graph and the Workshops panel, then the library's
/// renders (the host's `darkroom.network` comes after them, registered
/// by the host).
#[test]
fn the_workspace_points_listeners_run_in_a_declared_order() {
    let root = temp("order");
    let h = window(&root);
    assert_eq!(
        helios_gui::workspace_applied::order(&*h.state().app().handle().bus()),
        include_str!("../bus/workspace.txt")
    );
    drop(h);
    std::fs::remove_dir_all(&root).unwrap();
}

/// The listeners of egui-pop's `layout.changed` as the desktop registers
/// them: egui-pop's save of the layout's document, then the workspace's
/// copy of it (`tests/bus/layout.txt`).
#[test]
fn the_layout_point_saves_then_copies() {
    let root = temp("layout-order");
    let h = window(&root);
    assert_eq!(
        helios_gui::workspace_applied::layout_order(&*h.state().app().handle().bus()),
        include_str!("../bus/layout.txt")
    );
    drop(h);
    std::fs::remove_dir_all(&root).unwrap();
}

/// A change of the layout is saved once and copied once, after its
/// command, and nothing more while nothing changes.
#[test]
fn a_layout_change_is_saved_once_and_its_copy_follows() {
    let root = temp("layout-once");
    let config = root.join("config");
    let shell = Shell::new(helios_panels(), None);
    let catalogue = templates::catalogue(shell.panels(), shell.specs());
    let created = templates::create_workspace(
        &templates::find("helios.portrait-retouch").unwrap(),
        &catalogue,
        &root.join("work"),
        &config,
    )
    .unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&created.workspace)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &created.workspace).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let saves = |h: &Harness<'static, Shell>| {
        h.state()
            .app()
            .handle()
            .step_traces()
            .iter()
            .filter(|t| t.step == egui_pop::api::app::STEP_LAYOUT_SAVE)
            .count()
    };
    // An edited workspace keeps its state, the copy of the layout with it.
    h.state_mut()
        .edit_workspace(&json!({ "name": "Mine" }), &Origin::User)
        .unwrap();
    h.run();
    let before = saves(&h);
    let shown = h.state().layout();
    let panel = shown
        .placed()
        .into_iter()
        .find(|p| {
            // One of Helios's panels: the workspace's copy keeps only
            // those (egui-pop's own belong to the application).
            p.as_str().starts_with("helios.")
                && p.as_str() != "helios.viewport"
                && shown.area_of(p).is_some()
        })
        .cloned()
        .expect("a docked panel");
    // Moved to the other side (a dock: `workbench.move`).
    let to = match shown.area_of(&panel) {
        Some(egui_pop::model::panels::Area::StartSide) => egui_pop::model::panels::Area::EndSide,
        _ => egui_pop::model::panels::Area::StartSide,
    };
    let out = h.state().app().handle().move_panel(&panel, to).unwrap();
    assert!(out.ok(), "{out:?}");
    for _ in 0..3 {
        h.run();
    }
    assert_eq!(h.state().layout().area_of(&panel), Some(to), "moved");
    assert_eq!(saves(&h) - before, 1, "saved once");
    let files = helios_template::workspace::Files::new(&config, &created.workspace);
    let copy = files.load_on_disk().unwrap().bundle.layout;
    assert_eq!(
        copy.area_of(&panel),
        Some(to),
        "the copy follows: {panel} in {copy:?}"
    );
    std::thread::sleep(std::time::Duration::from_millis(1200));
    for _ in 0..3 {
        h.run();
    }
    assert_eq!(saves(&h) - before, 1, "nothing changed since");
    drop(h);
    std::fs::remove_dir_all(&root).unwrap();
}
