//! The circle's, the ellipse's and the gradient's handles on the image, one by one: a
//! drag writes what the handle shows and is one undo step, Shift snaps
//! (15° for a rotation, the axes and diagonals for a centre), Ctrl moves
//! a tenth of the pointer, a double click resets (a feather to none, a
//! rotation to 0°), the wheel adjusts as one undo step, Escape cancels.
//!
//! The circle's and the gradient's were written against the hand-made
//! editor first, so the controls' handles are held to the same
//! behaviour; the ellipse's checks its case's handles once moved.

use egui::{Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2};
use egui::{TouchPhase, vec2};
use egui_kittest::Harness;
use egui_pop_test::double_click_at;
use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

const W: u32 = 1200;
const H: u32 = 900;

fn graph() -> (Graph, helios_core::NodeId) {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(W, H), 4, |x, y, c| match c {
            0..=2 => 0.05 + 0.3 * (x + y) as f32 / (W + H) as f32,
            _ => 1.0,
        }),
    );
    let exp = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new().with("exposure", Value::Float(1.0)),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    let mask = g
        .add_module(
            "mask",
            ModuleRef::new(helios_module_masks::MaskCombine),
            &Params::new(),
        )
        .unwrap();
    let blend = g
        .add_module(
            "blend",
            ModuleRef::new(helios_module_masks::MaskBlend),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", blend, "input").unwrap();
    g.connect(exp, "output", blend, "processed").unwrap();
    g.connect(mask, "mask", blend, "mask").unwrap();
    (g, blend)
}

/// A harness with one shape of `kind` added and selected.
fn with(kind: &str) -> Harness<'static, Shell> {
    let (g, blend) = graph();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, blend, "output").unwrap();
    egui_pop::api::palettes::set_hidden(shell.app().handle(), true);
    let mut h = headless::harness_at(shell, vec2(1280.0, 800.0), 1.0);
    h.step();
    h.step();
    h.state_mut()
        .run(&format!("mask.add.{kind}"), json!(null), Origin::User)
        .unwrap();
    h.step();
    h.step();
    h
}

fn f(h: &Harness<'_, Shell>, path: &str) -> f64 {
    h.state()
        .values()
        .lock()
        .unwrap()
        .get(path)
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

fn steps(h: &Harness<'_, Shell>) -> Vec<String> {
    let store = h.state().values().lock().unwrap();
    store.undo_steps().iter().map(|s| s.label.clone()).collect()
}

/// Image pixels per point.
fn per_point(h: &Harness<'_, Shell>) -> f64 {
    let m = h.state().viewport().state().mapping().unwrap();
    f64::from(h.state().pixels_per_point()) / m.zoom
}

/// Where the selected shape's handle `name` is on screen.
fn handle(h: &Harness<'_, Shell>, name: &str) -> Pos2 {
    let v = h.state().viewport();
    let o = &v.masks().overlays()[0];
    let member = v.masks().members().iter().find(|m| m.node == o.id).unwrap();
    let i = member
        .outline
        .handles
        .iter()
        .position(|x| x.name == name)
        .unwrap_or_else(|| panic!("no handle {name}"));
    let k = o.handles.iter().find(|k| k.index as usize == i).unwrap();
    v.state().mapping().unwrap().image_to_screen(k.at)
}

fn modifiers(h: &mut Harness<'_, Shell>, m: Modifiers) {
    h.input_mut().events.push(Event::ModifiersChanged(m));
}

fn button(h: &mut Harness<'_, Shell>, pos: Pos2, pressed: bool, m: Modifiers) {
    h.input_mut().events.push(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: m,
    });
}

/// Presses at `from` and moves to `to` with `m` held; the button stays
/// down.
fn drag_to(h: &mut Harness<'_, Shell>, from: Pos2, to: Pos2, m: Modifiers) {
    modifiers(h, m);
    h.hover_at(from);
    h.step();
    button(h, from, true, m);
    h.step();
    for i in 1..=5 {
        modifiers(h, m);
        h.hover_at(from + (to - from) * (i as f32 / 5.0));
        h.step();
    }
}

fn release(h: &mut Harness<'_, Shell>, at: Pos2) {
    button(h, at, false, Modifiers::NONE);
    h.step();
    modifiers(h, Modifiers::NONE);
    h.step();
}

/// A whole drag, released.
fn drag(h: &mut Harness<'_, Shell>, from: Pos2, to: Pos2, m: Modifiers) {
    drag_to(h, from, to, m);
    release(h, to);
}

