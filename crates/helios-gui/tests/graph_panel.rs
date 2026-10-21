//! The Graph panel: the workspace's network in egui-pop's node graph
//! editor. It shows the network; moving a node and connecting two ports
//! are each one undo step that changes the workspace; the selection is
//! shared with the other panels both ways; a node's header carries the
//! engine's status; the overview stacks the nodes under their principal
//! node.

use std::path::{Path, PathBuf};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use helios_core::{Graph, Image, ModuleRef, Params, Region};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_template::workspace::Files;
use serde_json::{Value, json};

const GRAPH: &str = helios_gui::graph::GRAPH;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-graph-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A window on a workspace not made from a template (the blank network:
/// a source, an exposure and a mask into a blend), the Graph panel in
/// front.
fn window(root: &Path) -> (Harness<'static, Shell>, Files) {
    let config = root.join("config");
    let dir = root.join("photos");
    std::fs::create_dir_all(&dir).unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&dir)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &dir).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    run(&mut h, "helios.graph.focus", json!(null));
    (h, Files::new(&config, &dir))
}

/// Runs a command, then the frames its work takes: the editor's request,
/// its step, the workspace's step.
fn run(h: &mut Harness<'static, Shell>, id: &str, args: Value) -> Value {
    let out = h.state_mut().run(id, args, Origin::User).unwrap();
    frames(h);
    out
}

fn frames(h: &mut Harness<'static, Shell>) {
    for _ in 0..4 {
        h.run();
    }
}

fn read(h: &mut Harness<'static, Shell>) -> Value {
    h.state_mut()
        .run("graph.read", json!({ "graph": GRAPH }), Origin::User)
        .unwrap()
}

fn node<'a>(graph: &'a Value, id: &str) -> &'a Value {
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id)
        .unwrap_or_else(|| panic!("no node `{id}` in {graph}"))
}

fn steps(h: &Harness<'static, Shell>) -> usize {
    h.state().values.lock().unwrap().undo_steps().len()
}

fn undo(h: &mut Harness<'static, Shell>) {
    h.state()
        .values
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    frames(h);
}

