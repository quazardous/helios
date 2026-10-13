//! Drawn masks on the viewport: shapes created by commands, keys and
//! clicks, their handles dragged as one undo step, cancelled by Escape,
//! snapped by Shift and Ctrl, path points added and removed (not on a
//! locked path), and the handles where `outline()` says at every zoom and
//! scale.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_pop::viewport::overlay::Overlay;
use egui_pop_test::{at_scales, double_click_at};
use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};
use helios_gui::masks::{self, POINT, SHAPE, TOOL, distance_to};
use helios_gui::{Shell, headless, helios_panels};
use helios_module_masks::shape::path_points;
use helios_shell::commands::Origin;
use serde_json::json;

const W: u32 = 1200;
const H: u32 = 900;

/// A ramp, an exposure of one stop through a mask with no shape yet.
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

fn shell() -> Shell {
    let (g, blend) = graph();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, blend, "output").unwrap();
    shell
}

fn harness_at(scale: f32) -> Harness<'static, Shell> {
    let mut h = headless::harness_at(shell(), vec2(1280.0, 800.0), scale);
    h.step();
    h.step();
    h
}

fn run(h: &mut Harness<'_, Shell>, command: &str, args: serde_json::Value) {
    h.state_mut().run(command, args, Origin::User).unwrap();
    h.step();
    h.step();
}

