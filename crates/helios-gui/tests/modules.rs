//! The Modules panel and bypassing a module: `node.bypass` on the selected
//! module, Ctrl+/, the row's toggle; one undo step each, and the image
//! renders as if the module were absent.

use std::time::Duration;

use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_pop::model::ids::CommandId;
use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};
use helios_gui::modules::{self, COMMAND, KEY};
use helios_gui::params::SELECTION;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

const W: u32 = 64;
const H: u32 = 48;
const BYPASS: &str = "bypass/exp";

/// A flat grey through an exposure of +1 EV.
fn shell() -> Shell {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(W, H), 4, |_, _, c| match c {
            0..=2 => 0.2,
            _ => 1.0,
        }),
    );
    let exp = g
        .add_module(
            "exp",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new().with("exposure", Value::Float(1.0)),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, exp, "output").unwrap();
    shell
}

/// Steps frames until every tile is on screen and `done` holds.
fn until(h: &mut Harness<'_, Shell>, done: impl Fn(&Shell) -> bool) {
    for _ in 0..40 {
        h.step();
        let stats = h.state().viewport_stats();
        assert!(
            stats.wait_idle(Duration::from_mins(1)),
            "the render thread never answered: {stats:?}"
        );
        h.step();
        if h.state().viewport().state().tiles_complete() && done(h.state()) {
            return;
        }
    }
    panic!("the viewport never got there");
}

fn bypassed(s: &Shell) -> bool {
    s.values.lock().unwrap().get(BYPASS).unwrap().value == Value::Bool(true)
}

fn graph_bypassed(s: &Shell) -> bool {
    let v = s.viewport();
    let g = v.graph().unwrap();
    g.is_bypassed(g.find("exp").unwrap()).unwrap()
}

/// The red of the scene's centre, in the working space.
fn centre(s: &Shell) -> f32 {
    let scene = s.viewport_scene().unwrap();
    let r = scene.region();
    scene.pixel(r.x + i64::from(r.width) / 2, r.y + i64::from(r.height) / 2)[0]
}

fn select(h: &mut Harness<'_, Shell>, node: &str) {
    h.state()
        .values
        .lock()
        .unwrap()
        .set(helios_values::Set::new(
            SELECTION,
            helios_values::Change::To(Value::Text(node.into())),
            Origin::User,
        ))
        .unwrap();
}

#[test]
fn node_bypass_toggles_the_selected_module_as_one_undo_step() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    until(&mut h, |_| true);
    let lit = centre(h.state());
    assert!(lit > 0.3, "+1 EV brightens the grey: {lit}");

    // Nothing selected: the command says so.
    h.state_mut()
        .run(COMMAND, json!(null), Origin::User)
        .unwrap();
    h.step();
    assert!(!bypassed(h.state()));

    select(&mut h, "exp");
    h.state_mut()
        .run(COMMAND, json!(null), Origin::User)
        .unwrap();
    until(&mut h, |s| {
        graph_bypassed(s) && (centre(s) - 0.2).abs() < 0.02
    });
    assert!(bypassed(h.state()));
    // Its parameters stay.
    let ev = h
        .state()
        .values
        .lock()
        .unwrap()
        .get("node/exp/exposure")
        .unwrap()
        .value
        .clone();
    assert_eq!(ev, Value::Float(1.0));
    // One undo step, which brings it back.
    let label = h
        .state()
        .values
        .lock()
        .unwrap()
        .undo_steps()
        .last()
        .map(|s| s.label.clone());
    assert_eq!(label.as_deref(), Some("bypass exp"));
    h.state()
        .values
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    until(&mut h, |s| {
        !graph_bypassed(s) && (centre(s) - lit).abs() < 1e-4
    });

    // Named and set, by an agent.
    h.state_mut()
        .run(
            COMMAND,
            json!({ "node": "exp", "on": true }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    until(&mut h, graph_bypassed);
    h.state_mut()
        .run(
            COMMAND,
            json!({ "node": "exp", "on": false }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    until(&mut h, |s| !graph_bypassed(s));
    let last = h
        .state()
        .values
        .lock()
        .unwrap()
        .undo_steps()
        .last()
        .map(|s| s.label.clone());
    assert_eq!(last.as_deref(), Some("restore exp"));
}

#[test]
fn the_bypass_refuses_what_has_nothing_to_pass() {
    let mut store = helios_values::Store::new();
    let err = modules::bypass(&mut store, Some("source"), None, &Origin::User).unwrap_err();
    assert!(err.contains("cannot be bypassed"), "{err}");
    let err = modules::bypass(&mut store, None, None, &Origin::User).unwrap_err();
    assert!(err.contains("no module is selected"), "{err}");
}

#[test]
fn ctrl_slash_bypasses_the_selected_module() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    h.run();
    let keys = h
        .state()
        .app()
        .handle()
        .keys_for(&CommandId::from_static(COMMAND));
    assert!(keys.iter().any(|k| k == KEY), "{keys:?}");
    select(&mut h, "exp");
    h.key_press_modifiers(Modifiers::CTRL, Key::Slash);
    h.run();
    h.step();
    assert!(bypassed(h.state()), "Ctrl+/ bypasses it");
    h.key_press_modifiers(Modifiers::CTRL, Key::Slash);
    h.run();
    h.step();
    assert!(!bypassed(h.state()), "and brings it back");
}

#[test]
fn the_modules_panel_lists_the_modules_and_toggles_a_bypass() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    h.state_mut()
        .run("helios.modules.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
    let rows = {
        let s = h.state();
        let v = s.viewport();
        modules::rows(v.graph().unwrap(), &s.values.lock().unwrap())
    };
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].node, "exp");
    assert_eq!(rows[0].module, "exposure");
    assert_eq!(rows[0].bypassed, Some(false));

    h.get_by_label("exp").click();
    h.run();
    assert_eq!(
        h.state()
            .values
            .lock()
            .unwrap()
            .get(SELECTION)
            .unwrap()
            .value,
        Value::Text("exp".into())
    );
    h.get_by_label("Bypass").click();
    h.run();
    assert!(bypassed(h.state()), "the row's toggle bypasses it");
    h.get_by_label("Bypass").click();
    h.run();
    assert!(!bypassed(h.state()));
}
