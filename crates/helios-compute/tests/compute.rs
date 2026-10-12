//! The compute manager over fake GPUs: discovery, settings, routing,
//! fallback, budgets, calibration and commands. One test enumerates the
//! real adapters, tolerating none.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use helios_compute::{
    Adapters, Budget, CPU, Calibration, Device, DeviceKind, Job, Limits, Manager, OpenedGpu,
    RemoteSettings, RemoteState, Role, Settings, WgpuAdapters, commands,
};
use helios_core::{
    Accelerator, Bus, Describe, DeviceImage, GpuJob, Graph, Image, ModuleRef, Params, Region,
    Value, graph::SOURCE_PORT,
};
use helios_shell::commands::{Origin, Registry};
use helios_shell::when::Context;
use serde_json::json;

/// A fresh directory per test.
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("helios-compute-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn gpu(id: &str, name: &str, kind: DeviceKind) -> Device {
    Device {
        id: id.into(),
        name: name.into(),
        kind,
        backend: Some("vulkan".into()),
        driver: None,
        threads: None,
        limits: Some(Limits {
            max_buffer: 1 << 30,
            max_binding: 1 << 28,
        }),
    }
}

/// How a fake GPU behaves.
#[derive(Clone, Copy, PartialEq)]
enum Behaviour {
    Works,
    /// Does not open.
    NoDevice,
    /// Opens, then fails every run.
    Fails,
    /// Opens with smaller limits than listed: 1 MiB bindings.
    Shrinks,
}

/// A GPU that writes zeros, counting its runs.
struct FakeGpu {
    id: String,
    fails: bool,
    runs: Arc<AtomicU64>,
}

impl Accelerator for FakeGpu {
    fn id(&self) -> &str {
        &self.id
    }

    fn upload(&self, image: &Image) -> Result<DeviceImage, helios_core::AcceleratorError> {
        Ok(DeviceImage::new(
            image.region(),
            image.channels(),
            image.clone(),
        ))
    }

    fn download(&self, image: &DeviceImage) -> Result<Image, helios_core::AcceleratorError> {
        Ok(image.handle::<Image>().ok_or("not mine")?.clone())
    }

    fn run(
        &self,
        _: &str,
        _: &GpuJob,
        _: &DeviceImage,
        output: Region,
        channels: u32,
    ) -> Result<DeviceImage, helios_core::AcceleratorError> {
        if self.fails {
            return Err("device lost".into());
        }
        self.runs.fetch_add(1, Ordering::SeqCst);
        let zeros = Image::new(
            output,
            channels,
            vec![0.0; output.area() * channels as usize],
        );
        Ok(DeviceImage::new(output, channels, zeros))
    }
}

#[derive(Clone, Default)]
struct Fake {
    devices: Arc<Mutex<Vec<(Device, Behaviour)>>>,
    runs: Arc<AtomicU64>,
}

impl Fake {
    fn with(devices: Vec<(Device, Behaviour)>) -> Fake {
        Fake {
            devices: Arc::new(Mutex::new(devices)),
            runs: Arc::default(),
        }
    }
}

impl Adapters for Fake {
    fn list(&self) -> Vec<Device> {
        self.devices
            .lock()
            .unwrap()
            .iter()
            .map(|(d, _)| d.clone())
            .collect()
    }

    fn open(&self, id: &str) -> Result<OpenedGpu, String> {
        let devices = self.devices.lock().unwrap();
        let (d, b) = devices
            .iter()
            .find(|(d, _)| d.id == id)
            .ok_or("no such GPU")?;
        let shrinks = *b == Behaviour::Shrinks;
        match b {
            Behaviour::NoDevice => Err("no GPU device: out of memory".into()),
            b => Ok(OpenedGpu {
                accelerator: Arc::new(FakeGpu {
                    id: format!("fake {id}"),
                    fails: *b == Behaviour::Fails,
                    runs: self.runs.clone(),
                }),
                limits: if shrinks {
                    Some(Limits {
                        max_buffer: 1 << 20,
                        max_binding: 1 << 20,
                    })
                } else {
                    d.limits
                },
                note: shrinks.then(|| "the driver refused the adapter's limits".to_string()),
            }),
        }
    }
}

