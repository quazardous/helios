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

const APPLIED: &str = "workspace.applied";
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
    h.state().values().lock().unwrap().undo_steps().len()
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
        .undo(&Origin::User)
        .unwrap();
    h.run();
    let said = applied_since(&h, since);
    assert_eq!(said.len(), 1, "said once: {said:?}");
    assert_eq!(strings(&said[0]["changed"]), ["tools"], "{said:?}");
    assert_eq!(strings(&said[0]["ran"]), ["workspace.keys"], "{said:?}");
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
        .undo(&Origin::User)
        .unwrap();
    h.run();
    let said = applied_since(&h, since);
    assert_eq!(said.len(), 1, "the undo applied once: {said:?}");
    assert_network(&said[0]);
    drop(h);
    std::fs::remove_dir_all(&root).unwrap();
}

fn assert_network(said: &Value) {
    assert!(strings(&said["changed"]).contains(&"network"), "{said}");
    assert_eq!(
        strings(&said["ran"]),
        ["graph.template", "workshops.composites"],
        "{said}"
    );
}

/// The listeners of `workspace.applied` as the desktop registers them,
/// in the order they run: a reviewed dump (`tests/bus/workspace.txt`).
/// The keys first, then the places (the list before the scope that
/// reads it), then the graph and the Workshops panel.
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
