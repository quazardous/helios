//! The library's settings, declared here: the image map's records
//! ([`crate::map`], [`crate::map_job`]), the automatic imports'
//! safeguards ([`crate::auto_import`]) and the library folder.

use helios_config::{Concern, Kind, Setting};

/// The library's settings, held in the settings layers.
pub const SETTINGS: &[Setting] = &[
    Setting {
        key: "system.map.record",
        concern: Concern::Machine,
        label: "Image map records",
        about: "What the image map records: everywhere (every listing, and background scans of known volumes), listings (what Helios is shown, plus scans of volumes with a role) or import (only what is imported).",
        kind: Kind::Choice {
            options: &["everywhere", "listings", "import"],
            default: "listings",
        },
    },
    Setting {
        key: "system.map.limit",
        concern: Concern::Machine,
        label: "Files seen kept",
        about: "How many files seen but not in the Library the image map keeps; past it, those seen longest ago are forgotten first. Library files are never counted.",
        kind: Kind::Number {
            min: 1000.0,
            max: 100_000_000.0,
            default: 1_000_000.0,
            unit: "files",
            integer: true,
        },
    },
    Setting {
        key: "system.map.keepMissing",
        concern: Concern::Machine,
        label: "Missing files kept",
        about: "How long a file seen but not in the Library stays in the image map once it is missing from its place.",
        kind: Kind::Number {
            min: 0.0,
            max: 3650.0,
            default: 30.0,
            unit: "days",
            integer: true,
        },
    },
    Setting {
        key: "import.automatic.paused",
        concern: Concern::User,
        label: "Pause automatic imports",
        about: "While on, no source imports on its own: cards plugged in and new files in watched folders wait until it is off again.",
        kind: Kind::Toggle {
            default: false,
            on: "paused",
            off: "running",
        },
    },
    Setting {
        key: "import.automatic.ask_files",
        concern: Concern::User,
        label: "Ask before importing more photos than",
        about: "An automatic import of more photos than this asks first, in a notice that opens the import window. 0: never asks.",
        kind: Kind::Number {
            min: 0.0,
            max: 100_000.0,
            default: 500.0,
            unit: "photos",
            integer: true,
        },
    },
    Setting {
        key: "import.automatic.ask_gb",
        concern: Concern::User,
        label: "Ask before importing more than",
        about: "An automatic import larger than this asks first, in a notice that opens the import window. 0: never asks.",
        kind: Kind::Number {
            min: 0.0,
            max: 10_000.0,
            default: 20.0,
            unit: "GB",
            integer: false,
        },
    },
];

/// Settings the library keeps rather than the layers: declared so a form
/// draws them (their label, why, kind), written through the library. The
/// layers never hold them, so they are not in a schema.
///
/// - `library.folder`: where imports copy photos, kept by the library in
///   its catalogue ([`crate::import::library_root`]); the first run's
///   library folder step fills it.
pub const KEPT_ELSEWHERE: &[Setting] = &[Setting {
    key: "library.folder",
    concern: Concern::Project,
    label: "Library folder",
    about: "Where imports copy photos, and where picked photos go.",
    kind: Kind::Text { default: "" },
}];
