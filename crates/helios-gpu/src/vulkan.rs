//! Raw Vulkan access to the process's GPU device, for vkdt interop.
//!
//! vkdt's modules are ported to WGSL (ROADMAP, "Helios makes darktable and
//! vkdt meet"), but some code will want to run vkdt's own SPIR-V kernels,
//! or any Vulkan code, beside Helios's: a parity check against vkdt on the
//! same GPU, a module not ported yet. Running it on a second device would
//! mean a host copy at each crossing and a second device in the process,
//! which [`crate::shared`] forbids. This module opens the one device
//! instead, through wgpu-hal:
//!
//! - [`Gpu::vulkan`]: the raw handles (instance, physical device, device,
//!   queue family and index, the extensions enabled), plus `ash`'s function
//!   tables, valid while the [`VulkanDevice`] lives.
//! - [`Gpu::export_buffer`] / [`Gpu::import_buffer`]: an image of the
//!   engine ([`DeviceImage`]) as a `VkBuffer`, and a `VkBuffer` made on this
//!   device as an image the engine runs kernels on. Same memory, no copy.
//! - [`Gpu::export_texture`] / [`Gpu::import_texture`]: the same for a
//!   `VkImage` (vkdt's images are storage images) and a `wgpu::Texture`.
//! - [`Gpu::record`]: commands recorded by foreign code into a command
//!   buffer that wgpu submits, ordered with Helios's own work by full memory
//!   barriers before and after. Foreign code never submits to the queue
//!   itself: wgpu and the GUI submit to it from other threads, and a
//!   `VkQueue` must not be used by two threads at once.
//!
//! Getting the handles is safe; using them is `unsafe`, as any Vulkan call.
//! The rules a caller keeps are on each function. On a device that is not
//! Vulkan (Metal on macOS, DX12 or OpenGL elsewhere) every function says
//! so and the caller goes the portable way: a readback and an upload.
//!
//! No external memory extension is involved: the memory is shared because
//! the device is. Sharing with another device or process
//! (`VK_KHR_external_memory_fd`, dma-buf) is a later step.

// Raw Vulkan is unsafe by nature: this module is the one place in the crate
// that calls wgpu-hal and `ash`, and each `unsafe` block says what it relies
// on.
#![allow(unsafe_code)]

use std::ffi::CStr;

pub use ash;
use ash::vk;
use helios_core::{AcceleratorError, DeviceImage, Failure, Region};
use wgpu::hal::api::Vulkan;
use wgpu::hal::vulkan as hal;

use crate::{Gpu, Live};

/// What runs when an imported resource is no longer used by wgpu (its last
/// handle dropped and the GPU done with it): where the foreign side frees
/// it.
pub type Release = Box<dyn FnOnce() + Send + Sync + 'static>;

/// The usage flags an imported `VkBuffer` needs at least: what the engine's
/// images are (`STORAGE | COPY_SRC` in wgpu's terms).
pub const IMPORT_BUFFER_USAGE: vk::BufferUsageFlags = vk::BufferUsageFlags::from_raw(
    vk::BufferUsageFlags::STORAGE_BUFFER.as_raw() | vk::BufferUsageFlags::TRANSFER_SRC.as_raw(),
);

/// The raw Vulkan side of the process's device ([`Gpu::vulkan`]). The
/// handles stay valid while this lives, even if the [`Gpu`] rebuilds its
/// device after a loss meanwhile (they then name the lost device, whose
/// calls fail with `VK_ERROR_DEVICE_LOST`): ask again after a rebuild.
///
/// The `vk` handles are `#[repr(transparent)]` over Vulkan's own, so they
/// cross to C as they are (`Handle::as_raw`).
pub struct VulkanDevice {
    entry: ash::Entry,
    instance: ash::Instance,
    physical_device: vk::PhysicalDevice,
    device: ash::Device,
    queue: vk::Queue,
    queue_family_index: u32,
    queue_index: u32,
    api_version: u32,
    instance_extensions: Vec<&'static CStr>,
    device_extensions: Vec<&'static CStr>,
    /// Keeps the device, and so every handle above, alive.
    _keep: wgpu::Device,
}

impl VulkanDevice {
    /// The Vulkan loader's entry points, as wgpu loaded them.
    pub fn entry(&self) -> &ash::Entry {
        &self.entry
    }

    /// The process's `VkInstance` (`instance().handle()`), with its
    /// function table.
    pub fn instance(&self) -> &ash::Instance {
        &self.instance
    }

    /// The `VkPhysicalDevice` the device was opened on.
    pub fn physical_device(&self) -> vk::PhysicalDevice {
        self.physical_device
    }

    /// The `VkDevice` (`device().handle()`), with its function table. Never
    /// destroy it, nor wait on it while the GUI draws.
    pub fn device(&self) -> &ash::Device {
        &self.device
    }

