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
//!   layout with the composites' panel hints;
//! - [`shipped`]: the built-in templates and composite files;
//! - [`workspace`]: a workspace made from a template, as it is edited:
//!   its own files, its parts, saved back as a template, reset to its
//!   template, and parts of another imported as a copy.
//! - [`network`]: a workspace's network edited node by node (connect,
//!   disconnect, move, add, remove, paste copied nodes), the same edits
//!   for the desktop's graph view and the MCP tools.
//! - [`macros`]: macros made, opened, folded, dissolved and upgraded on
//!   a workspace, as pure steps giving the next state and the image's
//!   values to move.
//! - [`library`]: the user's library of composites
//!   (`<config>/composites/`), the macros published from a workspace.
//! - [`subworkspace`]: sub-workspaces as data, named scopes over nodes
//!   of a workspace's network, delegated, deleted, checked and copied.
//! - [`workshop`] (experimental): workshops' measures, zones and
//!   aiming; [`draft`]: a workshop an AI proposes, checked on a proxy of
//!   the image and applied as one step; [`session`]: the record of a
//!   workshop piloted on a test photo; [`generalise`]: the rows "Make it
//!   automatic" proposes from it; [`limits`]: the bounds a run stays
//!   within, and their checker.
//! - [`contexts`]: Helios's places and modes on egui-pop's context
//!   stack, shared by the desktop and the MCP server, and the tools
//!   agents move a workspace's stack with.

pub mod build;
pub mod bundle;
pub mod catalogue;
pub mod contexts;
pub mod draft;
pub mod format;
pub mod generalise;
pub mod library;
pub mod limits;
pub mod macros;
pub mod network;
pub mod schema;
pub mod session;
pub mod shipped;
pub mod subworkspace;
pub mod workshop;
pub mod workspace;

pub use build::{Built, SOURCE_PORT, Source, placeholder, raw_placeholder};
pub use bundle::{Bundle, Report, Text, Update, parse_composite, parse_template};
pub use catalogue::Catalogue;
pub use format::{
    CompositeFile, CompositeRef, ExportDesc, FORMAT, ImportDesc, LibraryDesc, Measure, Parameter,
    ParameterKind, QueryDesc, SortDesc, SourceKind, SubWorkspace, Target, Template, Workshop,
};
