//! The image shown keeps going through the workspace's network when the
//! network changes: in place, the same source image, its values and its
//! undo history kept. A graph edit, a delegation and a step moving the
//! image's values are each undone with the image open, the network and
//! the values coming back together.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui::vec2;
use egui_kittest::Harness;
use helios_core::{Image, Region, Value};
use helios_gui::{Shell, headless, helios_panels, templates};
use helios_shell::commands::Origin;
use helios_template::Source;
use serde_json::{Value as Json, json};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-keep-open-{name}-{}", std::process::id()));
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

fn run(h: &mut Harness<'static, Shell>, id: &str, args: Json) -> Json {
    let out = h.state_mut().run(id, args, Origin::User).unwrap();
    follow(h);
    out
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

fn stored(h: &Harness<'static, Shell>, path: &str) -> Option<f64> {
    h.state()
        .values
        .lock()
        .unwrap()
        .get(path)
        .and_then(|e| e.value.as_f64())
}

#[test]
fn a_graph_edit_keeps_the_image_and_is_undone_with_its_values() {
    let root = temp("graph-edit");
    let mut h = window(&root);
    set(&mut h, "node/exposure/exposure", 1.0);
    assert_eq!(shown(&h, "exposure", "exposure"), Some(1.0));
    // A node added: the image goes through the new network, its value
    // kept, the history too.
    run(
        &mut h,
        "network.add",
        json!({ "nodes": [{ "module": "sigmoid" }] }),
    );
    // Named after its module.
    let tone = "sigmoid".to_string();
    let contrast = format!("node/{tone}/contrast");
    assert!(shown(&h, &tone, "contrast").is_some(), "the new node shown");
    assert_eq!(shown(&h, "exposure", "exposure"), Some(1.0));
    assert_eq!(steps(&h), 2, "the value's step and the edit's");
    // Its value set, then the node removed: the value stays for an undo.
    set(&mut h, &contrast, 2.5);
    run(&mut h, "network.remove", json!({ "nodes": [tone] }));
    assert_eq!(shown(&h, &tone, "contrast"), None);
    assert_eq!(stored(&h, &contrast), Some(2.5));
    assert_eq!(steps(&h), 4);
    // Undone: the node back with the image's value.
    undo(&mut h);
    assert_eq!(shown(&h, &tone, "contrast"), Some(2.5));
    undo(&mut h);
    assert_eq!(shown(&h, &tone, "contrast"), Some(1.5), "its value undone");
    undo(&mut h);
    assert_eq!(shown(&h, &tone, "contrast"), None, "the add undone");
    assert_eq!(shown(&h, "exposure", "exposure"), Some(1.0));
    // And the step before the edits is still there.
    undo(&mut h);
    assert_eq!(shown(&h, "exposure", "exposure"), Some(0.0));
    assert_eq!(steps(&h), 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_delegation_is_undone_with_the_image_open() {
    let root = temp("delegate");
    let mut h = window(&root);
    set(&mut h, "node/exposure/exposure", 0.5);
    run(
        &mut h,
        "network.delegate",
        json!({ "name": "Tone", "nodes": ["exposure"] }),
    );
    assert_eq!(steps(&h), 2);
    let delegated = |h: &Harness<'static, Shell>| {
        !h.state()
            .workspace_state()
            .unwrap()
            .bundle
            .template
            .subworkspaces
            .is_empty()
    };
    assert!(delegated(&h));
    undo(&mut h);
    assert!(!delegated(&h));
    assert_eq!(shown(&h, "exposure", "exposure"), Some(0.5));
    undo(&mut h);
    assert_eq!(shown(&h, "exposure", "exposure"), Some(0.0));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_step_moving_values_is_one_undo_with_its_network() {
    let root = temp("moving");
    let mut h = window(&root);
    set(&mut h, "node/exposure/exposure", 1.25);
    // `exposure` renamed `lift`, its value moved with it: what a macro
    // made from nodes does.
    h.state_mut()
        .workspace_step_moving("rename exposure", &Origin::User, |now| {
            let mut net = now.bundle.template.network.clone();
            for n in &mut net.nodes {
                if n.name == "exposure" {
                    n.name = "lift".into();
                }
            }
            for c in &mut net.connections {
                for end in [&mut c.from, &mut c.to] {
                    if end == "exposure" {
                        *end = "lift".into();
                    }
                }
            }
            let next = now.edited(&json!({ "network": net }))?;
            let moves = BTreeMap::from([
                ("node/exposure/exposure".to_string(), None),
                ("node/lift/exposure".to_string(), Some(Value::Float(1.25))),
            ]);
            Ok((next, moves))
        })
        .unwrap();
    follow(&mut h);
    assert_eq!(shown(&h, "lift", "exposure"), Some(1.25), "moved and kept");
    assert_eq!(shown(&h, "exposure", "exposure"), None);
    assert_eq!(steps(&h), 2, "one step for the network and the values");
    undo(&mut h);
    assert_eq!(shown(&h, "exposure", "exposure"), Some(1.25));
    assert_eq!(shown(&h, "lift", "exposure"), None);
    // Redone: the moved value comes back with its node.
    h.state()
        .values
        .lock()
        .unwrap()
        .redo(&Origin::User)
        .unwrap();
    follow(&mut h);
    assert_eq!(shown(&h, "lift", "exposure"), Some(1.25));
    let _ = std::fs::remove_dir_all(&root);
}
