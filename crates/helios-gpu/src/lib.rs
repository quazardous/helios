//! The GPU backend: modules' WGSL variants run on wgpu (Vulkan on Linux and
//! Windows, Metal on macOS), on the same plans as their CPU code.
//!
//! A [`Gpu`] holds a device. A process has one, opened through [`shared`]
//! ([`shared::gpu`]), which the GUI's window draws with as well, so the
//! viewport and the engine share it and computed buffers need no copy to
//! be shown. [`Gpu::run`] uploads the input, runs the module's kernel and
//! reads the result back. A [`Gpu`] is also the engine's [`Accelerator`]:
//! an engine given one (`Engine::with_accelerator`) computes every step
//! that has a GPU variant on it, and images stay in GPU buffers from one
//! such step to the next. `Engine::render_device` hands a render's result
//! as a buffer on the device ([`Gpu::buffer`]), for a viewport that shares
//! it.
//!
//! A failure is typed for the engine ([`Failure`]): a kernel that does not
//! compile or run, memory over the device's budget ([`default_budget`],
//! every buffer counted), a wait past its deadline ([`DEFAULT_TIMEOUT`]:
//! no wait is unbounded) or a lost device (wgpu's callback). After a loss
//! or a hang the device is rebuilt on request ([`Accelerator::recover`]),
//! at most [`MAX_REBUILDS`] times. [`Gpu::inject`] provokes each failure,
//! for tests.
//!
//! [`check`] runs a module on both backends and compares: a GPU variant is
//! only trusted within its tolerance of the CPU reference.

pub mod shared;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use helios_core::{
    Accelerator, AcceleratorError, Context, DeviceImage, Error, Failure, GpuJob, Image, Memory,
    ModuleRef, Params, Region,
};
use wgpu::util::DeviceExt;

/// How long a wait for the device may take before the device counts as
/// hung (a readback waits for every kernel before it). Windows resets a
/// GPU that does not answer within about 2 s; a slow software device on a
/// large image needs more.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// How many times a device is rebuilt after a loss in a process; past
/// that, the GPU is given up for the session.
pub const MAX_REBUILDS: u32 = 2;

/// The limits [`Gpu::open`] asks a device for: all the adapter allows.
/// The compute manager plans with them before any device is open.
pub fn requested_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    adapter.limits()
}

/// The limits [`Gpu::open`] tries, in order: the adapter's, then wgpu's
/// defaults with the adapter's buffer sizes (what images need), then
/// wgpu's defaults.
fn limit_steps(adapter: &wgpu::Adapter) -> [(&'static str, wgpu::Limits); 3] {
    let full = requested_limits(adapter);
    let buffers = wgpu::Limits {
        max_buffer_size: full.max_buffer_size,
        max_storage_buffer_binding_size: full.max_storage_buffer_binding_size,
        ..wgpu::Limits::default()
    };
    [
        ("the adapter's limits", full),
        ("the adapter's buffer sizes", buffers),
        ("wgpu's default limits", wgpu::Limits::default()),
    ]
}

/// The memory a device may hold by default, in bytes, from what is known
/// without trusting a driver's memory figure (wgpu reports none, and
/// drivers report it wrongly; see docs/darktable-pitfalls/gpu-perf.md):
/// four of the largest buffers a kernel binds (a step's input and output,
/// and two images kept for the next steps), at most a quarter of the
/// machine's memory (`memory`, 2 GiB when not known), which an integrated
/// GPU shares. A device setting overrides it.
pub fn default_budget(limits: &wgpu::Limits, memory: Option<u64>) -> u64 {
    let binding = limits
        .max_storage_buffer_binding_size
        .min(limits.max_buffer_size);
    let by_limits = binding.saturating_mul(4);
    let by_memory = memory.map_or(2 << 30, |m| m / 4);
    by_limits.min(by_memory).max(1 << 20)
}

/// A failure to provoke on purpose, to test what happens then without a
/// broken driver (see [`Gpu::inject`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The next kernel to compile does not.
    Compile,
    /// The next kernel to run does not.
    Dispatch,
    /// The next allocation is refused, as over the memory budget.
    OutOfMemory,
    /// The next readback waits past its deadline: the device counts as hung.
    Timeout,
    /// The device is destroyed now: a real loss, which wgpu reports as one.
    DeviceLost,
}

