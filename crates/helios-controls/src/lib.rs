//! Helios controls: business controllers between a module's parameters
//! and whatever the person touches (`docs/design/controls.md`).
//!
//! - [`model`]: what a control is (purpose, value, units, ranges, feel,
//!   detents, read-out), the rack standard (footprints, connectors,
//!   cases, variants), modes and representations. No egui.
//! - [`registry`]: every control registered and described as data, the
//!   modules' patches onto parameters, implicit controls built from
//!   descriptors for the parameters nothing is patched to.
//! - [`catalogue`]: Helios's controls (exposure, for now).
//! - [`flow`]: begin, preview, commit (one undo step, through the
//!   vetoable `controls.change` step) or cancel (no trace), on
//!   `helios_values::Store`.
//! - [`commands`]: `controls.catalogue`, `controls.describe`,
//!   `controls.set`, `controls.nudge`, `controls.reset`, and what the
//!   `controls` collector reads.
//! - `render` (feature `render`): the renderers on egui-pop.

pub mod catalogue;
pub mod commands;
pub mod flow;
pub mod model;
pub mod registry;
#[cfg(feature = "render")]
pub mod render;

pub use model::{Control, Footprint, Mode, Representation, Surface, Variant};
pub use registry::{Patch, Registry, implicit};
