//! A module's on-image tool through the rack: a radial exposure (a
//! centre, a radius, a feather past it, an exposure inside), declared
//! with the rack's tools and `egui_hud`'s handle kinds only, with no egui
//! type, then driven headless as the viewport drives the masks' handles
//! (`docs/design/ui-responsibilities.md`, section 7, the worked example).

use helios_core::{Params, Value};
use helios_rack::handle::{Centre, Handle, HandleKind, Look};
use helios_rack::tools::{self, Drag, Point, Tool, ToolHandle};
use helios_shell::commands::Origin;
use helios_values::{Descriptor, Store, node_path};

// ---- The module: its parameters and its tool on the image. ----

/// The parameters, as the module declares them to the store.
fn params() -> Vec<(&'static str, Descriptor)> {
    vec![
        ("x", Descriptor::float("Centre x", -10_000.0, 10_000.0, 0.0)),
        ("y", Descriptor::float("Centre y", -10_000.0, 10_000.0, 0.0)),
        ("radius", Descriptor::float("Radius", 0.0, 10_000.0, 100.0)),
        ("feather", Descriptor::float("Feather", 0.0, 10_000.0, 25.0)),
        ("exposure", Descriptor::float("Exposure", -4.0, 4.0, 1.0)),
    ]
}

/// The tool: a centre to move it, a radius around it, a feather past the
/// radius (this tool's own range and default, narrower than the
/// parameter's).
fn tool() -> Tool {
    let centre = || Centre::default();
    Tool::new("radial-exposure", "radial_exposure")
        .with(ToolHandle::new(
            "center",
            Handle::new("centre", HandleKind::point(centre())).look(Look::Move),
        ))
        .with(ToolHandle::new(
            "radius",
            Handle::new("radius", HandleKind::radius(centre(), "radius")),
        ))
        .with(
            ToolHandle::new(
                "feather",
                Handle::new(
                    "feather",
                    HandleKind::feather(centre(), "radius", "feather"),
                )
                .reset(Some(25.0)),
            )
            .range(0.0, 200.0),
        )
}

/// What it does to a pixel at `at`: the exposure's gain, full inside the
/// radius, fading to none across the feather.
fn gain(p: &Params, at: Point) -> f64 {
    let f = |n| p.get(n).and_then(Value::as_f64).unwrap_or(0.0);
    let d = (at[0] - f("x")).hypot(at[1] - f("y"));
    let w = ((f("radius") + f("feather") - d) / f("feather").max(1e-9)).clamp(0.0, 1.0);
    (f("exposure") * w).exp2()
}

// ---- Driving it headless. ----

const NODE: &str = "radial";

fn store() -> Store {
    let mut s = Store::new();
    for (name, d) in params() {
        s.declare(&node_path(NODE, name), d);
    }
    s
}

/// The node's parameters, as the store holds them now.
fn now(s: &Store) -> Params {
    params().into_iter().fold(Params::new(), |p, (name, _)| {
        let v = s.get(&node_path(NODE, name)).unwrap().value.clone();
        p.with(name, v)
    })
}

