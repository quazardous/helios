//! The GPU backend: modules' WGSL variants run on wgpu (Vulkan on Linux and
//! Windows, Metal on macOS), on the same plans as their CPU code.
//!
//! A [`Gpu`] holds a device: its own ([`Gpu::new`]), or one handed over by
//! the GUI ([`Gpu::from_device`]) so the viewport and the engine share it
//! and computed buffers need no copy to be shown. [`Gpu::run`] uploads the
//! input, runs the module's kernel and reads the result back; keeping
//! buffers on the GPU along a network is the engine's next step.
//!
//! [`check`] runs a module on both backends and compares: a GPU variant is
//! only trusted within its tolerance of the CPU reference.

use helios_core::{Context, Error, Image, ModuleRef, Params, Region};
use wgpu::util::DeviceExt;

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The adapter's description, for reports.
    pub adapter: String,
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
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("helios"),
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU device: {e}"))?;
        Ok(Gpu {
            device,
            queue,
            adapter: format!("{} ({:?}, {:?})", info.name, info.device_type, info.backend),
        })
    }

    /// A device someone else opened (the GUI's).
    pub fn from_device(device: wgpu::Device, queue: wgpu::Queue, adapter: &str) -> Gpu {
        Gpu {
            device,
            queue,
            adapter: adapter.into(),
        }
    }

    /// Runs a module's GPU variant on one input, over the input's region.
    /// `Ok(None)` when the module has none.
    pub fn run(
        &self,
        module: &ModuleRef,
        params: &Params,
        context: &Context,
        input: &Image,
    ) -> Result<Option<Image>, Error> {
        let Some((kernel, uniforms)) = module.gpu_job(params, context, 1.0)? else {
            return Ok(None);
        };
        let m = module.manifest();
        let out_channels = m
            .outputs
            .first()
            .and_then(|p| p.channels())
            .unwrap_or(input.channels());
        let region = input.region();
        let pixels = region.area() as u32;
        let d = &self.device;
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&m.name),
            source: wgpu::ShaderSource::Wgsl(kernel.wgsl.into()),
        });
        let pipeline = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(&m.name),
            layout: None,
            module: &shader,
            entry_point: Some(kernel.entry),
            compilation_options: Default::default(),
            cache: None,
        });
        let storage = |data: &[u8], label: &str, usage| {
            d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: data,
                usage,
            })
        };
        let input_buf = storage(
            bytemuck::cast_slice(input.data()),
            "input",
            wgpu::BufferUsages::STORAGE,
        );
        let out_size = (pixels as u64) * u64::from(out_channels) * 4;
        let output_buf = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("output"),
            size: out_size.max(4),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let plan_buf = storage(
            bytemuck::cast_slice(&uniforms),
            "plan",
            wgpu::BufferUsages::UNIFORM,
        );
        let shape = [pixels, input.channels(), out_channels, 0u32];
        let shape_buf = storage(
            bytemuck::cast_slice(&shape),
            "shape",
            wgpu::BufferUsages::UNIFORM,
        );
        let readback = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: out_size.max(4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
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
        encoder.copy_buffer_to_buffer(&output_buf, 0, &readback, 0, out_size.max(4));
        self.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        d.poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| Error::Module {
                module: m.name.clone(),
                reason: format!("GPU: {e}"),
            })?;
        let view = slice.get_mapped_range().map_err(|e| Error::Module {
            module: m.name.clone(),
            reason: format!("GPU: {e}"),
        })?;
        let data: Vec<f32> = bytemuck::cast_slice(&view[..out_size as usize]).to_vec();
        drop(view);
        readback.unmap();
        Ok(Some(Image::new(region, out_channels, data)))
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
    let src = g.add_source("src", input.sample(region));
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
        .run(module, params, context, &input.sample(region))
        .map_err(|e| e.to_string())?
        .ok_or("no GPU variant")?;
    compare(&out, &cpu)
}
