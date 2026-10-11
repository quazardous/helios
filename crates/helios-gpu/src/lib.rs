//! The GPU backend: modules' WGSL variants run on wgpu (Vulkan on Linux and
//! Windows, Metal on macOS), on the same plans as their CPU code.
//!
//! A [`Gpu`] holds a device: its own ([`Gpu::new`]), or one handed over by
//! the GUI ([`Gpu::from_device`]) so the viewport and the engine share it
//! and computed buffers need no copy to be shown. [`Gpu::run`] uploads the
//! input, runs the module's kernel and reads the result back. A [`Gpu`] is
//! also the engine's [`Accelerator`]: an engine given one
//! (`Engine::with_accelerator`) computes every step that has a GPU variant
//! on it, and images stay in GPU buffers from one such step to the next.
//! `Engine::render_device` hands a render's result as a buffer on the
//! device ([`Gpu::buffer`]), for a viewport that shares it.
//!
//! [`check`] runs a module on both backends and compares: a GPU variant is
//! only trusted within its tolerance of the CPU reference.

use std::collections::HashMap;
use std::sync::Mutex;

use helios_core::{
    Accelerator, Context, DeviceImage, Error, GpuJob, Image, ModuleRef, Params, Region,
};
use wgpu::util::DeviceExt;

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The adapter's description, for reports.
    pub adapter: String,
    id: String,
    /// Compiled kernels, by source and entry point.
    pipelines: Mutex<HashMap<(&'static str, &'static str), wgpu::ComputePipeline>>,
}

