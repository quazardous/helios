//! Helios controls: Helios's side of the controls framework
//! (`egui_hudget`, `docs/design/controls.md`).
//!
//! - [`catalogue`]: Helios's business controls (exposure, for now).
//! - [`values`]: `helios_values::Store` behind the framework's store
//!   traits, and Helios's values, descriptors and parameter widgets
//!   mapped to the framework's.
//! - [`flow`]: the value flow on `helios_values::Store`.
//! - [`commands`]: the `controls.*` commands declared in the shell's
//!   command registry, and what the `controls` collector reads.
//! - [`HeliosRegistry`]: the standard registry (the catalogue) and the
//!   control patched to a store path, from a Helios descriptor.
//!
//! The framework's modules are re-exported as they are: [`model`],
//! [`slot`], [`card`], [`context`], [`resolve`], [`registry`], and
//! `render` (feature `render`).

pub mod catalogue;
pub mod commands;
pub mod flow;
pub mod values;

#[cfg(feature = "render")]
pub use egui_hudget::render;
pub use egui_hudget::{card, context, model, registry, resolve, slot};

pub use context::{ContextKeys, Rule, Tier};
pub use model::{Control, Kind, Mode, Representation, Surface, Variant};
pub use registry::{Patch, Registry};
pub use resolve::{CardPlacement, Layout, Placement, Visibility};
pub use slot::{Integration, Orientation, SizePolicy, Slot};

use helios_values::Descriptor;

/// The implicit control of a parameter no control is patched to, from
/// its Helios descriptor ([`egui_hudget::implicit`]).
pub fn implicit(path: &str, d: &Descriptor) -> Control {
    egui_hudget::implicit(path, &values::descriptor(d))
}

/// Helios's ways into the framework's registry.
pub trait HeliosRegistry {
    /// Helios's own catalogue ([`catalogue`]), with no patches.
    fn standard() -> Self;

    /// The control patched to a store path (`node/<node>/<param>`), from
    /// its descriptor's module.
    fn control_for(&self, path: &str, d: &Descriptor) -> Option<&Control>;
}

impl HeliosRegistry for Registry {
    fn standard() -> Self {
        catalogue::registry()
    }

    fn control_for(&self, path: &str, d: &Descriptor) -> Option<&Control> {
        self.control_at(path, d.module.as_deref())
    }
}
