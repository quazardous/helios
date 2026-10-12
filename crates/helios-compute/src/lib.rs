//! Compute units: the machines that run renders, and their devices.
//!
//! A compute unit is a machine. The local one is always there and is the
//! default; remote units are helios-node instances reached over their
//! socket ([`remote`]). The local unit's devices are its CPU and every GPU
//! wgpu sees ([`device`]); each has machine-scope settings (enabled, role,
//! priority, budgets) kept in a local file that is never synced
//! ([`settings`]), and a calibration: a short benchmark giving its speed
//! per module family ([`bench`]).
//!
//! The [`Manager`] answers "which device for this job" ([`Manager::route`])
//! and builds an engine for the answer ([`Route::engine`]): the chosen GPU
//! as its accelerator, the device's cache budget. A GPU disabled, absent,
//! that does not open or that fails a render falls back to the CPU, and the
//! answer says where the work ran and why. [`commands`] declares the
//! `compute.*` commands.

pub mod bench;
pub mod commands;
pub mod device;
pub mod remote;
pub mod settings;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use helios_core::{
    Accelerator, Bus, Context, Engine, Error, Graph, Image, ModuleRef, NodeId, Params, Region,
};
use serde::Serialize;
use serde_json::{Value as Json, json};

pub use device::{Adapters, CPU, Device, DeviceKind, Limits, OpenedGpu, WgpuAdapters};
pub use remote::RemoteState;
pub use settings::{Calibration, DeviceSettings, RemoteSettings, Role, Settings};

const MIB: u64 = 1 << 20;

/// A job to place.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Job {
    /// `Interactive` or `Batch` (`Both`: either kind of device).
    pub role: Role,
    /// The module's family (`tone`, `detail`…), to compare calibrations.
    pub family: Option<String>,
    /// Whether the module has a GPU variant.
    pub gpu: bool,
    pub pixels: u64,
    pub channels: u32,
}

impl Job {
    /// A module's job over a region.
    pub fn for_module(role: Role, module: &ModuleRef, params: &Params, region: Region) -> Job {
        let m = module.manifest();
        let gpu = m.inputs.len() == 1
            && m.outputs.len() == 1
            && matches!(
                module.gpu_job(params, &Context::new(), 1.0, region),
                Ok(Some(_))
            );
        Job {
            role,
            family: serde_json::to_value(m.family)
                .ok()
                .and_then(|f| f.as_str().map(String::from)),
            gpu,
            pixels: region.area() as u64,
            channels: m
                .outputs
                .first()
                .and_then(helios_core::PortSpec::channels)
                .unwrap_or(4),
        }
    }

    /// Bytes of one buffer of the job (its output).
    fn bytes(&self) -> u64 {
        self.pixels * u64::from(self.channels) * 4
    }
}

/// Where a job goes, and an engine to run it there.
#[derive(Clone)]
pub struct Route {
    /// The device id.
    pub device: String,
    pub name: String,
    pub kind: DeviceKind,
    /// Why this device, in words.
    pub reason: String,
    /// Why the device first chosen is not used, when it fell back to the CPU.
    pub fell_back: Option<String>,
    /// CPU threads the work may use.
    pub threads: usize,
    /// The engine's cache budget, in bytes (`None`: no limit; a device
    /// without one gets the default share of memory).
    pub cache_bytes: Option<usize>,
    accelerator: Option<Arc<dyn Accelerator>>,
    /// The CPU's cache budget, for a fallback.
    cpu_cache_bytes: Option<usize>,
}

impl std::fmt::Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.describe())
    }
}

/// A render, and where it ran.
pub struct Ran {
    pub image: Image,
    /// The device id.
    pub device: String,
    /// Why it did not run on the device routed to, when it fell back.
    pub fell_back: Option<String>,
}

impl Route {
    /// An engine configured for the device: its accelerator, its budget.
    pub fn engine(&self, bus: Arc<Bus>) -> Engine {
        let engine = Engine::with_budget(bus, self.cache_bytes);
        match &self.accelerator {
            Some(a) => engine.with_accelerator(a.clone()),
            None => engine,
        }
    }