    /// wgpu's one `VkQueue`. For identity and queue family ownership only:
    /// wgpu and the GUI submit to it from their threads, so submit through
    /// [`Gpu::record`], never with `vkQueueSubmit`.
    pub fn queue(&self) -> vk::Queue {
        self.queue
    }

    /// The family of [`queue`](Self::queue): graphics and compute.
    pub fn queue_family_index(&self) -> u32 {
        self.queue_family_index
    }

    /// The index of [`queue`](Self::queue) in its family.
    pub fn queue_index(&self) -> u32 {
        self.queue_index
    }

    /// The Vulkan version the instance was created for
    /// (`vk::api_version_major` and the like read it).
    pub fn api_version(&self) -> u32 {
        self.api_version
    }

    /// The instance extensions wgpu enabled.
    pub fn instance_extensions(&self) -> &[&'static CStr] {
        &self.instance_extensions
    }

    /// The device extensions wgpu enabled: a foreign kernel may use these
    /// and no others.
    pub fn device_extensions(&self) -> &[&'static CStr] {
        &self.device_extensions
    }
}

/// An engine image's `VkBuffer` ([`Gpu::export_buffer`]). The buffer stays
/// valid while this lives.
pub struct ExportedBuffer {
    /// The buffer. Pixels are `f32`, channels interleaved, rows top to
    /// bottom over `region`, tightly packed, from offset 0. Read it, never
    /// write it, never destroy it: the engine's cache shares it.
    pub buffer: vk::Buffer,
    /// Its size in bytes (at least the pixels' size).
    pub size: u64,
    pub region: Region,
    pub channels: u32,
    _keep: wgpu::Buffer,
}

/// A `wgpu::Texture`'s `VkImage` ([`Gpu::export_texture`]). The image stays
/// valid while this lives.
pub struct ExportedTexture {
    /// The image. Its layout is whatever wgpu left it in: foreign commands
    /// that use it go through [`Gpu::record`] and transition it from and
    /// back to `GENERAL`, or wgpu's next use reads garbage.
    pub image: vk::Image,
    /// Its format as Vulkan names it.
    pub format: vk::Format,
    pub extent: vk::Extent3D,
    _keep: wgpu::Texture,
}

impl Gpu {
    /// The device's raw Vulkan handles; an error, saying why, when the
    /// device is not a Vulkan one (Metal, DX12, OpenGL) or is lost.
    pub fn vulkan(&self) -> Result<VulkanDevice, String> {
        let live = self.vulkan_live()?;
        // SAFETY: the guard is only read, and dropped at the end of the
        // block; nothing is destroyed. The clones of `ash`'s tables do not
        // own their handles.
        unsafe {
            let hal = live
                .device
                .as_hal::<Vulkan>()
                .ok_or_else(|| not_vulkan("the device"))?;
            let shared = hal.shared_instance();
            Ok(VulkanDevice {
                entry: shared.entry().clone(),
                instance: shared.raw_instance().clone(),
                physical_device: hal.raw_physical_device(),
                device: hal.raw_device().clone(),
                queue: hal.raw_queue(),
                queue_family_index: hal.queue_family_index(),
                queue_index: hal.queue_index(),
                api_version: shared.instance_api_version(),
                instance_extensions: shared.extensions().to_vec(),
                device_extensions: hal.enabled_device_extensions().to_vec(),
                _keep: live.device.clone(),
            })
        }
    }

    /// The `VkBuffer` behind one of this GPU's images, without a copy.
    pub fn export_buffer(&self, image: &DeviceImage) -> Result<ExportedBuffer, String> {
        let live = self.vulkan_live()?;
        let buffer = Self::buffer_on(&live, image).map_err(|e| e.reason)?;
        // SAFETY: the handle is only read; `_keep` holds the wgpu buffer,
        // so wgpu does not destroy it while the export lives.
        let raw = unsafe {
            buffer
                .as_hal::<Vulkan>()
                .map(|b| b.raw_handle())
                .ok_or_else(|| not_vulkan("the image's buffer"))?
        };
        Ok(ExportedBuffer {
            buffer: raw,
            size: buffer.size(),
            region: image.region(),
            channels: image.channels(),
            _keep: buffer.clone(),
        })
    }