/// A device and what lives on it; replaced whole when the device is
/// rebuilt after a loss.
struct Live {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Compiled kernels, by source and entry point.
    pipelines: Mutex<HashMap<(&'static str, &'static str), wgpu::ComputePipeline>>,
    /// Why the device is unusable, once it is: lost (wgpu's callback), or
    /// hung past a wait's deadline.
    lost: Arc<Mutex<Option<String>>>,
}

impl Live {
    fn new(device: wgpu::Device, queue: wgpu::Queue) -> Live {
        let lost: Arc<Mutex<Option<String>>> = Arc::default();
        let flag = lost.clone();
        device.set_device_lost_callback(move |reason, message| {
            let mut lost = flag.lock().unwrap_or_else(PoisonError::into_inner);
            lost.get_or_insert_with(|| format!("the device was lost ({reason:?}): {message}"));
        });
        // wgpu panics on an error no scope caught; the GUI draws on this
        // device too, so such an error is reported, never fatal.
        device.on_uncaptured_error(Arc::new(|e| {
            eprintln!("helios-gpu: an error outside any check: {e}");
        }));
        Live {
            device,
            queue,
            pipelines: Mutex::default(),
            lost,
        }
    }

    fn lost(&self) -> Option<String> {
        self.lost
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn mark_lost(&self, why: String) {
        self.lost
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(why);
    }

    /// A wgpu error as the engine needs it: the device's loss first, then
    /// memory, else `otherwise`.
    fn failure(&self, e: &wgpu::Error, otherwise: Failure) -> AcceleratorError {
        if let Some(why) = self.lost() {
            return AcceleratorError::new(Failure::DeviceLost, why);
        }
        let failure = match e {
            wgpu::Error::OutOfMemory { .. } => Failure::OutOfMemory,
            _ => otherwise,
        };
        AcceleratorError::new(failure, e.to_string())
    }

    /// Runs `f` with every kind of wgpu error caught: its result, and the
    /// first error (out of memory before the others).
    fn scoped<T>(&self, f: impl FnOnce() -> T) -> (T, Option<wgpu::Error>) {
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let out = f();
        let internal = pollster::block_on(internal.pop());
        let memory = pollster::block_on(memory.pop());
        let validation = pollster::block_on(validation.pop());
        (out, memory.or(validation).or(internal))
    }
}

/// Bytes of device memory in use, against a budget.
struct Ledger {
    used: AtomicU64,
    budget: AtomicU64,
}

/// A share of the ledger, given back when the buffer it pays for goes.
struct Charge {
    ledger: Arc<Ledger>,
    bytes: u64,
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.ledger.used.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

pub struct Gpu {
    wgpu_adapter: wgpu::Adapter,
    /// The adapter's description, for reports.
    pub adapter: String,
    /// Why the device has smaller limits than its adapter, when the driver
    /// refused the adapter's own (see [`Gpu::open`]).
    pub limits_note: Option<String>,
    id: String,
    live: RwLock<Arc<Live>>,
    /// Held while the device is rebuilt, so it is rebuilt once a loss.
    rebuilding: Mutex<()>,
    rebuilds: AtomicU32,
    /// Why the device is given up for the session, once it is.
    dead: Mutex<Option<String>>,
    ledger: Arc<Ledger>,
    default_budget: u64,
    timeout_ms: AtomicU64,
    faults: Mutex<Vec<Fault>>,
}

impl Gpu {
    /// Opens a device on an adapter (through [`shared`], once a process),
    /// with the adapter's own limits ([`requested_limits`]): wgpu's
    /// defaults would cap a storage binding at 128 MiB, below a 24 MP f32
    /// image. A driver that refuses its adapter's maximum is asked for
    /// less, buffer sizes first, and [`Gpu::limits_note`] says so; the
    /// device's real limits are [`Gpu::limits`]. Its memory budget is
    /// [`default_budget`] until set.
    pub(crate) fn open(adapter: &wgpu::Adapter) -> Result<Gpu, String> {
        let info = adapter.get_info();
        let (device, queue, limits_note) = Self::request(adapter)?;
        let described = format!("{} ({:?}, {:?})", info.name, info.device_type, info.backend);
        let default_budget =
            default_budget(&device.limits(), helios_core::engine::physical_memory());
        Ok(Gpu {
            wgpu_adapter: adapter.clone(),
            id: format!("wgpu {described}"),
            adapter: described,
            limits_note,
            live: RwLock::new(Arc::new(Live::new(device, queue))),
            rebuilding: Mutex::new(()),
            rebuilds: AtomicU32::new(0),
            dead: Mutex::new(None),
            ledger: Arc::new(Ledger {
                used: AtomicU64::new(0),
                budget: AtomicU64::new(default_budget),
            }),
            default_budget,
            timeout_ms: AtomicU64::new(DEFAULT_TIMEOUT.as_millis() as u64),
            faults: Mutex::default(),
        })
    }

