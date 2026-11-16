//! Helios's followers of a setting changed (ADR 0029, egui-pop's
//! `settings.changed`): the value store says each change once, made in
//! the Settings app or by another client of the node, and each part that
//! follows a setting is a named listener doing its own part, in the pass
//! it is said. Nothing reads these settings again every pass, and none is
//! applied only at start.
//!
//! | listener | settings | what it does |
//! |---|---|---|
//! | [`CACHES_BUDGET`] | `system.cache.budget`, `system.cache.share` | the RAM tier's budget, to the cache service and to the engine the viewport, previews and exports render with |
//! | [`THUMBS_BUDGET`] | `system.cache.thumbnails`, `system.cache.textures` | the library's thumbnail cache limit (on disk), the grids' textures budget |
//! | [`PLUGINS_VALUES`] | a plugin's own settings | the running plugin is handed its new values, or restarted |

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, PoisonError};

use egui_pop::api::app::AppHandle;
use egui_pop::api::bus::bus_contract::{Bus, ListenOpts, Point as _};
use egui_pop::api::bus::{SettingChange, SettingsChanged, asking};
use helios_core::engine::Engine;

/// The owner of Helios's listeners of `settings.changed`.
pub const BUS_OWNER: &str = "helios.settings";
/// The render cache's budget follows `system.cache.budget` / `share`.
pub const CACHES_BUDGET: &str = "caches.budget";
/// The thumbnails' budgets follow `system.cache.thumbnails` / `textures`.
pub const THUMBS_BUDGET: &str = "thumbs.budget";
/// A plugin's values follow its settings.
pub const PLUGINS_VALUES: &str = "plugins.values";

const BUDGET: &str = "system.cache.budget";
const SHARE: &str = "system.cache.share";

/// A number setting as the value store holds it now.
fn number(app: &AppHandle, key: &str) -> Option<f64> {
    let store = app.store();
    let store = store.lock().unwrap_or_else(PoisonError::into_inner);
    store
        .get(&format!("settings/{key}"))
        .and_then(|e| e.value.to_json().as_f64())
}

/// A size setting in MiB, in bytes (at least 1 MiB), as
/// `Settings::mib` reads it.
fn mib(app: &AppHandle, key: &str) -> Option<u64> {
    number(app, key).map(|v| (v.max(1.0) as u64) * 1024 * 1024)
}

/// The render cache's budget as the settings say now: `budget` (MiB)
/// when set, else `share` of the machine's memory; `None` for no limit.
fn render_budget(app: &AppHandle) -> Option<usize> {
    match number(app, BUDGET).unwrap_or(0.0) as usize {
        0 => helios_cache::memory_share(
            number(app, SHARE).unwrap_or(0.0),
            helios_cache::physical_memory(),
        ),
        mib => Some(mib * 1024 * 1024),
    }
}

fn opts() -> ListenOpts {
    ListenOpts::new().owner(BUS_OWNER)
}

fn is(change: &SettingChange, keys: &[&str]) -> bool {
    keys.contains(&change.id())
}

/// Registers [`CACHES_BUDGET`] on `bus`: the RAM tier's budget, to the
/// cache service and to `engine`, in the call the setting changed.
pub fn listen_caches(bus: &dyn Bus, engine: Arc<Engine>) {
    bus.unlisten(SettingsChanged::ID, CACHES_BUDGET);
    let listened = bus.listen::<SettingsChanged>(CACHES_BUDGET, opts(), move |change, ()| {
        if !is(change, &[BUDGET, SHARE]) {
            return Ok(());
        }
        let Some(app) = asking() else { return Ok(()) };
        let budget = render_budget(&app);
        helios_cache::Service::global()
            .set_budget(helios_cache::Tier::Ram, budget.map(|b| b as u64));
        engine.set_budget(budget);
        Ok(())
    });
    if let Err(e) = listened {
        log::error!(target: "helios", "{CACHES_BUDGET}: {e}");
    }
}

/// The library's thumbnail cache, once opened, for [`THUMBS_BUDGET`].
pub type ThumbCacheSlot = Rc<RefCell<Option<Arc<helios_thumbs::ThumbCache>>>>;

/// Registers [`THUMBS_BUDGET`] on `bus`, local to the interface's thread
/// (the grids' textures are its state): the thumbnail cache's limit and
/// the textures' budget, in the call the setting changed.
pub fn listen_thumbs(
    bus: &dyn Bus,
    library: Rc<RefCell<crate::library::Library>>,
    cache: ThumbCacheSlot,
) {
    bus.unlisten(SettingsChanged::ID, THUMBS_BUDGET);
    let listened = bus.listen_local::<SettingsChanged>(THUMBS_BUDGET, opts(), move |change, ()| {
        let Some(app) = asking() else { return Ok(()) };
        if change.id() == crate::thumbnails::CACHE_LIMIT {
            if let (Some(c), Some(limit)) = (cache.borrow().as_ref(), mib(&app, change.id())) {
                c.set_limit(limit);
            }
        } else if change.id() == crate::thumb_textures::BUDGET_SETTING
            && let Some(bytes) = mib(&app, change.id())
        {
            let Ok(mut library) = library.try_borrow_mut() else {
                return Err("the library is being drawn".into());
            };
            library.set_texture_budget(bytes);
        }
        Ok(())
    });
    if let Err(e) = listened {
        log::error!(target: "helios", "{THUMBS_BUDGET}: {e}");
    }
}

/// Who is told a plugin's settings changed (the broker).
pub type PluginsTold = Arc<dyn Fn(&str) + Send + Sync>;

/// Registers [`PLUGINS_VALUES`] on `bus`: a change of one of `store`'s
/// settings is kept as the plugin's value at once and `told` with the
/// plugin's id, which hands the running plugin its new values.
pub fn listen_plugins(
    bus: &dyn Bus,
    store: crate::config::plugins::PluginSettingsStore,
    told: PluginsTold,
) {
    bus.unlisten(SettingsChanged::ID, PLUGINS_VALUES);
    let listened = bus.listen::<SettingsChanged>(PLUGINS_VALUES, opts(), move |change, ()| {
        let Some(plugin) = store.plugin_of(change.id()) else {
            return Ok(());
        };
        if let Some(app) = asking() {
            let value = {
                let s = app.store();
                let s = s.lock().unwrap_or_else(PoisonError::into_inner);
                s.get(&change.path).map(|e| e.value.to_json())
            };
            store.heard(change.id(), value);
        }
        told(&plugin);
        Ok(())
    });
    if let Err(e) = listened {
        log::error!(target: "helios", "{PLUGINS_VALUES}: {e}");
    }
}

/// The order Helios's and egui-pop's listeners of `settings.changed` run
/// in on `bus`.
pub fn order(bus: &dyn Bus) -> String {
    egui_pop::api::bus::order_dump(bus, &egui_pop::api::bus::SETTINGS_POINTS)
}
