//! The image map's settings in the interface (`docs/design/image-map.md`,
//! section 4): what the import window's sources list is recorded, as the
//! settings say (`system.map.record`, `system.map.defaults`,
//! `system.map.exclude`). The map's job, the thumbnail re-check and the
//! scans' recorder are the node's ([`helios_node_core::map`]).

use helios_library::map::{RECORD, Record};
use helios_storage::exclude::{DEFAULTS, EXCLUDE};

pub use helios_node_core::map::MapSettings;

/// The map's settings, from the resolved settings ([`crate::config`]).
pub fn settings(config: &helios_config::Config) -> MapSettings {
    MapSettings {
        record: Record::parse(&config.choice(RECORD)),
        defaults: config.toggle(DEFAULTS),
        exclude: config.text(EXCLUDE),
        prune: helios_library::map_job::Settings {
            limit: config.number(helios_library::map_job::LIMIT) as usize,
            keep_missing_days: config.number(helios_library::map_job::KEEP_MISSING) as u64,
            record: Record::parse(&config.choice(RECORD)),
        },
    }
}
