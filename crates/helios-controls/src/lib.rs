//! Helios controls: business controllers between a module's parameters
//! and whatever the person touches (`docs/design/controls.md`).
//!
//! - [`model`]: what a control is (purpose, value, units, ranges, feel,
//!   detents, read-out), the rack standard (connectors, cases and their
//!   chrome, variants, indicators), modes and representations. No egui.
//! - [`slot`]: what a control asks of a boxed surface: slots, size
//!   policies per representation, orientations, integration levels.
//! - [`card`]: a card or a case laid out on the slot engine
//!   (`egui_gridflow`), item by item.
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

pub mod card;
pub mod catalogue;
pub mod commands;
pub mod flow;
pub mod model;
pub mod registry;
#[cfg(feature = "render")]
pub mod render;
pub mod slot;

pub use model::{Control, Kind, Mode, Representation, Surface, Variant};
pub use registry::{Patch, Registry, implicit};
pub use slot::{Integration, Orientation, SizePolicy, Slot};
