//! Images stay in GPU memory along a chain of GPU steps: one upload, one
//! readback, the same result as the CPU. Without any GPU adapter (software
//! ones count), skipped, saying so.

use std::sync::Arc;

use helios_core::graph::SOURCE_PORT;
use helios_core::{
    Accelerator, Bus, Context, Engine, Graph, Image, Module, ModuleRef, NodeId, Params, Region,
    Value,
};
use helios_gpu::{Gpu, compare};

fn gpu() -> Option<std::sync::Arc<Gpu>> {
    match helios_gpu::shared::gpu() {
        Ok(g) => {
            eprintln!("GPU: {}", g.adapter);
            Some(g)
        }
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

/// Adds 0.1 to every channel; no GPU variant.
struct Lift;

impl Module for Lift {
    type Plan = ();
    fn manifest(&self) -> helios_core::Manifest {
        use helios_core::*;
        Manifest {
            name: "lift".into(),
            version: 1,
            description: String::new(),
            params: vec![],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
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
        let input = inputs[0].sample(output);
        let data = input.data().iter().map(|v| v + 0.1).collect();
        Ok(vec![Image::new(output, 4, data)])
    }
}

fn exposure() -> (ModuleRef, Params) {
    (
        ModuleRef::new(helios_module_exposure::Exposure),
        Params::new().with("exposure", Value::Float(1.0)),
    )
}

fn blur() -> (ModuleRef, Params) {
    (
        ModuleRef::new(helios_module_blur::Blur),
        Params::new().with("radius", Value::Float(3.0)),
    )
}

fn sigmoid() -> (ModuleRef, Params) {
    (
        ModuleRef::new(helios_module_sigmoid::Sigmoid),
        Params::new().with("contrast", Value::Float(1.8)),
    )
}

/// A source, then the steps in order: (graph, last node, its port).
fn chain(input: &Image, steps: Vec<(ModuleRef, Params)>) -> (Graph, NodeId, String) {
    let mut g = Graph::new();
    let mut prev = g.add_source("src", input.clone());
    let mut port = SOURCE_PORT.to_string();
    for (i, (module, params)) in steps.into_iter().enumerate() {
        let n = g
            .add_module(&format!("m{i}"), module.clone(), &params)
            .unwrap();
        g.connect(prev, &port, n, &module.manifest().inputs[0].name)
            .unwrap();
        prev = n;
        port.clone_from(&module.manifest().outputs[0].name);
    }
    (g, prev, port)
}

const REGION: Region = Region {
    x: 8,
    y: 6,
    width: 48,
    height: 36,
};

/// The CPU reference and a GPU engine's render of the same chain, with the
/// GPU engine.
fn both(gpu: Arc<Gpu>, steps: Vec<(ModuleRef, Params)>) -> (Image, Image, Engine) {
    let input = helios_testkit::noise(Region::sized(64, 48), 4, 11);
    let (g, node, port) = chain(&input, steps);
    let bus = Arc::new(Bus::new());
    let cpu = Engine::new(bus.clone())
        .render(&g, node, &port, REGION)
        .unwrap();
    let engine = Engine::new(bus).with_accelerator(gpu);
    let out = engine.render(&g, node, &port, REGION).unwrap();
    (cpu, out, engine)
}

#[test]
fn a_gpu_chain_uploads_once_and_reads_back_once() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let (cpu, out, engine) = both(gpu, vec![exposure(), blur(), sigmoid()]);
    let c = compare(&out, &cpu).unwrap();
    assert!(c.max_abs < 1e-4, "{c:?}");
    let n = engine.counters();
    assert_eq!((n.accelerated, n.uploads, n.readbacks), (3, 1, 1), "{n:?}");
}

#[test]
fn a_cpu_step_in_a_gpu_chain_costs_one_readback_and_one_upload() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let (cpu, out, engine) = both(
        gpu,
        vec![
            exposure(),
            blur(),
            (ModuleRef::new(Lift), Params::new()),
            sigmoid(),
        ],
    );
    let c = compare(&out, &cpu).unwrap();
    assert!(c.max_abs < 1e-4, "{c:?}");
    let n = engine.counters();
    assert_eq!((n.accelerated, n.uploads, n.readbacks), (3, 2, 2), "{n:?}");
}

#[test]
fn a_render_on_the_device_stays_there_and_matches_the_cpu() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let input = helios_testkit::noise(Region::sized(64, 48), 4, 5);
    let (g, node, port) = chain(&input, vec![exposure(), blur(), sigmoid()]);
    let bus = Arc::new(Bus::new());
    let cpu = Engine::new(bus.clone())
        .render(&g, node, &port, REGION)
        .unwrap();
    let engine = Engine::new(bus).with_accelerator(gpu.clone());
    let out = engine.render_device(&g, node, &port, REGION).unwrap();
    let n = engine.counters();
    assert_eq!((n.uploads, n.readbacks), (1, 0), "{n:?}");
    assert_eq!((out.region(), out.channels()), (REGION, 4));
    let buffer = gpu.buffer(&out).expect("a buffer on this device");
    assert!(buffer.size() >= out.bytes() as u64);
    let read = gpu.download(&out).unwrap();
    let c = compare(&read, &cpu).unwrap();
    assert!(c.max_abs < 1e-4, "{c:?}");
}
