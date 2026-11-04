//! Macros on a workspace as pure steps: made from nodes, opened, folded,
//! dissolved and upgraded. Each step gives the next state and the open
//! image's values to move; the image renders the same before and after,
//! with the same content key, since a macro's inner steps are the nodes it
//! packages.

use std::sync::Arc;

use egui_pop_model::layout::Layout;
use helios_core::{Bus, Engine, Image, ModuleRef, Region, Value};
use helios_template::format::Exposed;
use helios_template::macros::{Moves, Step, Values};
use helios_template::workspace::{self, State};
use helios_template::{Bundle, Catalogue, CompositeRef, Source, parse_template};
use serde_json::json;

fn catalogue() -> Catalogue {
    Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .module(ModuleRef::new(helios_module_sigmoid::Sigmoid))
}

/// `source → exp → tone → lift`, the view on `lift`, and `side` reading
/// `exp` alone; `exp` and `tone` in a sub-workspace.
fn state() -> State {
    let template = parse_template(
        &json!({
            "format": 1,
            "id": "test.macros",
            "version": 1,
            "meta": { "name": "Macros", "scenario": "A chain to package" },
            "network": {
                "sources": ["source"],
                "nodes": [
                    { "name": "exp", "module": "exposure",
                      "params": { "exposure": { "float": 0.5 } } },
                    { "name": "tone", "module": "sigmoid" },
                    { "name": "lift", "module": "exposure" },
                    { "name": "side", "module": "exposure" }
                ],
                "connections": [
                    { "from": "source", "port": "image", "to": "exp", "input": "input" },
                    { "from": "exp", "port": "output", "to": "tone", "input": "input" },
                    { "from": "tone", "port": "output", "to": "lift", "input": "input" },
                    { "from": "exp", "port": "output", "to": "side", "input": "input" }
                ],
                "view": { "node": "lift", "port": "output" },
                "presets": [
                    { "id": "bright", "name": "Bright",
                      "values": { "exp": { "exposure": { "float": 1.0 } },
                                  "tone": { "contrast": { "float": 2.0 } } } }
                ]
            },
            "subworkspaces": [
                { "id": "look", "name": "Look", "nodes": ["exp", "tone"] }
            ],
            "positions": { "exp": [10.0, 20.0], "tone": [200.0, 20.0] }
        })
        .to_string(),
    )
    .unwrap();
    State {
        meta: workspace::Meta {
            name: "macros".into(),
            ..workspace::Meta::default()
        },
        bundle: Bundle::new(template, Layout::default()),
    }
}

fn picture() -> Image {
    Image::from_fn(Region::sized(16, 12), 4, |x, y, c| {
        if c == 3 {
            1.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let v = (x * 3 + y * 5 + i64::from(c)) as f32 / 80.0;
            v
        }
    })
}

/// The view's pixels and content key, the image's values set on the graph.
fn render(state: &State, values: &Values) -> (Vec<f32>, helios_core::ContentHash) {
    let image = picture();
    let mut built = state
        .bundle
        .build_with(&catalogue(), Source::Picture(&image), None)
        .unwrap();
    for (path, v) in values {
        let Some(rest) = path.strip_prefix("node/") else {
            continue;
        };
        let (node, param) = rest.rsplit_once('/').unwrap();
        let id = built.nodes[node];
        built.graph.set_param(id, param, v.clone()).unwrap();
    }
    let engine = Engine::new(Arc::new(Bus::new()));
    let region = Region::sized(16, 12);
    let (view, port) = built.view.clone();
    let pixels = engine
        .render(&built.graph, view, &port, region)
        .unwrap()
        .to_vec();
    let key = engine.key(&built.graph, view, region).unwrap();
    (pixels, key)
}

fn moved(values: &Values, moves: &Moves) -> Values {
    let mut out = values.clone();
    for (path, v) in moves {
        match v {
            Some(v) => {
                out.insert(path.clone(), v.clone());
            }
            None => {
                out.remove(path);
            }
        }
    }
    out
}

fn nodes(names: &[&str]) -> Vec<String> {
    names.iter().map(ToString::to_string).collect()
}