    /// A device on `adapter`, with the most limits the driver accepts, and
    /// why they are less than the adapter's when they are.
    fn request(
        adapter: &wgpu::Adapter,
    ) -> Result<(wgpu::Device, wgpu::Queue, Option<String>), String> {
        let mut refused = Vec::new();
        for (name, limits) in limit_steps(adapter) {
            match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("helios"),
                required_limits: limits,
                ..Default::default()
            })) {
                Ok((device, queue)) => {
                    let note = (!refused.is_empty()).then(|| {
                        format!(
                            "opened with {name}: the driver refused {}",
                            refused.join("; ")
                        )
                    });
                    return Ok((device, queue, note));
                }
                Err(e) => refused.push(format!("{name} ({e})")),
            }
        }
        Err(format!("no GPU device: {}", refused.join("; ")))
    }

    /// The device as it is now (a new one after a rebuild).
    fn current(&self) -> Arc<Live> {
        self.live
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The device, if it is usable.
    fn live(&self) -> Result<Arc<Live>, AcceleratorError> {
        if let Some(why) = self
            .dead
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return Err(AcceleratorError::new(Failure::DeviceLost, why));
        }
        let live = self.current();
        if let Some(why) = live.lost() {
            return Err(AcceleratorError::new(Failure::DeviceLost, why));
        }
        Ok(live)
    }

    /// What the device allows: the limits the engine's jobs must fit.
    pub fn limits(&self) -> wgpu::Limits {
        self.current().device.limits()
    }

    /// What a window needs to draw with this device (eframe's
    /// `WgpuSetupExisting`): the process's instance, the adapter, the
    /// device and its queue.
    pub fn handles(&self) -> Handles {
        let live = self.current();
        Handles {
            instance: shared::instance().clone(),
            adapter: self.wgpu_adapter.clone(),
            device: live.device.clone(),
            queue: live.queue.clone(),
        }
    }

    /// Makes a failure happen (see [`Fault`]), to test what follows.
    /// [`Fault::DeviceLost`] destroys the device at once; the others strike
    /// the next operation they name.
    pub fn inject(&self, fault: Fault) {
        if fault == Fault::DeviceLost {
            let live = self.current();
            live.device.destroy();
            live.mark_lost("the device was destroyed (injected)".into());
            return;
        }
        self.faults
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(fault);
    }

    /// Whether `fault` was injected; it is consumed.
    fn struck(&self, fault: Fault) -> bool {
        let mut faults = self.faults.lock().unwrap_or_else(PoisonError::into_inner);
        match faults.iter().position(|f| *f == fault) {
            Some(i) => {
                faults.remove(i);
                true
            }
            None => false,
        }
    }

    /// How long a wait for the device may take ([`DEFAULT_TIMEOUT`] until
    /// set).
    pub fn set_timeout(&self, timeout: Duration) {
        self.timeout_ms
            .store(timeout.as_millis() as u64, Ordering::SeqCst);
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms.load(Ordering::SeqCst))
    }

    /// Sets the memory budget in bytes; `None`: [`default_budget`].
    pub fn set_budget(&self, bytes: Option<u64>) {
        self.ledger
            .budget
            .store(bytes.unwrap_or(self.default_budget), Ordering::SeqCst);
    }

    /// Device memory in use by images and transfers, and its budget.
    pub fn memory(&self) -> Memory {
        Memory {
            used: self.ledger.used.load(Ordering::SeqCst),
            budget: self.ledger.budget.load(Ordering::SeqCst),
        }
    }

    /// How many times the device was rebuilt after a loss.
    pub fn rebuilds(&self) -> u32 {
        self.rebuilds.load(Ordering::SeqCst)
    }

    /// Why the device is not usable now (lost, hung, or given up), if it
    /// is not.
    pub fn lost(&self) -> Option<String> {
        self.live().err().map(|e| e.reason)
    }

    /// Takes `bytes` of the memory budget, or says it is spent.
    fn reserve(&self, bytes: u64) -> Result<Charge, AcceleratorError> {
        let spent = |used: u64, budget: u64| {
            AcceleratorError::new(
                Failure::OutOfMemory,
                format!(
                    "{bytes} bytes more do not fit the GPU memory budget ({used} of {budget} bytes in use)"
                ),
            )
        };
        if self.struck(Fault::OutOfMemory) {
            return Err(AcceleratorError::new(
                Failure::OutOfMemory,
                "the allocation was refused (injected)",
            ));
        }
        let budget = self.ledger.budget.load(Ordering::SeqCst);
        let mut used = self.ledger.used.load(Ordering::SeqCst);
        loop {
            if used.saturating_add(bytes) > budget {
                return Err(spent(used, budget));
            }
            match self.ledger.used.compare_exchange(
                used,
                used + bytes,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Ok(Charge {
                        ledger: self.ledger.clone(),
                        bytes,
                    });
                }
                Err(now) => used = now,
            }
        }
    }

    /// Runs a module's GPU variant over `output`, reading `source` (edges
    /// replicated where the module reads past it, as the engine does; a
    /// module without inputs reads the engine's placeholder instead).
    /// `Ok(None)` when the module has none.
    pub fn run(
        &self,
        module: &ModuleRef,
        params: &Params,
        context: &Context,
        source: &Image,
        output: Region,
    ) -> Result<Option<Image>, Error> {
        let Some(job) = module.gpu_job(params, context, 1.0, output)? else {
            return Ok(None);
        };
        let m = module.manifest();
        // A module without inputs reads the engine's placeholder.
        let input = if m.inputs.is_empty() {
            ModuleRef::placeholder(output)
        } else {
            source.sample(job.input)
        };
        let channels = m
            .outputs
            .first()
            .and_then(helios_core::PortSpec::channels)
            .unwrap_or(input.channels());
        self.dispatch(&m.name, &job, &input, output, channels)
            .map(Some)
            .map_err(|e| Error::Module {
                module: m.name.clone(),
                reason: format!("GPU: {}", e.reason),
            })
    }

    /// The kernel's pipeline, compiled once a device. A kernel that does
    /// not compile is an error, never a panic.
    fn pipeline(
        &self,
        live: &Live,
        name: &str,
        job: &GpuJob,
    ) -> Result<wgpu::ComputePipeline, AcceleratorError> {
        if self.struck(Fault::Compile) {
            return Err(AcceleratorError::new(
                Failure::Compile,
                format!("kernel `{}`: did not compile (injected)", job.kernel.entry),
            ));
        }
        let key = (job.kernel.wgsl, job.kernel.entry);
        if let Some(p) = live
            .pipelines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return Ok(p.clone());
        }
        let d = &live.device;
        let (pipeline, error) = live.scoped(|| {
            let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(job.kernel.wgsl.into()),
            });
            d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: None,
                module: &shader,
                entry_point: Some(job.kernel.entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        });
        if let Some(e) = error {
            let mut e = live.failure(&e, Failure::Compile);
            e.reason = format!("kernel `{}`: {}", job.kernel.entry, e.reason);
            return Err(e);
        }
        live.pipelines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, pipeline.clone());
        Ok(pipeline)
    }

    /// The buffer behind an image this GPU holds (what
    /// `Engine::render_device` answers on an engine it accelerates); `None`
    /// for an image of another device, or of this GPU's device before it
    /// was rebuilt. Pixels are `f32`, channels interleaved, rows top to
    /// bottom over `image.region()`, tightly packed (`width * channels`
    /// floats a row); usages `STORAGE | COPY_SRC`. Read it, never write it:
    /// the engine's cache shares it.
    pub fn buffer<'a>(&self, image: &'a DeviceImage) -> Option<&'a wgpu::Buffer> {
        let live = self.current();
        image
            .handle::<Resident>()
            .filter(|r| r.device == live.device)
            .map(|r| &r.buffer)
    }

    /// The buffer behind `image` on the device `live`: an image of a lost
    /// device is gone with it.
    fn buffer_on<'a>(
        live: &Live,
        image: &'a DeviceImage,
    ) -> Result<&'a wgpu::Buffer, AcceleratorError> {
        match image.handle::<Resident>() {
            Some(r) if r.device == live.device => Ok(&r.buffer),
            Some(_) => Err(AcceleratorError::new(
                Failure::DeviceLost,
                "the image was on a device since lost",
            )),
            None => Err("the image is not in this GPU's memory".into()),
        }
    }

    /// Uploads `input`, runs the kernel over `output` and reads it back.
    fn dispatch(
        &self,
        name: &str,
        job: &GpuJob,
        input: &Image,
        output: Region,
        channels: u32,
    ) -> Result<Image, AcceleratorError> {
        let input = self.upload_image(input)?;
        let out = self.kernel(name, job, &input, output, channels)?;
        self.download_image(&out)
    }

    /// Copies a host image into a buffer on the device.
    fn upload_image(&self, image: &Image) -> Result<DeviceImage, AcceleratorError> {
        let live = self.live()?;
        let data: &[u8] = bytemuck::cast_slice(image.data());
        // A binding is never empty.
        let contents = if data.is_empty() { &[0; 4] } else { data };
        let charge = self.reserve(contents.len() as u64)?;
        let (buffer, error) = live.scoped(|| {
            live.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("image"),
                    contents,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                })
        });
        if let Some(e) = error {
            return Err(live.failure(&e, Failure::Other));
        }
        Ok(Self::resident(
            &live,
            image.region(),
            image.channels(),
            buffer,
            charge,
        ))
    }

    fn resident(
        live: &Live,
        region: Region,
        channels: u32,
        buffer: wgpu::Buffer,
        charge: Charge,
    ) -> DeviceImage {
        DeviceImage::new(
            region,
            channels,
            Resident {
                device: live.device.clone(),
                buffer,
                _charge: charge,
            },
        )
    }

    /// Runs the kernel over `output`, reading `input` on the device, into a
    /// new buffer on the device. Nothing is read back, nothing waited for.
    fn kernel(
        &self,
        name: &str,
        job: &GpuJob,
        input: &DeviceImage,
        output: Region,
        channels: u32,
    ) -> Result<DeviceImage, AcceleratorError> {
        let live = self.live()?;
        let input_buf = Self::buffer_on(&live, input)?;
        if input.region() != job.input {
            return Err(AcceleratorError::new(
                Failure::Dispatch,
                format!(
                    "input covers {}, the kernel reads {}",
                    input.region(),
                    job.input
                ),
            ));
        }
        let pipeline = self.pipeline(&live, name, job)?;
        if self.struck(Fault::Dispatch) {
            return Err(AcceleratorError::new(
                Failure::Dispatch,
                format!("kernel `{}`: did not run (injected)", job.kernel.entry),
            ));
        }
        let pixels = output.area() as u32;
        let (groups_x, groups_y) = grid(
            pixels,
            live.device.limits().max_compute_workgroups_per_dimension,
        )
        .map_err(|e| AcceleratorError::new(Failure::Dispatch, e))?;
        let out_size = (u64::from(pixels) * u64::from(channels) * 4).max(4);
        let charge = self.reserve(out_size)?;
        let d = &live.device;
        let (output_buf, error) = live.scoped(|| {
            let uniform = |data: &[u8], label: &str| {
                d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: data,
                    usage: wgpu::BufferUsages::UNIFORM,
                })
            };
            let output_buf = d.create_buffer(&wgpu::BufferDescriptor {
                label: Some("output"),
                size: out_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let plan_buf = uniform(bytemuck::cast_slice(&job.uniforms), "plan");
            let shape = [
                pixels,
                input.channels(),
                channels,
                groups_x * WORKGROUP,
                job.input.width,
                job.input.height,
                output.width,
                output.height,
                output.x as i32 as u32,
                output.y as i32 as u32,
                job.input.x as i32 as u32,
                job.input.y as i32 as u32,
            ];
            let shape_buf = uniform(bytemuck::cast_slice(&shape), "shape");
            let group = d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bindings"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: input_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: output_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: plan_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: shape_buf.as_entire_binding(),
                    },
                ],
            });
            let mut encoder = d.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(groups_x, groups_y, 1);
            }
            // Submitted, not waited for: the queue runs submissions in
            // order, so the next kernel or a readback sees this one's
            // result.
            live.queue.submit([encoder.finish()]);
            output_buf
        });
        if let Some(e) = error {
            return Err(live.failure(&e, Failure::Dispatch));
        }
        Ok(Self::resident(&live, output, channels, output_buf, charge))
    }

    /// Reads an image on the device back into host memory, waiting for the
    /// kernels that write it, at most [`Gpu::set_timeout`]: past that the
    /// device counts as hung.
    fn download_image(&self, image: &DeviceImage) -> Result<Image, AcceleratorError> {
        let live = self.live()?;
        let buffer = Self::buffer_on(&live, image)?;
        let size = (image.bytes() as u64).max(4);
        let _charge = self.reserve(size)?;
        let ((readback, index), error) = live.scoped(|| {
            let readback = live.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = live
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            encoder.copy_buffer_to_buffer(buffer, 0, &readback, 0, size);
            let index = live.queue.submit([encoder.finish()]);
            (readback, index)
        });
        if let Some(e) = error {
            return Err(live.failure(&e, Failure::Other));
        }
        let slice = readback.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let timeout = self.timeout();
        if self.struck(Fault::Timeout) {
            return Err(Self::hung(&live, timeout, " (injected)"));
        }
        let deadline = Instant::now() + timeout;
        match live.device.poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: Some(timeout),
        }) {
            Ok(_) => {}
            Err(wgpu::PollError::Timeout) => return Err(Self::hung(&live, timeout, "")),
            Err(e) => {
                return Err(match live.lost() {
                    Some(why) => AcceleratorError::new(Failure::DeviceLost, why),
                    None => AcceleratorError::new(Failure::Other, e.to_string()),
                });
            }
        }
        // The copy is done; the mapping's callback runs on a poll, which
        // the wait above normally was.
        let mapped = loop {
            match rx.try_recv() {
                Ok(r) => break r.map_err(|e| e.to_string()),
                Err(TryRecvError::Disconnected) => {
                    break Err("the mapping was dropped".to_string());
                }
                Err(TryRecvError::Empty) if Instant::now() >= deadline => {
                    return Err(Self::hung(&live, timeout, ""));
                }
                Err(TryRecvError::Empty) => {
                    let _ = live.device.poll(wgpu::PollType::Poll);
                    std::thread::yield_now();
                }
            }
        };
        if let Err(e) = mapped {
            return Err(match live.lost() {
                Some(why) => AcceleratorError::new(Failure::DeviceLost, why),
                None => AcceleratorError::new(Failure::Other, e),
            });
        }
        let view = slice
            .get_mapped_range()
            .map_err(|e| AcceleratorError::new(Failure::Other, e.to_string()))?;
        let data: Vec<f32> = bytemuck::cast_slice(&view[..image.bytes()]).to_vec();
        drop(view);
        readback.unmap();
        Ok(Image::new(image.region(), image.channels(), data))
    }

    /// The device did not answer within `timeout`: it counts as lost from
    /// now on, until rebuilt.
    fn hung(live: &Live, timeout: Duration, note: &str) -> AcceleratorError {
        let why = format!("the device did not answer within {timeout:?}{note}");
        live.mark_lost(why.clone());
        AcceleratorError::new(Failure::Timeout, why)
    }

    /// Rebuilds the device after a loss or a hang: once a loss (a thread
    /// that finds it rebuilt already is done), at most [`MAX_REBUILDS`]
    /// times a session; a rebuild that fails gives the GPU up.
    fn rebuild(&self) -> Result<(), String> {
        let _one = self
            .rebuilding
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(why) = self
            .dead
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return Err(why);
        }
        if self.current().lost().is_none() {
            return Ok(());
        }
        let give_up = |why: String| {
            *self.dead.lock().unwrap_or_else(PoisonError::into_inner) = Some(why.clone());
            Err(why)
        };
        if self.rebuilds.load(Ordering::SeqCst) >= MAX_REBUILDS {
            return give_up(format!(
                "the device was rebuilt {MAX_REBUILDS} times already"
            ));
        }
        self.rebuilds.fetch_add(1, Ordering::SeqCst);
        match Self::request(&self.wgpu_adapter) {
            Ok((device, queue, _)) => {
                *self.live.write().unwrap_or_else(PoisonError::into_inner) =
                    Arc::new(Live::new(device, queue));
                Ok(())
            }
            Err(e) => give_up(format!("no new device: {e}")),
        }
    }
}