fn selection(h: &Harness<'static, Shell>) -> Value {
    let store = h.state().values.lock().unwrap();
    match &store.get(helios_gui::params::SELECTION).unwrap().value {
        helios_core::Value::Text(t) => json!(t),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_panel_shows_the_workspace_network() {
    let root = temp("shows");
    let (mut h, _) = window(&root);
    let g = read(&mut h);
    for id in ["source", "exposure", "mask", "blend"] {
        node(&g, id);
    }
    assert_eq!(node(&g, "exposure")["kind"], "exposure");
    assert_eq!(node(&g, "exposure")["inputs"][0]["id"], "input");
    let wire = json!({
        "from": { "node": "source", "port": "image" },
        "to": { "node": "exposure", "port": "input" },
    });
    assert!(g["edges"].as_array().unwrap().contains(&wire), "{g}");
    // The nodes are named for screen readers and tests.
    assert!(h.query_by_label("exposure").is_some());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn moving_a_node_is_one_undo_step_kept_with_the_workspace() {
    let root = temp("move");
    let (mut h, files) = window(&root);
    let before = steps(&h);
    run(
        &mut h,
        "graph.edit",
        json!({
            "graph": GRAPH,
            "edit": "move",
            "moves": [{ "node": "exposure", "from": [260.0, 0.0], "to": [400.0, 300.0] }],
        }),
    );
    assert_eq!(steps(&h), before + 1, "one undo step");
    let state = h.state().workspace_state().unwrap();
    assert_eq!(state.bundle.template.positions["exposure"], [400.0, 300.0]);
    // Written at once to the workspace's files.
    assert_eq!(
        files.load().unwrap().bundle.template.positions["exposure"],
        [400.0, 300.0]
    );
    assert_eq!(
        node(&read(&mut h), "exposure")["pos"],
        json!([400.0, 300.0])
    );
    undo(&mut h);
    let state = h.state().workspace_state().unwrap();
    assert!(!state.bundle.template.positions.contains_key("exposure"));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn connecting_two_ports_is_one_undo_step_that_rewires_the_network() {
    let root = temp("connect");
    let (mut h, files) = window(&root);
    let before = steps(&h);
    // The source straight into the blend's processed input, in place of
    // the exposure's wire.
    run(
        &mut h,
        "graph.edit",
        json!({
            "graph": GRAPH,
            "edit": "connect",
            "from": { "node": "source", "port": "image" },
            "to": { "node": "blend", "port": "processed" },
        }),
    );
    assert_eq!(steps(&h), before + 1, "one undo step");
    let into = |h: &Harness<'static, Shell>| -> Vec<String> {
        h.state()
            .workspace_state()
            .unwrap()
            .bundle
            .template
            .network
            .connections
            .iter()
            .filter(|c| c.to == "blend" && c.input == "processed")
            .map(|c| c.from.clone())
            .collect()
    };
    assert_eq!(into(&h), ["source"]);
    assert!(
        files
            .load()
            .unwrap()
            .bundle
            .template
            .network
            .connections
            .iter()
            .any(|c| c.from == "source" && c.to == "blend")
    );
    undo(&mut h);
    assert_eq!(into(&h), ["exposure"]);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_selection_is_shared_with_the_other_panels_both_ways() {
    let root = temp("selection");
    let (mut h, _) = window(&root);
    // Selected in the graph: the selected module everywhere.
    run(
        &mut h,
        "graph.select",
        json!({ "graph": GRAPH, "nodes": ["exposure"] }),
    );
    assert_eq!(selection(&h), json!("exposure"));
    // Selected elsewhere (the Parameters or Masks panel, an agent): the
    // graph follows.
    run(
        &mut h,
        "param.set",
        json!({ "path": helios_gui::params::SELECTION, "value": "mask" }),
    );
    assert_eq!(read(&mut h)["selection"], json!(["mask"]));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_node_header_carries_the_engine_status() {
    let root = temp("status");
    let (mut h, _) = window(&root);
    // The image shown, through the workspace's first nodes.
    let (w, ht) = (64, 48);
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(
            Region::sized(w, ht),
            4,
            |_, _, c| {
                if c == 3 { 1.0 } else { 0.25 }
            },
        ),
    );
    let exp = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    h.state_mut().set_view_source(g, exp, "output").unwrap();
    run(
        &mut h,
        "graph.view",
        json!({ "graph": GRAPH, "level": "detail" }),
    );
    {
        // Rendered whole: the header says it is done, on the CPU.
        let shell = h.state();
        let v = shell.viewport();
        let graph = v.graph().unwrap();
        let id = graph.find("exposure").unwrap();
        v.engine()
            .render(graph, id, "output", Region::sized(w, ht))
            .unwrap();
    }
    frames(&mut h);
    assert!(
        !h.query_all_by_label("done · CPU")
            .collect::<Vec<_>>()
            .is_empty(),
        "no header says done"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_overview_stacks_nodes_under_their_principal_node() {
    let root = temp("stacks");
    let (mut h, _) = window(&root);
    run(
        &mut h,
        "graph.view",
        json!({ "graph": GRAPH, "level": "overview" }),
    );
    let g = read(&mut h);
    assert_eq!(g["level"], "overview");
    // The exposure and the mask lead to the blend, a merge and what the
    // viewport shows: stacked under it; the source and the blend stay.
    assert_eq!(node(&g, "exposure")["group"], "stack:blend");
    assert_eq!(node(&g, "mask")["group"], "stack:blend");
    assert_eq!(node(&g, "blend")["group"], Value::Null);
    assert_eq!(node(&g, "source")["group"], Value::Null);
    assert!(
        g["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == "stack:blend" && x["label"] == "blend"),
        "{g}"
    );
    // The stack is one button, which unfolds it.
    assert!(h.query_by_label_contains("Unfold").is_some());
    std::fs::remove_dir_all(&root).unwrap();
}