fn machine() -> Fake {
    Fake::with(vec![
        (
            gpu("gpu:vulkan:10de:2206", "Big GPU", DeviceKind::DiscreteGpu),
            Behaviour::Works,
        ),
        (
            gpu(
                "gpu:vulkan:8086:4680",
                "Small GPU",
                DeviceKind::IntegratedGpu,
            ),
            Behaviour::Works,
        ),
        (
            gpu("gpu:vulkan:10005:0000", "llvmpipe", DeviceKind::SoftwareGpu),
            Behaviour::Works,
        ),
    ])
}

fn exposure_job(role: Role) -> Job {
    Job::for_module(
        role,
        &ModuleRef::new(helios_module_exposure::Exposure),
        &Params::new(),
        Region::sized(64, 64),
    )
}

#[test]
fn discovery_lists_the_cpu_and_every_gpu_software_ones_off() {
    let d = dir("discovery");
    let m = Manager::open(&d, Box::new(machine())).unwrap();
    let devices = &m.settings().devices;
    assert_eq!(devices.len(), 4);
    assert!(devices[CPU].enabled);
    assert!(devices["gpu:vulkan:10de:2206"].enabled);
    assert!(!devices["gpu:vulkan:10005:0000"].enabled);
    // Written at once, under the injected directory.
    assert_eq!(Settings::load(&d).unwrap(), *m.settings());
    let list = m.list();
    let local = &list["units"][0];
    assert_eq!(local["name"], "local");
    assert_eq!(local["default"], true);
    assert!(
        local["devices"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["state"] == "present")
    );
}

#[test]
fn a_device_that_disappears_is_missing_its_settings_kept() {
    let d = dir("missing");
    let fake = machine();
    let mut m = Manager::open(&d, Box::new(fake.clone())).unwrap();
    m.set_role("Big GPU", Role::Batch).unwrap();
    m.set_budget(
        "big",
        &Budget {
            priority: Some(3),
            ..Default::default()
        },
    )
    .unwrap();
    drop(m);
    fake.devices.lock().unwrap().remove(0);
    let m = Manager::open(&d, Box::new(fake)).unwrap();
    let big = &m.settings().devices["gpu:vulkan:10de:2206"];
    assert_eq!(big.role, Role::Batch);
    assert_eq!(big.priority, 3);
    let list = m.list();
    let shown = list["units"][0]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "gpu:vulkan:10de:2206")
        .unwrap()
        .clone();
    assert_eq!(shown["state"], "missing");
    assert_eq!(shown["settings"]["name"], "Big GPU");
    // Never routed to, despite its priority.
    assert_eq!(
        m.route(&exposure_job(Role::Batch)).device,
        "gpu:vulkan:8086:4680"
    );
}

