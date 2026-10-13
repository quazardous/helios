//! egui-pop's image tools in the desktop, driven by raw input events:
//! the histogram reads the pipeline's output and its drag moves the
//! exposure, the navigator moves the viewport, a curve and a colour wheel
//! edit their parameters; each edit is one undo step.

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
    harness_with(shell(), panel)
}

fn harness_with(shell: Shell, panel: &str) -> Harness<'static, Shell> {
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
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

#[test]
fn the_navigator_moves_the_viewport() {
    use helios_gui::viewport::{CENTER_X, FIT};
    let mut h = harness_on("helios.navigator");
    for zoom in ["viewport.zoom.100", "viewport.zoom.in"] {
        h.state_mut().run(zoom, json!(null), Origin::User).unwrap();
        settle(&mut h);
    }
    assert!(f(&h, FIT) < 0.5, "zoomed in");
    let (x, before) = (f(&h, CENTER_X), steps(&h));
    // A click near the image's left edge in the navigator centres the
    // view there.
    let map = rect(&h, "helios.navigator.map");
    let at = map.left_center() + vec2(8.0, 0.0);
    h.hover_at(at);
    h.step();
    button(&mut h, at, true);
    h.step();
    button(&mut h, at, false);
    h.step();
    h.step();
    let moved = f(&h, CENTER_X);
    assert!(moved < x - 20.0, "{x} -> {moved}");
    // The viewport follows: what it shows moved left.
    let shown = h.state().viewport().state().mapping().unwrap().center[0];
    assert!((shown - moved).abs() < 1.0, "{shown} {moved}");
    // Where one looks is not an edit.
    assert_eq!(steps(&h), before);
    // The zoom control's fit fits.
    h.state_mut()
        .run("viewport.zoom.fit", json!(null), Origin::User)
        .unwrap();
    settle(&mut h);
    assert!(f(&h, FIT) > 0.5);
}

/// A curve and a colour wheel declared from module parameters' kinds and
/// hints, beside the exposure, the Parameters panel shown.
fn tools_harness() -> Harness<'static, Shell> {
    use helios_core::{ParamHint, ParamKind, ParamSpec, ParamWidget};
    use helios_values::Descriptor;
    let shell = shell();
    {
        let mut store = shell.values.lock().unwrap();
        let curve = ParamSpec {
            name: "curve".into(),
            description: "the tone curve".into(),
            kind: ParamKind::Floats {
                count: 16,
                min: 0.0,
                max: 1.0,
                default: vec![0.0, 0.0, 1.0, 1.0],
            },
        };
        let hint = ParamHint::new("curve").widget(ParamWidget::Curve);
        store.declare(CURVE, Descriptor::from_spec(&curve, Some(&hint)));
        let lift = ParamSpec {
            name: "lift".into(),
            description: "the shadows' colour".into(),
            kind: ParamKind::Floats {
                count: 3,
                min: -1.0,
                max: 1.0,
                default: vec![0.0, 0.0, 0.0],
            },
        };
        let hint = ParamHint::new("lift").widget(ParamWidget::Wheel);
        store.declare(LIFT, Descriptor::from_spec(&lift, Some(&hint)));
        store
            .set(Set::new(
                helios_gui::params::SELECTION,
                Change::To(Value::Text("grade".into())),
                Origin::User,
            ))
            .unwrap();
    }
    harness_with(shell, "helios.params")
}

const CURVE: &str = "node/grade/curve";
const LIFT: &str = "node/grade/lift";

fn list(h: &Harness<'_, Shell>, path: &str) -> Vec<f64> {
    match &h.state().values.lock().unwrap().get(path).unwrap().value {
        Value::List(l) => l.clone(),
        v => panic!("{path}: {v:?}"),
    }
}

#[test]
fn a_curve_edit_is_one_undo_step_and_changes_the_parameter() {
    let mut h = tools_harness();
    let r = rect(&h, &format!("param:{CURVE}"));
    let before = steps(&h);
    // A press on the identity line adds a point there; the drag lifts it.
    let at = r.center();
    drag(&mut h, at, at - vec2(0.0, r.height() / 5.0));
    let points = list(&h, CURVE);
    assert_eq!(points.len(), 6, "a point added: {points:?}");
    let (x, y) = (points[2], points[3]);
    assert!((x - 0.5).abs() < 0.1, "{points:?}");
    assert!(y > x + 0.1, "lifted: {points:?}");
    assert_eq!(steps(&h), before + 1, "one undo step");
    h.state_mut()
        .run("param.undo", json!(null), Origin::User)
        .unwrap();
    h.step();
    assert_eq!(list(&h, CURVE), vec![0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn a_wheel_drag_is_one_undo_step_and_moves_the_colour() {
    let mut h = tools_harness();
    let r = rect(&h, &format!("param:{LIFT}"));
    let before = steps(&h);
    let at = r.center();
    drag(&mut h, at, at + vec2(r.width() / 4.0, 0.0));
    let lift = list(&h, LIFT);
    assert_eq!(lift.len(), 3);
    assert!(lift[1] > 0.1, "some chroma: {lift:?}");
    assert_eq!(steps(&h), before + 1, "one undo step");
}