/// The wgpu objects behind a [`Gpu`], for a window that draws with them.
#[derive(Clone)]
pub struct Handles {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// Invocations in a workgroup, along x (every kernel's `@workgroup_size`).
const WORKGROUP: u32 = 64;

/// The workgroups that cover `pixels`, as a grid: as many along x as a
/// dimension allows (`max` per dimension), then rows. A kernel finds its
/// pixel at `id.y * x * WORKGROUP + id.x`; the last row may run past the
/// pixel count, which kernels check.
pub fn grid(pixels: u32, max: u32) -> Result<(u32, u32), String> {
    let groups = pixels.div_ceil(WORKGROUP).max(1);
    let x = groups.min(max.max(1));
    let y = groups.div_ceil(x);
    if y > max {
        return Err(format!(
            "{pixels} pixels need {x} x {y} workgroups, over the device's {max} per dimension"
        ));
    }
    Ok((x, y))
}

/// What a [`DeviceImage`] made by a [`Gpu`] holds: a storage buffer, the
/// device it lives on (an image is only used on its own device), and its
/// share of the memory budget.
struct Resident {
    device: wgpu::Device,
    buffer: wgpu::Buffer,
    _charge: Charge,
}

impl Accelerator for Gpu {
    fn id(&self) -> &str {
        &self.id
    }