impl Gpu {
    /// Opens the best adapter there is, software ones included (lavapipe in
    /// CI). An error when there is none.
    pub fn new() -> Result<Gpu, String> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
        Gpu::open(&adapter)
    }

    /// Opens a device on a given adapter (one the compute manager chose).
    pub fn open(adapter: &wgpu::Adapter) -> Result<Gpu, String> {
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("helios"),
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU device: {e}"))?;
        Ok(Gpu::from_device(
            device,
            queue,
            &format!("{} ({:?}, {:?})", info.name, info.device_type, info.backend),
        ))
    }

    /// A device someone else opened (the GUI's).
    pub fn from_device(device: wgpu::Device, queue: wgpu::Queue, adapter: &str) -> Gpu {
        Gpu {
            device,
            queue,
            adapter: adapter.into(),
            id: format!("wgpu {adapter}"),
            pipelines: Mutex::default(),
        }
    }

    /// Runs a module's GPU variant over `output`, reading `source` (edges
    /// replicated where the module reads past it, as the engine does).
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
        let input = source.sample(job.input);
        let m = module.manifest();
        let channels = m
            .outputs
            .first()
            .and_then(|p| p.channels())
            .unwrap_or(input.channels());
        self.dispatch(&m.name, &job, &input, output, channels)
            .map(Some)
            .map_err(|reason| Error::Module {
                module: m.name.clone(),
                reason: format!("GPU: {reason}"),
            })
    }

    /// The kernel's pipeline, compiled once. A kernel that does not
    /// compile is an error, never a panic.
    fn pipeline(&self, name: &str, job: &GpuJob) -> Result<wgpu::ComputePipeline, String> {
        let key = (job.kernel.wgsl, job.kernel.entry);
        if let Some(p) = self.pipelines.lock().expect("pipelines lock").get(&key) {
            return Ok(p.clone());
        }
        let d = &self.device;
        let scope = d.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(name),
            source: wgpu::ShaderSource::Wgsl(job.kernel.wgsl.into()),
        });
        let pipeline = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(name),
            layout: None,
            module: &shader,
            entry_point: Some(job.kernel.entry),
            compilation_options: Default::default(),
            cache: None,
        });
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(format!("kernel `{}`: {e}", job.kernel.entry));
        }
        self.pipelines
            .lock()
            .expect("pipelines lock")
            .insert(key, pipeline.clone());
        Ok(pipeline)
    }

    /// The buffer behind an image this GPU holds (what
    /// `Engine::render_device` answers on an engine it accelerates); `None`
    /// for an image of another device. Pixels are `f32`, channels
    /// interleaved, rows top to bottom over `image.region()`, tightly packed
    /// (`width * channels` floats a row); usages `STORAGE | COPY_SRC`. Read
    /// it, never write it: the engine's cache shares it.
    pub fn buffer<'a>(&self, image: &'a DeviceImage) -> Option<&'a wgpu::Buffer> {
        image
            .handle::<Resident>()
            .filter(|r| r.device == self.device)
            .map(|r| &r.buffer)
    }

    /// Uploads `input`, runs the kernel over `output` and reads it back.
    fn dispatch(
        &self,
        name: &str,
        job: &GpuJob,
        input: &Image,
        output: Region,
        channels: u32,
    ) -> Result<Image, String> {
        let input = self.upload_image(input);
        let out = self.kernel(name, job, &input, output, channels)?;
        self.download_image(&out)
    }

    /// Copies a host image into a buffer on the device.
    fn upload_image(&self, image: &Image) -> DeviceImage {
        let data: &[u8] = bytemuck::cast_slice(image.data());
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("image"),
                // A binding is never empty.
                contents: if data.is_empty() { &[0; 4] } else { data },
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            });
        self.resident(image.region(), image.channels(), buffer)
    }

    fn resident(&self, region: Region, channels: u32, buffer: wgpu::Buffer) -> DeviceImage {
        DeviceImage::new(
            region,
            channels,
            Resident {
                device: self.device.clone(),
                buffer,
            },
        )
    }

    /// Runs the kernel over `output`, reading `input` on the device, into a
    /// new buffer on the device. Nothing is read back.
    fn kernel(
        &self,
        name: &str,
        job: &GpuJob,
        input: &DeviceImage,
        output: Region,
        channels: u32,
    ) -> Result<DeviceImage, String> {
        let input_buf = self
            .buffer(input)
            .ok_or("the input is not in this device's memory")?;
        if input.region() != job.input {
            return Err(format!(
                "input covers {}, the kernel reads {}",
                input.region(),
                job.input
            ));
        }
        let pipeline = self.pipeline(name, job)?;
        let pixels = output.area() as u32;
        let d = &self.device;
        let scope = d.push_error_scope(wgpu::ErrorFilter::Validation);
        let uniform = |data: &[u8], label: &str| {
            d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: data,
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let out_size = (pixels as u64) * u64::from(channels) * 4;
        let output_buf = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("output"),
            size: out_size.max(4),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let plan_buf = uniform(bytemuck::cast_slice(&job.uniforms), "plan");
        let shape = [
            pixels,
            input.channels(),
            channels,
            0u32,
            job.input.width,
            job.input.height,
            output.width,
            output.height,
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
        let mut encoder = d.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(pixels.div_ceil(64), 1, 1);
        }
        // Submitted, not waited for: the queue runs submissions in order,
        // so the next kernel or a readback sees this one's result.
        self.queue.submit([encoder.finish()]);
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(e.to_string());
        }
        Ok(self.resident(output, channels, output_buf))
    }

    /// Reads an image on the device back into host memory, waiting for the
    /// kernels that write it.
    fn download_image(&self, image: &DeviceImage) -> Result<Image, String> {
        let buffer = self
            .buffer(image)
            .ok_or("the image is not in this device's memory")?;
        let size = image.bytes() as u64;
        let d = &self.device;
        let scope = d.push_error_scope(wgpu::ErrorFilter::Validation);
        let readback = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: size.max(4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = d.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(buffer, 0, &readback, 0, size.max(4));
        self.queue.submit([encoder.finish()]);
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(e.to_string());
        }
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        d.poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        let view = slice.get_mapped_range().map_err(|e| e.to_string())?;
        let data: Vec<f32> = bytemuck::cast_slice(&view[..size as usize]).to_vec();
        drop(view);
        readback.unmap();
        Ok(Image::new(image.region(), image.channels(), data))
    }
}

/// What a [`DeviceImage`] made by a [`Gpu`] holds: a storage buffer, and
/// the device it lives on (an image is only used on its own device).
struct Resident {
    device: wgpu::Device,
    buffer: wgpu::Buffer,
}

impl Accelerator for Gpu {
    fn id(&self) -> &str {
        &self.id
    }

    fn upload(&self, image: &Image) -> Result<DeviceImage, String> {
        Ok(self.upload_image(image))
    }

    fn download(&self, image: &DeviceImage) -> Result<Image, String> {
        self.download_image(image)
    }

    fn run(
        &self,
        module: &str,
        job: &GpuJob,
        input: &DeviceImage,
        output: Region,
        channels: u32,
    ) -> Result<DeviceImage, String> {
        self.kernel(module, job, input, output, channels)
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
    g.connect(
        src,
        helios_core::graph::SOURCE_PORT,
        node,
        &module.manifest().inputs[0].name,
    )
    .map_err(|e| e.to_string())?;
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
