//! A failing accelerator never fails a render: a step whose kernel fails
//! runs on the CPU, memory over budget evicts device content first, a lost
//! or hung device is rebuilt and the render replayed, or the engine goes on
//! without it. A fake accelerator injects the failures (its "device
//! memory" is host memory, its kernel computes what the CPU does), so every
//! result is compared with a CPU engine's, bit for bit.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

/// Where an injected failure strikes.
#[derive(Clone, Copy, PartialEq, Debug)]
enum At {
    Upload,
    Run,
    Download,
}

/// Bytes of the fake's memory held by live images.
struct Charge {
    bytes: u64,
    used: Arc<AtomicU64>,
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

/// A fake device image: its pixels, the device generation it lives on,
/// and its share of the budget.
struct Held {
    image: Image,
    generation: u64,
    _charge: Charge,
}

/// An accelerator that fails on demand.
#[derive(Default)]
struct Faulty {
    /// Failures to inject, each once, in order.
    faults: Mutex<Vec<(At, Failure)>>,
    /// Lost (or hung): every call fails until it is rebuilt.
    lost: Mutex<Option<Failure>>,
    /// Bumped by each rebuild: images of an older device are gone.
    generation: AtomicU64,
    /// Whether `recover` rebuilds; how often it was asked.
    recovers: bool,
    recoveries: AtomicU64,
    /// The memory budget, in bytes (0: none), and what is in use.
    budget: AtomicU64,
    used: Arc<AtomicU64>,
    runs: AtomicU64,
}

impl Faulty {
    fn recovering() -> Faulty {
        Faulty {
            recovers: true,
            ..Faulty::default()
        }
    }

    fn inject(&self, at: At, failure: Failure) {
        self.faults.lock().unwrap().push((at, failure));
    }

    /// The failure due here, if any; a device failure loses the device.
    fn strike(&self, at: At) -> Result<(), AcceleratorError> {
        if let Some(failure) = *self.lost.lock().unwrap() {
            return Err(AcceleratorError::new(failure, "the device is gone"));
        }
        let mut faults = self.faults.lock().unwrap();
        let Some(i) = faults.iter().position(|(a, _)| *a == at) else {
            return Ok(());
        };
        let (_, failure) = faults.remove(i);
        if failure.is_device() {
            *self.lost.lock().unwrap() = Some(failure);
        }
        Err(AcceleratorError::new(
            failure,
            format!("injected at {at:?}"),
        ))
    }

    fn hold(&self, image: Image) -> Result<DeviceImage, AcceleratorError> {
        let bytes = image.bytes() as u64;
        let budget = self.budget.load(Ordering::SeqCst);
        let used = self.used.fetch_add(bytes, Ordering::SeqCst) + bytes;
        if budget > 0 && used > budget {
            self.used.fetch_sub(bytes, Ordering::SeqCst);
            return Err(AcceleratorError::new(
                Failure::OutOfMemory,
                format!("{bytes} bytes over the budget"),
            ));
        }
        Ok(DeviceImage::new(
            image.region(),
            image.channels(),
            Held {
                image,
                generation: self.generation.load(Ordering::SeqCst),
                _charge: Charge {
                    bytes,
                    used: self.used.clone(),
                },
            },
        ))
    }

    fn read<'a>(&self, image: &'a DeviceImage) -> Result<&'a Image, AcceleratorError> {
        let held = image.handle::<Held>().ok_or("not mine")?;
        if held.generation != self.generation.load(Ordering::SeqCst) {
            return Err(AcceleratorError::new(
                Failure::DeviceLost,
                "an image of the lost device",
            ));
        }
        Ok(&held.image)
    }
}

impl Accelerator for Faulty {
    fn id(&self) -> &'static str {
        "faulty"
    }

    fn upload(&self, image: &Image) -> Result<DeviceImage, AcceleratorError> {
        self.strike(At::Upload)?;
        self.hold(image.clone())
    }

    fn download(&self, image: &DeviceImage) -> Result<Image, AcceleratorError> {
        self.strike(At::Download)?;
        Ok(self.read(image)?.clone())
    }

    fn run(
        &self,
        _: &str,
        job: &GpuJob,
        input: &DeviceImage,
        output: Region,
        channels: u32,
    ) -> Result<DeviceImage, AcceleratorError> {
        self.strike(At::Run)?;
        let input = self.read(input)?.sample(output);
        let data = input.data().iter().map(|v| v * job.uniforms[0]).collect();
        let out = self.hold(Image::new(output, channels, data))?;
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(out)
    }

    fn memory(&self) -> Option<Memory> {
        Some(Memory {
            used: self.used.load(Ordering::SeqCst),
            budget: self.budget.load(Ordering::SeqCst),
        })
    }

    fn recover(&self) -> Result<(), String> {
        self.recoveries.fetch_add(1, Ordering::SeqCst);
        if !self.recovers {
            return Err("the driver refused a new device".into());
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.lost.lock().unwrap() = None;
        Ok(())
    }
}

