//! Library operations over the vault: what brings images in and what
//! groups them (see `docs/library.md`).
//!
//! - [`import`]: files from a folder or a mounted card, referenced or
//!   copied into the library folder, renamed by a [`rename::Pattern`],
//!   their sidecars kept, exact duplicates found by content and handled
//!   by a policy, run as a job announced on the bus;
//! - [`signals`]: what an import measures of each image to find near
//!   duplicates (a perceptual hash, the capture time and exposure, frame
//!   numbers, a sharpness).

pub mod import;
pub mod rename;
pub mod signals;

pub use import::{Duplicates, Options, Placement, Report};
pub use signals::Signals;
