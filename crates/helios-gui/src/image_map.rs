//! The image map in the interface: what the import window's sources list
//! is recorded (`docs/design/image-map.md`, section 4), as the settings
//! say (`system.map.record`, `system.map.defaults`, `system.map.exclude`).

use std::path::PathBuf;
use std::sync::Arc;

use helios_library::map::{RECORD, Record, Recorder};
use helios_storage::exclude::{Blacklist, Checker, DEFAULTS, EXCLUDE};
use helios_vault::{Store, Vault};

use crate::import_scan::MapRecorder;

/// The map's settings, read once.
#[derive(Clone, Debug)]
pub struct MapSettings {
    pub record: Record,
    pub defaults: bool,
    /// The JSON of `system.map.exclude`.
    pub exclude: String,
}

/// The map's settings from the user's settings layer (its TOML text; none
/// or unreadable: the defaults).
pub fn settings(layer: Option<&str>) -> MapSettings {
    let mut config = helios_config::Config::new(helios_config::Schema::core());
    if let Some(text) = layer
        && let Err(problems) = config.load_str(helios_config::Layer::Global, text)
    {
        log::warn!("image map settings: {} problems", problems.len());
    }
    MapSettings {
        record: Record::parse(&config.choice(RECORD)),
        defaults: config.toggle(DEFAULTS),
        exclude: config.text(EXCLUDE),
    }
}

/// The recorder the import window's scans use: each makes one on its
/// thread, over the volumes known now. `helios` names Helios's own
/// folders (the vault's, its data and cache), which are never recorded.
pub fn recorder<S: Store + 'static>(
    vault: Arc<Vault<S>>,
    settings: MapSettings,
    helios: Vec<PathBuf>,
) -> MapRecorder {
    Arc::new(move |seen_by: &str| {
        if !settings.record.listings() {
            return None;
        }
        let blacklist = Blacklist::here(helios.clone(), settings.defaults, &settings.exclude)
            .unwrap_or_else(|e| {
                log::warn!("image map: {e}; the user's rules are left out");
                Blacklist::here(helios.clone(), settings.defaults, "").expect("no rule reads")
            });
        let mounted = helios_storage::system().volumes();
        let volumes = helios_storage::registry::merged(&vault, &mounted).unwrap_or(mounted);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Some(Recorder::new(
            Box::new(vault.clone()),
            Checker::new(blacklist),
            volumes,
            seen_by,
            now,
        ))
    })
}
