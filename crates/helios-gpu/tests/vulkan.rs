//! Raw Vulkan interop (`helios_gpu::vulkan`): the handles name the device
//! wgpu opened, and an image goes from the engine to foreign Vulkan code
//! and back on the device, with no copy through the host. Skipped, saying
//! why, without any GPU adapter or on a device that is not Vulkan (Metal,
//! DX12, OpenGL).

// Foreign Vulkan code is played here with `ash`, whose calls are unsafe.
#![allow(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use helios_core::{Accelerator, GpuJob, GpuKernel, Image, Region};
use helios_gpu::Gpu;
use helios_gpu::vulkan::ash::vk::{self, Handle};
use helios_gpu::vulkan::{IMPORT_BUFFER_USAGE, VulkanDevice};

/// The process's device and its Vulkan side; `None`, saying why, when
/// there is none.
fn vulkan() -> Option<(Arc<Gpu>, VulkanDevice)> {
    let gpu = match helios_gpu::shared::gpu() {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("skipped: {e}");
            return None;
        }
    };
    eprintln!("GPU: {}", gpu.adapter);
    match gpu.vulkan() {
        Ok(vk) => Some((gpu, vk)),
        Err(why) => {
            // Only a device of another API may refuse.
            let backend = gpu.handles().adapter.get_info().backend;
            assert_ne!(backend, wgpu::Backend::Vulkan, "{why}");
            assert!(why.contains("not Vulkan"), "{why}");
            eprintln!("skipped: {why}");
            None
        }
    }
}

#[test]
fn the_handles_name_the_device_wgpu_opened() {
    let _gpu = helios_testkit::gpu_lock();
    let Some((gpu, vk)) = vulkan() else { return };
    assert_ne!(vk.instance().handle().as_raw(), 0);
    assert_ne!(vk.physical_device().as_raw(), 0);
    assert_ne!(vk.device().handle().as_raw(), 0);
    assert_ne!(vk.queue().as_raw(), 0);
    assert!(vk::api_version_major(vk.api_version()) >= 1);
    // The physical device is the adapter's.
    let info = gpu.handles().adapter.get_info();
    let properties = unsafe {
        vk.instance()
            .get_physical_device_properties(vk.physical_device())
    };
    assert_eq!(properties.vendor_id, info.vendor);
    assert_eq!(properties.device_id, info.device);
    // The queue is in a compute family, and the device answers it for its
    // family and index.
    let families = unsafe {
        vk.instance()
            .get_physical_device_queue_family_properties(vk.physical_device())
    };
    let family = &families[vk.queue_family_index() as usize];
    assert!(family.queue_flags.contains(vk::QueueFlags::COMPUTE));
    assert!(vk.queue_index() < family.queue_count);
    let queue = unsafe {
        vk.device()
            .get_device_queue(vk.queue_family_index(), vk.queue_index())
    };
    assert_eq!(queue, vk.queue());
    // Every extension said enabled is one the physical device has.
    let supported = unsafe {
        vk.instance()
            .enumerate_device_extension_properties(vk.physical_device())
            .unwrap()
    };
    for name in vk.device_extensions() {
        assert!(
            supported
                .iter()
                .any(|p| p.extension_name_as_c_str() == Ok(*name)),
            "{name:?} is not an extension of the device"
        );
    }
}

/// Multiplies every channel by `plan[0].x`: a kernel of the engine's
/// shape (one invocation a pixel, 4 channels).
const SCALE: &str = r"
@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1>;
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 3>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    for (var c = 0u; c < 4u; c = c + 1u) {
        output[p * 4u + c] = input[p * 4u + c] * plan[0].x;
    }
}
";

/// A device-local memory type among `bits`.
fn device_local(vk: &VulkanDevice, bits: u32) -> u32 {
    let memory = unsafe {
        vk.instance()
            .get_physical_device_memory_properties(vk.physical_device())
    };
    (0..memory.memory_type_count)
        .find(|&i| {
            bits & (1 << i) != 0
                && memory.memory_types[i as usize]
                    .property_flags
                    .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        })
        .expect("a device-local memory type")
}

/// A buffer of `size` bytes in device memory, made by "foreign" code (as
/// vkdt would), and what frees it.
fn foreign_buffer(vk: &VulkanDevice, size: u64) -> (vk::Buffer, vk::DeviceMemory) {
    let device = vk.device();
    unsafe {
        let buffer = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(IMPORT_BUFFER_USAGE | vk::BufferUsageFlags::TRANSFER_DST)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .unwrap();
        let needs = device.get_buffer_memory_requirements(buffer);
        let memory = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(device_local(vk, needs.memory_type_bits)),
                None,
            )
            .unwrap();
        device.bind_buffer_memory(buffer, memory, 0).unwrap();
        (buffer, memory)
    }
}

/// Polls the device until `done`, at most 10 s.
fn poll_until(gpu: &Gpu, done: &AtomicBool) -> bool {
    let device = gpu.handles().device;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done.load(Ordering::SeqCst) && Instant::now() < deadline {
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(1)),
        });
        std::thread::sleep(Duration::from_millis(10));
    }
    done.load(Ordering::SeqCst)
}

