//! The `compute.*` commands, over a shared [`Manager`]:
//!
//! - `compute.list`: every unit and its devices, missing ones included;
//! - `compute.enable`, `compute.disable` (`device`);
//! - `compute.role` (`device`, `role`: `interactive`, `batch` or `both`);
//! - `compute.budget` (`device`; `threads`, `vram_mb`, `cache_mb`,
//!   `priority`; `null` clears a budget);
//! - `compute.benchmark` (`device`, or every enabled device);
//! - `compute.route` (`role`, `family`, `gpu`, `width`, `height`,
//!   `channels`): which device would take such a job, and why;
//! - `compute.remote` (`name`, `address`, `token_file`) declares a remote
//!   unit, `compute.probe` (`unit`) asks it what it runs.
//!
//! A device is named by its id, its name or a part of it naming one device.

use std::sync::{Arc, Mutex, MutexGuard};

use helios_shell::commands::{CommandSpec, Registry};
use serde_json::{Value as Json, json};

use crate::{Budget, Job, Manager, RemoteSettings, Role};

const CATEGORY: &str = "Compute";

fn lock(m: &Mutex<Manager>) -> Result<MutexGuard<'_, Manager>, String> {
    m.lock()
        .map_err(|_| "the compute manager is poisoned".to_string())
}

fn text<'a>(args: &'a Json, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("`{key}` is missing"))
}

/// A budget field: absent leaves it, `null` clears it, a number sets it.
fn field(args: &Json, key: &str) -> Result<Option<Option<u64>>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(Json::Null) => Ok(Some(None)),
        Some(v) => v
            .as_u64()
            .map(|n| Some(Some(n)))
            .ok_or_else(|| format!("`{key}` is a whole number or null, not {v}")),
    }
}

fn budget_of(args: &Json) -> Result<Budget, String> {
    let priority = match args.get("priority") {
        None => None,
        Some(v) => Some(
            v.as_i64()
                .and_then(|p| i32::try_from(p).ok())
                .ok_or_else(|| format!("`priority` is a whole number, not {v}"))?,
        ),
    };
    let budget = Budget {
        threads: field(args, "threads")?.map(|t| t.map(|n| n as usize)),
        vram_mb: field(args, "vram_mb")?,
        cache_mb: field(args, "cache_mb")?,
        priority,
    };
    if budget == Budget::default() {
        return Err("say what to set: `threads`, `vram_mb`, `cache_mb` or `priority`".into());
    }
    Ok(budget)
}

fn job_of(args: &Json) -> Result<Job, String> {
    let n = |key: &str, default: u64| args.get(key).and_then(Json::as_u64).unwrap_or(default);
    Ok(Job {
        role: match args.get("role").and_then(Json::as_str) {
            Some(r) => Role::parse(r)?,
            None => Role::Interactive,
        },
        family: args.get("family").and_then(Json::as_str).map(String::from),
        gpu: args.get("gpu").and_then(Json::as_bool).unwrap_or(true),
        pixels: n("width", 1024) * n("height", 1024),
        channels: n("channels", 4) as u32,
    })
}

/// Declares the `compute.*` commands on a registry, over a shared manager.
pub fn register(registry: &mut Registry, manager: Arc<Mutex<Manager>>) -> Result<(), String> {
    let m = manager.clone();
    registry.register(
        CommandSpec::new(
            "compute.list",
            "List the compute units and their devices",
            CATEGORY,
        ),
        move |_, _| Ok(lock(&m)?.list()),
    )?;
    for (id, title, enabled) in [
        ("compute.enable", "Enable a compute device (`device`)", true),
        (
            "compute.disable",
            "Disable a compute device (`device`)",
            false,
        ),
    ] {
        let m = manager.clone();
        registry.register(CommandSpec::new(id, title, CATEGORY), move |args, _| {
            let device = lock(&m)?.set_enabled(text(args, "device")?, enabled)?;
            Ok(json!({ "device": device, "enabled": enabled }))
        })?;
    }
    let m = manager.clone();
    registry.register(
        CommandSpec::new(
            "compute.role",
            "Set what work a device takes (`device`, `role`: interactive, batch or both)",
            CATEGORY,
        ),
        move |args, _| {
            let role = Role::parse(text(args, "role")?)?;
            let device = lock(&m)?.set_role(text(args, "device")?, role)?;
            Ok(json!({ "device": device, "role": role }))
        },
    )?;
    let m = manager.clone();
    registry.register(
        CommandSpec::new(
            "compute.budget",
            "Set a device's budgets (`device`; `threads`, `vram_mb`, `cache_mb`, `priority`)",
            CATEGORY,
        ),
        move |args, _| {
            let budget = budget_of(args)?;
            let mut m = lock(&m)?;
            let device = m.set_budget(text(args, "device")?, &budget)?;
            Ok(json!({ "device": device, "settings": m.settings().devices[&device] }))
        },
    )?;
    let m = manager.clone();
    registry.register(
        CommandSpec::new(
            "compute.benchmark",
            "Measure a device (`device`) or every enabled one",
            CATEGORY,
        ),
        move |args, _| {
            let device = args.get("device").and_then(Json::as_str);
            let reports = lock(&m)?.benchmark(device)?;
            Ok(Json::Array(
                reports
                    .into_iter()
                    .map(|r| match r.speeds {
                        Ok(speeds) => json!({
                            "device": r.device, "name": r.name, "mpx_per_s": speeds,
                        }),
                        Err(error) => json!({
                            "device": r.device, "name": r.name, "error": error,
                        }),
                    })
                    .collect(),
            ))
        },
    )?;
    let m = manager.clone();
    registry.register(
        CommandSpec::new(
            "compute.route",
            "Say which device would take a job (`role`, `family`, `gpu`, `width`, `height`)",
            CATEGORY,
        ),
        move |args, _| Ok(lock(&m)?.route(&job_of(args)?).describe()),
    )?;
    let m = manager.clone();
    registry.register(
        CommandSpec::new(
            "compute.remote",
            "Declare a remote unit (`name`, `address`, `token_file`)",
            CATEGORY,
        ),
        move |args, _| {
            let remote = RemoteSettings {
                name: text(args, "name")?.into(),
                address: text(args, "address")?.into(),
                token_file: args
                    .get("token_file")
                    .and_then(Json::as_str)
                    .map(Into::into),
            };
            lock(&m)?.add_remote(remote)?;
            Ok(json!({ "declared": text(args, "name")? }))
        },
    )?;
    let m = manager.clone();
    registry.register(
        CommandSpec::new(
            "compute.probe",
            "Ask a remote unit what it runs (`unit`)",
            CATEGORY,
        ),
        move |args, _| {
            let state = lock(&m)?.probe(text(args, "unit")?)?;
            Ok(serde_json::to_value(state).expect("states serialize"))
        },
    )?;
    Ok(())
}
