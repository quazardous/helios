//! The desktop's view of the cache service (`helios_cache`, see
//! `docs/design/cache.md`): the `cache.stats` and `cache.clear` commands
//! the Caches settings page reads and runs, and the `helios.cache` debug
//! collector agents and scenarios assert budgets with.

use egui_pop::core::debug::FnCollector;
use helios_cache::{Service, size_text};
use helios_core::Message;
use helios_shell::commands::CommandSpec;
use serde_json::{Value, json};

use crate::shell::Shell;

/// What the caches hold: each tier's budget and use, and per scope, kind
/// and tier the entries, bytes, pins and statistics; `kinds` rows carry
/// their texts for the Caches page.
pub const STATS_COMMAND: &str = "cache.stats";

/// Clears the caches: every kind, or `kind` alone.
pub const CLEAR_COMMAND: &str = "cache.clear";

/// The answer of [`STATS_COMMAND`]: the service's snapshot, each kind's
/// row with an `id` and its texts (`detail`, `sizeText`) through `t`.
pub fn stats(service: &Service, t: &dyn Fn(Message) -> String) -> Value {
    let mut snapshot = service.snapshot();
    if let Some(kinds) = snapshot["kinds"].as_array_mut() {
        for k in kinds {
            let tier = match k["tier"].as_str().unwrap_or_default() {
                "disk" => t(Message::new("gui-caches-tier-disk", "on disk")),
                "vram" => t(Message::new("gui-caches-tier-vram", "in video memory")),
                _ => t(Message::new("gui-caches-tier-ram", "in memory")),
            };
            let bytes = k["bytes"].as_u64().unwrap_or(0);
            let size = match k["budget"].as_u64() {
                Some(budget) => t(Message::new(
                    "gui-caches-size-of",
                    "{ $size } of { $budget }, { $entries } entries",
                )
                .with("size", size_text(bytes))
                .with("budget", size_text(budget))
                .with("entries", k["entries"].to_string())),
                None => t(
                    Message::new("gui-caches-size", "{ $size }, { $entries } entries")
                        .with("size", size_text(bytes))
                        .with("entries", k["entries"].to_string()),
                ),
            };
            k["id"] = json!(format!(
                "{}|{}",
                k["scope"].as_str().unwrap_or_default(),
                k["kind"].as_str().unwrap_or_default()
            ));
            k["detail"] = json!(format!(
                "{} · {tier}",
                k["kind"].as_str().unwrap_or_default()
            ));
            k["sizeText"] = json!(size);
        }
    }
    let used: u64 = snapshot["tiers"]
        .as_array()
        .map_or(0, |t| t.iter().filter_map(|t| t["used"].as_u64()).sum());
    snapshot["summary"] = json!({
        "text": t(Message::new("gui-caches-total", "Caches hold { $size }")
            .with("size", size_text(used))),
    });
    snapshot
}

/// Registers [`STATS_COMMAND`] and [`CLEAR_COMMAND`] on the shell, over
/// the process's service.
///
/// # Errors
/// A command cannot be registered.
pub fn register(shell: &mut Shell) -> Result<(), String> {
    let t = shell.texts();
    shell.register(
        CommandSpec::new(
            STATS_COMMAND,
            "What the caches hold: each tier's budget and use; per scope, kind and tier the entries, bytes, pins and statistics",
            "Help",
        )
        .read_only(),
        move |_, _| Ok(stats(Service::global(), &t)),
    )?;
    shell.register(
        CommandSpec::new(
            CLEAR_COMMAND,
            "Clears the caches (`kind`: one kind alone): what is shown now and the durable data are kept",
            "Help",
        ),
        |args, _| {
            let kind = args.get("kind").and_then(Value::as_str);
            let freed = Service::global().clear(kind);
            Ok(json!({ "freed": freed }))
        },
    )
}

/// The `helios.cache` collector: the service's snapshot.
pub fn collector() -> FnCollector {
    FnCollector::new(
        "helios.cache",
        "The cache service: each tier's budget and use (ram, vram, disk); per scope, kind and tier the entries, bytes, pins, and hits, misses, merged requests, evictions and bytes read and written.",
    )
    .snapshot(|_| Service::global().snapshot())
}

/// The render cache's budget in bytes from the resolved settings:
/// `system.cache.budget` (MiB) when set, else `system.cache.share` of
/// physical memory; `None` for no limit. Machine settings: the machine's
/// layer, or `--set`.
pub fn render_budget(settings: &crate::config::Settings) -> Option<usize> {
    settings
        .config()
        .cache_budget(helios_cache::physical_memory())
}

/// Applies the cache settings to the desktop: the RAM tier's budget
/// (`system.cache.budget` / `share`) to the cache service and to the
/// engine the viewport, previews and exports render with.
pub fn apply_settings(shell: &Shell, settings: &crate::config::Settings) {
    let budget = render_budget(settings);
    Service::global().set_budget(helios_cache::Tier::Ram, budget.map(|b| b as u64));
    shell.viewport().engine().set_budget(budget);
}
