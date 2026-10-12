//! Drawn mask shapes edited through the value store, as the viewport's
//! handles will edit them: a drag is one undo step, a path's points are
//! one list value, and the graph renders what the store holds.

use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, ModuleRef, NodeId, Params, Region, Value};
use helios_module_masks::shape::{path_points, path_values};
use helios_module_masks::{MaskCircle, MaskPath, PathPoint};
use helios_shell::commands::Origin;
use helios_values::*;

fn shapes() -> (Graph, NodeId, NodeId, Store) {
    let mut g = Graph::new();
    let circle = g
        .add_module("circle", ModuleRef::new(MaskCircle), &Params::new())
        .unwrap();
    let path = g
        .add_module("path", ModuleRef::new(MaskPath), &Params::new())
        .unwrap();
    let mut store = Store::new();
    bind_node(&mut store, &g, circle).unwrap();
    bind_node(&mut store, &g, path).unwrap();
    (g, circle, path, store)
}

fn set(store: &mut Store, path: &str, value: Value, gesture: Option<GestureId>) {
    let mut s = Set::new(path, Change::To(value), Origin::User);
    if let Some(g) = gesture {
        s = s.gesture(g);
    }
    store.set(s).unwrap();
}

#[test]
fn a_shape_s_parameters_are_values_in_image_pixels() {
    let (_, _, _, store) = shapes();
    let x = &store.get("node/circle/x").unwrap().descriptor;
    assert_eq!(x.unit.as_deref(), Some("px"));
    let points = &store.get("node/path/points").unwrap().descriptor;
    assert_eq!(points.ty, Type::Floats);
    assert_eq!(points.count, Some(64 * 6));
    assert!(store.get("node/path/invert").is_some());
    assert!(store.get("node/circle/opacity").is_some());
}

#[test]
fn dragging_a_shape_is_one_undo_step_and_the_graph_follows() {
    let (mut g, circle, _, mut store) = shapes();
    let engine = Engine::new(Arc::new(Bus::new()));
    let region = Region::sized(64, 48);
    let before = engine.render(&g, circle, "mask", region).unwrap();

    let drag = store.begin_gesture(Origin::User, "move circle");
    for (x, y) in [(20.0, 14.0), (30.0, 20.0), (40.0, 30.0)] {
        set(&mut store, "node/circle/x", Value::Float(x), Some(drag));
        set(&mut store, "node/circle/y", Value::Float(y), Some(drag));
    }
    store.end(drag);
    assert_eq!(sync(&mut store, &mut g).unwrap(), 2);
    let moved = engine.render(&g, circle, "mask", region).unwrap();
    assert_eq!(moved.pixel(40, 30)[0], 1.0);
    assert_eq!(moved.pixel(16, 12)[0], 0.0);

    assert_eq!(
        store.undo(&Origin::User).unwrap().as_deref(),
        Some("move circle")
    );
    sync(&mut store, &mut g).unwrap();
    let undone = engine.render(&g, circle, "mask", region).unwrap();
    assert_eq!(undone.data(), before.data());
}

#[test]
fn a_path_s_points_are_one_value_edited_and_undone_whole() {
    let (mut g, _, path, mut store) = shapes();
    let triangle = [
        PathPoint::corner(5.0, 5.0),
        PathPoint {
            before: [40.0, 0.0],
            after: [40.0, 30.0],
            ..PathPoint::corner(50.0, 15.0)
        },
        PathPoint::corner(10.0, 40.0),
    ];
    set(&mut store, "node/path/points", path_values(&triangle), None);
    sync(&mut store, &mut g).unwrap();
    let held = g
        .params(path)
        .unwrap()
        .and_then(|p| p.get("points").cloned());
    let Some(Value::List(held)) = held else {
        panic!("the graph holds the points");
    };
    assert_eq!(path_points(&held).unwrap(), triangle);

    // A point dragged: the whole list is written, one undo step.
    let mut moved = triangle;
    moved[2] = PathPoint::corner(12.0, 44.0);
    set(&mut store, "node/path/points", path_values(&moved), None);
    store.undo(&Origin::User).unwrap();
    assert_eq!(
        store.get("node/path/points").unwrap().value,
        path_values(&triangle)
    );
}
