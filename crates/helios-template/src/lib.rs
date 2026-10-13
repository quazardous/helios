//! Workspace templates: a ready-to-use combination of tools, network and
//! panel layout for one known scenario (see `docs/templates.md`).
//!
//! - [`format`]: the files' shapes, `template.json` ([`Template`]) and a
//!   composite file ([`CompositeFile`]), versioned from their first
//!   format ([`FORMAT`]);
//! - [`schema`]: their JSON schemas (`templates/schema/`) and a checker;
//! - [`Catalogue`]: what a template may name (modules, composites at each
//!   version, panels, commands), and composite files built as macros;
//! - [`Bundle`]: the `.helios-template` directory, read and written,
//!   validated against a catalogue ([`Bundle::validate`]) and built
//!   without a window ([`Bundle::build`]): its network as a graph, its
//!   layout with the composites' panel hints.

pub mod build;
pub mod bundle;
pub mod catalogue;
pub mod format;
pub mod schema;

pub use build::{Built, SOURCE_PORT, placeholder};
pub use bundle::{Bundle, Report, Text, Update, parse_composite, parse_template};
pub use catalogue::Catalogue;
pub use format::{CompositeFile, CompositeRef, FORMAT, ImportDesc, LibraryDesc, Template};
