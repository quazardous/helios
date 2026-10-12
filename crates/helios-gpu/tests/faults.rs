//! A failing GPU never fails a render, on a real device: each failure is
//! injected (`Gpu::inject`), and the render is compared with the CPU's.
//! Without any GPU adapter (software ones count), skipped, saying so.
//!
//! The tests share the process's device (`helios_gpu::shared`); two of
//! them rebuild it, the most a process allows (`MAX_REBUILDS`), so they
//! live in this binary alone.

use std::sync::Arc;

use helios_core::graph::SOURCE_PORT;
use helios_core::{Bus, Engine, Graph, Image, ModuleRef, NodeId, Params, Region, Value};
use helios_gpu::{Fault, Gpu, compare};

fn gpu() -> Option<Arc<Gpu>> {
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

const REGION: Region = Region {
    x: 8,
    y: 6,
    width: 48,
    height: 36,
};

/// exposure → blur → sigmoid over a noise image: (graph, last node).
fn chain(seed: u64) -> (Graph, NodeId) {
    let input = helios_testkit::noise(Region::sized(64, 48), 4, seed);
    let mut g = Graph::new();
    let mut prev = g.add_source("src", input);
    let mut port = SOURCE_PORT.to_string();
    let steps: [(&str, ModuleRef, Params); 3] = [
        (
            "exposure",
            ModuleRef::new(helios_module_exposure::Exposure),
            Params::new().with("exposure", Value::Float(1.0)),
        ),
        (
            "blur",
            ModuleRef::new(helios_module_blur::Blur),
            Params::new().with("radius", Value::Float(3.0)),
        ),
        (
            "sigmoid",
            ModuleRef::new(helios_module_sigmoid::Sigmoid),
            Params::new().with("contrast", Value::Float(1.8)),
        ),
    ];
    for (name, module, params) in steps {
        let n = g.add_module(name, module.clone(), &params).unwrap();
        g.connect(prev, &port, n, &module.manifest().inputs[0].name)
            .unwrap();
        prev = n;
        port.clone_from(&module.manifest().outputs[0].name);
    }
    (g, prev)
}

/// The CPU's render of a chain, and an engine on the GPU.
fn setup(gpu: &Arc<Gpu>, seed: u64) -> (Graph, NodeId, Image, Engine) {
    let (g, node) = chain(seed);
    let cpu = Engine::new(Arc::new(Bus::new()))
        .render(&g, node, "output", REGION)
        .unwrap();
    let engine = Engine::new(Arc::new(Bus::new())).with_accelerator(gpu.clone());
    (g, node, cpu, engine)
}

fn close(out: &Image, cpu: &Image) {
    let c = compare(out, cpu).unwrap();
    assert!(c.max_abs < 1e-4, "{c:?}");
}

fn events(engine: &Engine, kind: &str) -> Vec<serde_json::Value> {
    engine
        .bus()
        .since(0)
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.payload)
        .collect()
}

#[test]
fn a_kernel_that_fails_runs_its_step_on_the_cpu() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    for (seed, fault, failure) in [
        (1, Fault::Compile, "compile"),
        (2, Fault::Dispatch, "dispatch"),
        (3, Fault::OutOfMemory, "out_of_memory"),
    ] {
        let (g, node, cpu, engine) = setup(&gpu, seed);
        gpu.inject(fault);
        let out = engine.render(&g, node, "output", REGION).unwrap();
        close(&out, &cpu);
        let n = engine.counters();
        // One step fell back (the first: exposure), the others ran on the GPU.
        assert_eq!((n.fallbacks, n.accelerated), (1, 2), "{fault:?}: {n:?}");
        let fell = events(&engine, "engine.gpu_fell_back");
        assert_eq!(fell.len(), 1, "{fault:?}");
        assert_eq!(fell[0]["failure"], failure);
        assert_eq!(fell[0]["module"], "exposure");
    }
}

#[test]
fn a_spent_memory_budget_runs_steps_on_the_cpu() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let (g, node, cpu, engine) = setup(&gpu, 4);
    // Room for no image at all: every step falls back, none crashes.
    gpu.set_budget(Some(64));
    let out = engine.render(&g, node, "output", REGION);
    gpu.set_budget(None);
    close(&out.unwrap(), &cpu);
    let n = engine.counters();
    assert_eq!((n.fallbacks, n.accelerated), (3, 0), "{n:?}");
    let fell = events(&engine, "engine.gpu_fell_back");
    assert!(
        fell[0]["reason"]
            .as_str()
            .unwrap()
            .contains("GPU memory budget"),
        "{fell:?}"
    );
    assert!(gpu.memory().used <= gpu.memory().budget);
}

#[test]
fn every_buffer_is_counted_and_given_back() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let before = gpu.memory().used;
    {
        let (g, node, cpu, engine) = setup(&gpu, 5);
        close(&engine.render(&g, node, "output", REGION).unwrap(), &cpu);
        // The engine's cache keeps the steps' results on the device.
        assert!(gpu.memory().used > before);
    }
    // The engine gone, its device memory is back.
    assert_eq!(gpu.memory().used, before);
}

#[test]
fn a_hung_device_is_rebuilt_and_the_render_replayed() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let rebuilds = gpu.rebuilds();
    let (g, node, cpu, engine) = setup(&gpu, 6);
    // The readback of the result waits past its deadline.
    gpu.inject(Fault::Timeout);
    let out = engine.render(&g, node, "output", REGION).unwrap();
    close(&out, &cpu);
    assert_eq!(gpu.rebuilds(), rebuilds + 1);
    assert!(gpu.lost().is_none(), "{:?}", gpu.lost());
    let lost = events(&engine, "engine.gpu_lost");
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0]["failure"], "timeout");
    assert_eq!(lost[0]["recovered"], true);
    // Replayed on the new device: every step on the GPU again.
    assert_eq!(engine.counters().accelerated, 6);
    assert!(engine.gpu_off().is_none());
}

#[test]
fn a_lost_device_is_rebuilt_and_the_render_replayed() {
    let _gpu = helios_testkit::gpu_lock();
    let Some(gpu) = gpu() else { return };
    let rebuilds = gpu.rebuilds();
    let (g, node, cpu, engine) = setup(&gpu, 7);
    // A render before the loss leaves content on the device; destroying
    // it is a real loss, as a driver reset is.
    engine.render(&g, node, "output", REGION).unwrap();
    gpu.inject(Fault::DeviceLost);
    assert!(gpu.lost().is_some());
    let (h, last, cpu_h, _) = setup(&gpu, 8);
    let out = engine.render(&h, last, "output", REGION).unwrap();
    close(&out, &cpu_h);
    assert_eq!(gpu.rebuilds(), rebuilds + 1);
    assert!(gpu.lost().is_none(), "{:?}", gpu.lost());
    let lost = events(&engine, "engine.gpu_lost");
    assert_eq!(lost[0]["failure"], "device_lost");
    assert_eq!(lost[0]["recovered"], true);
    // What the engine cached on the lost device was dropped: the first
    // chain renders again, on the new device.
    close(&engine.render(&g, node, "output", REGION).unwrap(), &cpu);
}