fn value(s: &Store, name: &str) -> f64 {
    s.get(&node_path(NODE, name))
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

/// Drags `handle` to `to` (through `via` first) and ends the drag.
fn drag(s: &mut Store, handle: &str, via: Point, to: Point) {
    let d = begin(s, handle);
    d.update(s, via).unwrap();
    d.update(s, to).unwrap();
    let registry = helios_controls::catalogue::registry();
    assert!(d.commit(s, &registry, &helios_controls::flow::Steps::new()));
}

fn begin(s: &mut Store, handle: &str) -> Drag {
    let start = now(s);
    Drag::begin(s, &tool(), NODE, handle, start, Origin::User).unwrap()
}

fn labels(s: &Store) -> Vec<&str> {
    s.undo_steps().iter().map(|st| st.label.as_str()).collect()
}

#[test]
fn the_declaration_writes_what_each_handle_shows() {
    let t = tool();
    let names: Vec<_> = t.handles.iter().map(ToolHandle::name).collect();
    assert_eq!(names, ["centre", "radius", "feather"]);
    assert_eq!(t.handle("centre").unwrap().writes(), ["x", "y"]);
    assert_eq!(t.handle("radius").unwrap().writes(), ["radius"]);
    assert_eq!(t.handle("feather").unwrap().writes(), ["feather"]);
}

#[test]
fn each_drag_writes_its_parameters_as_one_undo_step() {
    let mut s = store();

    drag(&mut s, "centre", [10.0, 0.0], [40.0, 30.0]);
    assert_eq!((value(&s, "x"), value(&s, "y")), (40.0, 30.0));

    // 150 from the centre, whatever the direction.
    drag(&mut s, "radius", [200.0, 30.0], [40.0, 180.0]);
    assert_eq!(value(&s, "radius"), 150.0);

    // 30 past the radius.
    drag(&mut s, "feather", [240.0, 30.0], [220.0, 30.0]);
    assert_eq!(value(&s, "feather"), 30.0);

    assert_eq!(
        labels(&s),
        [
            "drag radial centre",
            "drag radial radius",
            "drag radial feather"
        ]
    );
    // Undone one step at a time.
    s.undo(&Origin::User).unwrap();
    assert_eq!(value(&s, "feather"), 25.0);
    s.undo(&Origin::User).unwrap();
    assert_eq!(value(&s, "radius"), 100.0);
    s.undo(&Origin::User).unwrap();
    assert_eq!((value(&s, "x"), value(&s, "y")), (0.0, 0.0));
}

#[test]
fn the_tool_s_own_feather_range_and_default_hold() {
    let mut s = store();
    // Far past the tool's 200: held at it (the parameter allows more).
    drag(&mut s, "feather", [200.0, 0.0], [900.0, 0.0]);
    assert_eq!(value(&s, "feather"), 200.0);
    // A double click: back to the tool's default, not the kind's 0.
    let registry = helios_controls::catalogue::registry();
    let steps = helios_controls::flow::Steps::new();
    assert!(tools::reset(
        &mut s,
        &registry,
        &steps,
        &tool(),
        NODE,
        "feather",
        Origin::User
    ));
    assert_eq!(value(&s, "feather"), 25.0);
    assert_eq!(labels(&s).len(), 2);
}

#[test]
fn escape_gives_a_drag_up_with_no_trace() {
    let mut s = store();
    let d = begin(&mut s, "radius");
    d.update(&mut s, [300.0, 0.0]).unwrap();
    assert_eq!(value(&s, "radius"), 300.0);
    d.cancel(&mut s);
    assert_eq!(value(&s, "radius"), 100.0);
    assert!(labels(&s).is_empty());
    assert!(s.open_gesture().is_none());
}

#[test]
fn the_render_and_the_handles_follow_the_drags() {
    let mut s = store();
    let t = tool();
    let at = [180.0, 0.0];
    // Outside the radius and its feather: untouched.
    assert_eq!(gain(&now(&s), at), 1.0);
    assert_eq!(
        t.handle("feather").unwrap().position(&now(&s)),
        [125.0, 0.0]
    );

    // Live while dragging, before the drag ends.
    let d = begin(&mut s, "radius");
    d.update(&mut s, [200.0, 0.0]).unwrap();
    assert_eq!(gain(&now(&s), at), 2.0);
    assert_eq!(t.handle("radius").unwrap().position(&now(&s)), [200.0, 0.0]);
    assert_eq!(
        t.handle("feather").unwrap().position(&now(&s)),
        [225.0, 0.0]
    );

    // Escape: the render comes back too.
    d.cancel(&mut s);
    assert_eq!(gain(&now(&s), at), 1.0);

    // Moving the centre carries the light and every handle with it.
    drag(&mut s, "centre", [0.0, 0.0], [180.0, 0.0]);
    assert_eq!(gain(&now(&s), at), 2.0);
    assert_eq!(t.handle("radius").unwrap().position(&now(&s)), [280.0, 0.0]);
}
