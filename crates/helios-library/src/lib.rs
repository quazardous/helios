//! Library operations over the vault: what brings images in and what
//! groups them (see `docs/library.md`).
//!
//! - [`import`]: files from a folder or a mounted card, referenced or
//!   copied into the library folder, renamed by a [`rename::Pattern`],
//!   their sidecars kept, exact duplicates found by content and handled
//!   by a policy, run as a job announced on the bus;
//! - [`import_history`]: a record of each import (sources, choices,
//!   counts, when, who) kept in the vault, and its params to run it again;
//! - [`fetch`]: import from a connector whose items are fetched (a remote
//!   source): bytes landed in part files with resume, hashed in BLAKE3 and
//!   the claimed algorithm, always a verified copy in the library folder,
//!   duplicates found before download, sightings on the source's volume;
//! - [`capture`]: capture metadata (time, camera, lens, settings, size)
//!   kept as the vault's read-only fields, at import and by a resumable
//!   backfill job over the images imported before;
//! - [`cull`]: the culling signals (a focus score and where the focus
//!   is, clipping per channel, the exposure's levels), at import and by a
//!   resumable backfill job over the images imported before;
//! - [`backup`]: the verified backup job, copying library copies to the
//!   backup volumes of the library's strategy, and its triggers;
//! - [`darktable`]: darktable's sidecars kept as versions (each sidecar of
//!   a photo, base and duplicates, a version of the one image), by an
//!   import or alone (`helios import-darktable`);
//! - [`map`]: recording the image map, what a listing found as
//!   sightings, the blacklist and forgotten folders left out;
//! - [`signals`]: what an import measures of each image to find near
//!   duplicates (a perceptual hash, the capture time and exposure, frame
//!   numbers, a sharpness);
//! - [`export`]: a batch export over library images planned as a
//!   request: each image's file and head version, its output name
//!   decided once;
//! - [`stacks`]: the `stacks.suggest` decision, which proposes stacks over
//!   a selection through a resolver picked by name, and applies them only
//!   when accepted.

pub mod backup;
pub mod capture;
pub mod cull;
pub mod darktable;
pub mod export;
pub mod fetch;
pub mod import;
pub mod import_history;
pub mod map;
pub mod map_job;
pub mod rename;
pub mod signals;
pub mod stacks;
pub mod thumb_check;

pub use import::{Duplicates, Options, Placement, Report};
pub use signals::Signals;
