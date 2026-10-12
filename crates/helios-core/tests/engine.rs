//! Engine behaviour with small fake modules: caching by content, shared
//! prefixes, single flight, contract checks and events.

use std::sync::Arc;
use std::thread;

use helios_core::*;

/// Multiplies every channel by `gain`.
struct Gain;

impl Module for Gain {
    type Plan = f32;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "gain".into(),
            version: 1,
            description: "multiply by a constant".into(),
            params: vec![ParamSpec::float("gain", "factor", 0.0, 10.0, 1.0)],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Tone,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<f32, String> {
        Ok(params.float("gain") as f32)
    }

    fn process(&self, gain: &f32, inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        Ok(vec![Image::new(
            output,
            1,
            inputs[0].data().iter().map(|v| v * gain).collect(),
        )])
    }
}

/// Horizontal box blur of radius 1: reads one pixel on each side.
struct Blur;

impl Module for Blur {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "blur".into(),
            version: 1,
            description: "3-pixel horizontal box blur".into(),
            params: vec![],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs {
                overlap: 1,
                ..Needs::default()
            },
            kind: ModuleKind::Process,
            family: Family::Detail,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            (input.pixel(x - 1, y)[0] + input.pixel(x, y)[0] + input.pixel(x + 1, y)[0]) / 3.0
        })])
    }
}

/// Breaks its contract: returns two channels where one is declared.
struct Liar;

impl Module for Liar {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "liar".into(),
            ..Gain.manifest()
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), _: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        Ok(vec![Image::from_fn(output, 2, |_, _, _| 0.0)])
    }
}

fn ramp() -> Image {
    Image::from_fn(Region::sized(16, 8), 1, |x, y, _| (x + 100 * y) as f32)
}

fn engine() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

fn gain(value: f64) -> Params {
    Params::new().with("gain", Value::Float(value))
}

#[test]
fn renders_a_chain() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let x2 = g
        .add_module("x2", ModuleRef::new(Gain), &gain(2.0))
        .unwrap();
    g.connect(src, "image", x2, "input").unwrap();
    let out = engine()
        .render(&g, x2, "output", Region::new(3, 2, 2, 1))
        .unwrap();
    assert_eq!(out.data(), &[406.0, 408.0]);
}

#[test]
fn rendering_twice_computes_nothing_the_second_time() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let x2 = g
        .add_module("x2", ModuleRef::new(Gain), &gain(2.0))
        .unwrap();
    g.connect(src, "image", x2, "input").unwrap();
    let e = engine();
    let r = Region::sized(16, 8);
    let first = e.render(&g, x2, "output", r).unwrap();
    let before = e.counters();
    let second = e.render(&g, x2, "output", r).unwrap();
    let delta = e.counters().since(&before);
    assert_eq!((delta.computed, delta.source_reads), (0, 0));
    assert!(first.same_buffer(&second), "the same content is held once");
}

#[test]
fn changing_a_parameter_recomputes_only_downstream() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let a = g.add_module("a", ModuleRef::new(Gain), &gain(2.0)).unwrap();
    let b = g.add_module("b", ModuleRef::new(Gain), &gain(3.0)).unwrap();
    g.connect(src, "image", a, "input").unwrap();
    g.connect(a, "output", b, "input").unwrap();
    let e = engine();
    let r = Region::sized(16, 8);
    e.render(&g, b, "output", r).unwrap();
    g.set_param(b, "gain", Value::Float(4.0)).unwrap();
    let before = e.counters();
    let out = e.render(&g, b, "output", r).unwrap();
    assert_eq!(
        e.counters().since(&before).computed,
        1,
        "only `b` is computed again"
    );
    assert_eq!(out.pixel(1, 0)[0], 8.0);
}

#[test]
fn planning_counts_the_content_keys_it_computes() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let a = g.add_module("a", ModuleRef::new(Gain), &gain(2.0)).unwrap();
    let b = g.add_module("b", ModuleRef::new(Gain), &gain(3.0)).unwrap();
    g.connect(src, "image", a, "input").unwrap();
    g.connect(a, "output", b, "input").unwrap();
    let e = engine();
    let r = Region::sized(16, 8);
    e.render(&g, b, "output", r).unwrap();
    assert_eq!(e.counters().keys, 3, "one key per step planned");
    // Planning runs again on every render, cached or not.
    let before = e.counters();
    e.render(&g, b, "output", r).unwrap();
    assert_eq!(e.counters().since(&before).keys, 3);
}

