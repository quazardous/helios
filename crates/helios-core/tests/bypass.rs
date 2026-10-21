//! A bypassed node renders as if it were absent: its output is the input
//! it passes through, with that input's content and content key, while it
//! keeps its parameters and wiring.

use std::sync::Arc;

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

/// Mixes its input towards `other` by `mask`: three one-channel inputs.
struct Mix;

impl Module for Mix {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "mix".into(),
            version: 1,
            description: "mix two images by a mask".into(),
            params: vec![],
            inputs: vec![
                PortSpec::image("input", 1),
                PortSpec::image("other", 1),
                PortSpec::image("mask", 1),
            ],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Tone,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            let (a, b, m) = (
                inputs[0].pixel(x, y)[0],
                inputs[1].pixel(x, y)[0],
                inputs[2].pixel(x, y)[0],
            );
            a + (b - a) * m
        })])
    }
}

/// A one-channel constant: no input, nothing to pass through.
struct Constant;

impl Module for Constant {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "constant".into(),
            version: 1,
            description: "a constant image".into(),
            params: vec![],
            inputs: vec![],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Tone,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), _: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        Ok(vec![Image::from_fn(output, 1, |_, _, _| 0.5)])
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

/// `src -> a (x2) -> b -> c (x5)`, with `b` a blur or a gain of 3, and the
/// same chain without `b`. Returns both graphs and their `c`, and `b`.
fn chains(blur: bool) -> (Graph, NodeId, NodeId, Graph, NodeId) {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let a = g.add_module("a", ModuleRef::new(Gain), &gain(2.0)).unwrap();
    let b = if blur {
        g.add_module("b", ModuleRef::new(Blur), &Params::new())
    } else {
        g.add_module("b", ModuleRef::new(Gain), &gain(3.0))
    }
    .unwrap();
    let c = g.add_module("c", ModuleRef::new(Gain), &gain(5.0)).unwrap();
    g.connect(src, "image", a, "input").unwrap();
    g.connect(a, "output", b, "input").unwrap();
    g.connect(b, "output", c, "input").unwrap();

    let mut without = Graph::new();
    let src = without.add_source("src", ramp());
    let a = without
        .add_module("a", ModuleRef::new(Gain), &gain(2.0))
        .unwrap();
    let wc = without
        .add_module("c", ModuleRef::new(Gain), &gain(5.0))
        .unwrap();
    without.connect(src, "image", a, "input").unwrap();
    without.connect(a, "output", wc, "input").unwrap();
    (g, c, b, without, wc)
}

#[test]
fn a_bypassed_chain_equals_the_chain_without_the_node() {
    for blur in [false, true] {
        let (mut g, c, b, without, wc) = chains(blur);
        g.set_bypass(b, true).unwrap();
        let e = engine();
        for r in [Region::sized(16, 8), Region::new(3, 2, 5, 3)] {
            let bypassed = e.render(&g, c, "output", r).unwrap();
            let expected = e.render(&without, wc, "output", r).unwrap();
            assert_eq!(bypassed.data(), expected.data(), "blur {blur}, {r:?}");
            assert_eq!(
                e.key(&g, c, r).unwrap(),
                e.key(&without, wc, r).unwrap(),
                "the content key follows: blur {blur}, {r:?}"
            );
        }
    }
}

#[test]
fn a_bypassed_chain_equals_the_chain_without_the_node_at_a_scale() {
    let (mut g, c, b, without, wc) = chains(true);
    g.set_bypass(b, true).unwrap();
    let e = engine();
    let r = Region::sized(8, 4);
    let bypassed = e.render_at(&g, c, "output", r, 0.5).unwrap();
    let expected = e.render_at(&without, wc, "output", r, 0.5).unwrap();
    assert_eq!(bypassed.data(), expected.data());
}

