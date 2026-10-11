//! The devices of the local machine: its CPU, and every GPU wgpu sees.
//!
//! Enumerating and opening GPUs goes through [`Adapters`], so tests inject
//! fake ones; [`WgpuAdapters`] is the real thing.

use std::sync::Arc;

use helios_core::Accelerator;
use serde::{Deserialize, Serialize};

/// The id of the local CPU.
pub const CPU: &str = "cpu";

/// What a device is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// The machine's processor, running modules' CPU code.
    Cpu,
    DiscreteGpu,
    IntegratedGpu,
    VirtualGpu,
    /// A GPU emulated on the CPU (llvmpipe, lavapipe): listed, off by default.
    SoftwareGpu,
    OtherGpu,
}

impl DeviceKind {
    /// Whether it runs GPU variants.
    pub fn is_gpu(self) -> bool {
        self != DeviceKind::Cpu
    }

    /// The order devices are preferred in before any calibration: higher first.
    pub fn rank(self) -> u8 {
        match self {
            DeviceKind::DiscreteGpu => 5,
            DeviceKind::IntegratedGpu => 4,
            DeviceKind::VirtualGpu => 3,
            DeviceKind::OtherGpu => 2,
            DeviceKind::Cpu => 1,
            DeviceKind::SoftwareGpu => 0,
        }
    }
}

/// What a GPU allows, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// The largest buffer it creates.
    pub max_buffer: u64,
    /// The largest buffer a kernel binds: a tile's input or output.
    pub max_binding: u64,
}

/// A device as discovered.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Device {
    /// Stable across runs: settings are kept under it.
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    /// The graphics API it is reached through (`vulkan`, `metal`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    /// Hardware threads, for the CPU.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
}

impl Device {
    /// The local CPU.
    pub fn cpu() -> Device {
        Device {
            id: CPU.into(),
            name: "CPU".into(),
            kind: DeviceKind::Cpu,
            backend: None,
            driver: None,
            threads: Some(std::thread::available_parallelism().map_or(1, std::num::NonZero::get)),
            limits: None,
        }
    }
}

/// The GPUs of the machine: listed, and opened as the engine's accelerator.
pub trait Adapters: Send + Sync {
    /// Every GPU there is now, ids stable across runs and unique.
    fn list(&self) -> Vec<Device>;

    /// Opens a device listed under `id`.
    fn open(&self, id: &str) -> Result<Arc<dyn Accelerator>, String>;
}

/// The id of an adapter: its backend and PCI ids, then a number when the
/// machine has several identical ones.
#[allow(
    clippy::maybe_infinite_iter,
    reason = "`taken` is finite, so an unused number is found"
)]
pub fn gpu_id(backend: &str, vendor: u32, device: u32, taken: &[String]) -> String {
    let base = format!("gpu:{backend}:{vendor:04x}:{device:04x}");
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}#{n}"))
        .find(|id| !taken.contains(id))
        .expect("an unused number exists")
}

/// The GPUs wgpu sees.
pub struct WgpuAdapters {
    instance: wgpu::Instance,
}

impl Default for WgpuAdapters {
    fn default() -> Self {
        Self::new()
    }
}

impl WgpuAdapters {
    pub fn new() -> Self {
        Self {
            instance: wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            ),
        }
    }

    /// Adapters on the primary APIs (Vulkan, Metal, DX12); OpenGL only when
    /// none is there, as it shows the same GPUs again.
    fn adapters(&self) -> Vec<(Device, wgpu::Adapter)> {
        let mut adapters =
            pollster::block_on(self.instance.enumerate_adapters(wgpu::Backends::PRIMARY));
        if adapters.is_empty() {
            adapters =
                pollster::block_on(self.instance.enumerate_adapters(wgpu::Backends::SECONDARY));
        }
        let mut found: Vec<(Device, wgpu::Adapter)> = Vec::new();
        for adapter in adapters {
            let info = adapter.get_info();
            let limits = adapter.limits();
            let backend = format!("{:?}", info.backend).to_lowercase();
            let taken: Vec<String> = found.iter().map(|(d, _)| d.id.clone()).collect();
            let device = Device {
                id: gpu_id(&backend, info.vendor, info.device, &taken),
                name: info.name.clone(),
                kind: match info.device_type {
                    wgpu::DeviceType::DiscreteGpu => DeviceKind::DiscreteGpu,
                    wgpu::DeviceType::IntegratedGpu => DeviceKind::IntegratedGpu,
                    wgpu::DeviceType::VirtualGpu => DeviceKind::VirtualGpu,
                    wgpu::DeviceType::Cpu => DeviceKind::SoftwareGpu,
                    wgpu::DeviceType::Other => DeviceKind::OtherGpu,
                },
                backend: Some(backend),
                driver: Some(
                    format!("{} {}", info.driver, info.driver_info)
                        .trim()
                        .to_string(),
                )
                .filter(|d| !d.is_empty()),
                threads: None,
                limits: Some(Limits {
                    max_buffer: limits.max_buffer_size,
                    max_binding: limits.max_storage_buffer_binding_size,
                }),
            };
            found.push((device, adapter));
        }
        found
    }
}

impl Adapters for WgpuAdapters {
    fn list(&self) -> Vec<Device> {
        self.adapters().into_iter().map(|(d, _)| d).collect()
    }

    fn open(&self, id: &str) -> Result<Arc<dyn Accelerator>, String> {
        let (_, adapter) = self
            .adapters()
            .into_iter()
            .find(|(d, _)| d.id == id)
            .ok_or_else(|| format!("no GPU `{id}` here"))?;
        Ok(Arc::new(helios_gpu::Gpu::open(&adapter)?))
    }
}