#[test]
fn a_source_given_another_image_renders_it_and_keeps_the_first_cached() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let x2 = g
        .add_module("x2", ModuleRef::new(Gain), &gain(2.0))
        .unwrap();
    g.connect(src, "image", x2, "input").unwrap();
    let e = engine();
    let first = e.render(&g, x2, "output", Region::sized(16, 8)).unwrap();
    let other = Image::from_fn(Region::sized(4, 2), 1, |x, _, _| x as f32);
    g.set_source(src, other).unwrap();
    assert_eq!(g.extent(x2).unwrap(), Some(Region::sized(4, 2)));
    let second = e.render(&g, x2, "output", Region::sized(4, 2)).unwrap();
    assert_eq!(second.data(), &[0.0, 2.0, 4.0, 6.0, 0.0, 2.0, 4.0, 6.0]);
    g.set_source(src, ramp()).unwrap();
    let before = e.counters();
    let again = e.render(&g, x2, "output", Region::sized(16, 8)).unwrap();
    assert_eq!(e.counters().since(&before).computed, 0, "found by content");
    assert!(first.same_buffer(&again));
    assert!(g.set_source(x2, ramp()).is_err(), "a module takes no image");
}

#[test]
fn identical_prefixes_are_computed_once() {
    // Two branches with the same module and parameters on the same source:
    // one computation, whatever the node names.
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let a = g.add_module("a", ModuleRef::new(Gain), &gain(2.0)).unwrap();
    let b = g.add_module("b", ModuleRef::new(Gain), &gain(2.0)).unwrap();
    g.connect(src, "image", a, "input").unwrap();
    g.connect(src, "image", b, "input").unwrap();
    let e = engine();
    let r = Region::sized(16, 8);
    let ia = e.render(&g, a, "output", r).unwrap();
    let ib = e.render(&g, b, "output", r).unwrap();
    assert_eq!(e.counters().computed, 2, "the source region and one gain");
    assert!(ia.same_buffer(&ib));
}

#[test]
fn concurrent_renders_of_the_same_content_compute_it_once() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let x2 = g
        .add_module("x2", ModuleRef::new(Gain), &gain(2.0))
        .unwrap();
    g.connect(src, "image", x2, "input").unwrap();
    let e = engine();
    let r = Region::sized(16, 8);
    let images: Vec<Image> = thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|_| s.spawn(|| e.render(&g, x2, "output", r).unwrap()))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(e.counters().computed, 2);
    assert!(images.iter().all(|i| i.same_buffer(&images[0])));
}

#[test]
fn tiled_render_equals_whole_render_with_overlap() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let blur = g
        .add_module("blur", ModuleRef::new(Blur), &Params::new())
        .unwrap();
    g.connect(src, "image", blur, "input").unwrap();
    let e = engine();
    let whole = e.render(&g, blur, "output", Region::sized(16, 8)).unwrap();
    for tile in Region::sized(16, 8).tiles(5) {
        let part = e.render(&g, blur, "output", tile).unwrap();
        assert_eq!(part.data(), whole.sample(tile).data(), "tile {tile}");
    }
    // Edges replicate the border pixel.
    assert_eq!(whole.pixel(0, 0)[0], (0.0 + 0.0 + 1.0) / 3.0);
}

#[test]
fn the_graph_refuses_cycles_wrong_ports_and_bad_params() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let a = g
        .add_module("a", ModuleRef::new(Gain), &Params::new())
        .unwrap();
    let b = g
        .add_module("b", ModuleRef::new(Gain), &Params::new())
        .unwrap();
    g.connect(src, "image", a, "input").unwrap();
    g.connect(a, "output", b, "input").unwrap();
    assert!(matches!(
        g.connect(b, "output", a, "input"),
        Err(Error::Cycle { .. })
    ));
    assert!(matches!(
        g.connect(src, "nope", a, "input"),
        Err(Error::Port { .. })
    ));
    assert!(matches!(
        g.set_param(a, "gain", Value::Float(99.0)),
        Err(Error::Param { .. })
    ));
    let unconnected = g
        .add_module("c", ModuleRef::new(Gain), &Params::new())
        .unwrap();
    assert!(matches!(
        engine().render(&g, unconnected, "output", Region::sized(1, 1)),
        Err(Error::Unconnected { .. })
    ));
}

#[test]
fn a_module_breaking_its_contract_is_caught() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let liar = g
        .add_module("liar", ModuleRef::new(Liar), &Params::new())
        .unwrap();
    g.connect(src, "image", liar, "input").unwrap();
    let err = engine()
        .render(&g, liar, "output", Region::sized(2, 2))
        .unwrap_err();
    assert!(matches!(err, Error::Contract { .. }), "{err}");
}

