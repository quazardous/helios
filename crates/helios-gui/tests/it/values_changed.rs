//! A change to the values reaches the graph shown on `values.changed`:
//! written while the panels draw, in the same pass, after the drawing;
//! written outside the main window's pass (an application window, drawn
//! after it), it waits for the main window's next pass, asked for it.

use std::cell::Cell;
use std::rc::Rc;

use egui::vec2;
use egui_kittest::Harness;
use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

const EV: &str = "node/exposure/exposure";

/// An exposure over a grey source, shown, its parameters values.
fn shell() -> Shell {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(32, 24), 4, |_, _, _| 0.2),
    );
    let exp = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, exp, "output").unwrap();
    shell
}

fn harness(shell: Shell) -> Harness<'static, Shell> {
    let mut h = headless::harness(shell, vec2(1024.0, 700.0));
    h.state_mut()
        .run("helios.params.focus", json!(null), Origin::User)
        .unwrap();
    for _ in 0..3 {
        h.step();
    }
    h
}

fn write(shell: &Shell, ev: f64) {
    shell
        .values()
        .lock()
        .unwrap()
        .set(helios_values::Set::new(
            EV,
            helios_values::Change::To(Value::Float(ev)),
            Origin::User,
        ))
        .unwrap();
}

/// The exposure the graph shown holds, as it is: nothing synced here.
fn shown(shell: &Shell) -> f64 {
    let viewport = shell.viewport();
    let g = viewport.graph().unwrap();
    let id = g.find("exposure").unwrap();
    g.params(id)
        .unwrap()
        .unwrap()
        .get("exposure")
        .unwrap()
        .as_f64()
        .unwrap()
}

#[test]
fn a_value_written_while_the_panels_draw_reaches_the_graph_in_the_same_pass() {
    let mut shell = shell();
    let armed = Rc::new(Cell::new(None::<f64>));
    let (values, arm) = (shell.values().clone(), armed.clone());
    shell.set_content("helios.params", move |_ui| {
        if let Some(ev) = arm.take() {
            values
                .lock()
                .unwrap()
                .set(helios_values::Set::new(
                    EV,
                    helios_values::Change::To(Value::Float(ev)),
                    Origin::User,
                ))
                .unwrap();
        }
    });
    let mut h = harness(shell);
    assert_eq!(shown(h.state()), 0.0);
    armed.set(Some(1.5));
    h.step();
    assert_eq!(armed.get(), None, "the panel drew and wrote");
    assert_eq!(
        shown(h.state()),
        1.5,
        "the graph follows in the pass the value was written in"
    );
}

#[test]
fn a_value_written_outside_the_main_pass_waits_for_the_next_one_asked_for_it() {
    let mut h = harness(shell());
    // Written after the main window's pass, as a window drawn apart does
    // (the calibration window): asked for the main window's next pass.
    write(h.state(), 0.75);
    assert_eq!(shown(h.state()), 0.0, "nothing is heard while drawing");
    assert!(
        h.state().app().handle().asks_waiting() > 0,
        "the change waits for the main window's next pass, the bus wakes it"
    );
    h.step();
    assert_eq!(shown(h.state()), 0.75);
}

#[test]
fn a_drag_s_changes_made_between_two_passes_are_heard_in_the_next_one() {
    let mut h = harness(shell());
    let waiting = h.state().app().handle().asks_waiting();
    for ev in [0.25, 0.5, 1.0] {
        write(h.state(), ev);
    }
    assert_eq!(
        h.state().app().handle().asks_waiting(),
        waiting + 3,
        "one change per write"
    );
    h.step();
    assert_eq!(shown(h.state()), 1.0, "the graph holds the last value");
}