fn f(h: &Harness<'_, Shell>, path: &str) -> f64 {
    h.state()
        .values
        .lock()
        .unwrap()
        .get(path)
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

fn text(h: &Harness<'_, Shell>, path: &str) -> String {
    match &h.state().values.lock().unwrap().get(path).unwrap().value {
        Value::Text(t) | Value::Choice(t) => t.clone(),
        v => panic!("{path}: {v:?}"),
    }
}

fn steps(h: &Harness<'_, Shell>) -> Vec<String> {
    let store = h.state().values.lock().unwrap();
    store.undo_steps().iter().map(|s| s.label.clone()).collect()
}

fn path_len(h: &Harness<'_, Shell>, node: &str) -> usize {
    let store = h.state().values.lock().unwrap();
    let v = store.get(&format!("node/{node}/points")).unwrap();
    path_points(v.value.as_list().unwrap()).unwrap().len()
}

fn mapping(h: &Harness<'_, Shell>) -> egui_pop::viewport::Mapping {
    h.state().viewport().state().mapping().unwrap()
}

fn overlay(h: &Harness<'_, Shell>) -> Overlay {
    h.state().viewport().masks().overlays()[0].clone()
}

/// Where the handle named `name` of the selected shape is on screen.
fn handle(h: &Harness<'_, Shell>, name: &str) -> Pos2 {
    let v = h.state().viewport();
    let member = &v
        .masks()
        .members()
        .iter()
        .find(|m| m.node == v.masks().overlays()[0].id)
        .unwrap()
        .clone();
    let i = member
        .outline
        .handles
        .iter()
        .position(|x| x.name == name)
        .unwrap_or_else(|| panic!("no handle {name}"));
    let o = &v.masks().overlays()[0];
    let k = o.handles.iter().find(|k| k.index as usize == i).unwrap();
    v.state().mapping().unwrap().image_to_screen(k.at)
}

fn button(h: &mut Harness<'_, Shell>, pos: Pos2, pressed: bool, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    });
}

/// Presses at `from`, moves to `to` in a few steps; leaves the button
/// down.
fn drag_to(h: &mut Harness<'_, Shell>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.input_mut()
        .events
        .push(Event::ModifiersChanged(modifiers));
    h.hover_at(from);
    h.step();
    button(h, from, true, modifiers);
    h.step();
    for i in 1..=5 {
        h.input_mut()
            .events
            .push(Event::ModifiersChanged(modifiers));
        h.hover_at(from + (to - from) * (i as f32 / 5.0));
        h.step();
    }
}

fn release(h: &mut Harness<'_, Shell>, at: Pos2) {
    button(h, at, false, Modifiers::NONE);
    h.step();
    h.input_mut()
        .events
        .push(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn click(h: &mut Harness<'_, Shell>, at: Pos2) {
    h.hover_at(at);
    h.step();
    button(h, at, true, Modifiers::NONE);
    button(h, at, false, Modifiers::NONE);
    h.step();
    h.step();
}

#[test]
fn each_shape_kind_is_created_in_the_mask_at_the_view_s_centre() {
    let mut h = harness_at(1.0);
    for (kind, node) in [
        ("circle", "circle"),
        ("ellipse", "ellipse"),
        ("gradient", "gradient"),
    ] {
        run(&mut h, &format!("mask.add.{kind}"), json!(null));
        assert_eq!(text(&h, SHAPE), node);
        // Wired into the mask's next port, its values in the store.
        let v = h.state().viewport();
        let g = v.graph().unwrap();
        let mask = g.find("mask").unwrap();
        let wired: Vec<_> = g.input_nodes(mask).unwrap().into_iter().flatten().collect();
        assert!(wired.contains(&g.find(node).unwrap()), "{kind}");
        drop(v);
        // At the centre of the view (fitted: the image's).
        assert!((f(&h, &format!("node/{node}/x")) - f64::from(W) / 2.0).abs() < 1.0);
        assert!((f(&h, &format!("node/{node}/y")) - f64::from(H) / 2.0).abs() < 1.0);
    }
    assert!((f(&h, "node/circle/radius") - f64::from(H) / 6.0).abs() < 1.0);
    // A path by clicks: three points, then the first closes it.
    run(&mut h, "mask.add.path", json!(null));
    assert_eq!(text(&h, TOOL), "path");
    let m = mapping(&h);
    let corners = [[300.0, 200.0], [700.0, 250.0], [500.0, 600.0]];
    for c in corners {
        click(&mut h, m.image_to_screen(c));
    }
    assert_eq!(masks::draft(&h.state().values.lock().unwrap()).len(), 3);
    click(&mut h, m.image_to_screen(corners[0]));
    assert_eq!(text(&h, TOOL), "none");
    assert_eq!(text(&h, SHAPE), "path");
    assert_eq!(path_len(&h, "path"), 3);
    // An agent's path, made at once.
    run(
        &mut h,
        "mask.add.path",
        json!({ "corners": [[10, 10], [100, 10], [50, 90]], "params": { "feather": 6 } }),
    );
    assert_eq!(text(&h, SHAPE), "path2");
    assert_eq!(f(&h, "node/path2/feather"), 6.0);
    // Selecting and creating are no undo steps.
    assert!(steps(&h).is_empty(), "{:?}", steps(&h));
}

#[test]
fn the_shape_keys_bind_while_the_image_has_the_focus() {
    let mut h = harness_at(1.0);
    let c = h.state().target_rect("viewport.display").unwrap().center();
    click(&mut h, c + vec2(-300.0, 0.0));
    h.key_press(Key::C);
    h.step();
    h.step();
    assert_eq!(text(&h, SHAPE), "circle");
    // Enter and Escape bind only while a path is drawn.
    h.key_press(Key::P);
    h.step();
    h.step();
    assert_eq!(text(&h, TOOL), "path");
    h.key_press(Key::Escape);
    h.step();
    h.step();
    assert_eq!(text(&h, TOOL), "none");
    egui_pop_test::assert_keymap_clean(h.state().app());
}

#[test]
fn a_drag_is_one_undo_step_and_ctrl_z_takes_it_back() {
    let mut h = harness_at(1.0);
    run(&mut h, "mask.add.circle", json!(null));
    let r0 = f(&h, "node/circle/radius");
    let from = handle(&h, "radius");
    drag_to(&mut h, from, from + vec2(40.0, 0.0), Modifiers::NONE);
    // Held: an agent's write is refused meanwhile.
    let refused = h.state_mut().run(
        "param.set",
        json!({ "path": "node/circle/radius", "value": 5.0 }),
        Origin::Agent("a1".into()),
    );
    assert!(refused.is_err());
    release(&mut h, from + vec2(40.0, 0.0));
    let zoom = mapping(&h).zoom / f64::from(h.state().pixels_per_point());
    let r1 = f(&h, "node/circle/radius");
    assert!((r1 - (r0 + 40.0 / zoom)).abs() < 1.0, "{r0} → {r1}");
    assert_eq!(steps(&h), ["drag circle radius"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.step();
    h.step();
    assert_eq!(f(&h, "node/circle/radius"), r0);
}

#[test]
fn escape_cancels_a_drag_and_leaves_no_undo_step() {
    let mut h = harness_at(1.0);
    run(&mut h, "mask.add.ellipse", json!(null));
    let (x, y) = (f(&h, "node/ellipse/x"), f(&h, "node/ellipse/y"));
    let from = handle(&h, "centre");
    drag_to(&mut h, from, from + vec2(60.0, 30.0), Modifiers::NONE);
    assert_ne!(f(&h, "node/ellipse/x"), x);
    h.key_press(Key::Escape);
    h.step();
    release(&mut h, from + vec2(60.0, 30.0));
    assert_eq!((f(&h, "node/ellipse/x"), f(&h, "node/ellipse/y")), (x, y));
    assert!(steps(&h).is_empty(), "{:?}", steps(&h));
    let store = h.state().values.lock().unwrap();
    assert!(store.open_gesture().is_none());
}

#[test]
fn shift_snaps_the_angle_and_ctrl_moves_finely() {
    let mut h = harness_at(1.0);
    run(&mut h, "mask.add.ellipse", json!(null));
    let from = handle(&h, "radius_x");
    drag_to(&mut h, from, from + vec2(-10.0, 37.0), Modifiers::SHIFT);
    release(&mut h, from + vec2(-10.0, 37.0));
    let rot = f(&h, "node/ellipse/rotation");
    assert!(
        rot != 0.0 && (rot / 15.0 - (rot / 15.0).round()).abs() < 1e-6,
        "{rot}"
    );

    let x0 = f(&h, "node/ellipse/x");
    let from = handle(&h, "centre");
    drag_to(&mut h, from, from + vec2(100.0, 0.0), Modifiers::COMMAND);
    release(&mut h, from + vec2(100.0, 0.0));
    let zoom = mapping(&h).zoom / f64::from(h.state().pixels_per_point());
    let moved = f(&h, "node/ellipse/x") - x0;
    assert!((moved - 10.0 / zoom).abs() < 0.5, "{moved}");
}

/// Hovers `at` and lets the status line follow (it says what the frame
/// before found under the pointer).
fn hover(h: &mut Harness<'_, Shell>, at: Pos2) {
    h.hover_at(at);
    for _ in 0..3 {
        h.step();
    }
}

#[test]
fn a_hovered_handle_says_its_gestures_in_the_status_line() {
    let mut h = harness_at(1.0);
    run(&mut h, "mask.add.ellipse", json!(null));
    // A radius turns the ellipse: Shift snaps the angle; the wheel
    // adjusts it; a double click resets nothing.
    let radius = "Shift: snap the angle to 15° steps · Ctrl: fine moves · wheel: adjust";
    let at = handle(&h, "radius_x");
    hover(&mut h, at);
    assert_eq!(h.state().status_text().as_deref(), Some(radius));
    h.get_by_label(radius);
    // The feather: reset by a double click.
    let at = handle(&h, "feather");
    hover(&mut h, at);
    assert_eq!(
        h.state().status_text().as_deref(),
        Some("Ctrl: fine moves · double-click: reset · wheel: adjust")
    );
    // The centre moves along the axes with Shift.
    let at = handle(&h, "centre");
    hover(&mut h, at);
    assert_eq!(
        h.state().status_text().as_deref(),
        Some("Shift: along the axes and diagonals · Ctrl: fine moves")
    );
    // Away from the handles, the hint goes.
    let away = h
        .state()
        .target_rect("viewport.display")
        .unwrap()
        .left_top()
        + vec2(5.0, 5.0);
    hover(&mut h, away);
    assert!(h.state().viewport().masks().hovered().is_none());
    assert_ne!(h.state().status_text().as_deref(), Some(radius));
    assert!(h.query_by_label(radius).is_none());
    // In the interface's language.
    let app = h.state().app().handle().clone();
    egui_pop::i18n::plugin::set_language(&app, "fr").unwrap();
    let at = handle(&h, "radius_x");
    hover(&mut h, at);
    assert_eq!(
        h.state().status_text().as_deref(),
        Some("Maj : aligner l’angle par pas de 15° · Ctrl : déplacement fin · molette : ajuster")
    );
}

#[test]
fn path_points_are_added_on_the_edge_and_removed_with_delete() {
    let mut h = harness_at(1.0);
    run(
        &mut h,
        "mask.add.path",
        json!({ "corners": [[300, 200], [800, 200], [550, 650]] }),
    );
    let m = mapping(&h);
    // A double click on the edge adds a point there, the curve kept.
    double_click_at(&mut h, m.image_to_screen([550.0, 200.0]));
    h.step();
    assert_eq!(path_len(&h, "path"), 4);
    assert_eq!(f(&h, POINT), 1.0);
    assert_eq!(steps(&h), ["add a point to path"]);
    // The view did not fit again (the double click was the edge's).
    // Delete removes the selected point.
    h.key_press(Key::Delete);
    h.step();
    h.step();
    assert_eq!(path_len(&h, "path"), 3);
    assert_eq!(f(&h, POINT), -1.0);
    // A click on a corner selects it; a path keeps three points.
    click(&mut h, m.image_to_screen([800.0, 200.0]));
    assert_eq!(f(&h, POINT), 1.0);
    let refused = h
        .state_mut()
        .run("mask.point.remove", json!(null), Origin::User);
    h.step();
    h.step();
    assert!(refused.is_ok(), "queued");
    assert_eq!(path_len(&h, "path"), 3);
    // Agents add and remove points by commands.
    run(&mut h, "mask.point.add", json!({ "x": 420, "y": 420 }));
    assert_eq!(path_len(&h, "path"), 4);
    run(&mut h, "mask.point.remove", json!({ "point": 0 }));
    assert_eq!(path_len(&h, "path"), 3);
    // Ctrl+Z walks the four point edits back.
    for _ in 0..4 {
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.step();
        h.step();
    }
    assert_eq!(path_len(&h, "path"), 3);
    assert_eq!(steps(&h), Vec::<String>::new());
}

#[test]
fn a_locked_path_keeps_its_points() {
    let mut h = harness_at(1.0);
    run(
        &mut h,
        "mask.add.path",
        json!({ "corners": [[300, 200], [800, 200], [800, 650], [300, 650]] }),
    );
    run(
        &mut h,
        "mask.member.lock",
        json!({ "member": "path", "locked": true }),
    );
    assert_eq!(steps(&h), ["lock path"]);
    // A double click on its edge adds no point.
    let m = mapping(&h);
    double_click_at(&mut h, m.image_to_screen([550.0, 200.0]));
    h.step();
    assert_eq!(path_len(&h, "path"), 4);
    // Nor do commands add, remove or smooth one.
    run(&mut h, "mask.point.add", json!({ "x": 550, "y": 200 }));
    run(&mut h, "mask.point.remove", json!({ "point": 0 }));
    run(&mut h, "mask.point.smooth", json!({ "point": 0 }));
    assert_eq!(path_len(&h, "path"), 4);
    assert_eq!(steps(&h), ["lock path"]);
    // Unlocked, it takes points again.
    run(
        &mut h,
        "mask.member.lock",
        json!({ "member": "path", "locked": false }),
    );
    run(&mut h, "mask.point.add", json!({ "x": 550, "y": 200 }));
    assert_eq!(path_len(&h, "path"), 5);
}

#[test]
fn a_corner_drag_moves_its_handles_and_a_smooth_point_shows_them() {
    let mut h = harness_at(1.0);
    run(
        &mut h,
        "mask.add.path",
        json!({ "corners": [[300, 200], [800, 200], [550, 650]] }),
    );
    run(&mut h, "mask.select", json!({ "point": 1 }));
    // Sharp: the Bézier handles sit on the corner and are not shown.
    assert_eq!(overlay(&h).handles.len(), 3);
    run(&mut h, "mask.point.smooth", json!(null));
    assert_eq!(overlay(&h).handles.len(), 5);
    let from = handle(&h, "after.1");
    drag_to(&mut h, from, from + vec2(0.0, 30.0), Modifiers::NONE);
    release(&mut h, from + vec2(0.0, 30.0));
    let store = h.state().values.lock().unwrap();
    let p = path_points(
        store
            .get("node/path/points")
            .unwrap()
            .value
            .as_list()
            .unwrap(),
    )
    .unwrap();
    // The other handle mirrors it around the corner.
    let mid = [
        f64::midpoint(p[1].before[0], p[1].after[0]),
        f64::midpoint(p[1].before[1], p[1].after[1]),
    ];
    assert!(
        (mid[0] - 800.0).abs() < 1e-6 && (mid[1] - 200.0).abs() < 1e-6,
        "{p:?}"
    );
}

/// Every handle shown is on the shape's edge or its feather's outline,
/// or is its centre.
fn handles_on_outline(h: &Harness<'_, Shell>) {
    let v = h.state().viewport();
    let m = v.state().mapping().unwrap();
    for o in v.masks().overlays() {
        let member = v.masks().members().iter().find(|x| x.node == o.id).unwrap();
        for k in &o.handles {
            let name = &member.outline.handles[k.index as usize].name;
            let p = m.image_to_screen(k.at);
            let ol = &member.outline;
            let on_edge = distance_to(&ol.edge, ol.closed, &m, p);
            let on_feather = if ol.closed {
                distance_to(&ol.feather, true, &m, p)
            } else {
                ol.feather
                    .chunks(2)
                    .map(|s| distance_to(s, false, &m, p))
                    .fold(f32::INFINITY, f32::min)
            };
            let ok = name == "centre"
                || name.starts_with("before")
                || name.starts_with("after")
                || on_edge < 0.5
                || on_feather < 0.5;
            assert!(
                ok,
                "{} {name}: {on_edge} from the edge, {on_feather} from the feather",
                o.id
            );
            // Where outline() says, but a gradient's line handles.
            if member.module != "mask_gradient" {
                assert_eq!(k.at, member.outline.handles[k.index as usize].at);
            }
        }
    }
}

#[test]
fn handles_land_on_the_outline_at_every_zoom_and_scale() {
    at_scales(|s| {
        let mut h = harness_at(s);
        run(
            &mut h,
            "mask.add.ellipse",
            json!({ "params": { "rotation": 30 } }),
        );
        run(
            &mut h,
            "mask.add.gradient",
            json!({ "params": { "rotation": -20 } }),
        );
        run(&mut h, "mask.add.circle", json!(null));
        run(
            &mut h,
            "mask.add.path",
            json!({ "corners": [[300, 200], [800, 200], [550, 650]] }),
        );
        for zoom in ["viewport.zoom.fit", "viewport.zoom.100", "viewport.zoom.in"] {
            run(&mut h, zoom, json!(null));
            for shape in ["ellipse", "gradient", "circle", "path"] {
                run(&mut h, "mask.select", json!({ "shape": shape }));
                assert_eq!(h.state().viewport().masks().overlays().len(), 1);
                handles_on_outline(&h);
            }
            // A gradient's line handles stay clear of its anchor.
            run(&mut h, "mask.select", json!({ "shape": "gradient" }));
            let c = handle(&h, "centre");
            for name in ["rotation", "feather"] {
                assert!(
                    handle(&h, name).distance(c) > 2.0 * egui_pop::viewport::overlay::HIT_RADIUS
                );
            }
        }
    });
}

#[test]
fn agents_move_handles_and_selection_is_no_undo_step() {
    let mut h = harness_at(1.0);
    run(&mut h, "mask.add.gradient", json!(null));
    run(&mut h, "mask.add.circle", json!(null));
    run(&mut h, "mask.select", json!({ "shape": "gradient" }));
    assert_eq!(text(&h, "view/graph/selection"), "gradient");
    assert!(steps(&h).is_empty());
    let x = f(&h, "node/gradient/x");
    let y = f(&h, "node/gradient/y");
    // The rotation handle straight up from the anchor: 90°.
    h.state_mut()
        .run(
            "mask.handle.move",
            json!({ "handle": "rotation", "x": x, "y": y - 100.0 }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    h.step();
    h.step();
    assert!((f(&h, "node/gradient/rotation") - 90.0).abs() < 1e-9);
    assert_eq!(steps(&h), ["drag gradient rotation"]);
    // A click on the other shape's edge selects it.
    let m = mapping(&h);
    let (cx, cy, r) = (
        f(&h, "node/circle/x"),
        f(&h, "node/circle/y"),
        f(&h, "node/circle/radius"),
    );
    // Left of it: the gradient, now vertical, has its handles above
    // and below.
    click(&mut h, m.image_to_screen([cx - r, cy]));
    assert_eq!(text(&h, SHAPE), "circle");
}

#[test]
fn the_shapes_not_selected_are_drawn_faint_with_no_handles() {
    let mut h = harness_at(1.0);
    run(
        &mut h,
        "mask.add.path",
        json!({ "corners": [[100, 100], [500, 100], [300, 500]] }),
    );
    run(&mut h, "mask.add.circle", json!(null));
    let v = h.state().viewport();
    assert_eq!(v.masks().members().len(), 2);
    assert_eq!(v.masks().overlays().len(), 1, "the selected shape's only");
    // egui-pop draws them: the selected shape emphasized, the path faint
    // as a guide (no handles), the circle's feather dashed.
    assert_eq!(
        v.masks().overlays()[0].style.emphasis,
        egui_pop::viewport::overlay::Emphasis::Selected
    );
    let path = v.masks().guides().iter().find(|g| g.id == "path").unwrap();
    assert_eq!(
        path.style.emphasis,
        egui_pop::viewport::overlay::Emphasis::Faint
    );
    assert!(path.handles.is_empty());
    let feather = v.masks().guides().iter().find(|g| g.id == "circle/feather");
    assert!(feather.is_some_and(|g| g.style.dashed && g.handles.is_empty()));
    let m = v.state().mapping().unwrap();
    drop(v);
    // Every tile on screen first: a picture runs the harness until the
    // window stops asking for frames.
    for _ in 0..40 {
        h.step();
        let stats = h.state().viewport_stats();
        assert!(stats.wait_idle(std::time::Duration::from_mins(1)));
        h.step();
        if h.state().viewport().state().tiles_complete() {
            break;
        }
    }
    let shot = headless::snapshot(&mut h, "viewport.display").unwrap();
    // The path's top edge is drawn: its pixels stand out from those just
    // below it (the ramp under them barely changes).
    let r = h.state().target_rect("viewport.display").unwrap();
    let ppp = h.state().pixels_per_point();
    let px = |p: Pos2| {
        let q = shot.get_pixel(
            ((p.x - r.min.x) * ppp) as u32,
            ((p.y - r.min.y) * ppp) as u32,
        );
        q.0.iter().take(3).map(|c| i32::from(*c)).sum::<i32>()
    };
    let on = m.image_to_screen([300.0, 100.0]);
    let off = on + vec2(0.0, 8.0);
    // The line is thin: it is looked for within two pixels of where the
    // mapping puts it (antialiased, it may fall between two rows).
    let edge = (-2..=2)
        .map(|dy| (px(on + vec2(0.0, dy as f32)) - px(off)).abs())
        .max()
        .unwrap_or(0);
    assert!(
        edge > 30,
        "{} vs {} (best {edge}); at {on:?} in {r:?}, mapping {m:?}",
        px(on),
        px(off)
    );
}

#[test]
fn a_path_s_bezier_handles_are_controls() {
    use egui_pop::viewport::overlay::HandleKind;
    use helios_gui::masks::kind_of;
    assert_eq!(kind_of("corner.0"), HandleKind::Point);
    assert_eq!(kind_of("before.0"), HandleKind::Control);
    assert_eq!(kind_of("after.2"), HandleKind::Control);
    assert_eq!(kind_of("centre"), HandleKind::Move);
}
