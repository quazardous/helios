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
        .build_with(&templates::modules(), Source::picture(&image), None)
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
        .values()
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
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap()
        .expect("a step to undo");
    follow(h);
}

fn steps(h: &Harness<'static, Shell>) -> usize {
    h.state().values().lock().unwrap().undo_steps().len()
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
            .values()
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
        let mut store = h.state().values().lock().unwrap();
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

#[test]
fn a_macro_s_interface_edited_is_one_step_and_keeps_the_image() {
    let root = temp("interface");
    let mut h = window(&root);
    set(&mut h, "node/exposure/exposure", 1.0);
    run(
        &mut h,
        "macro.make",
        json!({ "nodes": ["exposure"], "name": "Look" }),
    );
    let before = steps(&h);
    let exposed = json!([{
        "name": "brightness", "node": "exposure", "param": "exposure",
        "label": "Brightness", "group": "Light", "min": -2.0, "max": 2.0,
    }]);
    let dry = run(
        &mut h,
        "macro.interface",
        json!({ "node": "look", "exposed": exposed, "dry_run": true }),
    );
    assert_eq!(
        dry["to"],
        json!({ "id": "user.look", "version": 2 }),
        "{dry}"
    );
    assert_eq!(steps(&h), before, "a dry run changes nothing");
    let done = run(
        &mut h,
        "macro.interface",
        json!({ "node": "look", "exposed": exposed }),
    );
    assert_eq!(
        done["renamed"],
        json!({ "exposure": "brightness" }),
        "{done}"
    );
    assert_eq!(steps(&h), before + 1, "one step");
    assert_eq!(shown(&h, "look", "brightness"), Some(1.0), "the value kept");
    // The Parameters panel's descriptor carries the group.
    let group = h
        .state()
        .values()
        .lock()
        .unwrap()
        .get("node/look/brightness")
        .and_then(|e| e.descriptor.group.clone());
    assert_eq!(group.as_deref(), Some("Light"));
    // A range wider than the inner one is refused, the entry named.
    let e = answer(
        &mut h,
        "macro.interface",
        json!({ "node": "look", "exposed": [{
            "name": "brightness", "node": "exposure", "param": "exposure", "min": -40.0,
        }] }),
    )
    .unwrap_err();
    assert!(e.contains("exposed[0]"), "{e}");
    undo(&mut h);
    assert_eq!(
        shown(&h, "look", "exposure"),
        Some(1.0),
        "back to version 1"
    );
    assert_eq!(shown(&h, "look", "brightness"), None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_macro_made_from_the_form_opens_its_interface_editor_answered_as_a_form() {
    let root = temp("editor");
    let mut h = window(&root);
    set(&mut h, "node/exposure/exposure", 1.0);
    // The name asked, then the editor on the macro made.
    run(&mut h, "macro.make", json!({ "nodes": ["exposure"] }));
    run(
        &mut h,
        "dialog.answer",
        json!({ "values": { "name": "Look" } }),
    );
    follow(&mut h);
    assert!(network(&h).iter().any(|(n, _)| n == "look"));
    let current = run(&mut h, "dialog.current", json!(null));
    let text = current.to_string();
    assert!(text.contains("Interface of look"), "{current}");
    assert!(
        text.contains("user.look@2"),
        "the file it saves as: {current}"
    );
    // An agent answers it as a form, with the list.
    run(
        &mut h,
        "dialog.answer",
        json!({ "values": { "exposed": [{
            "name": "brightness", "node": "exposure", "param": "exposure",
            "group": "Light",
        }] } }),
    );
    follow(&mut h);
    assert_eq!(shown(&h, "look", "brightness"), Some(1.0), "the value kept");
    let pinned = h
        .state()
        .workspace_state()
        .unwrap()
        .bundle
        .template
        .network
        .nodes
        .iter()
        .find(|n| n.name == "look")
        .and_then(|n| n.composite.clone())
        .unwrap();
    assert_eq!(pinned.version, 2);
    let _ = std::fs::remove_dir_all(&root);
}

/// The version `node` is pinned to, and whether it is open.
fn pinned(h: &Harness<'static, Shell>, node: &str) -> (u32, bool) {
    let state = h.state().workspace_state().unwrap();
    let n = state
        .bundle
        .template
        .network
        .nodes
        .iter()
        .find(|n| n.name == node)
        .unwrap_or_else(|| panic!("no node {node}"))
        .clone();
    (n.composite.unwrap().version, n.open)
}

/// The answer of the last `id` run, from the history.
fn last_answer(h: &Harness<'static, Shell>, id: &str) -> Json {
    h.state()
        .history()
        .into_iter()
        .rev()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("{id} never ran"))
        .result
        .unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// A macro `look` of the exposure (its exposure set to 1 and exposed, or
/// those in `expose`), and a second node `look_2` on the same file, fed
/// by the source.
fn two_looks(h: &mut Harness<'static, Shell>, expose: Json) {
    set(h, "node/exposure/exposure", 1.0);
    run(
        h,
        "macro.make",
        json!({ "nodes": ["exposure"], "name": "Look", "expose": expose }),
    );
    let added = run(
        h,
        "network.add",
        json!({ "nodes": [{ "composite": "user.look@1" }] }),
    );
    assert!(added.to_string().contains("look_2"), "{added}");
    run(
        h,
        "network.connect",
        json!({ "from": "source", "port": "image", "to": "look_2", "input": "input" }),
    );
    assert_eq!(pinned(h, "look_2"), (1, false));
}

#[test]
fn a_macro_changed_while_open_folds_by_the_answer_given_each_one_undo_step() {
    let root = temp("fold-changed");
    let mut h = window(&root);
    two_looks(&mut h, Json::Null);
    run(&mut h, "macro.open", json!({ "node": "look" }));
    // An inner parameter it does not expose, changed on the image.
    set(&mut h, "node/look.exposure/black", 0.1);
    let before = steps(&h);
    // Folded without an answer: asked, nothing done yet.
    let asked = run(&mut h, "macro.fold", json!({ "node": "look" }));
    assert_eq!(asked["asked"], true, "{asked}");
    assert_eq!(asked["changed"], json!(["node/look.exposure/black"]));
    let dialog = run(&mut h, "dialog.current", json!(null)).to_string();
    for label in [
        "Save as version 2",
        "Expose them",
        "Discard",
        "Keep it opened",
    ] {
        assert!(dialog.contains(label), "{label} in {dialog}");
    }
    // Kept open: nothing changes.
    run(&mut h, "dialog.answer", json!({ "choice": "keep" }));
    follow(&mut h);
    assert_eq!(pinned(&h, "look"), (1, true));
    assert_eq!(steps(&h), before);
    // Discarded: back to version 1's inside, one step.
    run(
        &mut h,
        "macro.fold",
        json!({ "node": "look", "changes": "discard" }),
    );
    assert_eq!(pinned(&h, "look"), (1, false));
    assert_eq!(steps(&h), before + 1);
    undo(&mut h);
    assert_eq!(pinned(&h, "look"), (1, true));
    assert_eq!(shown(&h, "look.exposure", "black"), Some(0.1), "undone");
    // Exposed: version 2 shows it, its value kept on the image.
    let exposed = run(
        &mut h,
        "macro.fold",
        json!({ "node": "look", "changes": "expose" }),
    );
    assert_eq!(exposed["exposed"], json!(["black"]), "{exposed}");
    assert_eq!(pinned(&h, "look"), (2, false));
    assert_eq!(shown(&h, "look", "black"), Some(0.1));
    assert_eq!(shown(&h, "look", "exposure"), Some(1.0));
    undo(&mut h);
    assert_eq!(pinned(&h, "look"), (1, true));
    assert_eq!(shown(&h, "look.exposure", "black"), Some(0.1), "undone");
    // Saved as version 2, the dialog's default, answered as a person
    // would: the change is the file's; the other node keeps version 1,
    // offered version 2 as an update.
    run(&mut h, "macro.fold", json!({ "node": "look" }));
    run(&mut h, "dialog.answer", json!({ "choice": "version" }));
    follow(&mut h);
    let saved = last_answer(&h, "macro.fold");
    assert_eq!(
        saved["composite"],
        json!({ "id": "user.look", "version": 2 }),
        "{saved}"
    );
    assert_eq!(pinned(&h, "look"), (2, false));
    assert_eq!(
        pinned(&h, "look_2"),
        (1, false),
        "the other node keeps its version"
    );
    assert_eq!(
        saved["updates"],
        json!([{ "node": "look_2", "composite": { "id": "user.look", "version": 1 }, "available": 2 }])
    );
    let state = h.state().workspace_state().unwrap();
    let black = |version: u32| {
        state
            .bundle
            .composites
            .iter()
            .find(|f| f.id == "user.look" && f.version == version)
            .and_then(|f| f.nodes[0].params.get("black").cloned())
    };
    assert_eq!(black(2), Some(Value::Float(0.1)));
    assert_eq!(black(1), None, "version 1's file unchanged");
    assert_eq!(steps(&h), before + 1);
    undo(&mut h);
    assert_eq!(pinned(&h, "look"), (1, true));
    assert_eq!(shown(&h, "look.exposure", "black"), Some(0.1), "undone");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_update_on_offer_carries_values_by_name_and_says_what_it_dropped() {
    let root = temp("upgrade");
    let mut h = window(&root);
    two_looks(
        &mut h,
        json!([
            { "name": "exposure", "node": "exposure", "param": "exposure" },
            { "name": "black", "node": "exposure", "param": "black" }
        ]),
    );
    set(&mut h, "node/look_2/exposure", 0.5);
    set(&mut h, "node/look_2/black", -0.1);
    // Version 2 no longer exposes `black`: offered to the other node.
    let edited = run(
        &mut h,
        "macro.interface",
        json!({ "node": "look", "exposed": [
            { "name": "exposure", "node": "exposure", "param": "exposure" }
        ] }),
    );
    assert_eq!(pinned(&h, "look"), (2, false));
    assert_eq!(
        edited["updates"],
        json!([{ "node": "look_2", "composite": { "id": "user.look", "version": 1 }, "available": 2 }]),
        "{edited}"
    );
    let before = steps(&h);
    let dry = run(
        &mut h,
        "macro.upgrade",
        json!({ "node": "look_2", "dry_run": true }),
    );
    assert_eq!(dry["dropped"], json!(["black"]), "{dry}");
    assert_eq!(steps(&h), before);
    let up = run(&mut h, "macro.upgrade", json!({ "node": "look_2" }));
    assert_eq!(up["to"], json!({ "id": "user.look", "version": 2 }), "{up}");
    assert_eq!(up["dropped"], json!(["black"]));
    assert_eq!(up["updates"], json!([]));
    assert_eq!(pinned(&h, "look_2"), (2, false));
    assert_eq!(
        shown(&h, "look_2", "exposure"),
        Some(0.5),
        "carried by name"
    );
    assert_eq!(shown(&h, "look_2", "black"), None, "dropped");
    assert_eq!(steps(&h), before + 1, "one step");
    undo(&mut h);
    assert_eq!(pinned(&h, "look_2"), (1, false));
    assert_eq!(shown(&h, "look_2", "black"), Some(-0.1), "undone");
    assert_eq!(shown(&h, "look_2", "exposure"), Some(0.5));
    let _ = std::fs::remove_dir_all(&root);
}

/// Runs a command as `origin` and answers its result (see [`answer`]).
fn answer_as(
    h: &mut Harness<'static, Shell>,
    id: &str,
    args: Json,
    origin: Origin,
) -> Result<Json, String> {
    let before = h.state().history().len();
    let out = h.state_mut().run(id, args, origin);
    follow(h);
    match out {
        Ok(v) if v == json!({ "queued": true }) => {
            h.state()
                .history()
                .into_iter()
                .skip(before)
                .rev()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id} never ran"))
                .result
        }
        other => other,
    }
}

#[test]
fn a_macro_published_with_the_user_s_answer_is_offered_to_every_workspace_and_never_undone() {
    let root = temp("publish");
    let mut h = window(&root);
    let config = root.join("config");
    let file = config.join("composites/user.look@1.helios-composite.json");
    set(&mut h, "node/exposure/exposure", 1.0);
    run(
        &mut h,
        "macro.make",
        json!({ "nodes": ["exposure"], "name": "Look" }),
    );
    let steps_made = steps(&h);
    // A dry run says where it would write.
    let dry = run(
        &mut h,
        "macro.publish",
        json!({ "node": "look", "dry_run": true }),
    );
    assert_eq!(dry["file"], json!(file.display().to_string()), "{dry}");
    assert!(!file.exists());
    // An agent's call asks the user: nothing written.
    let asked = answer_as(
        &mut h,
        "macro.publish",
        json!({ "node": "look", "confirmed": true }),
        Origin::Agent("test".into()),
    )
    .unwrap();
    assert_eq!(asked["asked"], true, "{asked}");
    assert!(!file.exists());
    let dialog = run(&mut h, "dialog.current", json!(null));
    assert!(dialog.to_string().contains("not undone"), "{dialog}");
    // The agent answering it is asked again; the user's answer writes.
    answer_as(
        &mut h,
        "dialog.answer",
        json!({ "answer": "primary" }),
        Origin::Agent("test".into()),
    )
    .unwrap();
    assert!(!file.exists(), "an agent never confirms it");
    run(&mut h, "dialog.answer", json!({ "answer": "primary" }));
    assert!(file.exists(), "published on the user's answer");
    let published = last_answer(&h, "macro.publish");
    assert_eq!(published["undoable"], false, "{published}");
    // Not an undo step.
    assert_eq!(steps(&h), steps_made);
    // Twice: refused, the file kept.
    let e = answer(&mut h, "macro.publish", json!({ "node": "look" })).unwrap_err();
    assert!(e.contains("in your library already"), "{e}");
    // The graph's Add menu offers it, in its Macros section.
    run(&mut h, "helios.graph.focus", json!(null));
    let g = run(
        &mut h,
        "graph.read",
        json!({ "graph": helios_gui::graph::GRAPH }),
    );
    let kind = g["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["id"] == "user.look@1")
        .unwrap_or_else(|| panic!("not offered: {g}"))
        .clone();
    assert_eq!(kind["title"], "Macros › Look", "{kind}");
    // Undone back past the macro: the published file stays.
    undo(&mut h);
    assert!(!network(&h).iter().any(|(n, _)| n == "look"));
    assert!(file.exists(), "undo never removes a published file");
    // Another workspace adds it from the library, and keeps a copy.
    let other = root.join("other");
    std::fs::create_dir_all(&other).unwrap();
    h.state_mut().open_workspace(&config, &other).unwrap();
    follow(&mut h);
    let added = run(
        &mut h,
        "network.add",
        json!({ "nodes": [{ "composite": "user.look@1" }] }),
    );
    assert!(added.to_string().contains("look"), "{added}");
    let state = h.state().workspace_state().unwrap();
    assert!(
        state
            .bundle
            .composites
            .iter()
            .any(|f| f.id == "user.look" && f.version == 1),
        "copied into the workspace"
    );
    let _ = std::fs::remove_dir_all(&root);
}
