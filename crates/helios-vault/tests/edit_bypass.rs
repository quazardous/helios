//! A bypassed node in an image's edit: saved on its entry, given back to
//! a graph.

use helios_core::*;
use helios_vault::edit;
use serde_json::json;

/// Multiplies by `gain`; one input, one output.
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

/// No input: nothing to pass through.
struct Constant;

impl Module for Constant {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "constant".into(),
            inputs: vec![],
            params: vec![],
            ..Gain.manifest()
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), _: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        Ok(vec![Image::from_fn(output, 1, |_, _, _| 0.5)])
    }
}

fn graph() -> (Graph, NodeId, NodeId) {
    let mut g = Graph::new();
    let src = g.add_source("src", Image::from_fn(Region::sized(4, 4), 1, |_, _, _| 1.0));
    let a = g
        .add_module("a", ModuleRef::new(Gain), &Params::new())
        .unwrap();
    let b = g
        .add_module("b", ModuleRef::new(Gain), &Params::new())
        .unwrap();
    g.connect(src, "image", a, "input").unwrap();
    g.connect(a, "output", b, "input").unwrap();
    (g, a, b)
}

#[test]
fn a_bypassed_node_is_saved_on_its_entry_and_given_back() {
    let (mut g, a, b) = graph();
    g.set_bypass(b, true).unwrap();
    let saved = edit::of_graph(&g);
    let entry = |name: &str| {
        saved["modules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == name)
            .cloned()
            .unwrap()
    };
    assert_eq!(entry("b")["bypassed"], true);
    assert!(entry("a").get("bypassed").is_none());
    assert_eq!(
        edit::bypassed(&saved).into_iter().collect::<Vec<_>>(),
        ["b"]
    );

    let (mut fresh, fa, fb) = graph();
    fresh.set_bypass(fa, true).unwrap();
    let skipped = edit::apply_to_graph(&mut fresh, &saved).unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    assert!(fresh.is_bypassed(fb).unwrap(), "bypassed as saved");
    assert!(
        !fresh.is_bypassed(fa).unwrap(),
        "an entry without it is not"
    );
    assert!(!g.is_bypassed(a).unwrap());
}

#[test]
fn a_node_that_cannot_be_bypassed_is_reported() {
    let mut g = Graph::new();
    g.add_module("k", ModuleRef::new(Constant), &Params::new())
        .unwrap();
    let e = json!({ "modules": [{ "name": "k", "module": "constant", "bypassed": true }] });
    let skipped = edit::apply_to_graph(&mut g, &e).unwrap();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert!(skipped[0].contains("`k`"), "{skipped:?}");
}