fn image_values() -> Values {
    Values::from([
        ("node/exp/exposure".into(), Value::Float(1.25)),
        ("node/tone/contrast".into(), Value::Float(2.5)),
        ("node/lift/exposure".into(), Value::Float(-0.5)),
    ])
}

fn made() -> (Step, Values) {
    let values = image_values();
    let step = state()
        .macro_made(
            &catalogue(),
            &nodes(&["tone", "exp"]),
            "Warm look",
            None,
            &values,
        )
        .unwrap();
    (step, values)
}

#[test]
fn a_macro_made_renders_the_same_with_the_same_key() {
    let before = state();
    let (step, values) = made();
    let after_values = moved(&values, &step.values);
    assert_eq!(render(&before, &values), render(&step.state, &after_values));

    let t = &step.state.bundle.template;
    let names: Vec<&str> = t.network.nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(
        names,
        ["warm_look", "lift", "side"],
        "in the first node's place"
    );
    let r = CompositeRef {
        id: "user.warm-look".into(),
        version: 1,
    };
    assert_eq!(t.network.nodes[0].composite.as_ref(), Some(&r));
    // One input (the source), two outputs (read by lift and by side).
    assert_eq!(step.answer["inputs"], json!(["input"]));
    assert_eq!(step.answer["outputs"], json!(["output", "output_2"]));
    // The changed values exposed, moved under the macro.
    assert_eq!(
        step.values,
        Moves::from([
            ("node/exp/exposure".into(), None),
            ("node/tone/contrast".into(), None),
            ("node/warm_look/exposure".into(), Some(Value::Float(1.25))),
            ("node/warm_look/contrast".into(), Some(Value::Float(2.5))),
        ])
    );
    // The file: in order, the workspace's values inside, in the bundle.
    let file = step
        .state
        .bundle
        .composites
        .iter()
        .find(|f| f.reference() == r)
        .unwrap();
    let inner: Vec<&str> = file.nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(
        inner,
        ["exp", "tone"],
        "each node reads only nodes before it"
    );
    assert_eq!(
        file.nodes[0].params.get("exposure"),
        Some(&Value::Float(0.5)),
        "the exposed default is the workspace's value"
    );
    // Its sub-workspace, position and preset follow it.
    assert_eq!(t.subworkspaces[0].nodes, ["warm_look"]);
    assert_eq!(t.positions.get("warm_look"), Some(&[10.0, 20.0]));
    assert!(!t.positions.contains_key("tone"));
    assert_eq!(
        t.network.presets[0].values["warm_look"].get("contrast"),
        Some(&Value::Float(2.0))
    );
    // A second one with the same name gets ids and names of its own.
    let again = step
        .state
        .macro_made(
            &catalogue(),
            &nodes(&["lift"]),
            "Warm look",
            None,
            &after_values,
        )
        .unwrap();
    assert_eq!(again.answer["node"], "warm_look_2");
    assert_eq!(again.answer["composite"]["id"], "user.warm-look-2");
}

#[test]
fn what_is_exposed_may_be_chosen_and_the_rest_is_the_macros() {
    let values = image_values();
    let expose = [Exposed {
        name: "strength".into(),
        node: "tone".into(),
        param: "contrast".into(),
        label: Some("Strength".into()),
        ..Exposed::default()
    }];
    let step = state()
        .macro_made(
            &catalogue(),
            &nodes(&["exp", "tone"]),
            "look",
            Some(&expose),
            &values,
        )
        .unwrap();
    let after_values = moved(&values, &step.values);
    assert_eq!(
        after_values.get("node/look/strength"),
        Some(&Value::Float(2.5))
    );
    assert!(!after_values.contains_key("node/look/exposure"));
    let file = step.state.bundle.composites.last().unwrap();
    assert_eq!(
        file.nodes[0].params.get("exposure"),
        Some(&Value::Float(1.25)),
        "an image value not exposed is the macro's now"
    );
    assert_eq!(
        render(&state(), &values),
        render(&step.state, &after_values)
    );
}