fn undo(h: &mut Harness<'_, Shell>) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.step();
    h.step();
}

/// Wheel notches over `at`, then frames until the wheel's group closes
/// into an undo step.
fn wheel(h: &mut Harness<'_, Shell>, at: Pos2, notches: f32) {
    let before = steps(h).len();
    h.hover_at(at);
    h.step();
    h.input_mut().events.push(Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: vec2(0.0, notches),
        phase: TouchPhase::Move,
        modifiers: Modifiers::NONE,
        source: MouseWheelSource::Unknown,
    });
    for _ in 0..120 {
        h.step();
        if steps(h).len() > before && h.state().values().lock().unwrap().open_gesture().is_none() {
            break;
        }
    }
}

fn no_gesture_left(h: &Harness<'_, Shell>) {
    assert!(h.state().values().lock().unwrap().open_gesture().is_none());
}

/// Moves the pointer off the shapes, so the next hover starts clean.
fn away(h: &mut Harness<'_, Shell>) {
    let r = h.state().target_rect("viewport.display").unwrap();
    h.hover_at(r.left_top() + vec2(10.0, 10.0));
    h.step();
}

// ------------------------------------------------------------------ circle

#[test]
fn circle_centre_drags_snaps_to_the_axes_and_moves_finely() {
    let mut h = with("circle");
    let (x0, y0) = (f(&h, "node/circle/x"), f(&h, "node/circle/y"));
    let k = per_point(&h);
    let from = handle(&h, "centre");
    drag(&mut h, from, from + vec2(50.0, 20.0), Modifiers::NONE);
    assert!((f(&h, "node/circle/x") - (x0 + 50.0 * k)).abs() < 1.0);
    assert!((f(&h, "node/circle/y") - (y0 + 20.0 * k)).abs() < 1.0);
    assert_eq!(steps(&h), ["drag circle centre"]);
    undo(&mut h);
    assert_eq!((f(&h, "node/circle/x"), f(&h, "node/circle/y")), (x0, y0));
    // Shift: held to the nearest axis or diagonal (here the x axis).
    let from = handle(&h, "centre");
    drag(&mut h, from, from + vec2(60.0, 8.0), Modifiers::SHIFT);
    assert!((f(&h, "node/circle/y") - y0).abs() < 1e-6);
    assert!(f(&h, "node/circle/x") > x0 + 50.0 * k);
    undo(&mut h);
    // Ctrl: a tenth of the pointer.
    let from = handle(&h, "centre");
    drag(&mut h, from, from + vec2(100.0, 0.0), Modifiers::COMMAND);
    assert!((f(&h, "node/circle/x") - (x0 + 10.0 * k)).abs() < 0.5);
    no_gesture_left(&h);
}

#[test]
fn circle_radius_drags_wheels_and_has_no_reset() {
    let mut h = with("circle");
    let r0 = f(&h, "node/circle/radius");
    let k = per_point(&h);
    let from = handle(&h, "radius");
    drag(&mut h, from, from + vec2(40.0, 0.0), Modifiers::NONE);
    let r1 = f(&h, "node/circle/radius");
    assert!((r1 - (r0 + 40.0 * k)).abs() < 1.0, "{r0} → {r1}");
    assert_eq!(steps(&h), ["drag circle radius"]);
    undo(&mut h);
    assert_eq!(f(&h, "node/circle/radius"), r0);
    // A double click on it leaves it as it is.
    let at = handle(&h, "radius");
    double_click_at(&mut h, at);
    h.step();
    assert_eq!(f(&h, "node/circle/radius"), r0);
    away(&mut h);
    // The wheel: a ratio, one undo step however many notches.
    let at = handle(&h, "radius");
    wheel(&mut h, at, 2.0);
    let r2 = f(&h, "node/circle/radius");
    assert!(r2 > r0, "{r0} → {r2}");
    assert_eq!(steps(&h), ["adjust circle radius"]);
    undo(&mut h);
    assert_eq!(f(&h, "node/circle/radius"), r0);
    no_gesture_left(&h);
}