#[test]
fn every_computed_step_is_an_event_caused_by_its_render() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let x2 = g
        .add_module("x2", ModuleRef::new(Gain), &gain(2.0))
        .unwrap();
    g.connect(src, "image", x2, "input").unwrap();
    let e = engine();
    e.render(&g, x2, "output", Region::sized(4, 4)).unwrap();
    let events = e.bus().since(0);
    let kinds: Vec<_> = events.iter().map(|ev| ev.kind).collect();
    assert_eq!(
        kinds,
        [
            "engine.render_requested",
            "engine.step_computed",
            "engine.step_computed"
        ]
    );
    assert!(events[1..].iter().all(|ev| ev.cause == Some(events[0].seq)));
    assert_eq!(
        events[1].payload["name"], "src",
        "inputs complete before their consumer"
    );
    assert_eq!(e.describe()["counters"]["computed"], 2);
    assert_eq!(g.describe()["nodes"][1]["params"]["gain"]["float"], 2.0);
}

/// Adds its optional inputs to its first: a member left unconnected adds nothing.
struct Sum;

impl Module for Sum {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "sum".into(),
            version: 1,
            description: "sum of the connected inputs".into(),
            params: vec![],
            inputs: vec![
                PortSpec::image("a", 1).optional(),
                PortSpec::image("b", 1).optional(),
            ],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Merge,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        assert_eq!(inputs.len(), 2, "one image per declared input");
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            inputs
                .iter()
                .filter(|i| !i.is_absent())
                .map(|i| i.pixel(x, y)[0])
                .sum()
        })])
    }
}

#[test]
fn an_optional_input_may_stay_unconnected() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let one = g
        .add_module("one", ModuleRef::new(Sum), &Params::new())
        .unwrap();
    let other = g
        .add_module("other", ModuleRef::new(Sum), &Params::new())
        .unwrap();
    let both = g
        .add_module("both", ModuleRef::new(Sum), &Params::new())
        .unwrap();
    g.connect(src, "image", one, "a").unwrap();
    g.connect(src, "image", other, "b").unwrap();
    g.connect(src, "image", both, "a").unwrap();
    g.connect(src, "image", both, "b").unwrap();
    let e = engine();
    let r = ramp().region();
    let a = e.render(&g, one, "output", r).unwrap();
    let b = e.render(&g, other, "output", r).unwrap();
    assert_eq!(a.data(), ramp().data());
    assert_eq!(a.data(), b.data());
    assert_ne!(
        e.key(&g, one, r).unwrap(),
        e.key(&g, other, r).unwrap(),
        "the port an input is connected to is part of the key"
    );
    let twice = e.render(&g, both, "output", r).unwrap();
    assert_eq!(twice.pixel(1, 0)[0], 2.0 * ramp().pixel(1, 0)[0]);
    assert_eq!(g.extent(other).unwrap(), Some(r), "the extent follows b");
}

/// A chain of `n` nodes, each reading the previous one on both ports: 2^n
/// paths from the last node to the source. The source and the last node.
fn diamonds(g: &mut Graph, n: usize) -> (NodeId, NodeId) {
    let src = g.add_source("src", ramp());
    let (mut last, mut port) = (src, "image");
    for i in 0..n {
        let node = g
            .add_module(&format!("sum{i}"), ModuleRef::new(Sum), &Params::new())
            .unwrap();
        g.connect(last, port, node, "a").unwrap();
        g.connect(last, port, node, "b").unwrap();
        (last, port) = (node, "output");
    }
    (src, last)
}

#[test]
fn the_cycle_check_visits_a_shared_node_once() {
    // Each connection checks for a cycle upstream: walking every path
    // would take 2^40 steps.
    let mut g = Graph::new();
    let (_, last) = diamonds(&mut g, 40);
    let first = g.find("sum0").unwrap();
    assert!(matches!(
        g.connect(last, "output", first, "b"),
        Err(Error::Cycle { .. })
    ));
}

#[test]
fn a_shared_input_is_planned_once_however_many_paths_lead_to_it() {
    // 2^30 paths from the last node to the source, 31 steps.
    let mut g = Graph::new();
    let (_, last) = diamonds(&mut g, 30);
    let e = engine();
    let r = ramp().region();
    let out = e.render(&g, last, "output", r).unwrap();
    assert_eq!(out.pixel(1, 0)[0], 2f32.powi(30) * ramp().pixel(1, 0)[0]);
    let c = e.counters();
    assert_eq!(c.keys, 31, "one key per step, not per path");
    assert_eq!(c.computed, 31);
    // A status walks the shared steps once too.
    let before = e.counters();
    assert_eq!(e.status(&g, last, r).unwrap().len(), 31);
    assert_eq!(e.counters().since(&before).keys, 31);
}
