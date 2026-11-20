//! Storage, as a feature of the desktop: the Storage and Backup pages of
//! the Settings app ([`settings`]), read and written through commands
//! ([`admin`]) over a link to the node ([`link`]); what storage keeps
//! between frames ([`page`]); a backup's end and the cards safe to
//! format ([`backup_end`]); what storage read, handed to whoever shows it
//! ([`read`]).
//!
//! It knows the bus and its own contract tables ([`backup_end::POINTS`],
//! [`read::POINTS`]), egui-pop's Settings app, and the domain crates. It
//! knows nothing of the desktop's shell: the desktop mounts it (registers
//! its commands, declares its points, lends it a [`page::Tell`] and steps
//! it each frame), and the Library, the import window and the health
//! check hear it through the bus.

// The rows and groups are built from the link's maps, always the
// standard hasher: generic hashers would only widen the API.
#![allow(clippy::implicit_hasher)]

pub mod admin;
pub mod backup_end;
pub mod link;
pub mod page;
pub mod read;
pub mod settings;

pub use link::{NodeStorage, StorageLink, card_of};
pub use page::{StoragePage, StorageRead, backup_ended, backup_line, refresh_volumes, role_text};

/// Opens the Settings app at Storage › Volumes; `volume` (an id) marks
/// that volume and scrolls to it.
pub const COMMAND: &str = "library.storage";

/// Backs up now what is below the library's strategy.
pub const BACKUP_NOW: &str = "library.backupNow";

/// The fact the Settings app's read kernel hears when the storage link
/// read something new (a disk came or went, a scan, an edit saved).
pub const VOLUMES_CHANGED: &str = "storage.volumes.changed";

/// The read policies of storage's read-only commands, for the Settings
/// app's read kernel: each command, how long its answer stays fresh, and
/// the facts that make it out of date.
pub const READ_POLICIES: &[(&str, Option<std::time::Duration>, &[&str])] = &[
    (
        admin::VOLUMES,
        Some(std::time::Duration::from_secs(30)),
        &["storage.*", "helios.library.*"],
    ),
    (
        admin::STRATEGY,
        Some(std::time::Duration::from_secs(30)),
        &["storage.*", "helios.library.*"],
    ),
];