#[test]
fn circle_feather_drags_resets_wheels_and_escape_cancels() {
    let mut h = with("circle");
    let (r, f0) = (f(&h, "node/circle/radius"), f(&h, "node/circle/feather"));
    let k = per_point(&h);
    // The feather handle sits left of the centre, at radius + feather.
    let from = handle(&h, "feather");
    drag(&mut h, from, from + vec2(-30.0, 0.0), Modifiers::NONE);
    let f1 = f(&h, "node/circle/feather");
    assert!((f1 - (f0 + 30.0 * k)).abs() < 1.0, "{f0} → {f1}");
    assert_eq!(f(&h, "node/circle/radius"), r);
    assert_eq!(steps(&h), ["drag circle feather"]);
    // Inside the radius: no feather, never a negative one.
    let from = handle(&h, "feather");
    let centre = handle(&h, "centre");
    drag(&mut h, from, centre + vec2(5.0, 0.0), Modifiers::NONE);
    assert_eq!(f(&h, "node/circle/feather"), 0.0);
    undo(&mut h);
    assert!((f(&h, "node/circle/feather") - f1).abs() < 1e-9);
    // Escape mid-drag: back where it was, no undo step.
    let from = handle(&h, "feather");
    drag_to(&mut h, from, from + vec2(-50.0, 0.0), Modifiers::NONE);
    assert_ne!(f(&h, "node/circle/feather"), f1);
    h.key_press(Key::Escape);
    h.step();
    release(&mut h, from + vec2(-50.0, 0.0));
    assert!((f(&h, "node/circle/feather") - f1).abs() < 1e-9);
    assert_eq!(steps(&h), ["drag circle feather"]);
    // A double click: no feather, one undo step.
    let at = handle(&h, "feather");
    double_click_at(&mut h, at);
    h.step();
    assert_eq!(f(&h, "node/circle/feather"), 0.0);
    assert_eq!(steps(&h), ["drag circle feather", "reset circle feather"]);
    undo(&mut h);
    assert!((f(&h, "node/circle/feather") - f1).abs() < 1e-9);
    away(&mut h);
    // The wheel scales it.
    let at = handle(&h, "feather");
    wheel(&mut h, at, -2.0);
    assert!(f(&h, "node/circle/feather") < f1);
    assert_eq!(steps(&h), ["drag circle feather", "adjust circle feather"]);
    no_gesture_left(&h);
}

// ---------------------------------------------------------------- gradient

#[test]
fn gradient_anchor_drags_and_undoes() {
    let mut h = with("gradient");
    let (x0, y0) = (f(&h, "node/gradient/x"), f(&h, "node/gradient/y"));
    let k = per_point(&h);
    let from = handle(&h, "centre");
    drag(&mut h, from, from + vec2(-40.0, 30.0), Modifiers::NONE);
    assert!((f(&h, "node/gradient/x") - (x0 - 40.0 * k)).abs() < 1.0);
    assert!((f(&h, "node/gradient/y") - (y0 + 30.0 * k)).abs() < 1.0);
    assert_eq!(steps(&h), ["drag gradient centre"]);
    undo(&mut h);
    assert_eq!(
        (f(&h, "node/gradient/x"), f(&h, "node/gradient/y")),
        (x0, y0)
    );
    // Shift: along a diagonal.
    let from = handle(&h, "centre");
    drag(&mut h, from, from + vec2(50.0, 46.0), Modifiers::SHIFT);
    let (dx, dy) = (f(&h, "node/gradient/x") - x0, f(&h, "node/gradient/y") - y0);
    assert!((dx - dy).abs() < 1e-6 && dx > 0.0, "{dx}, {dy}");
    no_gesture_left(&h);
}

#[test]
fn gradient_rotation_drags_snaps_resets_and_wheels() {
    let mut h = with("gradient");
    // Dragged up from the anchor's right: the angle turns anticlockwise
    // on screen, a positive rotation.
    let from = handle(&h, "rotation");
    let c = handle(&h, "centre");
    drag(&mut h, from, from + vec2(0.0, -37.0), Modifiers::NONE);
    let free = f(&h, "node/gradient/rotation");
    let d = from + vec2(0.0, -37.0) - c;
    let want = f64::from((-d.y).atan2(d.x).to_degrees());
    assert!((free - want).abs() < 0.5, "{free} vs {want}");
    assert_eq!(steps(&h), ["drag gradient rotation"]);
    undo(&mut h);
    assert_eq!(f(&h, "node/gradient/rotation"), 0.0);
    // Shift: to 15° steps.
    let from = handle(&h, "rotation");
    drag(&mut h, from, from + vec2(-10.0, -37.0), Modifiers::SHIFT);
    let rot = f(&h, "node/gradient/rotation");
    assert!(
        rot != 0.0 && (rot / 15.0 - (rot / 15.0).round()).abs() < 1e-6,
        "{rot}"
    );
    // A double click: back to 0°.
    let at = handle(&h, "rotation");
    double_click_at(&mut h, at);
    h.step();
    assert_eq!(f(&h, "node/gradient/rotation"), 0.0);
    assert_eq!(
        steps(&h),
        ["drag gradient rotation", "reset gradient rotation"]
    );
    undo(&mut h);
    assert!((f(&h, "node/gradient/rotation") - rot).abs() < 1e-9);
    away(&mut h);
    // The wheel turns it, one undo step.
    let at = handle(&h, "rotation");
    wheel(&mut h, at, 1.0);
    let turned = f(&h, "node/gradient/rotation");
    assert!(turned > rot, "{rot} → {turned}");
    assert_eq!(
        steps(&h),
        ["drag gradient rotation", "adjust gradient rotation"]
    );
    no_gesture_left(&h);
}

