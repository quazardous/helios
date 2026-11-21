//! A change to the values reaches the graph shown and the edit's marks
//! on `values.changed`: written while the panels draw, in the same pass,
//! after the drawing; written outside the main window's pass (an
//! application window, drawn after it), it waits for the main window's
//! next pass, asked for it, and is heard there before the drawing.

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
            ModuleRef::new(helios_modules::exposure::Exposure),
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

#[test]
fn a_value_written_on_another_thread_is_heard_at_the_next_pass() {
    let mut h = harness(shell());
    // As a camera look fitted off the frame writes it.
    let values = h.state().values().clone();
    std::thread::spawn(move || {
        values
            .lock()
            .unwrap()
            .set(helios_values::Set::new(
                EV,
                helios_values::Change::To(Value::Float(0.5)),
                Origin::User,
            ))
            .unwrap();
    })
    .join()
    .unwrap();
    assert_eq!(shown(h.state()), 0.0, "never heard on the writing thread");
    h.step();
    assert_eq!(shown(h.state()), 0.5);
}

/// Whether the edit's marks, as last passed through `edit.changed`, name
/// the exposure as edited: nothing read here.
fn marked(shell: &Shell) -> bool {
    shell.edit_marks().nodes.contains("exposure")
}

#[test]
fn the_edit_s_marks_follow_a_value_written_while_the_panels_draw_in_the_same_pass() {
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
    assert!(!marked(h.state()));
    armed.set(Some(1.5));
    h.step();
    assert_eq!(armed.get(), None, "the panel drew and wrote");
    assert!(
        marked(h.state()),
        "the marks follow in the pass the value was written in"
    );
    armed.set(Some(0.0));
    h.step();
    assert!(!marked(h.state()), "back to its default: unmarked");
}

#[test]
fn the_edit_s_marks_follow_a_value_written_outside_the_pass_or_on_another_thread_at_the_next() {
    let mut h = harness(shell());
    write(h.state(), 0.75);
    assert!(!marked(h.state()), "nothing is heard while drawing");
    h.step();
    assert!(marked(h.state()));
    let values = h.state().values().clone();
    std::thread::spawn(move || {
        values
            .lock()
            .unwrap()
            .set(helios_values::Set::new(
                EV,
                helios_values::Change::To(Value::Float(0.0)),
                Origin::User,
            ))
            .unwrap();
    })
    .join()
    .unwrap();
    assert!(marked(h.state()), "never heard on the writing thread");
    h.step();
    assert!(!marked(h.state()));
}

/// What the journal said of values changed since `before`.
fn said(shell: &Shell, before: egui_pop::api::host::Seq) -> Vec<serde_json::Value> {
    shell
        .app()
        .handle()
        .events_since(before)
        .into_iter()
        .filter(|e| e.name == helios_gui::values_changed::PARAM_CHANGED)
        .map(|e| e.data)
        .collect()
}

#[test]
fn a_slider_drag_is_said_once_on_the_journal_when_it_ends() {
    let mut h = harness(shell());
    let before = h.state().app().handle().last_event();
    let slider = h.state().target_rect(&format!("param:{EV}")).unwrap();
    let from = slider.center();
    h.hover_at(from);
    h.step();
    h.drag_at(from);
    h.step();
    let mut moved = Vec::new();
    for k in 1..=8 {
        h.hover_at(from + vec2(6.0 * k as f32, 0.0));
        h.step();
        moved.push(shown(h.state()));
    }
    assert!(
        moved.windows(2).any(|w| w[0] != w[1]),
        "the graph follows the drag frame by frame: {moved:?}"
    );
    assert!(
        said(h.state(), before).is_empty(),
        "nothing said while the drag goes on"
    );
    h.drop_at(from + vec2(48.0, 0.0));
    for _ in 0..3 {
        h.step();
    }
    let said = said(h.state(), before);
    assert_eq!(said.len(), 1, "one change for the whole drag: {said:?}");
    assert_eq!(said[0]["paths"], json!([EV]));
    assert_eq!(said[0]["step"], true);
    assert_eq!(said[0]["origin"]["kind"], "user");
    assert_eq!(said[0]["surface"], helios_gui::values_changed::MAIN_SURFACE);
    let (first, last) = (
        said[0]["seq"][0].as_u64().unwrap(),
        said[0]["seq"][1].as_u64().unwrap(),
    );
    assert!(first < last, "the drag's frames gathered: {said:?}");
    assert_eq!(
        h.state().values().lock().unwrap().undo_steps().len(),
        1,
        "one undo step"
    );
}

#[test]
fn a_write_and_its_undo_are_said_once_each() {
    let mut h = harness(shell());
    let before = h.state().app().handle().last_event();
    write(h.state(), 0.5);
    h.step();
    h.state()
        .values()
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    h.step();
    let said = said(h.state(), before);
    assert_eq!(said.len(), 2, "{said:?}");
    for s in &said {
        assert_eq!(s["paths"], json!([EV]));
        assert_eq!(s["step"], true);
    }
    assert_eq!(shown(h.state()), 0.0, "undone");
}