    /// Renders on the device, falling back to the CPU when the GPU fails:
    /// step by step in the engine (a kernel that fails runs on the CPU, a
    /// lost device is rebuilt or left), else the whole render.
    pub fn render(
        &self,
        bus: Arc<Bus>,
        graph: &Graph,
        node: NodeId,
        port: &str,
        region: Region,
    ) -> Result<Ran, Error> {
        let engine = self.engine(bus.clone());
        let first = engine.render(graph, node, port, region);
        match first {
            Ok(image) => {
                let n = engine.counters();
                let (device, fell_back) = match engine.fallback_reason() {
                    // Every step that would have run on the GPU ran on the CPU.
                    Some(why) if n.accelerated == 0 => {
                        (CPU.into(), Some(format!("{} failed: {why}", self.name)))
                    }
                    Some(why) => (
                        self.device.clone(),
                        Some(format!(
                            "{} step(s) fell back to the CPU: {why}",
                            n.fallbacks
                        )),
                    ),
                    None => (self.device.clone(), self.fell_back.clone()),
                };
                Ok(Ran {
                    image,
                    device,
                    fell_back,
                })
            }
            Err(e) if self.accelerator.is_some() => {
                let image = Engine::with_budget(bus, self.cpu_cache_bytes)
                    .render(graph, node, port, region)?;
                Ok(Ran {
                    image,
                    device: CPU.into(),
                    fell_back: Some(format!("{} failed: {e}", self.name)),
                })
            }
            Err(e) => Err(e),
        }
    }

    pub fn describe(&self) -> Json {
        json!({
            "device": self.device,
            "name": self.name,
            "kind": self.kind,
            "reason": self.reason,
            "fell_back": self.fell_back,
            "threads": self.threads,
            "cache_bytes": self.cache_bytes,
        })
    }
}

/// What a benchmark gave for a device.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BenchReport {
    pub device: String,
    pub name: String,
    /// Megapixels per second by family, or why it failed.
    pub speeds: Result<BTreeMap<String, f64>, String>,
}

/// What a budget change asks; `None` leaves a field as it is, `Some(None)`
/// clears it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Budget {
    pub threads: Option<Option<usize>>,
    pub vram_mb: Option<Option<u64>>,
    pub cache_mb: Option<Option<u64>>,
    pub priority: Option<i32>,
}

/// A GPU opened, or why it did not open.
type Opened = Result<OpenedGpu, String>;

/// The compute units of this machine, their devices and settings.
pub struct Manager {
    dir: PathBuf,
    adapters: Box<dyn Adapters>,
    /// The devices there now: the CPU first.
    present: Vec<Device>,
    settings: Settings,
    remotes: BTreeMap<String, RemoteState>,
    /// GPUs opened, or why they did not open.
    opened: Mutex<HashMap<String, Opened>>,
    /// The side of the benchmark's tile.
    bench_tile: u32,
}

impl Manager {
    /// The settings in `dir` (see [`settings::default_dir`]) over the
    /// devices `adapters` finds. Devices seen for the first time get their
    /// default settings, written at once.
    pub fn open(dir: &Path, adapters: Box<dyn Adapters>) -> Result<Manager, String> {
        let settings = Settings::load(dir)?;
        let mut manager = Manager {
            dir: dir.to_path_buf(),
            adapters,
            present: Vec::new(),
            settings,
            remotes: BTreeMap::new(),
            opened: Mutex::default(),
            bench_tile: bench::TILE,
        };
        manager.rescan()?;
        Ok(manager)
    }

    /// Looks for devices again (real GPUs are enumerated once a process,
    /// see `helios_gpu::shared`); GPUs that failed to open are tried again.
    pub fn rescan(&mut self) -> Result<(), String> {
        self.present = std::iter::once(Device::cpu())
            .chain(self.adapters.list())
            .collect();
        self.opened.lock().expect("opened lock").clear();
        let mut new = false;
        for d in &self.present {
            if let Some(s) = self.settings.devices.get_mut(&d.id) {
                // Its name may change with a driver; keep the latest.
                s.name.clone_from(&d.name);
                s.kind = d.kind;
            } else {
                self.settings
                    .devices
                    .insert(d.id.clone(), DeviceSettings::new(d));
                new = true;
            }
        }
        if new { self.save() } else { Ok(()) }
    }

