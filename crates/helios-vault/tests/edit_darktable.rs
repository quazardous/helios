//! A darktable edit given to a graph: its items are named after
//! darktable's operations, and land on the raw chain's nodes.

use helios_core::*;
use helios_vault::edit;
use serde_json::json;

/// Multiplies by `red`, standing for the white balance.
struct Balance;

impl Module for Balance {
    type Plan = f32;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "whitebalance".into(),
            version: 1,
            description: "multiply by a constant".into(),
            params: vec![ParamSpec::float("red", "factor", 0.0, 10.0, 1.0)],
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
        Ok(params.float("red") as f32)
    }

    fn process(&self, red: &f32, inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        Ok(vec![Image::new(
            output,
            1,
            inputs[0].data().iter().map(|v| v * red).collect(),
        )])
    }
}

#[test]
fn darktable_s_temperature_lands_on_the_white_balance_node() {
    let mut g = Graph::new();
    let src = g.add_source("src", Image::from_fn(Region::sized(2, 2), 1, |_, _, _| 1.0));
    let wb = g
        .add_module("whitebalance", ModuleRef::new(Balance), &Params::new())
        .unwrap();
    g.connect(src, "image", wb, "input").unwrap();
    let imported = json!({ "modules": [
        { "name": "temperature", "module": "whitebalance", "params": { "red": { "float": 2.5 } } },
        { "name": "lens", "module": "lens", "params": {} }
    ] });
    let skipped = edit::apply_to_graph(&mut g, &imported).unwrap();
    assert_eq!(skipped, ["`lens`: no such node"]);
    assert_eq!(
        g.params(wb).unwrap().unwrap().get("red"),
        Some(&Value::Float(2.5))
    );
    // Only for the module it maps to: a `temperature` running something
    // else finds no node.
    let other = json!({ "modules": [
        { "name": "temperature", "module": "exposure", "params": {} }
    ] });
    let skipped = edit::apply_to_graph(&mut g, &other).unwrap();
    assert_eq!(skipped, ["`temperature`: no such node"]);
}
