//! The thumbnails' settings, declared here, by the crate that owns the
//! on-disk thumbnail cache.

use helios_config::{Concern, Kind, Setting};

/// The thumbnail cache's settings.
pub const SETTINGS: &[Setting] = &[Setting {
    key: "system.cache.thumbnails",
    concern: Concern::Machine,
    label: "Thumbnail cache",
    about: "Disk space the library's thumbnail cache (.helios/thumbs beside the catalogue) may use before evicting the thumbnails used least recently.",
    kind: Kind::Number {
        min: 16.0,
        max: 1_048_576.0,
        default: 2048.0,
        unit: "MiB",
        integer: true,
    },
}];