#[test]
fn an_image_goes_to_foreign_vulkan_code_and_back_without_a_readback() {
    let _gpu = helios_testkit::gpu_lock();
    let Some((gpu, vk)) = vulkan() else { return };
    let region = Region::sized(37, 23);
    let data: Vec<f32> = (0..region.area() * 4).map(|i| i as f32).collect();
    let bytes = (data.len() * 4) as u64;
    let host = gpu.upload(&Image::new(region, 4, data)).unwrap();

    // The engine's image, as a VkBuffer: the same buffer, read-only.
    let exported = gpu.export_buffer(&host).unwrap();
    assert_ne!(exported.buffer.as_raw(), 0);
    assert!(exported.size >= bytes);
    assert_eq!((exported.region, exported.channels), (region, 4));
    let again = gpu.export_buffer(&host).unwrap();
    assert_eq!(again.buffer, exported.buffer);

    // Foreign code copies it on the device into a buffer of its own,
    // recorded into wgpu's queue after the upload.
    let (theirs, memory) = foreign_buffer(&vk, bytes);
    let source = exported.buffer;
    unsafe {
        gpu.record("foreign copy", &[&host], |device, cmd| {
            device.cmd_copy_buffer(
                cmd,
                source,
                theirs,
                &[vk::BufferCopy {
                    src_offset: 0,
                    dst_offset: 0,
                    size: bytes,
                }],
            );
        })
        .unwrap();
    }
    drop((exported, again, host));

    // Its buffer comes back as an engine image: no copy, the same VkBuffer.
    let released = Arc::new(AtomicBool::new(false));
    let release = {
        let (device, released) = (vk.device().clone(), released.clone());
        Box::new(move || {
            unsafe {
                device.destroy_buffer(theirs, None);
                device.free_memory(memory, None);
            }
            released.store(true, Ordering::SeqCst);
        })
    };
    let used = gpu.memory().used;
    let imported = unsafe { gpu.import_buffer(theirs, bytes, region, 4, release) }.unwrap();
    assert_eq!(gpu.memory().used, used + bytes, "counted in the budget");
    assert_eq!(gpu.export_buffer(&imported).unwrap().buffer, theirs);

    // A kernel of the engine reads it; the only readback is the check.
    let job = GpuJob {
        kernel: GpuKernel {
            wgsl: SCALE,
            entry: "main",
        },
        uniforms: vec![2.0, 0.0, 0.0, 0.0],
        input: region,
    };
    let scaled = Accelerator::run(gpu.as_ref(), "scale", &job, &imported, region, 4).unwrap();
    let out = gpu.download(&scaled).unwrap();
    for (i, v) in out.data().iter().enumerate() {
        assert_eq!(*v, i as f32 * 2.0, "value {i}");
    }

    // Once the engine drops it, the foreign side frees it.
    drop((imported, scaled));
    assert!(poll_until(&gpu, &released), "the release ran");
    assert_eq!(gpu.memory().used, used);
}

#[test]
fn a_texture_goes_both_ways_without_a_copy() {
    let _gpu = helios_testkit::gpu_lock();
    let Some((gpu, vk)) = vulkan() else { return };
    let handles = gpu.handles();
    let (width, height) = (16u32, 8u32);
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let desc = wgpu::TextureDescriptor {
        label: Some("interop"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    };
    let texels: Vec<f32> = (0..width * height * 4).map(|i| i as f32).collect();
    let row = width * 16;
    let ours = handles.device.create_texture(&desc);
    handles.queue.write_texture(
        ours.as_image_copy(),
        bytemuck::cast_slice(&texels),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(row),
            rows_per_image: Some(height),
        },
        size,
    );

    // A wgpu texture, as a VkImage.
    let exported = gpu.export_texture(&ours).unwrap();
    assert_ne!(exported.image.as_raw(), 0);
    assert_eq!(exported.format, vk::Format::R32G32B32A32_SFLOAT);
    assert_eq!(
        (
            exported.extent.width,
            exported.extent.height,
            exported.extent.depth
        ),
        (width, height, 1)
    );

    // A VkImage made by foreign code, as a wgpu texture: wgpu copies into
    // it and reads it back, on the very image.
    let device = vk.device();
    let (theirs, memory) = unsafe {
        let image = device
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R32G32B32A32_SFLOAT)
                    .extent(exported.extent)
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .unwrap();
        let needs = device.get_image_memory_requirements(image);
        let memory = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(needs.size)
                    .memory_type_index(device_local(&vk, needs.memory_type_bits)),
                None,
            )
            .unwrap();
        device.bind_image_memory(image, memory, 0).unwrap();
        (image, memory)
    };
    let released = Arc::new(AtomicBool::new(false));
    let release = {
        let (device, released) = (device.clone(), released.clone());
        Box::new(move || {
            unsafe {
                device.destroy_image(theirs, None);
                device.free_memory(memory, None);
            }
            released.store(true, Ordering::SeqCst);
        })
    };
    let imported =
        unsafe { gpu.import_texture(theirs, &desc, wgpu::TextureUses::UNINITIALIZED, release) }
            .unwrap();
    assert_eq!(gpu.export_texture(&imported).unwrap().image, theirs);

    let readback = handles.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("check"),
        size: u64::from(row * height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = handles
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_texture(ours.as_image_copy(), imported.as_image_copy(), size);
    encoder.copy_texture_to_buffer(
        imported.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        size,
    );
    handles.queue.submit([encoder.finish()]);
    let mapped = Arc::new(AtomicBool::new(false));
    let flag = mapped.clone();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        r.unwrap();
        flag.store(true, Ordering::SeqCst);
    });
    assert!(poll_until(&gpu, &mapped), "the check was read back");
    {
        let view = readback.slice(..).get_mapped_range().unwrap();
        let got: &[f32] = bytemuck::cast_slice(&view);
        assert_eq!(got, texels.as_slice());
    }
    readback.unmap();

    // Once wgpu drops it, the foreign side frees it.
    drop((exported, ours, imported));
    assert!(poll_until(&gpu, &released), "the release ran");
}
