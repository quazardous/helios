//! egui-pop's image tools in the desktop, driven by raw input events:
//! the histogram reads the pipeline's output and its drag moves the
//! exposure as one undo step.

use std::time::Duration;

use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_values::{Change, Set};
use serde_json::json;

const EV: &str = "node/exposure/exposure";

/// A grey ramp through an exposure, shown in the viewport.
fn shell() -> Shell {
    let (w, h) = (900u32, 600u32);
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(w, h), 4, |x, _, c| match c {
            0..=2 => 0.02 + 0.3 * x as f32 / w as f32,
            _ => 1.0,
        }),
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

/// Frames until the preview of the output has landed and is drawn.
fn settle(h: &mut Harness<'_, Shell>) {
    for _ in 0..4 {
        h.step();
        assert!(
            h.state().preview().wait(Duration::from_secs(30)),
            "the preview renders"
        );
    }
    h.step();
    h.step();
}

fn harness_on(panel: &str) -> Harness<'static, Shell> {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    h.step();
    h.state_mut()
        .run(&format!("{panel}.focus"), json!(null), Origin::User)
        .unwrap();
    settle(&mut h);
    h
}

fn f(h: &Harness<'_, Shell>, path: &str) -> f64 {
    let store = h.state().values.lock().unwrap();
    let v = &store.get(path).unwrap().value;
    v.as_f64().or_else(|| v.as_bool().map(f64::from)).unwrap()
}

fn steps(h: &Harness<'_, Shell>) -> usize {
    h.state().values.lock().unwrap().undo_steps().len()
}

fn rect(h: &Harness<'_, Shell>, target: &str) -> egui::Rect {
    h.state().target_rect(target).unwrap()
}

fn button(h: &mut Harness<'_, Shell>, at: Pos2, pressed: bool) {
    h.input_mut().events.push(Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

/// Presses at `from`, moves to `to` in a few frames, releases there.
fn drag(h: &mut Harness<'_, Shell>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.step();
    button(h, from, true);
    h.step();
    for i in 1..=5 {
        h.hover_at(from + (to - from) * (i as f32 / 5.0));
        h.step();
    }
    button(h, to, false);
    h.step();
    h.step();
}

/// The mean of an image's red channel.
fn mean_red(image: &Image) -> f64 {
    let n = image.channels() as usize;
    let px: Vec<f64> = image
        .data()
        .chunks_exact(n)
        .map(|p| f64::from(p[0]))
        .collect();
    px.iter().sum::<f64>() / px.len() as f64
}

#[test]
fn the_histogram_reads_the_pipeline_s_output() {
    let mut h = harness_on("helios.scopes");
    let before = h
        .state()
        .scope_image()
        .expect("the scopes measure the output");
    assert!(before.region().width <= helios_gui::preview::SIDE);
    // Two stops brighter: the scopes follow the edit.
    h.state()
        .values
        .lock()
        .unwrap()
        .set(Set::new(EV, Change::To(Value::Float(2.0)), Origin::User))
        .unwrap();
    settle(&mut h);
    let after = h.state().scope_image().unwrap();
    let ratio = mean_red(&after) / mean_red(&before);
    assert!((ratio - 4.0).abs() < 0.05, "{ratio}");
    // egui-pop's histogram, as a slider to assistive technology.
    assert!(rect(&h, "helios.scopes.picture").width() > 100.0);
}

#[test]
fn a_drag_on_the_histogram_moves_the_exposure_as_one_undo_step() {
    let mut h = harness_on("helios.scopes");
    let hist = rect(&h, "helios.scopes.picture");
    let (ev, before) = (f(&h, EV), steps(&h));
    let from = hist.center();
    drag(&mut h, from, from + vec2(hist.width() / 8.0, 0.0));
    let moved = f(&h, EV) - ev;
    // The stops scale spans 16 stops: an eighth of the width is two
    // (less what the pointer moves before a drag starts).
    assert!(moved > 1.5 && moved < 2.2, "{moved}");
    assert_eq!(steps(&h), before + 1, "one undo step");
    h.state_mut()
        .run("param.undo", json!(null), Origin::User)
        .unwrap();
    h.step();
    assert!((f(&h, EV) - ev).abs() < 1e-9);
}
