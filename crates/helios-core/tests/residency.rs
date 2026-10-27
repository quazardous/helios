//! Images stay in the accelerator's memory along a chain of accelerated
//! steps: counted uploads and readbacks, with a fake accelerator whose
//! "device memory" is host memory, so no GPU is needed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use helios_core::*;

/// Multiplies by `gain`; has a (fake) GPU variant.
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
        let input = inputs[0].sample(output);
        Ok(vec![Image::new(
            output,
            1,
            input.data().iter().map(|v| v * gain).collect(),
        )])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: "",
            entry: "gain",
        })
    }

    fn gpu_uniforms(&self, gain: &f32) -> Vec<f32> {
        vec![*gain]
    }
}

/// Adds one; CPU only.
struct Offset;

impl Module for Offset {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "offset".into(),
            version: 1,
            description: String::new(),
            params: vec![],
            inputs: vec![PortSpec::image("input", 1)],
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
        let input = inputs[0].sample(output);
        Ok(vec![Image::new(
            output,
            1,
            input.data().iter().map(|v| v + 1.0).collect(),
        )])
    }
}

/// Holds "device" images as host images; runs `gain` kernels. Counts its
/// own transfers, to check the engine's counters against.
#[derive(Default)]
struct Fake {
    uploads: AtomicU64,
    downloads: AtomicU64,
}

impl Accelerator for Fake {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn upload(&self, image: &Image) -> Result<DeviceImage, helios_core::AcceleratorError> {
        self.uploads.fetch_add(1, Ordering::SeqCst);
        Ok(DeviceImage::new(
            image.region(),
            image.channels(),
            image.clone(),
        ))
    }

    fn download(&self, image: &DeviceImage) -> Result<Image, helios_core::AcceleratorError> {
        self.downloads.fetch_add(1, Ordering::SeqCst);
        Ok(image.handle::<Image>().ok_or("not mine")?.clone())
    }

    fn run(
        &self,
        _: &str,
        job: &GpuJob,
        input: &DeviceImage,
        output: Region,
        channels: u32,
    ) -> Result<DeviceImage, helios_core::AcceleratorError> {
        let input = input.handle::<Image>().ok_or("not mine")?.sample(output);
        let data = input.data().iter().map(|v| v * job.uniforms[0]).collect();
        Ok(DeviceImage::new(
            output,
            channels,
            Image::new(output, channels, data),
        ))
    }
}

const SIZE: u32 = 16;
const BYTES: usize = (SIZE * SIZE * 4) as usize;

/// A source, then the modules in order: (graph, last node).
fn chain(modules: &[ModuleRef]) -> (Graph, NodeId) {
    let mut g = Graph::new();
    let mut prev = g.add_source(
        "src",
        Image::from_fn(Region::sized(SIZE, SIZE), 1, |x, y, _| (x + y) as f32),
    );
    let mut port = graph::SOURCE_PORT;
    for (i, m) in modules.iter().enumerate() {
        let params = match m.manifest().name.as_str() {
            "gain" => Params::new().with("gain", Value::Float(2.0)),
            _ => Params::new(),
        };
        let n = g.add_module(&format!("m{i}"), m.clone(), &params).unwrap();
        g.connect(prev, port, n, "input").unwrap();
        prev = n;
        port = "output";
    }
    (g, prev)
}

fn accelerated() -> (Engine, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    let engine = Engine::new(Arc::new(Bus::new())).with_accelerator(fake.clone());
    (engine, fake)
}

fn region() -> Region {
    Region::sized(SIZE, SIZE)
}

