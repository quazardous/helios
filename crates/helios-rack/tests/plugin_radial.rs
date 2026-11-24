//! A plugin's on-image tool, decoded from its rack declaration: the
//! radial exposure of `radial_exposure.rs` (a centre, a radius, a
//! feather past it held in the tool's 0–200, reset to 25), shipped as
//! JSON (`fixtures/plugin_radial.json`) on the plugin's own values
//! (`plugin/<id>/<key>`), driven headless as the viewport drives it: one
//! undo step per drag, in the plugins' history scope; Escape leaves no
//! step. A handle of a kind this Helios does not know is left out with
//! its reason, and the tool keeps its other handles.

use helios_controls::HeliosRegistry as _;
use helios_core::Params;
use helios_rack::plugin::{self, Declared};
use helios_rack::tools::{self, Drag, Point, Target};
use helios_shell::commands::Origin;
use helios_values::{HistoryScope, Store};
use serde_json::{Value as Json, json};

const PLUGIN: &str = "org.example.radial";
const FILE: &str = include_str!("fixtures/plugin_radial.json");

fn declared(json: &str) -> Declared {
    let mut d = Declared::new(PLUGIN);
    d.read(
        "ui/tools.json",
        json,
        &helios_controls::Registry::standard(),
    );
    d
}

fn store(d: &Declared) -> Store {
    let mut s = Store::new();
    for (key, descriptor) in &d.values {
        s.declare(&plugin::path(PLUGIN, key), descriptor.clone());
    }
    s
}

fn value(s: &Store, key: &str) -> f64 {
    s.get(&plugin::path(PLUGIN, key))
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

/// The plugin's values as the handles read them.
fn now(s: &Store, d: &Declared) -> Params {
    d.values.iter().fold(Params::new(), |p, (key, _)| {
        let v = s.get(&plugin::path(PLUGIN, key)).unwrap().value.clone();
        p.with(key, v)
    })
}

fn begin(s: &mut Store, d: &Declared, handle: &str) -> Drag {
    let start = now(s, d);
    let t = &d.tool("radial").unwrap().tool;
    Drag::begin(s, t, PLUGIN, handle, start, Origin::User).unwrap()
}

fn drag(s: &mut Store, d: &Declared, handle: &str, via: Point, to: Point) {
    let g = begin(s, d, handle);
    g.update(s, via).unwrap();
    g.update(s, to).unwrap();
    let registry = helios_controls::catalogue::registry();
    assert!(g.commit(s, &registry, &helios_controls::flow::Steps::new()));
}

fn steps(s: &mut Store) -> Vec<String> {
    s.in_history(HistoryScope::Plugin, |s| {
        s.undo_steps().iter().map(|st| st.label.clone()).collect()
    })
}

#[test]
fn the_decoded_tool_writes_the_plugin_s_values_one_step_per_drag() {
    let d = declared(FILE);
    assert_eq!(d.problems, Vec::<String>::new());
    let t = &d.tool("radial").unwrap().tool;
    assert_eq!(t.target, Target::Plugin);
    let names: Vec<_> = t.handles.iter().map(tools::ToolHandle::name).collect();
    assert_eq!(names, ["centre", "radius", "feather"]);
    assert_eq!(t.handle("centre").unwrap().writes(), ["x", "y"]);

    let mut s = store(&d);
    drag(&mut s, &d, "centre", [10.0, 0.0], [40.0, 30.0]);
    assert_eq!((value(&s, "x"), value(&s, "y")), (40.0, 30.0));
    drag(&mut s, &d, "radius", [200.0, 30.0], [40.0, 180.0]);
    assert_eq!(value(&s, "radius"), 150.0);
    drag(&mut s, &d, "feather", [240.0, 30.0], [220.0, 30.0]);
    assert_eq!(value(&s, "feather"), 30.0);
    assert_eq!(
        steps(&mut s),
        [
            format!("drag {PLUGIN} centre"),
            format!("drag {PLUGIN} radius"),
            format!("drag {PLUGIN} feather"),
        ]
    );
    assert!(s.undo_steps().is_empty(), "nothing on the image's line");
    s.in_history(HistoryScope::Plugin, |s| s.undo(&Origin::User))
        .unwrap();
    assert_eq!(value(&s, "feather"), 25.0);
}

#[test]
fn the_tool_s_own_range_and_reset_hold_and_escape_leaves_no_step() {
    let d = declared(FILE);
    let mut s = store(&d);
    drag(&mut s, &d, "feather", [200.0, 0.0], [900.0, 0.0]);
    assert_eq!(value(&s, "feather"), 200.0, "held in the tool's range");
    let registry = helios_controls::catalogue::registry();
    let flow = helios_controls::flow::Steps::new();
    let t = &d.tool("radial").unwrap().tool;
    assert!(tools::reset(
        &mut s,
        &registry,
        &flow,
        t,
        PLUGIN,
        "feather",
        Origin::User
    ));
    assert_eq!(value(&s, "feather"), 25.0);
    assert_eq!(steps(&mut s).len(), 2);

    let g = begin(&mut s, &d, "radius");
    g.update(&mut s, [300.0, 0.0]).unwrap();
    assert_eq!(value(&s, "radius"), 300.0);
    g.cancel(&mut s);
    assert_eq!(value(&s, "radius"), 100.0);
    assert_eq!(steps(&mut s).len(), 2, "no step for a drag given up");
}

#[test]
fn an_unknown_kind_or_a_foreign_value_is_left_out_and_the_rest_stays() {
    let mut decl: Json = serde_json::from_str(FILE).unwrap();
    let handles = &mut decl["tools"][0]["handles"]["handles"];
    handles[0] = json!({"name": "spiral", "kind": "spiral", "turns": 3});
    handles
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "ghost", "kind": "radius",
                     "centre": {"x": "x", "y": "y"}, "radius": "elsewhere"}));
    handles
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "code", "kind": "custom"}));
    let d = declared(&decl.to_string());
    let t = &d.tool("radial").unwrap().tool;
    let names: Vec<_> = t.handles.iter().map(tools::ToolHandle::name).collect();
    assert_eq!(names, ["radius", "feather"]);
    let said = d.problems.join("\n");
    for reason in [
        "tool `radial`: handle `spiral`: unknown kind `spiral`",
        "handle `ghost`: `elsewhere` is not a number value the plugin declares",
        "handle `code`: unknown kind `custom`",
    ] {
        assert!(said.contains(reason), "{reason}\n{said}");
    }

    // A tool whose every handle is left out is left out too; a text not
    // the plugin's refuses the tool.
    let mut decl: Json = serde_json::from_str(FILE).unwrap();
    decl["tools"][0]["handles"]["handles"] = json!([{"name": "s", "kind": "spiral"}]);
    decl["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "stolen", "label": "gui-ok",
                     "handles": {"format": "egui_hud-handles/1", "handles": []}}));
    let d = declared(&decl.to_string());
    assert!(d.tools.is_empty());
    let said = d.problems.join("\n");
    assert!(said.contains("no handle left"), "{said}");
    assert!(
        said.contains("tool `stolen`: `gui-ok` is not a message id"),
        "{said}"
    );
}