const SIZE: u32 = 16;

/// A source, then `n` gains of 2, 3, 4...: (graph, last node).
fn chain(seed: u32, n: usize) -> (Graph, NodeId) {
    let mut g = Graph::new();
    let mut prev = g.add_source(
        "src",
        Image::from_fn(Region::sized(SIZE, SIZE), 1, move |x, y, _| {
            (x * 3 + y + i64::from(seed)) as f32
        }),
    );
    let mut port = graph::SOURCE_PORT;
    for i in 0..n {
        let params = Params::new().with("gain", Value::Float(2.0 + i as f64));
        let node = g
            .add_module(&format!("g{i}"), ModuleRef::new(Gain), &params)
            .unwrap();
        g.connect(prev, port, node, "input").unwrap();
        prev = node;
        port = "output";
    }
    (g, prev)
}

fn cpu(g: &Graph, node: NodeId) -> Image {
    Engine::new(Arc::new(Bus::new()))
        .render(g, node, "output", Region::sized(SIZE, SIZE))
        .unwrap()
}

fn engine(fake: &Arc<Faulty>) -> Engine {
    Engine::new(Arc::new(Bus::new())).with_accelerator(fake.clone())
}

fn render(engine: &Engine, g: &Graph, node: NodeId) -> Image {
    engine
        .render(g, node, "output", Region::sized(SIZE, SIZE))
        .unwrap()
}

/// The same pixels, bit for bit, over the same region.
fn same(a: &Image, b: &Image) {
    assert_eq!((a.region(), a.channels()), (b.region(), b.channels()));
    assert_eq!(a.data(), b.data());
}

/// The payloads of the events of a kind, in order.
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
    for failure in [Failure::Compile, Failure::Dispatch, Failure::Other] {
        let fake = Arc::new(Faulty::default());
        let e = engine(&fake);
        let (g, last) = chain(1, 3);
        fake.inject(At::Run, failure);
        let out = render(&e, &g, last);
        same(&out, &cpu(&g, last));
        let n = e.counters();
        // The first step fell back; the next two ran on the device.
        assert_eq!((n.fallbacks, n.accelerated), (1, 2), "{failure:?}");
        let fell = events(&e, "engine.gpu_fell_back");
        assert_eq!(fell.len(), 1);
        assert_eq!(fell[0]["name"], "g0");
        assert_eq!(fell[0]["module"], "gain");
        assert_eq!(fell[0]["failure"], serde_json::to_value(failure).unwrap());
        assert_eq!(fell[0]["message"]["key"], "engine-gpu-fell-back");
        assert!(
            fell[0]["message"]["english"]
                .as_str()
                .is_some_and(|m| m.starts_with("gain ran on the CPU"))
        );
        assert!(e.fallback_reason().unwrap().contains("injected"));
        assert!(e.gpu_off().is_none());
    }
}

#[test]
fn an_upload_that_fails_runs_its_step_on_the_cpu() {
    let fake = Arc::new(Faulty::default());
    let e = engine(&fake);
    let (g, last) = chain(2, 2);
    fake.inject(At::Upload, Failure::Other);
    same(&render(&e, &g, last), &cpu(&g, last));
    let n = e.counters();
    // The first step could not read its source on the device; the second
    // reads the first's host result, uploaded.
    assert_eq!((n.fallbacks, n.accelerated), (1, 1));
}

#[test]
fn memory_over_budget_evicts_device_content_first() {
    let fake = Arc::new(Faulty::default());
    let e = engine(&fake);
    let image = u64::from(SIZE * SIZE * 4);
    // Room for a step's input and output, and one more image.
    fake.budget.store(3 * image, Ordering::SeqCst);
    let (a, last_a) = chain(3, 3);
    same(&render(&e, &a, last_a), &cpu(&a, last_a));
    assert_eq!(e.counters().fallbacks, 0);
    // The first chain's device content stays cached, holding the budget:
    // another chain needs it back.
    let held = fake.used.load(Ordering::SeqCst);
    assert!(held > 0, "device content is cached");
    let before = e.counters();
    let (b, last_b) = chain(4, 3);
    same(&render(&e, &b, last_b), &cpu(&b, last_b));
    let n = e.counters().since(&before);
    assert_eq!(n.fallbacks, 0, "evicting made room: {n:?}");
    assert_eq!(n.accelerated, 3);
    assert!(n.evictions > 0, "{n:?}");
    assert!(fake.used.load(Ordering::SeqCst) <= 3 * image);
}