#[test]
fn gradient_feather_drags_resets_wheels_and_moves_finely() {
    let mut h = with("gradient");
    let f0 = f(&h, "node/gradient/feather");
    let k = per_point(&h);
    // At rotation 0 the band runs across the image: its handle is above
    // the line by half the feather; dragged up, the band widens twice as
    // fast.
    let from = handle(&h, "feather");
    drag(&mut h, from, from + vec2(0.0, -20.0), Modifiers::NONE);
    let f1 = f(&h, "node/gradient/feather");
    assert!((f1 - (f0 + 40.0 * k)).abs() < 1.0, "{f0} → {f1}");
    assert_eq!(steps(&h), ["drag gradient feather"]);
    undo(&mut h);
    assert_eq!(f(&h, "node/gradient/feather"), f0);
    // Ctrl: a tenth of the pointer.
    let from = handle(&h, "feather");
    drag(&mut h, from, from + vec2(0.0, -100.0), Modifiers::COMMAND);
    let f2 = f(&h, "node/gradient/feather");
    assert!((f2 - (f0 + 20.0 * k)).abs() < 1.0, "{f0} → {f2}");
    // A double click: no band.
    let at = handle(&h, "feather");
    double_click_at(&mut h, at);
    h.step();
    assert_eq!(f(&h, "node/gradient/feather"), 0.0);
    undo(&mut h);
    assert!((f(&h, "node/gradient/feather") - f2).abs() < 1e-9);
    away(&mut h);
    let at = handle(&h, "feather");
    wheel(&mut h, at, 2.0);
    assert!(f(&h, "node/gradient/feather") > f2);
    assert_eq!(
        steps(&h),
        ["drag gradient feather", "adjust gradient feather"]
    );
    no_gesture_left(&h);
}

// ----------------------------------------------------------------- ellipse

#[test]
fn ellipse_radii_turn_it_and_its_feather_resets_and_wheels() {
    let mut h = with("ellipse");
    let ry = f(&h, "node/ellipse/radius_y");
    let k = per_point(&h);
    // The first radius dragged straight down: 90°, its length the
    // distance to the centre.
    let c = handle(&h, "centre");
    let from = handle(&h, "radius_x");
    let to = c + vec2(0.0, 80.0);
    drag(&mut h, from, to, Modifiers::NONE);
    assert!((f(&h, "node/ellipse/rotation") - 90.0).abs() < 0.5);
    assert!((f(&h, "node/ellipse/radius_x") - 80.0 * k).abs() < 1.0);
    assert_eq!(f(&h, "node/ellipse/radius_y"), ry);
    assert_eq!(steps(&h), ["drag ellipse radius_x"]);
    undo(&mut h);
    assert_eq!(f(&h, "node/ellipse/rotation"), 0.0);
    away(&mut h);
    // The wheel over the second radius scales it alone.
    let at = handle(&h, "radius_y");
    wheel(&mut h, at, 2.0);
    assert!(f(&h, "node/ellipse/radius_y") > ry);
    assert_eq!(f(&h, "node/ellipse/rotation"), 0.0);
    assert_eq!(steps(&h), ["adjust ellipse radius_y"]);
    // A double click on the feather: none.
    let at = handle(&h, "feather");
    double_click_at(&mut h, at);
    h.step();
    assert_eq!(f(&h, "node/ellipse/feather"), 0.0);
    assert_eq!(
        steps(&h),
        ["adjust ellipse radius_y", "reset ellipse feather"]
    );
    no_gesture_left(&h);
}