#[test]
fn a_macro_that_cannot_be_made_says_why() {
    let values = image_values();
    let refused = |names: &[&str], values: &Values| {
        state()
            .macro_made(&catalogue(), &nodes(names), "m", None, values)
            .unwrap_err()
    };
    assert!(refused(&[], &values).contains("none given"));
    assert!(refused(&["source"], &values).contains("is a source"));
    assert!(refused(&["nope"], &values).contains("no node `nope`"));
    assert!(refused(&["exp", "exp"], &values).contains("twice"));
    // `tone` reads `exp` and feeds `lift`: taking exp and lift leaves it
    // between them, a cycle once folded.
    let e = refused(&["exp", "lift"], &values);
    assert!(e.contains("`tone` reads the selection and feeds it"), "{e}");
    // exp and tone are in the sub-workspace, lift is not.
    let e = refused(&["tone", "lift"], &values);
    assert!(e.contains("cross the sub-workspace `look`"), "{e}");
    let mut bypassed = values.clone();
    bypassed.insert("bypass/tone".into(), Value::Bool(true));
    let e = refused(&["exp", "tone"], &bypassed);
    assert!(e.contains("`tone` is bypassed"), "{e}");
    let e = state()
        .macro_made(&catalogue(), &nodes(&["exp"]), " ", None, &values)
        .unwrap_err();
    assert!(e.contains("name"), "{e}");
}

#[test]
fn a_macro_opens_and_folds_without_rendering_anything_new() {
    let (made, values) = made();
    let values = moved(&values, &made.values);
    let opened = made
        .state
        .macro_opened(&catalogue(), "warm_look", &values)
        .unwrap();
    assert_eq!(
        opened.values,
        Moves::from([
            (
                "node/warm_look.exp/exposure".into(),
                Some(Value::Float(1.25))
            ),
            (
                "node/warm_look.tone/contrast".into(),
                Some(Value::Float(2.5))
            ),
        ]),
        "the exposed values onto the inner nodes showing them"
    );
    assert!(opened.state.bundle.template.network.nodes[0].open);
    // Opened: the inner nodes are in the graph; the exposed values are
    // the macro node's, so they are given in the workspace here.
    let mut opened_state = opened.state.clone();
    for (p, v) in [("exposure", 1.25), ("contrast", 2.5)] {
        opened_state.bundle.template.network.nodes[0]
            .params
            .set(p, Value::Float(v));
    }
    let lift = Values::from([("node/lift/exposure".into(), Value::Float(-0.5))]);
    assert_eq!(render(&made.state, &values), render(&opened_state, &lift));
    assert!(
        opened
            .state
            .macro_opened(&catalogue(), "warm_look", &values)
            .unwrap_err()
            .contains("already open")
    );
    // A change to an inner node while open is a change to the macro:
    // folding refuses it for now.
    let mut changed = values.clone();
    changed.insert(
        "node/warm_look.tone/white_target".into(),
        Value::Float(300.0),
    );
    let e = opened
        .state
        .macro_folded(&catalogue(), "warm_look", &changed)
        .unwrap_err();
    assert!(e.contains("changed while open"), "{e}");
    let folded = opened
        .state
        .macro_folded(&catalogue(), "warm_look", &values)
        .unwrap();
    assert_eq!(folded.state, made.state);
    // An exposed value changed on its inner node goes back to the macro.
    let mut moved = values.clone();
    moved.insert("node/warm_look.tone/contrast".into(), Value::Float(3.0));
    let folded = opened
        .state
        .macro_folded(&catalogue(), "warm_look", &moved)
        .unwrap();
    assert_eq!(
        folded.values.get("node/warm_look/contrast"),
        Some(&Some(Value::Float(3.0)))
    );
    // A module does not open; a bypassed macro does not either.
    assert!(
        made.state
            .macro_opened(&catalogue(), "lift", &values)
            .unwrap_err()
            .contains("not a macro")
    );
    let mut bypassed = values.clone();
    bypassed.insert("bypass/warm_look".into(), Value::Bool(true));
    assert!(
        made.state
            .macro_opened(&catalogue(), "warm_look", &bypassed)
            .unwrap_err()
            .contains("bypassed")
    );
}

