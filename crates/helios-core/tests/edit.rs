//! Editing a network: unplugging an input, removing a node (its chain
//! closed up behind it), and the ports a node offers.

use std::sync::Arc;

use helios_core::*;

/// Adds one to every value.
struct AddOne;

impl Module for AddOne {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "add_one".into(),
            version: 1,
            description: String::new(),
            params: vec![],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Other,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, _: &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let s = inputs[0].sample(output);
        let data = s.data().iter().map(|v| v + 1.0).collect();
        Ok(vec![Image::new(output, 1, data)])
    }
}

fn chain() -> (Graph, [NodeId; 4]) {
    let mut g = Graph::new();
    let src = g.add_source("src", Image::from_fn(Region::sized(4, 4), 1, |_, _, _| 0.0));
    let a = g
        .add_module("a", ModuleRef::new(AddOne), &Params::new())
        .unwrap();
    let b = g
        .add_module("b", ModuleRef::new(AddOne), &Params::new())
        .unwrap();
    let c = g
        .add_module("c", ModuleRef::new(AddOne), &Params::new())
        .unwrap();
    g.connect(src, "image", a, "input").unwrap();
    g.connect(a, "output", b, "input").unwrap();
    g.connect(b, "output", c, "input").unwrap();
    (g, [src, a, b, c])
}

fn value(g: &Graph, node: &str) -> f32 {
    let e = Engine::new(Arc::new(Bus::new()));
    let id = g.find(node).unwrap();
    e.render(g, id, "output", Region::sized(4, 4))
        .unwrap()
        .pixel(0, 0)[0]
}

#[test]
fn removing_a_node_closes_the_chain_and_renumbers() {
    let (mut g, [_, _, b, _]) = chain();
    assert_eq!(value(&g, "c"), 3.0);
    g.remove(b).unwrap();
    assert_eq!(g.ids().count(), 3);
    assert_eq!(g.find("c"), Some(NodeId(2)));
    assert_eq!(g.find("b"), None);
    assert_eq!(value(&g, "c"), 2.0, "c now reads from a");
    assert_eq!(
        g.extent(g.find("c").unwrap()).unwrap(),
        Some(Region::sized(4, 4))
    );
}

#[test]
fn an_unplugged_input_is_empty_and_ports_are_named() {
    let (mut g, [src, a, _, c]) = chain();
    g.disconnect(c, "input").unwrap();
    assert_eq!(g.input_nodes(c).unwrap(), [None]);
    assert!(g.disconnect(src, "input").is_err(), "a source has no input");
    assert_eq!(g.output_names(src).unwrap(), ["image"]);
    assert_eq!(g.output_names(a).unwrap(), ["output"]);
}