    /// Benchmarks over a tile of another side: a smaller one is quicker
    /// and less telling (tests use one).
    pub fn with_bench_tile(mut self, side: u32) -> Self {
        self.bench_tile = side;
        self
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn save(&self) -> Result<(), String> {
        self.settings.save(&self.dir)
    }

    fn present(&self, id: &str) -> Option<&Device> {
        self.present.iter().find(|d| d.id == id)
    }

    /// A device's id from its id, its name or a part of its name that
    /// names one device only.
    pub fn resolve(&self, query: &str) -> Result<String, String> {
        let devices = &self.settings.devices;
        if devices.contains_key(query) {
            return Ok(query.into());
        }
        let q = query.to_lowercase();
        let exact: Vec<&String> = devices
            .iter()
            .filter(|(_, s)| s.name.to_lowercase() == q)
            .map(|(id, _)| id)
            .collect();
        let found = if exact.is_empty() {
            devices
                .iter()
                .filter(|(_, s)| s.name.to_lowercase().contains(&q))
                .map(|(id, _)| id)
                .collect()
        } else {
            exact
        };
        match found.as_slice() {
            [id] => Ok((*id).clone()),
            [] => Err(format!(
                "no device `{query}`; there are: {}",
                devices.keys().cloned().collect::<Vec<_>>().join(", ")
            )),
            many => Err(format!(
                "`{query}` names several devices: {}",
                many.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    fn change(
        &mut self,
        query: &str,
        f: impl FnOnce(&mut DeviceSettings) -> Result<(), String>,
    ) -> Result<String, String> {
        let id = self.resolve(query)?;
        let s = self.settings.devices.get_mut(&id).expect("resolved");
        f(s)?;
        self.save()?;
        Ok(id)
    }

    pub fn set_enabled(&mut self, query: &str, enabled: bool) -> Result<String, String> {
        self.change(query, |s| {
            s.enabled = enabled;
            Ok(())
        })
    }

    pub fn set_role(&mut self, query: &str, role: Role) -> Result<String, String> {
        self.change(query, |s| {
            s.role = role;
            Ok(())
        })
    }

    /// Changes a device's budgets. Threads are a CPU budget, GPU memory a
    /// GPU one.
    pub fn set_budget(&mut self, query: &str, budget: &Budget) -> Result<String, String> {
        self.change(query, |s| {
            if let Some(threads) = budget.threads {
                if s.kind.is_gpu() {
                    return Err("`threads` is a CPU budget".into());
                }
                if threads == Some(0) {
                    return Err("`threads` is at least 1".into());
                }
                s.threads = threads;
            }
            if let Some(vram) = budget.vram_mb {
                if !s.kind.is_gpu() {
                    return Err("`vram_mb` is a GPU budget".into());
                }
                s.vram_mb = vram;
            }
            if let Some(cache) = budget.cache_mb {
                s.cache_mb = cache;
            }
            if let Some(priority) = budget.priority {
                s.priority = priority;
            }
            Ok(())
        })
    }

    /// Declares (or redeclares) a remote unit; it is not probed yet.
    pub fn add_remote(&mut self, remote: RemoteSettings) -> Result<(), String> {
        if remote.name == "local" {
            return Err("`local` is this machine".into());
        }
        self.remotes.remove(&remote.name);
        self.settings.remotes.retain(|r| r.name != remote.name);
        self.settings.remotes.push(remote);
        self.save()
    }

    /// Asks a remote unit what it runs.
    pub fn probe(&mut self, name: &str) -> Result<RemoteState, String> {
        let remote = self
            .settings
            .remotes
            .iter()
            .find(|r| r.name == name)
            .ok_or_else(|| format!("no remote unit `{name}`"))?;
        let state = remote::probe(remote);
        self.remotes.insert(name.into(), state.clone());
        Ok(state)
    }

    /// Every unit and its devices, missing ones included.
    pub fn list(&self) -> Json {
        let devices: Vec<Json> = self
            .settings
            .devices
            .iter()
            .map(|(id, s)| {
                let present = self.present(id);
                let mut v = json!({
                    "id": id,
                    "state": if present.is_some() { "present" } else { "missing" },
                    "settings": s,
                });
                if let Some(d) = present {
                    v["device"] = serde_json::to_value(d).expect("devices serialize");
                }
                if let Some(Ok(o)) = self.opened.lock().expect("opened lock").get(id) {
                    v["opened"] = json!({ "limits": o.limits, "note": o.note });
                }
                v
            })
            .collect();
        let mut units = vec![json!({
            "name": "local",
            "kind": "local",
            "default": true,
            "devices": devices,
        })];
        for r in &self.settings.remotes {
            units.push(json!({
                "name": r.name,
                "kind": "remote",
                "address": r.address,
                "probe": self.remotes.get(&r.name).cloned().unwrap_or_default(),
                "devices": Json::Null,
                "note": "a node does not report its devices yet; no work is routed to it",
            }));
        }
        json!({ "units": units })
    }

    /// A GPU opened, its memory budget set from its settings (`vram_mb`,
    /// else the device's default).
    fn opened(&self, id: &str) -> Result<OpenedGpu, String> {
        let mut opened = self.opened.lock().expect("opened lock");
        let gpu = opened
            .entry(id.into())
            .or_insert_with(|| self.adapters.open(id))
            .clone();
        if let Ok(gpu) = &gpu {
            let vram = self.settings.devices.get(id).and_then(|s| s.vram_mb);
            gpu.accelerator.set_memory_budget(vram.map(|mb| mb * MIB));
        }
        gpu
    }

    /// What a GPU allows: its opened device's limits once it is open,
    /// before that the survey's, which are what it is opened with.
    fn limits(&self, d: &Device) -> Option<Limits> {
        match self.opened.lock().expect("opened lock").get(&d.id) {
            Some(Ok(OpenedGpu {
                limits: Some(limits),
                ..
            })) => Some(*limits),
            _ => d.limits,
        }
    }

    /// Whether a job fits a GPU with `limits`: one buffer within what it
    /// binds and creates, input and output within its memory budget.
    fn fits(job: &Job, limits: Option<Limits>, s: &DeviceSettings) -> bool {
        let bytes = job.bytes();
        let binds = limits.is_none_or(|l| bytes <= l.max_binding && bytes <= l.max_buffer);
        let budget = s.vram_mb.is_none_or(|mb| 2 * bytes <= mb * MIB);
        binds && budget
    }

    fn cpu_threads(&self) -> usize {
        let hw = self.present[0].threads.unwrap_or(1);
        self.settings
            .devices
            .get(CPU)
            .and_then(|s| s.threads)
            .unwrap_or(hw)
            .min(hw)
    }

    fn cache_bytes(&self, id: &str) -> Option<usize> {
        match self.settings.devices.get(id).and_then(|s| s.cache_mb) {
            Some(mb) => Some((mb * MIB) as usize),
            None => helios_core::engine::memory_share(
                helios_core::engine::DEFAULT_CACHE_SHARE,
                helios_core::engine::physical_memory(),
            ),
        }
    }

    /// Which device for a job: among the devices present, enabled, of a
    /// role that takes it (and for a GPU, with the job's module running
    /// there and fitting its memory), the highest priority; then the
    /// fastest measured for the job's family when every one is calibrated,
    /// else by kind (a discrete GPU first). The CPU when nothing else is.
    pub fn route(&self, job: &Job) -> Route {
        if !job.gpu {
            return self.on_cpu("the module has no GPU variant".into(), None);
        }
        let mut candidates: Vec<(&Device, &DeviceSettings)> = self
            .present
            .iter()
            .filter_map(|d| self.settings.devices.get(&d.id).map(|s| (d, s)))
            .filter(|(d, s)| {
                s.enabled
                    && s.role.takes(job.role)
                    && (!d.kind.is_gpu() || Self::fits(job, self.limits(d), s))
            })
            .collect();
        let cpu = &self.present[0];
        let Some(top) = candidates.iter().map(|(_, s)| s.priority).max() else {
            return self.on_cpu(
                "no enabled device takes this job; the CPU runs it".into(),
                None,
            );
        };
        candidates.retain(|(_, s)| s.priority == top);
        let family = job.family.as_deref().unwrap_or("other");
        let speed = |s: &DeviceSettings| {
            s.calibration
                .as_ref()
                .and_then(|c| c.speeds.get(family))
                .copied()
        };
        let (chosen, reason) = if candidates.len() == 1 {
            (candidates[0], "the only device that takes it".to_string())
        } else if candidates.iter().all(|(_, s)| speed(s).is_some()) {
            let best = *candidates
                .iter()
                .max_by(|a, b| speed(a.1).partial_cmp(&speed(b.1)).expect("finite"))
                .expect("candidates");
            let all: Vec<String> = candidates
                .iter()
                .map(|(d, s)| format!("{} {:.1} Mpx/s", d.name, speed(s).expect("all")))
                .collect();
            (
                best,
                format!("fastest measured for {family}: {}", all.join(", ")),
            )
        } else {
            let best = *candidates
                .iter()
                .max_by_key(|(d, _)| (d.kind.rank(), std::cmp::Reverse(d.id.clone())))
                .expect("candidates");
            (
                best,
                format!(
                    "not every device is calibrated for {family}: chose by kind ({})",
                    serde_json::to_value(best.0.kind)
                        .expect("kinds serialize")
                        .as_str()
                        .unwrap_or_default()
                ),
            )
        };
        let reason = if top != 0 {
            format!("priority {top}; {reason}")
        } else {
            reason
        };
        let (d, s) = chosen;
        if d.id == cpu.id {
            return self.on_cpu(reason, None);
        }
        match self.opened(&d.id) {
            // The device opened with less than its survey said (a driver
            // refused its adapter's limits): route again on what it allows.
            Ok(opened) if !Self::fits(job, opened.limits.or(d.limits), s) => self.route(job),
            Ok(OpenedGpu { accelerator, .. }) => Route {
                device: d.id.clone(),
                name: d.name.clone(),
                kind: d.kind,
                reason,
                fell_back: None,
                threads: self.cpu_threads(),
                cache_bytes: self.cache_bytes(&d.id),
                accelerator: Some(accelerator),
                cpu_cache_bytes: self.cache_bytes(CPU),
            },
            Err(e) => self.on_cpu(
                format!("{reason}; {} did not open, the CPU runs it", d.name),
                Some(e),
            ),
        }
    }

    fn on_cpu(&self, reason: String, fell_back: Option<String>) -> Route {
        let cpu = &self.present[0];
        Route {
            device: cpu.id.clone(),
            name: cpu.name.clone(),
            kind: cpu.kind,
            reason,
            fell_back,
            threads: self.cpu_threads(),
            cache_bytes: self.cache_bytes(CPU),
            accelerator: None,
            cpu_cache_bytes: self.cache_bytes(CPU),
        }
    }

    /// Calibrates one device (`Some`, even disabled) or every enabled one
    /// present, and keeps the speeds with its settings.
    pub fn benchmark(&mut self, query: Option<&str>) -> Result<Vec<BenchReport>, String> {
        let ids: Vec<String> = match query {
            Some(q) => {
                let id = self.resolve(q)?;
                if self.present(&id).is_none() {
                    return Err(format!("`{id}` is missing: it cannot be measured"));
                }
                vec![id]
            }
            None => self
                .present
                .iter()
                .filter(|d| self.settings.devices.get(&d.id).is_some_and(|s| s.enabled))
                .map(|d| d.id.clone())
                .collect(),
        };
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut reports = Vec::new();
        for id in ids {
            let device = self.present(&id).expect("present").clone();
            let speeds = if device.kind.is_gpu() {
                self.opened(&id)
                    .and_then(|o| bench::calibrate(Some(o.accelerator), self.bench_tile))
            } else {
                bench::calibrate(None, self.bench_tile)
            };
            if let Ok(speeds) = &speeds {
                let s = self.settings.devices.get_mut(&id).expect("present");
                s.calibration = Some(Calibration {
                    speeds: speeds.clone(),
                    at,
                });
            }
            reports.push(BenchReport {
                device: id,
                name: device.name,
                speeds,
            });
        }
        self.save()?;
        Ok(reports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_fits_the_limits_it_is_given() {
        let gpu = Device {
            id: "gpu".into(),
            name: "GPU".into(),
            kind: DeviceKind::DiscreteGpu,
            backend: None,
            driver: None,
            threads: None,
            limits: None,
        };
        let s = DeviceSettings::new(&gpu);
        // 24 MP RGBA f32: 384 MB, one buffer.
        let job = Job {
            role: Role::Both,
            family: None,
            gpu: true,
            pixels: 24_000_000,
            channels: 4,
        };
        let limits = |max_buffer: u64, max_binding: u64| {
            Some(Limits {
                max_buffer,
                max_binding,
            })
        };
        // wgpu's defaults: 256 MiB buffers, 128 MiB bindings.
        assert!(!Manager::fits(&job, limits(256 << 20, 128 << 20), &s));
        // A binding large enough in a buffer too small.
        assert!(!Manager::fits(&job, limits(256 << 20, 2 << 30), &s));
        assert!(Manager::fits(&job, limits(2 << 30, 2 << 30), &s));
        assert!(
            Manager::fits(&job, None, &s),
            "unknown limits do not refuse"
        );
        // The memory budget counts input and output.
        let budget = DeviceSettings {
            vram_mb: Some(512),
            ..s
        };
        assert!(!Manager::fits(&job, limits(2 << 30, 2 << 30), &budget));
    }
}