#[test]
fn a_workspace_s_own_macro_is_added_and_wired_as_another_node() {
    let (made, _) = made();
    let (added, answer) = helios_template::network::edited(
        &made.state,
        &catalogue(),
        "network.add",
        &json!({ "nodes": [{ "composite": "user.warm-look@1" }] }),
    )
    .unwrap();
    assert_eq!(answer["added"], json!(["warm_look_2"]), "{answer}");
    let (wired, _) = helios_template::network::edited(
        &added,
        &catalogue(),
        "network.connect",
        &json!({ "from": "source", "port": "image", "to": "warm_look_2", "input": "input" }),
    )
    .unwrap();
    assert!(wired.problems(&catalogue()).is_empty());
}

#[test]
fn a_dissolved_macro_gives_back_its_nodes_and_renders_the_same() {
    let (made, values) = made();
    let values = moved(&values, &made.values);
    let step = made
        .state
        .macro_dissolved(&catalogue(), "warm_look", &values)
        .unwrap();
    let after = moved(&values, &step.values);
    assert_eq!(render(&made.state, &values), render(&step.state, &after));
    assert_eq!(
        render(&state(), &image_values()),
        render(&step.state, &after)
    );
    let t = &step.state.bundle.template;
    let names: Vec<&str> = t.network.nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(
        names,
        ["exp", "tone", "lift", "side"],
        "their names were free"
    );
    assert_eq!(after, image_values(), "the values back on the inner nodes");
    let sorted = |c: &[helios_template::format::Connection]| {
        let mut v: Vec<String> = c
            .iter()
            .map(|c| format!("{}.{} -> {}.{}", c.from, c.port, c.to, c.input))
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        sorted(&t.network.connections),
        sorted(&state().bundle.template.network.connections),
        "wired as before"
    );
    assert_eq!(t.subworkspaces[0].nodes, ["exp", "tone"]);
    // The composite file stays for other users.
    assert!(
        step.state
            .bundle
            .composites
            .iter()
            .any(|f| f.id == "user.warm-look")
    );
}

#[test]
fn an_upgraded_macro_carries_its_values_by_name() {
    let (made, values) = made();
    let values = moved(&values, &made.values);
    // Version 2: `exposure` narrowed to [0, 1], `contrast` no longer
    // exposed.
    let mut v2 = made.state.bundle.composites.last().unwrap().clone();
    v2.version = 2;
    v2.exposed = vec![Exposed {
        name: "exposure".into(),
        node: "exp".into(),
        param: "exposure".into(),
        min: Some(0.0),
        max: Some(1.0),
        ..Exposed::default()
    }];
    let cat = catalogue().composite(v2);
    let step = made
        .state
        .macro_upgraded(&cat, "warm_look", None, &values)
        .unwrap();
    assert_eq!(step.answer["dropped"], json!(["contrast"]));
    assert_eq!(step.answer["clamped"], json!(["exposure"]));
    assert_eq!(
        step.answer["preset_values_dropped"],
        json!(["bright: contrast"])
    );
    assert_eq!(
        step.values,
        Moves::from([
            ("node/warm_look/exposure".into(), Some(Value::Float(1.0))),
            ("node/warm_look/contrast".into(), None),
        ])
    );
    let node = &step.state.bundle.template.network.nodes[0];
    assert_eq!(node.composite.as_ref().unwrap().version, 2);
    assert!(
        step.state
            .bundle
            .composites
            .iter()
            .any(|f| f.id == "user.warm-look" && f.version == 2)
    );
    let e = step
        .state
        .macro_upgraded(&cat, "warm_look", None, &values)
        .unwrap_err();
    assert!(e.contains("newest"), "{e}");
}

/// The made macro's interface, `exposure` renamed `brightness` (its
/// range narrowed to `[min, max]`, in a group), `contrast` no longer
/// exposed and `white_target` exposed as `white`.
fn reshaped(min: f64, max: f64) -> Vec<Exposed> {
    vec![
        Exposed {
            name: "brightness".into(),
            node: "exp".into(),
            param: "exposure".into(),
            group: Some("Light".into()),
            min: Some(min),
            max: Some(max),
            ..Exposed::default()
        },
        Exposed {
            name: "white".into(),
            node: "tone".into(),
            param: "white_target".into(),
            group: Some("Light".into()),
            ..Exposed::default()
        },
    ]
}

