//! The Graph panel: the workspace's network in egui-pop's node graph
//! editor. It shows the network; moving a node and connecting two ports
//! are each one undo step that changes the workspace; the selection is
//! shared with the other panels both ways, and selecting a node brings
//! up the Parameters panel; a node's header carries the
//! engine's status; the overview stacks the nodes under their principal
//! node; the 58-node masks stress network stays fluid.

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
fn selecting_a_node_brings_up_the_parameters_panel() {
    use egui_pop::api::workbench::Change;
    use egui_pop::model::ids::PanelId;
    let root = temp("reveal");
    let (mut h, _) = window(&root);
    let params = PanelId::from_static(helios_gui::params::PANEL);
    let graph = PanelId::from_static(helios_gui::graph::PANEL);
    h.state()
        .app()
        .handle()
        .change_workbench(Change::Hide(params.clone()));
    frames(&mut h);
    assert!(!h.state().layout().shows(&params), "closed first");
    // Selected in the graph: the panel is back, the graph still shown.
    run(
        &mut h,
        "graph.select",
        json!({ "graph": GRAPH, "nodes": ["exposure"] }),
    );
    let layout = h.state().layout();
    assert!(layout.shows(&params), "the Parameters panel opened");
    assert!(layout.shows(&graph));
    assert_eq!(selection(&h), json!("exposure"));
    // An empty selection leaves the panels as they are.
    h.state()
        .app()
        .handle()
        .change_workbench(Change::Hide(params.clone()));
    frames(&mut h);
    run(
        &mut h,
        "graph.select",
        json!({ "graph": GRAPH, "nodes": [] }),
    );
    assert!(!h.state().layout().shows(&params));
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
    // Done, or served from the cache when the viewport's tiles found
    // what the render above computed.
    let says =
        |h: &Harness<'static, Shell>, text: &str| h.query_all_by_label(text).next().is_some();
    assert!(
        says(&h, "done · CPU") || says(&h, "cached · CPU"),
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

/// Copied nodes paste into the network renamed on a clash, wired from
/// the node their input came from, as one undo step.
#[test]
fn copied_nodes_paste_renamed_and_wired_as_one_undo_step() {
    use egui_pop::api::clipboard::{self, CopyRequest, PasteRequest};
    use egui_pop::model::commands::Origin as PopOrigin;
    use helios_gui::clipboard::GRAPH_ENDPOINT;

    let root = temp("paste");
    let (mut h, _) = window(&root);
    let app = h.state().app().handle().clone();
    h.state().clip_graph().pin(vec!["exposure".into()]);
    let out = clipboard::copy(&app, CopyRequest::new(GRAPH_ENDPOINT), &PopOrigin::User);
    h.state().clip_graph().unpin();
    assert!(out.ok(), "{out:?}");
    let before = steps(&h);
    // A dry run says the name it would give, and adds nothing.
    let plan = clipboard::paste(
        &app,
        PasteRequest::new(GRAPH_ENDPOINT).dry_run(),
        &PopOrigin::User,
    );
    let plan = plan.result().unwrap().clone().unwrap();
    assert!(
        plan["changes"].as_str().unwrap().contains("exposure_2"),
        "{plan}"
    );
    frames(&mut h);
    assert_eq!(steps(&h), before);
    let out = clipboard::paste(&app, PasteRequest::new(GRAPH_ENDPOINT), &PopOrigin::User);
    assert!(out.ok(), "{out:?}");
    frames(&mut h);
    let net =
        |h: &Harness<'static, Shell>| h.state().workspace_state().unwrap().bundle.template.network;
    let n = net(&h);
    assert!(n.nodes.iter().any(|d| d.name == "exposure_2"), "{n:?}");
    assert!(
        n.connections
            .iter()
            .any(|c| c.from == "source" && c.to == "exposure_2" && c.input == "input"),
        "{n:?}"
    );
    assert_eq!(steps(&h), before + 1, "one undo step");
    undo(&mut h);
    assert!(!net(&h).nodes.iter().any(|d| d.name == "exposure_2"));
    std::fs::remove_dir_all(&root).unwrap();
}

/// The longest a frame of the Graph panel may take on average, at each
/// level, over the masks stress network (debug builds on the bench box
/// included).
const FRAME_BUDGET: std::time::Duration = std::time::Duration::from_millis(100);

/// The masks stress network (three masked modules, 58 nodes) as a
/// workspace's network: its nodes named without dots (a dot names an
/// opened composite's inner node), its source the workspace's.
fn stress_network() -> Value {
    use helios_module_masks::stress;
    let built = stress::build(stress::picture(Region::sized(8, 6)), 3, |k| {
        (
            ModuleRef::new(helios_module_exposure::Exposure),
            Params::new().with("exposure", helios_core::Value::Float(0.5 - k as f64 * 0.4)),
        )
    })
    .unwrap();
    let g = &built.graph;
    let name = |id| {
        if id == built.source {
            "source".to_string()
        } else {
            g.name(id).unwrap().replace('.', "_")
        }
    };
    let mut nodes = Vec::new();
    let mut connections = Vec::new();
    for id in g.ids() {
        let Some(manifest) = g.manifest(id).unwrap() else {
            continue;
        };
        nodes.push(json!({
            "name": name(id),
            "module": manifest.name,
            "params": g.params(id).unwrap().unwrap_or_default(),
        }));
        for (input, from) in manifest.inputs.iter().zip(g.input_ports(id).unwrap()) {
            if let Some((from, port)) = from {
                connections.push(json!({
                    "from": name(from), "port": port, "to": name(id), "input": input.name,
                }));
            }
        }
    }
    json!({
        "sources": ["source"],
        "nodes": nodes,
        "connections": connections,
        "view": { "node": name(built.output()), "port": "output" },
    })
}

#[test]
fn the_masks_stress_network_stays_fluid() {
    let root = temp("fluid");
    let (mut h, _) = window(&root);
    h.state_mut()
        .edit_workspace(&json!({ "network": stress_network() }), &Origin::User)
        .unwrap();
    frames(&mut h);
    let g = read(&mut h);
    assert!(
        g["nodes"].as_array().unwrap().len() >= 58,
        "the network is shown whole: {}",
        g["nodes"].as_array().unwrap().len()
    );
    for level in ["overview", "ports", "detail"] {
        run(
            &mut h,
            "graph.view",
            json!({ "graph": GRAPH, "level": level }),
        );
        run(&mut h, "graph.view", json!({ "graph": GRAPH, "fit": true }));
        let frames = 20;
        let start = std::time::Instant::now();
        for _ in 0..frames {
            h.step();
        }
        let mean = start.elapsed() / frames;
        assert!(
            mean <= FRAME_BUDGET,
            "{level}: {mean:?} a frame, over {FRAME_BUDGET:?}"
        );
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_wire_style_is_kept_between_sessions() {
    use egui_pop::model::values::{Change, Set, Value as PopValue};
    let kept = |h: &Harness<'static, Shell>| {
        let store = h.state().app().handle().store();
        let store = store.lock().unwrap();
        store
            .get(helios_gui::graph::WIRES_KEPT)
            .map(|e| e.value.clone())
    };
    let wires = |h: &Harness<'static, Shell>| {
        let store = h.state().values.lock().unwrap();
        store.get(helios_gui::graph::WIRES).map(|e| e.value.clone())
    };
    let root = temp("wires");
    let (mut h, _) = window(&root);
    // Picked (here by an agent): kept with the interface's settings.
    run(
        &mut h,
        "param.set",
        json!({ "path": helios_gui::graph::WIRES, "value": "orthogonal" }),
    );
    assert_eq!(kept(&h), Some(PopValue::Choice("orthogonal".into())));
    drop(h);
    // The next session starts with the style kept.
    let config = root.join("config");
    let dir = root.join("photos");
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&dir)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &dir).unwrap();
    shell
        .app()
        .handle()
        .store()
        .lock()
        .unwrap()
        .set(
            Set::new(
                helios_gui::graph::WIRES_KEPT,
                Change::To(PopValue::Choice("straight".into())),
                egui_pop::model::commands::Origin::User,
            )
            .untracked(),
        )
        .unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    run(&mut h, "helios.graph.focus", json!(null));
    assert_eq!(
        wires(&h),
        Some(helios_core::Value::Choice("straight".into()))
    );
    std::fs::remove_dir_all(&root).unwrap();
}
