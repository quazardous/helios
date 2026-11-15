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
use egui_pop::model::ids::PanelId;
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
    h.state().values().lock().unwrap().undo_steps().len()
}

fn undo(h: &mut Harness<'static, Shell>) {
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    frames(h);
}

fn selection(h: &Harness<'static, Shell>) -> Value {
    let store = h.state().values().lock().unwrap();
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
    for id in ["source", "base", "exposure", "mask", "blend"] {
        node(&g, id);
    }
    assert_eq!(node(&g, "exposure")["kind"], "exposure");
    assert_eq!(node(&g, "exposure")["inputs"][0]["id"], "input");
    let wire = json!({
        "from": { "node": "base", "port": "output" },
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
            .any(|c| c.from == "base" && c.to == "exposure_2" && c.input == "input"),
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

/// Checks a frame time against [`FRAME_BUDGET`]. Over budget fails, except
/// under `HELIOS_SHARED_MACHINE` (set by the bench box's shared test jobs:
/// other jobs and throttling slow every frame there), where it only warns.
/// A real regression still fails on CI and in the box's `test-times` job,
/// which holds the machine alone (see docs/bench.md).
fn within_budget(what: &str, mean: std::time::Duration) {
    if mean <= FRAME_BUDGET {
        return;
    }
    let message = format!("{what}: {mean:?} a frame, over {FRAME_BUDGET:?}");
    if std::env::var_os("HELIOS_SHARED_MACHINE").is_some_and(|v| !v.is_empty() && v != "0") {
        eprintln!("warning (shared machine, budget not enforced): {message}");
    } else {
        panic!("{message}");
    }
}

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

/// The Graph panel draws the masks stress network within
/// [`FRAME_BUDGET`] at each level. The budget only warns on a shared
/// machine (see [`within_budget`]).
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
        within_budget(level, start.elapsed() / frames);
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
        let store = h.state().values().lock().unwrap();
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

/// The canvas's one-time navigation hint.
const NAV_HINT: &str = "Middle-drag, Space+drag or H to pan · Ctrl+wheel to zoom · F to frame";
/// The canvas's hand mode indicator.
const NAV_HAND: &str = "Hand: drag to pan (H to leave)";
/// The graph's overview, in a corner of the canvas.
const OVERVIEW: &str = "Graph overview";

/// The layer drawn on top at a point of the main window.
fn top_at(h: &Harness<'static, Shell>, at: egui::Pos2) -> Option<egui::LayerId> {
    h.ctx.layer_id_at(at)
}

/// An overlay: its name, its centre and its layer, as found over the
/// bare canvas, before any window opens.
type Found = (&'static str, egui::Pos2, egui::LayerId);

/// An overlay of the canvas, found by its accessible name: its rect, and
/// the layer drawn on top at its centre, which is its own.
fn overlay(h: &Harness<'static, Shell>, label: &str) -> (egui::Rect, egui::LayerId) {
    let rect = h
        .query_by_label(label)
        .unwrap_or_else(|| {
            let layers = h.ctx.memory(|m| {
                let ls: Vec<_> = m
                    .areas()
                    .visible_layer_ids()
                    .into_iter()
                    .map(|l| (l, m.area_rect(l.id)))
                    .collect();
                format!("modal {:?}, layers {ls:?}", m.top_modal_layer())
            });
            panic!("`{label}` shows over the graph: {layers}")
        })
        .rect();
    let layer = top_at(h, rect.center()).expect("a layer under the overlay");
    (rect, layer)
}

/// Each overlay lies above the canvas (an empty spot of it, `empty`, is
/// in another layer) and out of the foreground order, where it would lie
/// over every window, popup and dialog.
fn above_the_canvas(h: &Harness<'static, Shell>, labels: &[&str], empty: egui::Pos2) {
    let canvas = top_at(h, empty).expect("the canvas's layer");
    for label in labels {
        let (_, layer) = overlay(h, label);
        assert_ne!(layer, canvas, "`{label}` lies above the canvas");
        assert!(
            !matches!(layer.order, egui::Order::Foreground | egui::Order::Tooltip),
            "`{label}` is drawn in {:?}, over any window",
            layer.order
        );
    }
}

/// The floating Masks palette moved over each overlay: it lies over
/// them. It goes back where it was after.
fn under_the_palette(h: &mut Harness<'static, Shell>, labels: &[Found]) {
    let masks = PanelId::new("helios.masks").unwrap();
    let before = h.state().layout().floating_of(&masks).map(|f| f.pos);
    for &(label, _, layer) in labels {
        let (rect, _) = overlay(h, label);
        let at = rect.center();
        run(
            h,
            "helios.masks.float",
            json!({ "pos": [at.x - 30.0, at.y - 30.0] }),
        );
        let on_top = top_at(h, at);
        assert_ne!(
            on_top,
            Some(layer),
            "`{label}` is drawn over the floating Masks palette"
        );
    }
    match before {
        Some(pos) => run(h, "helios.masks.float", json!({ "pos": pos })),
        None => run(h, "helios.masks.dock", json!(null)),
    };
}

/// A popup (the canvas's context menu) opened just above and left of
/// each overlay, so that it hangs over it: it lies over the overlay.
fn under_a_popup(h: &mut Harness<'static, Shell>, labels: &[Found]) {
    for &(label, _, layer) in labels {
        let (rect, _) = overlay(h, label);
        let at = rect.left_top() - vec2(4.0, 4.0);
        h.hover_at(at);
        h.run();
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.step();
        }
        frames(h);
        // The popups' layers that hang over the overlay.
        let over: Vec<egui::Rect> = h.ctx.memory(|m| {
            m.areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|l| l.order == egui::Order::Foreground && *l != layer)
                .filter_map(|l| m.area_rect(l.id))
                .map(|r| r.intersect(rect))
                .filter(egui::Rect::is_positive)
                .collect()
        });
        let shared = over
            .first()
            .unwrap_or_else(|| panic!("a context menu opened at {at:?} hangs over `{label}`"));
        assert_ne!(
            top_at(h, shared.center()),
            Some(layer),
            "`{label}` is drawn over a popup"
        );
        h.key_press(egui::Key::Escape);
        frames(h);
    }
}

/// The import window, an `egui::Window` drawn inside the main window,
/// dragged by its title bar over each overlay: it lies over them. It
/// stays open.
fn under_the_import_window(h: &mut Harness<'static, Shell>, labels: &[Found]) {
    let title = "Import photos";
    run(h, "library.importWindow", json!(null));
    let mut shown = None;
    for _ in 0..headless::WINDOW_APPEARS_WITHIN {
        h.step();
        shown = headless::window_rect(h, title);
        if shown.is_some() {
            break;
        }
    }
    shown.expect("the import window shows");
    for &(label, _, layer) in labels {
        let (rect, _) = overlay(h, label);
        let to = rect.center();
        let shown = headless::window_rect(h, title).unwrap();
        let grip = egui::pos2(shown.center().x, shown.top() + 10.0);
        h.hover_at(grip);
        h.run();
        h.drag_at(grip);
        h.step();
        for k in 1..=8 {
            h.hover_at(grip + (to - grip) * (k as f32 / 8.0));
            h.step();
        }
        h.drop_at(to);
        frames(h);
        let moved = headless::window_rect(h, title).unwrap();
        assert!(
            moved.contains(to),
            "the window covers `{label}`: {moved:?}, {to:?}"
        );
        let on_top = top_at(h, to);
        assert_ne!(
            on_top,
            Some(layer),
            "`{label}` is drawn over the import window"
        );
        assert_eq!(
            on_top.map(|l| l.order),
            Some(egui::Order::Middle),
            "{on_top:?}"
        );
    }
}

/// A dialog over the main window (a form asked for): it lies over every
/// overlay.
fn under_a_dialog(h: &mut Harness<'static, Shell>, labels: &[Found]) {
    run(h, "library.importSource", json!(null));
    frames(h);
    assert!(
        h.state().target_rect("helios.dialog").is_ok(),
        "a dialog shows"
    );
    for (label, at, layer) in labels {
        assert_ne!(
            top_at(h, *at),
            Some(*layer),
            "`{label}` is drawn over a dialog"
        );
    }
}

/// Soft windows, drawn inside the main window (the import window, the
/// floating Masks palette, a popup, a dialog), lie over the canvas's
/// overlays, which lie above the canvas: here the hint and the hand mode
/// indicator.
#[test]
fn the_canvas_hint_never_shows_over_a_window_or_a_dialog() {
    let root = temp("hint-under");
    let (mut h, _) = window(&root);
    // The pointer over the graph brings up its one-time hint, in a layer
    // of its own above the canvas.
    let graph = h.state().target_rect(GRAPH).unwrap();
    // An empty spot of the canvas, away from the nodes, the toolbar and
    // the overview.
    let empty = graph.left_bottom() + vec2(24.0, -24.0);
    h.hover_at(empty);
    frames(&mut h);
    // Hand mode: its indicator in the canvas's top-left corner.
    h.key_press(egui::Key::H);
    frames(&mut h);
    let labels = [NAV_HINT, NAV_HAND];
    above_the_canvas(&h, &labels, empty);
    let found = labels.map(|l| {
        let (r, layer) = overlay(&h, l);
        (l, r.center(), layer)
    });
    under_the_palette(&mut h, &found);
    under_a_popup(&mut h, &found);
    under_the_import_window(&mut h, &found);
    under_a_dialog(&mut h, &found);
    std::fs::remove_dir_all(&root).unwrap();
}

/// The graph's overview, shown once the network is much larger than the
/// view, lies above the canvas and under the soft windows too.
#[test]
fn the_graph_overview_lies_under_the_windows_over_the_canvas() {
    let root = temp("overview-under");
    let (mut h, _) = window(&root);
    let graph = h.state().target_rect(GRAPH).unwrap();
    let empty = graph.left_bottom() + vec2(24.0, -24.0);
    // Zoomed in far enough that the network is much larger than the
    // view: the overview shows.
    h.hover_at(graph.center());
    h.run();
    for _ in 0..6 {
        h.event(egui::Event::Zoom(2.0));
        h.step();
    }
    h.hover_at(empty);
    frames(&mut h);
    let labels = [OVERVIEW];
    above_the_canvas(&h, &labels, empty);
    let found = labels.map(|l| {
        let (r, layer) = overlay(&h, l);
        (l, r.center(), layer)
    });
    under_the_palette(&mut h, &found);
    under_a_popup(&mut h, &found);
    under_the_import_window(&mut h, &found);
    under_a_dialog(&mut h, &found);
    std::fs::remove_dir_all(&root).unwrap();
}