#[test]
fn a_chain_of_accelerated_steps_uploads_once_and_reads_back_once() {
    let gain = ModuleRef::new(Gain);
    let (g, last) = chain(&[gain.clone(), gain.clone(), gain]);
    let cpu = Engine::new(Arc::new(Bus::new()))
        .render(&g, last, "output", region())
        .unwrap();
    let (engine, fake) = accelerated();
    let out = engine.render(&g, last, "output", region()).unwrap();
    assert_eq!(out.data(), cpu.data());
    let c = engine.counters();
    assert_eq!((c.accelerated, c.uploads, c.readbacks), (3, 1, 1), "{c:?}");
    assert_eq!(fake.uploads.load(Ordering::SeqCst), 1);
    assert_eq!(fake.downloads.load(Ordering::SeqCst), 1);
    // The source in host memory, three device images, the read back copy.
    assert_eq!(engine.describe()["cache"]["bytes"], 5 * BYTES as u64);

    // Asked again: from the cache, read back copy included.
    let before = engine.counters();
    engine.render(&g, last, "output", region()).unwrap();
    let d = engine.counters().since(&before);
    assert_eq!((d.computed, d.uploads, d.readbacks), (0, 0, 0), "{d:?}");
}

#[test]
fn a_cpu_step_in_the_middle_reads_back_and_uploads_again() {
    let gain = ModuleRef::new(Gain);
    let (g, last) = chain(&[gain.clone(), ModuleRef::new(Offset), gain]);
    let cpu = Engine::new(Arc::new(Bus::new()))
        .render(&g, last, "output", region())
        .unwrap();
    let (engine, _) = accelerated();
    let out = engine.render(&g, last, "output", region()).unwrap();
    assert_eq!(out.data(), cpu.data());
    let c = engine.counters();
    assert_eq!((c.accelerated, c.uploads, c.readbacks), (2, 2, 2), "{c:?}");
}

#[test]
fn a_device_render_is_not_read_back() {
    let gain = ModuleRef::new(Gain);
    let (g, last) = chain(&[gain.clone(), gain]);
    let cpu = Engine::new(Arc::new(Bus::new()))
        .render(&g, last, "output", region())
        .unwrap();
    let (engine, _) = accelerated();
    let out = engine.render_device(&g, last, "output", region()).unwrap();
    assert_eq!((out.region(), out.channels()), (region(), 1));
    assert_eq!(out.handle::<Image>().unwrap().data(), cpu.data());
    let c = engine.counters();
    assert_eq!((c.uploads, c.readbacks), (1, 0), "{c:?}");

    // A result computed on the CPU is uploaded.
    let (g, last) = chain(&[ModuleRef::new(Offset)]);
    let before = engine.counters();
    engine.render_device(&g, last, "output", region()).unwrap();
    let d = engine.counters().since(&before);
    assert_eq!((d.accelerated, d.uploads, d.readbacks), (0, 1, 0), "{d:?}");
}

#[test]
fn a_device_render_needs_an_accelerator() {
    let (g, last) = chain(&[ModuleRef::new(Gain)]);
    let err = Engine::new(Arc::new(Bus::new()))
        .render_device(&g, last, "output", region())
        .unwrap_err();
    assert_eq!(err, Error::NoAccelerator);
}

#[test]
fn an_accelerator_attached_later_runs_the_next_renders() {
    let gain = ModuleRef::new(Gain);
    let (g, last) = chain(&[gain.clone(), gain]);
    let engine = Engine::new(Arc::new(Bus::new()));
    let cpu = engine.render(&g, last, "output", region()).unwrap();
    assert_eq!(engine.counters().accelerated, 0);

    let fake = Arc::new(Fake::default());
    assert!(engine.attach_accelerator(fake.clone()));
    // One accelerator in an engine's life.
    assert!(!engine.attach_accelerator(Arc::new(Fake::default())));
    let out = engine.render_device(&g, last, "output", region()).unwrap();
    assert_eq!(out.handle::<Image>().unwrap().data(), cpu.data());
    let c = engine.counters();
    assert_eq!((c.accelerated, c.uploads, c.readbacks), (2, 1, 0), "{c:?}");
    assert_eq!(fake.uploads.load(Ordering::SeqCst), 1);
}

#[test]
fn the_cpu_path_moves_nothing() {
    let (g, last) = chain(&[ModuleRef::new(Gain), ModuleRef::new(Offset)]);
    let engine = Engine::new(Arc::new(Bus::new()));
    engine.render(&g, last, "output", region()).unwrap();
    let c = engine.counters();
    assert_eq!((c.accelerated, c.uploads, c.readbacks), (0, 0, 0), "{c:?}");
}