#[test]
fn an_interface_edited_is_a_new_version_and_the_image_renders_the_same() {
    let (made, values) = made();
    let values = moved(&values, &made.values);
    let interface = made
        .state
        .macro_interface(&catalogue(), "warm_look")
        .unwrap();
    assert_eq!(interface.exposed.len(), 2);
    assert_eq!(interface.next.version, 2);
    assert!(
        interface
            .inner
            .iter()
            .any(|p| p.node == "tone" && p.spec.name == "white_target"),
        "every inner parameter is offered"
    );
    let step = made
        .state
        .macro_interfaced(&catalogue(), "warm_look", &reshaped(-2.0, 2.0), &values)
        .unwrap();
    assert_eq!(
        step.answer["to"],
        json!({ "id": "user.warm-look", "version": 2 })
    );
    assert_eq!(step.answer["renamed"], json!({ "exposure": "brightness" }));
    assert_eq!(step.answer["unexposed"], json!(["contrast"]));
    assert_eq!(step.answer["clamped"], json!([]));
    assert_eq!(
        step.values,
        Moves::from([
            ("node/warm_look/exposure".into(), None),
            ("node/warm_look/brightness".into(), Some(Value::Float(1.25))),
            ("node/warm_look/contrast".into(), None),
        ])
    );
    // Version 1 stays for its other users; the node is on version 2,
    // whose file holds the value no longer exposed.
    let files: Vec<u32> = step
        .state
        .bundle
        .composites
        .iter()
        .filter(|f| f.id == "user.warm-look")
        .map(|f| f.version)
        .collect();
    assert_eq!(files, [1, 2]);
    let v2 = step.state.bundle.composites.last().unwrap();
    let tone = v2.nodes.iter().find(|n| n.name == "tone").unwrap();
    assert_eq!(tone.params.get("contrast"), Some(&Value::Float(2.5)));
    let after = moved(&values, &step.values);
    assert_eq!(
        render(&made.state, &values),
        render(&step.state, &after),
        "the same image, the same content key"
    );
    // The group reaches the macro's hints.
    let cat = step.state.bundle.catalogue(&catalogue());
    let built = cat
        .build_composite(
            &CompositeRef {
                id: "user.warm-look".into(),
                version: 2,
            },
            0,
        )
        .unwrap();
    let hints = helios_core::Composite::param_hints(&built);
    assert!(
        hints.iter().all(|h| h.group.as_deref() == Some("Light")),
        "{hints:?}"
    );
    // Narrowed again: the value clamped, version 3.
    let again = step
        .state
        .macro_interfaced(&catalogue(), "warm_look", &reshaped(-1.0, 1.0), &after)
        .unwrap();
    assert_eq!(again.answer["to"]["version"], 3);
    assert_eq!(again.answer["clamped"], json!(["brightness"]));
    assert_eq!(
        again.values,
        Moves::from([("node/warm_look/brightness".into(), Some(Value::Float(1.0)))])
    );
}

#[test]
fn an_interface_that_does_not_fit_is_refused_with_its_reason() {
    let (made, values) = made();
    let values = moved(&values, &made.values);
    let file = made.state.bundle.composites.last().unwrap();
    let e = made
        .state
        .macro_interfaced(&catalogue(), "warm_look", &file.exposed, &values)
        .unwrap_err();
    assert!(e.contains("unchanged"), "{e}");
    let e = made
        .state
        .macro_interfaced(&catalogue(), "warm_look", &reshaped(-40.0, 2.0), &values)
        .unwrap_err();
    assert!(e.contains("exposed[0]"), "{e}");
    let opened = made
        .state
        .macro_opened(&catalogue(), "warm_look", &values)
        .unwrap();
    let e = opened
        .state
        .macro_interfaced(
            &catalogue(),
            "warm_look",
            &reshaped(-2.0, 2.0),
            &moved(&values, &opened.values),
        )
        .unwrap_err();
    assert!(e.contains("fold it first"), "{e}");
}