    /// A `VkBuffer` made on this device as an image of the engine, without
    /// a copy: pixels `f32`, channels interleaved, rows top to bottom over
    /// `region`, tightly packed, from offset 0. It counts in the memory
    /// budget. `release` runs once wgpu no longer uses the buffer, to free
    /// it and its memory.
    ///
    /// # Safety
    ///
    /// - `buffer` was created on [`Gpu::vulkan`]'s device with at least
    ///   [`IMPORT_BUFFER_USAGE`] and `size` bytes, its memory bound, and it
    ///   stays valid until `release` runs;
    /// - its content is written by work already finished or submitted
    ///   through [`Gpu::record`] (a write that wgpu does not know about is
    ///   not waited for);
    /// - nothing writes it once imported: the engine's cache may share it.
    pub unsafe fn import_buffer(
        &self,
        buffer: vk::Buffer,
        size: u64,
        region: Region,
        channels: u32,
        release: Release,
    ) -> Result<DeviceImage, AcceleratorError> {
        let live = self.vulkan_live().map_err(AcceleratorError::from)?;
        let needed = (region.area() as u64 * u64::from(channels) * 4).max(4);
        if size < needed {
            return Err(format!(
                "the buffer holds {size} bytes, {region} x {channels} channels need {needed}"
            )
            .into());
        }
        let charge = self.reserve(size)?;
        let (imported, error) = live.scoped(|| {
            // SAFETY: the caller keeps the rules above, which are wgpu's
            // for `create_buffer_from_hal`; wgpu never destroys an
            // externally owned buffer, `release` does.
            unsafe {
                let hal = hal::Buffer::from_raw_externally_owned(buffer, release);
                live.device.create_buffer_from_hal::<Vulkan>(
                    hal,
                    &wgpu::BufferDescriptor {
                        label: Some("imported"),
                        size,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                        mapped_at_creation: false,
                    },
                )
            }
        });
        if let Some(e) = error {
            return Err(live.failure(&e, Failure::Other));
        }
        Ok(Self::resident(&live, region, channels, imported, charge))
    }

    /// The `VkImage` behind a texture made on this device, without a copy.
    pub fn export_texture(&self, texture: &wgpu::Texture) -> Result<ExportedTexture, String> {
        self.vulkan_live()?;
        // SAFETY: the handle is only read; `_keep` holds the texture.
        let image = unsafe {
            texture
                .as_hal::<Vulkan>()
                .map(|t| t.raw_handle())
                .ok_or_else(|| not_vulkan("the texture"))?
        };
        // SAFETY: the adapter's guard is only read.
        let format = unsafe {
            let adapter = self
                .wgpu_adapter
                .as_hal::<Vulkan>()
                .ok_or_else(|| not_vulkan("the adapter"))?;
            adapter.texture_format_as_raw(texture.format())
        };
        let size = texture.size();
        Ok(ExportedTexture {
            image,
            format,
            extent: vk::Extent3D {
                width: size.width,
                height: size.height,
                depth: size.depth_or_array_layers,
            },
            _keep: texture.clone(),
        })
    }

    /// A `VkImage` made on this device as a `wgpu::Texture`, without a
    /// copy. `release` runs once wgpu no longer uses the image, to free it
    /// and its memory. `state` is the image's state for wgpu now
    /// (`wgpu::TextureUses::UNINITIALIZED` when its layout is `UNDEFINED`;
    /// `wgpu::TextureUses::STORAGE_READ_WRITE` for `GENERAL`).
    ///
    /// # Safety
    ///
    /// - `image` was created on [`Gpu::vulkan`]'s device as `desc` says
    ///   (size, format, mip levels, samples, usages), its memory bound, and
    ///   it stays valid until `release` runs;
    /// - its layout matches `state`, and its content is written by work
    ///   already finished or submitted through [`Gpu::record`].
    pub unsafe fn import_texture(
        &self,
        image: vk::Image,
        desc: &wgpu::TextureDescriptor<'_>,
        state: wgpu::TextureUses,
        release: Release,
    ) -> Result<wgpu::Texture, AcceleratorError> {
        let live = self.vulkan_live().map_err(AcceleratorError::from)?;
        let hal_desc = wgpu::hal::TextureDescriptor {
            label: desc.label,
            size: desc.size,
            mip_level_count: desc.mip_level_count,
            sample_count: desc.sample_count,
            dimension: desc.dimension,
            format: desc.format,
            usage: texture_uses(desc.usage),
            memory_flags: wgpu::hal::MemoryFlags::empty(),
            view_formats: desc.view_formats.to_vec(),
        };
        let (texture, error) = live.scoped(|| {
            // SAFETY: the caller keeps the rules above, which are wgpu's
            // for `create_texture_from_hal`; the memory is the caller's
            // (`External`), and `release` frees the image.
            unsafe {
                let hal_device = live
                    .device
                    .as_hal::<Vulkan>()
                    .ok_or_else(|| not_vulkan("the device"))?;
                let hal = hal_device.texture_from_raw(
                    image,
                    &hal_desc,
                    Some(release),
                    hal::TextureMemory::External,
                );
                drop(hal_device);
                Ok::<_, String>(
                    live.device
                        .create_texture_from_hal::<Vulkan>(hal, desc, state),
                )
            }
        });
        if let Some(e) = error {
            return Err(live.failure(&e, Failure::Other));
        }
        texture.map_err(AcceleratorError::from)
    }