#[test]
fn memory_too_small_for_a_step_runs_it_on_the_cpu() {
    let fake = Arc::new(Faulty::default());
    let e = engine(&fake);
    // Not even one image fits.
    fake.budget.store(16, Ordering::SeqCst);
    let (g, last) = chain(5, 2);
    same(&render(&e, &g, last), &cpu(&g, last));
    let n = e.counters();
    assert_eq!((n.fallbacks, n.accelerated), (2, 0));
    let fell = events(&e, "engine.gpu_fell_back");
    assert_eq!(fell[0]["failure"], "out_of_memory");
}

#[test]
fn a_timeout_rebuilds_the_device_and_replays_the_render() {
    let fake = Arc::new(Faulty::recovering());
    let e = engine(&fake);
    let (g, last) = chain(6, 3);
    // The render's result does not come back in time.
    fake.inject(At::Download, Failure::Timeout);
    same(&render(&e, &g, last), &cpu(&g, last));
    assert_eq!(fake.recoveries.load(Ordering::SeqCst), 1);
    let n = e.counters();
    assert_eq!(n.device_losses, 1);
    // Computed twice on the device: before and after the rebuild.
    assert_eq!(n.accelerated, 6, "{n:?}");
    let lost = events(&e, "engine.gpu_lost");
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0]["failure"], "timeout");
    assert_eq!(lost[0]["recovered"], true);
    assert!(e.gpu_off().is_none());
}

#[test]
fn a_lost_device_that_cannot_be_rebuilt_leaves_the_cpu_for_the_session() {
    let fake = Arc::new(Faulty::default());
    let e = engine(&fake);
    let (g, last) = chain(7, 3);
    fake.inject(At::Run, Failure::DeviceLost);
    same(&render(&e, &g, last), &cpu(&g, last));
    assert_eq!(fake.recoveries.load(Ordering::SeqCst), 1);
    let off = e.gpu_off().expect("the GPU is off");
    assert!(
        off.contains("the rest of this session renders on the CPU"),
        "{off}"
    );
    let lost = events(&e, "engine.gpu_lost");
    assert_eq!(lost[0]["recovered"], false);
    assert_eq!(lost[0]["message"]["english"], off.as_str());
    assert_eq!(lost[0]["message"]["key"], "engine-gpu-lost");
    // Later renders do not try the device again.
    let runs = fake.runs.load(Ordering::SeqCst);
    let (h, last_h) = chain(8, 2);
    same(&render(&e, &h, last_h), &cpu(&h, last_h));
    assert_eq!(fake.runs.load(Ordering::SeqCst), runs);
    assert_eq!(e.counters().accelerated, 0);
    assert_eq!(
        e.render_device(&h, last_h, "output", Region::sized(SIZE, SIZE))
            .unwrap_err(),
        Error::NoAccelerator
    );
}

#[test]
fn a_device_lost_again_after_its_rebuild_leaves_the_cpu() {
    let fake = Arc::new(Faulty::recovering());
    let e = engine(&fake);
    let (g, last) = chain(9, 2);
    fake.inject(At::Run, Failure::DeviceLost);
    fake.inject(At::Run, Failure::DeviceLost);
    same(&render(&e, &g, last), &cpu(&g, last));
    // Rebuilt once; lost again, the engine stops asking.
    assert_eq!(fake.recoveries.load(Ordering::SeqCst), 1);
    assert!(e.gpu_off().is_some());
    let lost = events(&e, "engine.gpu_lost");
    assert_eq!(lost.len(), 2);
    assert_eq!(lost[0]["recovered"], true);
    assert_eq!(lost[1]["recovered"], false);
    assert_eq!(e.counters().device_losses, 2);
}

#[test]
fn a_lost_device_drops_its_cached_content() {
    let fake = Arc::new(Faulty::recovering());
    let e = engine(&fake);
    let (g, last) = chain(10, 2);
    same(&render(&e, &g, last), &cpu(&g, last));
    // A longer chain over the same first steps: their device content is
    // cached, but the device is lost before it is read.
    let (g3, last3) = chain(10, 3);
    fake.inject(At::Run, Failure::DeviceLost);
    same(&render(&e, &g3, last3), &cpu(&g3, last3));
    assert!(e.gpu_off().is_none());
    assert_eq!(fake.recoveries.load(Ordering::SeqCst), 1);
}