#[test]
fn routing_follows_roles_then_kind() {
    let d = dir("roles");
    let mut m = Manager::open(&d, Box::new(machine())).unwrap();
    let r = m.route(&exposure_job(Role::Interactive));
    assert_eq!(r.device, "gpu:vulkan:10de:2206", "{}", r.reason);
    assert!(r.reason.contains("discrete_gpu"), "{}", r.reason);
    m.set_role("Big GPU", Role::Batch).unwrap();
    assert_eq!(
        m.route(&exposure_job(Role::Interactive)).device,
        "gpu:vulkan:8086:4680"
    );
    assert_eq!(
        m.route(&exposure_job(Role::Batch)).device,
        "gpu:vulkan:10de:2206"
    );
    // A priority goes before the kind.
    m.set_budget(
        CPU,
        &Budget {
            priority: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    let r = m.route(&exposure_job(Role::Batch));
    assert_eq!(r.device, CPU);
    assert!(r.reason.starts_with("priority 1"), "{}", r.reason);
}

#[test]
fn calibrated_devices_are_chosen_by_measured_speed() {
    let d = dir("speeds");
    let mut settings = Settings::default();
    let fake = machine();
    for dev in std::iter::once(Device::cpu()).chain(fake.list()) {
        let mut s = helios_compute::DeviceSettings::new(&dev);
        let speed = match dev.kind {
            DeviceKind::IntegratedGpu => 900.0,
            DeviceKind::DiscreteGpu => 500.0,
            _ => 100.0,
        };
        s.calibration = Some(Calibration {
            speeds: BTreeMap::from([("tone".into(), speed), ("detail".into(), 10.0)]),
            at: 0,
        });
        settings.devices.insert(dev.id.clone(), s);
    }
    settings.save(&d).unwrap();
    let m = Manager::open(&d, Box::new(fake)).unwrap();
    let r = m.route(&exposure_job(Role::Interactive));
    assert_eq!(r.device, "gpu:vulkan:8086:4680");
    assert!(
        r.reason.contains("fastest measured for tone"),
        "{}",
        r.reason
    );
    assert!(r.reason.contains("Small GPU 900.0 Mpx/s"), "{}", r.reason);
}

#[test]
fn the_cpu_takes_what_no_gpu_can() {
    let d = dir("fallback");
    let fake = Fake::with(vec![
        (
            gpu("gpu:vulkan:1:1", "Broken", DeviceKind::DiscreteGpu),
            Behaviour::NoDevice,
        ),
        (
            gpu("gpu:vulkan:2:2", "Flaky", DeviceKind::IntegratedGpu),
            Behaviour::Fails,
        ),
    ]);
    let mut m = Manager::open(&d, Box::new(fake)).unwrap();
    // A GPU that does not open.
    let r = m.route(&exposure_job(Role::Interactive));
    assert_eq!(r.device, CPU);
    assert!(r.fell_back.as_deref().unwrap().contains("out of memory"));
    assert!(r.reason.contains("Broken did not open"), "{}", r.reason);
    // A GPU that fails its render: it runs on the CPU, and says so.
    m.set_enabled("Broken", false).unwrap();
    let r = m.route(&exposure_job(Role::Interactive));
    assert_eq!(r.device, "gpu:vulkan:2:2");
    let (g, node) = exposure_graph();
    let ran = r
        .render(
            Arc::new(Bus::new()),
            &g,
            node,
            "output",
            Region::sized(64, 64),
        )
        .unwrap();
    assert_eq!(ran.device, CPU);
    assert!(ran.fell_back.unwrap().contains("device lost"));
    assert!(ran.image.data().iter().any(|v| *v != 0.0));
    // Every GPU disabled.
    m.set_enabled("Flaky", false).unwrap();
    let r = m.route(&exposure_job(Role::Interactive));
    assert_eq!(r.device, CPU);
    assert!(r.fell_back.is_none());
    // A module without a GPU variant.
    m.set_enabled("Flaky", true).unwrap();
    let mut job = exposure_job(Role::Interactive);
    job.gpu = false;
    let r = m.route(&job);
    assert_eq!(r.device, CPU);
    assert!(r.reason.contains("no GPU variant"), "{}", r.reason);
}

#[test]
fn even_a_disabled_cpu_takes_what_nothing_else_does() {
    let d = dir("nocpu");
    let mut m = Manager::open(&d, Box::new(Fake::default())).unwrap();
    m.set_enabled(CPU, false).unwrap();
    let r = m.route(&exposure_job(Role::Batch));
    assert_eq!(r.device, CPU);
    assert!(r.reason.contains("the CPU runs it"), "{}", r.reason);
}

fn exposure_graph() -> (Graph, helios_core::NodeId) {
    let mut g = Graph::new();
    let src = g.add_source(
        "src",
        Image::from_fn(Region::sized(64, 64), 4, |x, y, _| {
            (x + y) as f32 / 128.0 + 0.1
        }),
    );
    let node = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new().with("exposure", Value::Float(1.0)),
        )
        .unwrap();
    g.connect(src, SOURCE_PORT, node, "input").unwrap();
    (g, node)
}

#[test]
fn budgets_reach_the_engine_and_the_route() {
    let d = dir("budgets");
    let fake = machine();
    let mut m = Manager::open(&d, Box::new(fake.clone())).unwrap();
    m.set_budget(
        "Big GPU",
        &Budget {
            cache_mb: Some(Some(64)),
            vram_mb: Some(Some(1)),
            ..Default::default()
        },
    )
    .unwrap();
    m.set_budget(
        CPU,
        &Budget {
            threads: Some(Some(1)),
            cache_mb: Some(Some(8)),
            ..Default::default()
        },
    )
    .unwrap();
    // A small job fits 1 MiB: on the GPU, with its cache budget.
    let r = m.route(&exposure_job(Role::Interactive));
    assert_eq!(r.device, "gpu:vulkan:10de:2206");
    assert_eq!(r.cache_bytes, Some(64 << 20));
    assert_eq!(r.threads, 1);
    let engine = r.engine(Arc::new(Bus::new()));
    assert_eq!(engine.describe()["cache"]["budget"], json!(64 << 20));
    let (g, node) = exposure_graph();
    let ran = r
        .render(
            Arc::new(Bus::new()),
            &g,
            node,
            "output",
            Region::sized(64, 64),
        )
        .unwrap();
    assert_eq!(ran.device, "gpu:vulkan:10de:2206");
    assert!(fake.runs.load(Ordering::SeqCst) > 0);
    // A large one does not: the integrated GPU takes it.
    let mut big = exposure_job(Role::Interactive);
    big.pixels = 1024 * 1024;
    assert_eq!(m.route(&big).device, "gpu:vulkan:8086:4680");
    // Nor what a GPU binds.
    big.pixels = 1 << 28;
    let r = m.route(&big);
    assert_eq!(r.device, CPU);
    assert_eq!(r.cache_bytes, Some(8 << 20));
    // Budgets that make no sense are refused.
    let threads = Budget {
        threads: Some(Some(2)),
        ..Default::default()
    };
    assert!(m.set_budget("Big GPU", &threads).is_err());
    let vram = Budget {
        vram_mb: Some(Some(2)),
        ..Default::default()
    };
    assert!(m.set_budget(CPU, &vram).is_err());
}

#[test]
fn settings_survive_a_round_trip() {
    let d = dir("roundtrip");
    let mut m = Manager::open(&d, Box::new(machine())).unwrap();
    m.set_enabled("llvmpipe", true).unwrap();
    m.set_role(CPU, Role::Batch).unwrap();
    m.set_budget(
        "Small",
        &Budget {
            vram_mb: Some(Some(512)),
            cache_mb: Some(Some(256)),
            priority: Some(-2),
            ..Default::default()
        },
    )
    .unwrap();
    m.add_remote(RemoteSettings {
        name: "studio".into(),
        address: "tcp://127.0.0.1:7700".into(),
        token_file: Some("/run/helios/token".into()),
    })
    .unwrap();
    let saved = m.settings().clone();
    drop(m);
    let m = Manager::open(&d, Box::new(machine())).unwrap();
    assert_eq!(*m.settings(), saved);
    let small = &saved.devices["gpu:vulkan:8086:4680"];
    assert_eq!(
        (small.vram_mb, small.cache_mb, small.priority),
        (Some(512), Some(256), -2)
    );
    // Clearing a budget.
    let mut m = m;
    m.set_budget(
        "Small",
        &Budget {
            vram_mb: Some(None),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        Settings::load(&d).unwrap().devices["gpu:vulkan:8086:4680"].vram_mb,
        None
    );
}

#[test]
fn a_benchmark_measures_each_family_and_keeps_it() {
    let d = dir("bench");
    let mut m = Manager::open(&d, Box::new(machine()))
        .unwrap()
        .with_bench_tile(64);
    let reports = m.benchmark(None).unwrap();
    // The CPU and the two enabled GPUs, not the software one.
    assert_eq!(reports.len(), 3);
    for r in &reports {
        let speeds = r.speeds.as_ref().unwrap();
        assert!(speeds["tone"] > 0.0 && speeds["detail"] > 0.0, "{speeds:?}");
    }
    let saved = Settings::load(&d).unwrap();
    assert!(saved.devices[CPU].calibration.is_some());
    assert!(saved.devices["gpu:vulkan:10005:0000"].calibration.is_none());
    // Every candidate is now calibrated: the route says by how much.
    let r = m.route(&exposure_job(Role::Interactive));
    assert!(
        r.reason.contains("fastest measured for tone"),
        "{}",
        r.reason
    );
}

#[test]
fn a_gpu_that_fails_its_benchmark_says_so() {
    let d = dir("benchfail");
    let fake = Fake::with(vec![(
        gpu("gpu:vulkan:2:2", "Flaky", DeviceKind::IntegratedGpu),
        Behaviour::Fails,
    )]);
    let mut m = Manager::open(&d, Box::new(fake))
        .unwrap()
        .with_bench_tile(64);
    let reports = m.benchmark(Some("Flaky")).unwrap();
    assert!(
        reports[0]
            .speeds
            .as_ref()
            .unwrap_err()
            .contains("device lost")
    );
    assert!(m.settings().devices["gpu:vulkan:2:2"].calibration.is_none());
}

#[test]
fn the_commands_drive_the_manager() {
    let d = dir("commands");
    let m = Arc::new(Mutex::new(
        Manager::open(&d, Box::new(machine()))
            .unwrap()
            .with_bench_tile(64),
    ));
    let mut r = Registry::new();
    commands::register(&mut r, m.clone()).unwrap();
    let cx = Context::new();
    let mut run = |id: &str, args| r.run(id, args, Origin::User, &cx);
    let list = run("compute.list", json!({})).unwrap();
    assert_eq!(list["units"][0]["devices"].as_array().unwrap().len(), 4);
    assert_eq!(
        run("compute.disable", json!({ "device": "Big GPU" })).unwrap()["device"],
        "gpu:vulkan:10de:2206"
    );
    run("compute.enable", json!({ "device": "llvmpipe" })).unwrap();
    run(
        "compute.role",
        json!({ "device": "small", "role": "batch" }),
    )
    .unwrap();
    assert!(run("compute.role", json!({ "device": "small", "role": "fast" })).is_err());
    let b = run(
        "compute.budget",
        json!({ "device": "cpu", "threads": 2, "cache_mb": 128, "priority": 5 }),
    )
    .unwrap();
    assert_eq!(b["settings"]["threads"], 2);
    assert!(run("compute.budget", json!({ "device": "cpu" })).is_err());
    assert!(
        run("compute.enable", json!({ "device": "GPU" })).is_err(),
        "ambiguous"
    );
    let route = run(
        "compute.route",
        json!({ "role": "interactive", "family": "tone" }),
    )
    .unwrap();
    assert_eq!(route["device"], "cpu");
    assert!(route["reason"].as_str().unwrap().starts_with("priority 5"));
    let bench = run("compute.benchmark", json!({ "device": "cpu" })).unwrap();
    assert!(bench[0]["mpx_per_s"]["tone"].as_f64().unwrap() > 0.0);
    run(
        "compute.remote",
        json!({ "name": "studio", "address": d.join("nowhere.sock").to_str().unwrap() }),
    )
    .unwrap();
    let probe = run("compute.probe", json!({ "unit": "studio" })).unwrap();
    assert_eq!(probe["state"], "unreachable");
    let s = m.lock().unwrap();
    let saved = Settings::load(s.dir()).unwrap();
    assert!(!saved.devices["gpu:vulkan:10de:2206"].enabled);
    assert!(saved.devices["gpu:vulkan:10005:0000"].enabled);
    assert_eq!(saved.devices["gpu:vulkan:8086:4680"].role, Role::Batch);
    assert_eq!(saved.devices[CPU].cache_mb, Some(128));
}

#[cfg(unix)]
#[test]
fn a_remote_unit_is_probed_over_its_socket() {
    use helios_inspect::Inspector;

    struct Modules;
    impl Describe for Modules {
        fn describe(&self) -> serde_json::Value {
            json!({ "modules": { "exposure": {}, "blur": {} } })
        }
    }

    let d = dir("remote");
    std::fs::create_dir_all(&d).unwrap();
    let socket = d.join("node.sock");
    let inspector = Inspector::new(Arc::new(Bus::new())).with("modules", Arc::new(Modules));
    helios_inspect::serve(inspector, &socket).unwrap();
    let mut m = Manager::open(&d, Box::new(Fake::default())).unwrap();
    m.add_remote(RemoteSettings {
        name: "node".into(),
        address: socket.to_str().unwrap().into(),
        token_file: None,
    })
    .unwrap();
    assert_eq!(m.list()["units"][1]["probe"]["state"], "not_probed");
    let state = m.probe("node").unwrap();
    assert_eq!(
        state,
        RemoteState::Reachable {
            modules: vec!["blur".into(), "exposure".into()]
        }
    );
    assert_eq!(m.list()["units"][1]["probe"]["state"], "reachable");
}

#[test]
fn jobs_are_planned_with_the_opened_devices_limits() {
    let d = dir("shrinks");
    let fake = Fake::with(vec![(
        gpu("gpu:vulkan:1:1", "Shrinking", DeviceKind::DiscreteGpu),
        Behaviour::Shrinks,
    )]);
    let m = Manager::open(&d, Box::new(fake)).unwrap();
    // 64 x 64 RGBA f32 is 64 KiB: it fits the 1 MiB the device opened with.
    let small = exposure_job(Role::Interactive);
    assert_eq!(m.route(&small).device, "gpu:vulkan:1:1");
    // 4 MiB fits the survey's 256 MiB, not the device: the CPU takes it.
    let large = Job::for_module(
        Role::Interactive,
        &ModuleRef::new(helios_module_exposure::Exposure),
        &Params::new(),
        Region::sized(1024, 1024),
    );
    assert_eq!(m.route(&large).device, CPU);
    // The opened device and why it allows less are listed.
    let listed = &m.list()["units"][0]["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "gpu:vulkan:1:1")
        .cloned()
        .unwrap();
    assert_eq!(listed["opened"]["limits"]["max_binding"], 1 << 20);
    assert!(
        listed["opened"]["note"]
            .as_str()
            .unwrap()
            .contains("refused")
    );
    // Opened first by a large job: it is routed again on what opened.
    let m = Manager::open(
        &dir("shrinks-first"),
        Box::new(Fake::with(vec![(
            gpu("gpu:vulkan:1:1", "Shrinking", DeviceKind::DiscreteGpu),
            Behaviour::Shrinks,
        )])),
    )
    .unwrap();
    assert_eq!(m.route(&large).device, CPU);
    assert_eq!(m.route(&small).device, "gpu:vulkan:1:1");
}

#[test]
fn a_real_gpu_opens_with_the_limits_it_is_listed_with() {
    let _gpu = helios_testkit::gpu_lock();
    let adapters = WgpuAdapters::new();
    let mut opened_one = false;
    for d in adapters.list() {
        // One device a process: once one is open, the others say so.
        let opened = match adapters.open(&d.id) {
            Ok(o) => o,
            Err(e) if opened_one && e.contains("one device a process") => continue,
            Err(e) => panic!("{}: {e}", d.name),
        };
        opened_one = true;
        eprintln!("{} {}: {:?} {:?}", d.id, d.name, opened.limits, opened.note);
        if opened.note.is_none() {
            assert_eq!(opened.limits, d.limits, "{}", d.name);
        }
    }
}

#[test]
fn the_real_adapters_are_listed_whatever_there_is() {
    let _gpu = helios_testkit::gpu_lock();
    let adapters = WgpuAdapters::new();
    let devices = adapters.list();
    eprintln!("{} adapter(s)", devices.len());
    for d in &devices {
        eprintln!("  {} {} {:?}", d.id, d.name, d.kind);
        assert!(d.kind.is_gpu());
        assert!(d.id.starts_with("gpu:"));
    }
    let mut ids: Vec<&String> = devices.iter().map(|d| &d.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), devices.len(), "ids are unique");
    let d = dir("real");
    let mut m = Manager::open(&d, Box::new(adapters))
        .unwrap()
        .with_bench_tile(64);
    assert_eq!(m.settings().devices.len(), devices.len() + 1);
    // Whatever was chosen runs a render, on its GPU or back on the CPU.
    let r = m.route(&exposure_job(Role::Interactive));
    eprintln!("routed to {}: {}", r.device, r.reason);
    let (g, node) = exposure_graph();
    let ran = r
        .render(
            Arc::new(Bus::new()),
            &g,
            node,
            "output",
            Region::sized(64, 64),
        )
        .unwrap();
    eprintln!("ran on {} ({:?})", ran.device, ran.fell_back);
    // The first GPU, through helios-gpu, measured.
    if let Some(first) = devices.first() {
        let reports = m.benchmark(Some(&first.id)).unwrap();
        eprintln!("{}: {:?}", first.name, reports[0].speeds);
        assert!(reports[0].speeds.is_ok());
    }
}