    /// Lets foreign code record Vulkan commands (a vkdt kernel's dispatch,
    /// a copy) into a command buffer that wgpu then submits on its queue,
    /// after Helios's work submitted so far and before what comes next.
    /// `record` gets the device's function table and the command buffer,
    /// already begun; full memory barriers are recorded before and after
    /// it, so its reads see Helios's writes and Helios's next reads see its
    /// writes. The buffers of `keep` stay alive until the GPU is done with
    /// the submission, even if their images are dropped meanwhile. Nothing
    /// is waited for.
    ///
    /// # Safety
    ///
    /// `record` records valid commands on objects of this device, does not
    /// begin, end, reset or submit the command buffer, and returns every
    /// image it uses to the layout wgpu expects. Objects the commands use
    /// and wgpu does not own (pipelines, descriptor sets, foreign buffers)
    /// stay alive until the submission is done (`wgpu::Device::poll`, or
    /// [`Gpu::download`](helios_core::Accelerator::download) of a later
    /// result).
    pub unsafe fn record(
        &self,
        label: &str,
        keep: &[&DeviceImage],
        record: impl FnOnce(&ash::Device, vk::CommandBuffer),
    ) -> Result<(), AcceleratorError> {
        let live = self.vulkan_live().map_err(AcceleratorError::from)?;
        let held = keep
            .iter()
            .map(|image| Self::buffer_on(&live, image).cloned())
            .collect::<Result<Vec<_>, _>>()?;
        // SAFETY: the guard is only read and dropped at once; the clone of
        // the table does not own the device.
        let device = unsafe {
            live.device
                .as_hal::<Vulkan>()
                .map(|d| d.raw_device().clone())
                .ok_or_else(|| not_vulkan("the device"))?
        };
        let (recorded, error) = live.scoped(|| {
            let mut encoder = live
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
            // SAFETY: wgpu opened the encoder; the encoder is not touched
            // while the closure records; the caller keeps the rules above.
            let recorded = unsafe {
                encoder.as_hal_mut::<Vulkan, _, _>(|hal| {
                    let cmd = hal?.raw_handle();
                    full_barrier(&device, cmd);
                    record(&device, cmd);
                    full_barrier(&device, cmd);
                    Some(())
                })
            };
            if recorded.is_some() {
                live.queue.submit([encoder.finish()]);
                live.queue.on_submitted_work_done(move || drop(held));
            }
            recorded
        });
        if let Some(e) = error {
            return Err(live.failure(&e, Failure::Dispatch));
        }
        recorded.ok_or_else(|| not_vulkan("the command encoder").into())
    }

    /// The device, if it is usable and a Vulkan one.
    fn vulkan_live(&self) -> Result<std::sync::Arc<Live>, String> {
        let backend = self.wgpu_adapter.get_info().backend;
        if backend != wgpu::Backend::Vulkan {
            return Err(format!(
                "the GPU runs on {backend}, not Vulkan: raw Vulkan handles exist only there"
            ));
        }
        self.live().map_err(|e| e.reason)
    }
}

fn not_vulkan(what: &str) -> String {
    format!("{what} is not a Vulkan object")
}

/// Every write before visible to every read and write after, at every
/// stage: what orders foreign commands with wgpu's, which tracks only its
/// own.
///
/// # Safety
///
/// `cmd` is a command buffer of `device` being recorded.
unsafe fn full_barrier(device: &ash::Device, cmd: vk::CommandBuffer) {
    let barrier = vk::MemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
        .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE);
    // SAFETY: as the function's.
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[barrier],
            &[],
            &[],
        );
    }
}

/// The states a texture of `usage` may be in, as wgpu-hal names them.
fn texture_uses(usage: wgpu::TextureUsages) -> wgpu::TextureUses {
    let mut uses = wgpu::TextureUses::empty();
    for (from, to) in [
        (wgpu::TextureUsages::COPY_SRC, wgpu::TextureUses::COPY_SRC),
        (wgpu::TextureUsages::COPY_DST, wgpu::TextureUses::COPY_DST),
        (
            wgpu::TextureUsages::TEXTURE_BINDING,
            wgpu::TextureUses::RESOURCE,
        ),
        (
            wgpu::TextureUsages::STORAGE_BINDING,
            wgpu::TextureUses::STORAGE_READ_ONLY
                | wgpu::TextureUses::STORAGE_WRITE_ONLY
                | wgpu::TextureUses::STORAGE_READ_WRITE,
        ),
        (
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            wgpu::TextureUses::COLOR_TARGET,
        ),
    ] {
        if usage.contains(from) {
            uses |= to;
        }
    }
    uses
}
