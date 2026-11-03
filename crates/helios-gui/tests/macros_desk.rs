//! Macros on the desktop: made from nodes, opened, folded and dissolved
//! with an image open, each one undo step that moves the image's values
//! with the network; the Graph panel shows an opened macro's nodes in a
//! group, and Ctrl+G makes a macro of the nodes selected.

use std::path::{Path, PathBuf};

use egui::vec2;
use egui_kittest::Harness;
use helios_core::{Image, Region, Value};
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use helios_template::Source;
use serde_json::{Value as Json, json};

fn temp(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("helios-macros-desk-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A window on a blank workspace (a source, an exposure and a mask into a
/// blend) with an image open through its network.
fn window(root: &Path) -> Harness<'static, Shell> {
    let config = root.join("config");
    let dir = root.join("photos");
    std::fs::create_dir_all(&dir).unwrap();
    let mut shell = Shell::builder(helios_panels(), Some(config.join("ui")))
        .workspace(&dir)
        .build();
    shell.load_layout().unwrap();
    shell.open_workspace(&config, &dir).unwrap();
    let image = Image::from_fn(
        Region::sized(16, 12),
        4,
        |_, _, c| {
            if c == 3 { 1.0 } else { 0.3 }
        },
    );
    let built = shell
        .workspace_bundle()
        .unwrap()
        .build_with(&templates::modules(), Source::Picture(&image), None)
        .unwrap();
    shell
        .set_view_source(built.graph, built.view.0, &built.view.1)
        .unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

/// The frames a step takes, then what the host does when the network
/// changed: the image kept open through it.
fn follow(h: &mut Harness<'static, Shell>) {
    for _ in 0..3 {
        h.run();
    }
    if h.state_mut().take_network_changed() {
        assert!(h.state_mut().keep_image_open().unwrap(), "kept in place");
    }
    for _ in 0..2 {
        h.run();
    }
}

/// Runs a command as a person would and answers its result: a command
/// queued runs at the next frame, its answer in the history.
fn answer(h: &mut Harness<'static, Shell>, id: &str, args: Json) -> Result<Json, String> {
    let out = h.state_mut().run(id, args, Origin::User);
    follow(h);
    match out {
        Ok(v) if v == json!({ "queued": true }) => {
            h.state()
                .history()
                .into_iter()
                .rev()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id} never ran"))
                .result
        }
        other => other,
    }
}

fn run(h: &mut Harness<'static, Shell>, id: &str, args: Json) -> Json {
    answer(h, id, args).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn set(h: &mut Harness<'static, Shell>, path: &str, v: f64) {
    h.state()
        .values
        .lock()
        .unwrap()
        .set(helios_values::Set::new(
            path,
            helios_values::Change::To(Value::Float(v)),
            Origin::User,
        ))
        .unwrap();
    follow(h);
}

fn undo(h: &mut Harness<'static, Shell>) {
    h.state()
        .values
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap()
        .expect("a step to undo");
    follow(h);
}

fn steps(h: &Harness<'static, Shell>) -> usize {
    h.state().values.lock().unwrap().undo_steps().len()
}

/// A parameter of a node of the graph shown; `None` without that node.
fn shown(h: &Harness<'static, Shell>, node: &str, param: &str) -> Option<f64> {
    let viewport = h.state().viewport();
    let g = viewport.graph()?;
    let id = g.find(node)?;
    g.params(id).unwrap()?.get(param)?.as_f64()
}

fn network(h: &Harness<'static, Shell>) -> Vec<(String, bool)> {
    h.state()
        .workspace_state()
        .unwrap()
        .bundle
        .template
        .network
        .nodes
        .iter()
        .map(|n| (n.name.clone(), n.open))
        .collect()
}

#[test]
fn a_macro_made_opened_folded_and_dissolved_keeps_the_image_and_undoes_in_one_step() {
    let root = temp("cycle");
    let mut h = window(&root);
    set(&mut h, "node/exposure/exposure", 1.0);
    // Made of the exposure: its changed value exposed and moved.
    let made = run(
        &mut h,
        "macro.make",
        json!({ "nodes": ["exposure"], "name": "Look" }),
    );
    assert_eq!(made["node"], "look", "{made}");
    assert_eq!(made["composite"]["id"], "user.look");
    assert_eq!(shown(&h, "look", "exposure"), Some(1.0), "moved and kept");
    assert_eq!(shown(&h, "exposure", "exposure"), None);
    assert_eq!(steps(&h), 2);
    // Selected: what one looks at now.
    assert_eq!(
        h.state()
            .values
            .lock()
            .unwrap()
            .get(helios_gui::params::SELECTION)
            .map(|e| e.value.clone()),
        Some(Value::Text("look".into()))
    );
    // Opened: its inner node in the graph, its value given by the macro.
    run(&mut h, "macro.open", json!({ "node": "look" }));
    assert!(network(&h).contains(&("look".into(), true)));
    assert_eq!(shown(&h, "look.exposure", "exposure"), Some(1.0));
    run(&mut h, "helios.graph.focus", json!(null));
    let g = run(
        &mut h,
        "graph.read",
        json!({ "graph": helios_gui::graph::GRAPH }),
    );
    let inner = g["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "look.exposure")
        .unwrap_or_else(|| panic!("no inner node in {g}"));
    assert_eq!(inner["group"], "macro:look", "{inner}");
    // Folded again by Ctrl+Enter's command on its inner node.
    run(&mut h, "macro.toggle", json!({ "node": "look.exposure" }));
    assert!(network(&h).contains(&("look".into(), false)));
    // Dissolved: the exposure back, the value on it.
    run(&mut h, "macro.dissolve", json!({ "node": "look" }));
    assert_eq!(shown(&h, "exposure", "exposure"), Some(1.0));
    assert_eq!(shown(&h, "look", "exposure"), None);
    // Each was one step: undone back to the macro, then to the start.
    undo(&mut h);
    assert_eq!(shown(&h, "look", "exposure"), Some(1.0));
    undo(&mut h);
    undo(&mut h);
    undo(&mut h);
    assert_eq!(shown(&h, "exposure", "exposure"), Some(1.0));
    assert_eq!(shown(&h, "look", "exposure"), None);
    assert!(network(&h).iter().any(|(n, _)| n == "exposure"));
    undo(&mut h);
    assert_eq!(shown(&h, "exposure", "exposure"), Some(0.0));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_dry_run_changes_nothing_and_a_refusal_says_why() {
    let root = temp("dry");
    let mut h = window(&root);
    let dry = run(
        &mut h,
        "macro.make",
        json!({ "nodes": ["exposure"], "name": "Look", "dry_run": true }),
    );
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["node"], "look");
    assert!(network(&h).iter().any(|(n, _)| n == "exposure"));
    assert_eq!(steps(&h), 0);
    let e = answer(
        &mut h,
        "macro.make",
        json!({ "nodes": ["source"], "name": "Look" }),
    )
    .unwrap_err();
    assert!(e.contains("is a source"), "{e}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_selected_nodes_are_made_a_macro_by_default() {
    let root = temp("selected");
    let mut h = window(&root);
    {
        let mut store = h.state().values.lock().unwrap();
        store
            .set(helios_values::Set::new(
                helios_gui::shell::GRAPH_SELECTED,
                helios_values::Change::To(Value::Text(json!(["exposure"]).to_string())),
                Origin::User,
            ))
            .unwrap();
    }
    let made = run(&mut h, "macro.make", json!({ "name": "Tone" }));
    assert_eq!(made["nodes"], json!(["exposure"]));
    assert!(network(&h).iter().any(|(n, _)| n == "tone"));
    // Without a name, a form asks for it.
    run(&mut h, "macro.make", json!({ "nodes": ["tone"] }));
    let dialog = run(&mut h, "dialog.current", json!(null));
    assert!(dialog.to_string().contains("name"), "{dialog}");
    let _ = std::fs::remove_dir_all(&root);
}
