//! The normalized node status: idle, done, stale, failed and running, read
//! from the engine without computing anything.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use helios_core::*;

/// Multiplies by `gain`.
struct Gain;

impl Module for Gain {
    type Plan = f32;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "gain".into(),
            version: 1,
            description: String::new(),
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

/// Always fails.
struct Broken;

impl Module for Broken {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "broken".into(),
            kind: ModuleKind::Sink,
            family: Family::Io,
            ..Gain.manifest()
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, _: &(), _: &[&Image], _: Region) -> Result<Vec<Image>, String> {
        Err("disk full".into())
    }
}

/// Says when it starts, then waits to be released: lets a test see a node
/// running without sleeping.
struct Gate {
    started: Mutex<Sender<()>>,
    release: Mutex<Receiver<()>>,
}

impl Module for Gate {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "gate".into(),
            ..Gain.manifest()
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, _: &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        self.started.lock().unwrap().send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Ok(vec![inputs[0].sample(output)])
    }
}

const R: Region = Region::sized(8, 4);

fn chain(module: ModuleRef) -> (Graph, NodeId, NodeId) {
    let mut g = Graph::new();
    let src = g.add_source("src", Image::from_fn(R, 1, |x, _, _| x as f32));
    let node = g.add_module("node", module, &Params::new()).unwrap();
    g.connect(src, "image", node, "input").unwrap();
    (g, src, node)
}

fn states(e: &Engine, g: &Graph, node: NodeId) -> Vec<(String, NodeState)> {
    e.status(g, node, R)
        .unwrap()
        .into_iter()
        .map(|s| (s.name, s.state))
        .collect()
}

fn engine() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

#[test]
fn idle_then_done_then_shared() {
    let (g, _, node) = chain(ModuleRef::new(Gain));
    let e = engine();
    assert_eq!(
        states(&e, &g, node),
        [
            ("src".into(), NodeState::Idle),
            ("node".into(), NodeState::Idle)
        ]
    );
    e.render(&g, node, "output", R).unwrap();
    e.render(&g, node, "output", R).unwrap();
    let status = e.status(&g, node, R).unwrap();
    assert!(status.iter().all(|s| s.state == NodeState::Done));
    let header = &status[1];
    assert_eq!(
        (header.node_type, header.family, header.role),
        (NodeType::Process, Family::Tone, Role::General)
    );
    assert_eq!(
        header.load,
        Load {
            requests: 2,
            computed: 1,
            shared: 1,
            running: 0
        }
    );
    assert_eq!(status[0].node_type, NodeType::Source);
}

#[test]
fn a_parameter_change_makes_the_node_stale_not_its_inputs() {
    let (mut g, _, node) = chain(ModuleRef::new(Gain));
    let e = engine();
    e.render(&g, node, "output", R).unwrap();
    g.set_param(node, "gain", Value::Float(2.0)).unwrap();
    assert_eq!(
        states(&e, &g, node),
        [
            ("src".into(), NodeState::Done),
            ("node".into(), NodeState::Stale)
        ]
    );
    e.render(&g, node, "output", R).unwrap();
    assert_eq!(states(&e, &g, node)[1].1, NodeState::Done);
}

#[test]
fn a_failure_is_reported_with_the_node_type() {
    let (g, _, node) = chain(ModuleRef::new(Broken));
    let e = engine();
    assert!(e.render(&g, node, "output", R).is_err());
    let status = e.status(&g, node, R).unwrap();
    assert_eq!(
        (status[1].state, status[1].node_type),
        (NodeState::Failed, NodeType::Sink)
    );
}

#[test]
fn a_node_being_computed_is_running() {
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let gate = Gate {
        started: Mutex::new(started_tx),
        release: Mutex::new(release_rx),
    };
    let (g, _, node) = chain(ModuleRef::new(gate));
    let e = engine();
    thread::scope(|s| {
        let render = s.spawn(|| e.render(&g, node, "output", R).unwrap());
        started_rx.recv().unwrap();
        let status = e.status(&g, node, R).unwrap();
        assert_eq!(status[1].state, NodeState::Running);
        assert_eq!(status[1].load.running, 1);
        release_tx.send(()).unwrap();
        render.join().unwrap();
    });
    assert_eq!(states(&e, &g, node)[1].1, NodeState::Done);
}

#[test]
fn two_graphs_on_one_engine_do_not_mix_their_history() {
    let (g1, _, n1) = chain(ModuleRef::new(Gain));
    let (mut g2, _, n2) = chain(ModuleRef::new(Gain));
    g2.set_param(n2, "gain", Value::Float(3.0)).unwrap();
    let e = engine();
    e.render(&g1, n1, "output", R).unwrap();
    assert_eq!(
        states(&e, &g2, n2)[1].1,
        NodeState::Idle,
        "not stale: g2 never asked"
    );
}