#[test]
fn a_bypassed_chain_reuses_the_content_of_the_chain_without_the_node() {
    let (mut g, c, b, without, wc) = chains(false);
    g.set_bypass(b, true).unwrap();
    let e = engine();
    let r = Region::sized(16, 8);
    e.render(&without, wc, "output", r).unwrap();
    let before = e.counters();
    e.render(&g, c, "output", r).unwrap();
    assert_eq!(
        e.counters().since(&before).computed,
        0,
        "the same keys: nothing to compute"
    );
}

#[test]
fn a_bypassed_node_renders_its_input_and_keeps_its_parameters() {
    let (mut g, c, b, _, _) = chains(false);
    let e = engine();
    let r = Region::sized(16, 8);
    let original = e.render(&g, c, "output", r).unwrap();
    let key = e.key(&g, c, r).unwrap();
    g.set_bypass(b, true).unwrap();
    assert!(g.is_bypassed(b).unwrap());
    assert_eq!(g.params(b).unwrap().unwrap().float("gain"), 3.0);
    // The node itself shows what it passes through: `a`'s output.
    let a = g.find("a").unwrap();
    assert_eq!(
        e.render(&g, b, "output", r).unwrap().data(),
        e.render(&g, a, "output", r).unwrap().data()
    );
    assert_ne!(e.key(&g, c, r).unwrap(), key);
    // Back: the same content as before, from the cache.
    g.set_bypass(b, false).unwrap();
    assert!(!g.is_bypassed(b).unwrap());
    assert_eq!(e.key(&g, c, r).unwrap(), key);
    let before = e.counters();
    let back = e.render(&g, c, "output", r).unwrap();
    assert_eq!(back.data(), original.data());
    assert_eq!(e.counters().since(&before).computed, 0);
}

#[test]
fn a_bypassed_node_reads_only_the_input_it_passes() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let mix = g
        .add_module("mix", ModuleRef::new(Mix), &Params::new())
        .unwrap();
    g.connect(src, "image", mix, "input").unwrap();
    // `other` and `mask` are left unconnected: rendering it fails...
    let r = Region::sized(16, 8);
    assert!(matches!(
        engine().render(&g, mix, "output", r),
        Err(Error::Unconnected { .. })
    ));
    // ...but bypassed, it reads its first input only.
    g.set_bypass(mix, true).unwrap();
    let out = engine().render(&g, mix, "output", r).unwrap();
    assert_eq!(out.data(), ramp().data());
}

#[test]
fn a_bypassed_node_with_its_main_input_unconnected_says_so() {
    let mut g = Graph::new();
    let x2 = g
        .add_module("x2", ModuleRef::new(Gain), &gain(2.0))
        .unwrap();
    g.set_bypass(x2, true).unwrap();
    let err = engine()
        .render(&g, x2, "output", Region::sized(4, 4))
        .unwrap_err();
    assert!(err.to_string().contains("bypassed"), "{err}");
}

#[test]
fn a_source_or_a_node_with_nothing_to_pass_is_not_bypassed() {
    let mut g = Graph::new();
    let src = g.add_source("src", ramp());
    let k = g
        .add_module("k", ModuleRef::new(Constant), &Params::new())
        .unwrap();
    assert!(!g.can_bypass(src).unwrap());
    assert!(!g.can_bypass(k).unwrap());
    assert!(g.set_bypass(src, true).is_err());
    assert!(g.set_bypass(k, true).is_err());
    assert!(!g.is_bypassed(k).unwrap());
    // Bringing back what is not bypassed is no error.
    g.set_bypass(k, false).unwrap();
}

#[test]
fn the_description_says_which_nodes_are_bypassed() {
    let (mut g, _, b, _, _) = chains(false);
    let d = g.describe();
    assert!(d["nodes"][b.0].get("bypassed").is_none());
    g.set_bypass(b, true).unwrap();
    let d = g.describe();
    assert_eq!(d["nodes"][b.0]["bypassed"], true);
}
