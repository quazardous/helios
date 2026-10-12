//! The process's one wgpu instance and one GPU device.
//!
//! Every wgpu object of a Helios process comes from here: the engine's
//! accelerator ([`gpu`]), the compute manager's survey of adapters
//! ([`adapters`]) and the GUI's window, to which `helios-gui` hands this
//! very device (eframe's `WgpuSetup::Existing`) instead of letting eframe
//! open its own.
//!
//! The reason is the Vulkan loader before 1.4.350: a thread that
//! enumerates devices (`vkEnumeratePhysicalDevices`) or closes one, while
//! another names an object (`vkSetDebugUtilsObjectNameEXT`, which wgpu
//! calls in debug builds), reads freed memory and crashes the process (see
//! docs/testing-strategy.md). With one instance, adapters enumerated once
//! and one device never closed, nothing enumerates or closes while work
//! runs. A second instance or device is a bug: [`instances_created`] and
//! [`devices_opened`] count them, debug builds assert, and a test checks
//! that no other source file creates an instance.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crate::Gpu;

static INSTANCE: OnceLock<wgpu::Instance> = OnceLock::new();
static INSTANCES: AtomicUsize = AtomicUsize::new(0);
static ADAPTERS: OnceLock<Vec<wgpu::Adapter>> = OnceLock::new();
/// The device opened, with the index of its adapter in [`adapters`].
static DEVICE: Mutex<Option<(usize, Arc<Gpu>)>> = Mutex::new(None);
static DEVICES: AtomicUsize = AtomicUsize::new(0);

/// The process's wgpu instance, created on first use (backends and flags
/// from wgpu's environment variables, no display handle).
pub fn instance() -> &'static wgpu::Instance {
    INSTANCE.get_or_init(|| {
        let before = INSTANCES.fetch_add(1, Ordering::SeqCst);
        debug_assert_eq!(before, 0, "a second wgpu instance in this process");
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env())
    })
}

/// How many wgpu instances this process created: at most 1.
pub fn instances_created() -> usize {
    INSTANCES.load(Ordering::SeqCst)
}

/// Whether `instance` is the process's own (a window handed something
/// else made a second one).
pub fn owns(instance: &wgpu::Instance) -> bool {
    INSTANCE.get() == Some(instance)
}

/// Every adapter, enumerated once per process: those of the primary APIs
/// (Vulkan, Metal, DX12), else of the others (OpenGL), which show the same
/// GPUs again. A GPU plugged in later is not seen before a restart:
/// enumerating again is what races with work in flight.
pub fn adapters() -> &'static [wgpu::Adapter] {
    ADAPTERS.get_or_init(|| {
        let instance = instance();
        let primary = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::PRIMARY));
        if primary.is_empty() {
            pollster::block_on(instance.enumerate_adapters(wgpu::Backends::SECONDARY))
        } else {
            primary
        }
    })
}

/// How preferred an adapter is for computing, higher first: a discrete
/// GPU, then an integrated one, then the rest, software last.
pub fn rank(info: &wgpu::AdapterInfo) -> u8 {
    match info.device_type {
        wgpu::DeviceType::DiscreteGpu => 4,
        wgpu::DeviceType::IntegratedGpu => 3,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 1,
        wgpu::DeviceType::Cpu => 0,
    }
}

/// The process's device, opened on the best adapter ([`rank`]) the first
/// time; whatever adapter it was opened on afterwards.
pub fn gpu() -> Result<Arc<Gpu>, String> {
    gpu_choosing(|adapters| {
        adapters
            .iter()
            .enumerate()
            .max_by_key(|(i, a)| (rank(&a.get_info()), std::cmp::Reverse(*i)))
            .map(|(i, _)| i)
    })
}

/// The process's device, opened on the adapter `choose` picks among
/// [`adapters`] the first time (a test harness prefers a software one, for
/// pictures that do not depend on the machine); once a device is open,
/// that device, whatever `choose` would pick.
pub fn gpu_choosing(
    choose: impl FnOnce(&[wgpu::Adapter]) -> Option<usize>,
) -> Result<Arc<Gpu>, String> {
    let mut slot = DEVICE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((_, gpu)) = slot.as_ref() {
        return Ok(gpu.clone());
    }
    let adapters = adapters();
    let index = choose(adapters).ok_or("no GPU adapter")?;
    open(&mut slot, index)
}

/// The process's device on the adapter at `index` in [`adapters`]: opened
/// there if no device is open yet; an error if it is open on another
/// adapter (one device a process).
pub fn gpu_on(index: usize) -> Result<Arc<Gpu>, String> {
    let mut slot = DEVICE.lock().unwrap_or_else(PoisonError::into_inner);
    match slot.as_ref() {
        Some((i, gpu)) if *i == index => Ok(gpu.clone()),
        Some((_, gpu)) => Err(format!(
            "this process's GPU device is open on {} already: one device a process",
            gpu.adapter
        )),
        None => open(&mut slot, index),
    }
}

/// The device open now, if any, and its adapter's index.
pub fn opened() -> Option<(usize, Arc<Gpu>)> {
    DEVICE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// How many devices this process opened: at most 1.
pub fn devices_opened() -> usize {
    DEVICES.load(Ordering::SeqCst)
}

fn open(slot: &mut Option<(usize, Arc<Gpu>)>, index: usize) -> Result<Arc<Gpu>, String> {
    let adapter = adapters()
        .get(index)
        .ok_or_else(|| format!("no GPU adapter #{index}"))?;
    let gpu = Arc::new(Gpu::open(adapter)?);
    let before = DEVICES.fetch_add(1, Ordering::SeqCst);
    debug_assert_eq!(before, 0, "a second GPU device in this process");
    *slot = Some((index, gpu.clone()));
    Ok(gpu)
}