    fn upload(&self, image: &Image) -> Result<DeviceImage, AcceleratorError> {
        self.upload_image(image)
    }

    fn download(&self, image: &DeviceImage) -> Result<Image, AcceleratorError> {
        self.download_image(image)
    }

    fn run(
        &self,
        module: &str,
        job: &GpuJob,
        input: &DeviceImage,
        output: Region,
        channels: u32,
    ) -> Result<DeviceImage, AcceleratorError> {
        self.kernel(module, job, input, output, channels)
    }

    fn memory(&self) -> Option<Memory> {
        Some(Gpu::memory(self))
    }

    fn set_memory_budget(&self, bytes: Option<u64>) {
        self.set_budget(bytes);
    }

    fn recover(&self) -> Result<(), String> {
        self.rebuild()
    }
}

/// How far a GPU result is from the CPU reference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Comparison {
    /// The largest absolute difference.
    pub max_abs: f32,
    /// The largest difference relative to the reference value's size (at
    /// least 1, so that values near zero compare absolutely).
    pub max_rel: f32,
}

/// Compares two images of the same shape.
pub fn compare(gpu: &Image, cpu: &Image) -> Result<Comparison, String> {
    if gpu.region() != cpu.region() || gpu.channels() != cpu.channels() {
        return Err("different shapes".into());
    }
    let mut c = Comparison {
        max_abs: 0.0,
        max_rel: 0.0,
    };
    for (a, b) in gpu.data().iter().zip(cpu.data()) {
        let d = (a - b).abs();
        if !d.is_finite() {
            return Err(format!("{a} against {b}"));
        }
        c.max_abs = c.max_abs.max(d);
        c.max_rel = c.max_rel.max(d / b.abs().max(1.0));
    }
    Ok(c)
}

/// Runs a module on both backends over `region` of `input` and compares.
pub fn check(
    gpu: &Gpu,
    module: &ModuleRef,
    params: &Params,
    context: &Context,
    input: &Image,
    region: Region,
) -> Result<Comparison, String> {
    let mut g = helios_core::Graph::new();
    let src = g.add_source("src", input.clone());
    let node = g
        .add_module("m", module.clone(), params)
        .map_err(|e| e.to_string())?;
    g.set_context(node, context.clone())
        .map_err(|e| e.to_string())?;
    // A module without inputs (a generator) leaves the source unread.
    if let Some(port) = module.manifest().inputs.first() {
        g.connect(src, helios_core::graph::SOURCE_PORT, node, &port.name)
            .map_err(|e| e.to_string())?;
    }
    let engine = helios_core::Engine::new(std::sync::Arc::new(helios_core::Bus::new()));
    let port = &module.manifest().outputs[0].name;
    let cpu = engine
        .render(&g, node, port, region)
        .map_err(|e| e.to_string())?;
    let out = gpu
        .run(module, params, context, input, region)
        .map_err(|e| e.to_string())?
        .ok_or("no GPU variant")?;
    compare(&out, &cpu)
}
