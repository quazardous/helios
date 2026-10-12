//! The device is opened with its adapter's limits, not wgpu's defaults
//! (128 MiB per storage binding), so the images the compute manager admits
//! bind on it. Without any GPU adapter (software ones count), skipped,
//! saying so. Headless: no window, no display handle.

use helios_core::{Accelerator, GpuJob, GpuKernel, Image, Region};
use helios_gpu::Gpu;

fn adapter() -> Option<wgpu::Adapter> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    })) {
        Ok(a) => Some(a),
        Err(e) => {
            eprintln!("skipped: no GPU adapter: {e}");
            None
        }
    }
}

#[test]
fn the_opened_device_has_its_adapters_limits() {
    let Some(adapter) = adapter() else { return };
    let gpu = Gpu::open(&adapter).unwrap();
    eprintln!("GPU: {}", gpu.adapter);
    let (asked, got) = (helios_gpu::requested_limits(&adapter), gpu.limits());
    assert_eq!(asked, adapter.limits());
    if let Some(note) = &gpu.limits_note {
        // A driver that refuses its own maximum still gets the buffer sizes.
        eprintln!("{note}");
        assert_eq!(got.max_buffer_size, asked.max_buffer_size);
        assert_eq!(
            got.max_storage_buffer_binding_size,
            asked.max_storage_buffer_binding_size
        );
    } else {
        assert_eq!(got, asked);
    }
}

/// Copies the last 64 pixels of a large input into a 64-pixel output.
const TAIL: &str = r"
@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1>;
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.x;
    if (p >= shape[0].x) {
        return;
    }
    let at = (shape[1].x * shape[1].y - shape[0].x + p) * 4u;
    for (var c = 0u; c < 4u; c = c + 1u) {
        output[p * 4u + c] = input[at + c] * plan[0].x;
    }
}
";

#[test]
fn a_storage_binding_over_128_mib_computes() {
    let Some(adapter) = adapter() else { return };
    // 4096 x 2112 RGBA f32: 132 MiB, above wgpu's default binding limit.
    let input_region = Region::sized(4096, 2112);
    let pixels = input_region.area();
    let bytes = (pixels * 16) as u64;
    assert!(bytes > 128 << 20);
    let limits = adapter.limits();
    if limits.max_storage_buffer_binding_size < bytes || limits.max_buffer_size < bytes {
        eprintln!("skipped: the adapter binds at most {limits:?}");
        return;
    }
    let gpu = Gpu::open(&adapter).unwrap();
    eprintln!("GPU: {}", gpu.adapter);
    let mut data = vec![0.0f32; pixels * 4];
    for (i, px) in data.chunks_exact_mut(4).enumerate() {
        // Pixel numbers below 2^24 are exact in f32.
        px[0] = i as f32;
        px[3] = 1.0;
    }
    let input = gpu
        .upload(&Image::new(input_region, 4, data))
        .expect("uploads");
    let output = Region::sized(64, 1);
    let job = GpuJob {
        kernel: GpuKernel {
            wgsl: TAIL,
            entry: "main",
        },
        uniforms: vec![2.0, 0.0, 0.0, 0.0],
        input: input_region,
    };
    let result = Accelerator::run(&gpu, "tail", &job, &input, output, 4).unwrap();
    let result = gpu.download(&result).unwrap();
    for p in 0..64 {
        let expected = (pixels - 64 + p) as f32 * 2.0;
        let got = result.pixel(i64::try_from(p).unwrap(), 0);
        assert_eq!(got, [expected, 0.0, 0.0, 2.0], "pixel {p}");
    }
}
