//! Masks as a sub-network: darktable's flexi masks (a tree of groups) built
//! from small modules connected port to port. See `docs/masks.md`.
//!
//! **Convention.** A mask is an ordinary image with one channel (layout
//! `y`), values from 0 (the module does nothing there) to 1 (it applies
//! fully). Mask ports are declared with [`mask_port`]. Modules here clamp
//! what they read to that range only where an operator needs it, as
//! darktable does.
//!
//! - [`MaskCombine`]: a group. Folds up to [`MAX_MEMBERS`] optional
//!   members in port order with one operator, each with its opacity and
//!   inversion, then applies the group's inversion and opacity.
//! - [`MaskFeather`], [`MaskBlur`], [`MaskLevels`]: refinement, at element
//!   scope (on a member, before it reaches its group) or at group scope (on
//!   a group's output). [`MaskLevels`] also closes a refined group with its
//!   inversion and opacity, so the order stays fold, refinement, inversion,
//!   opacity.
//! - [`MaskBlend`]: mixes a module's input and output through a mask.
//! - [`MaskFromLuma`]: a member made from the image's luminance; it also
//!   passes its input through, a module with a second (mask) output.
//! - [`MaskParametric`]: a member from ranges of the image's channels
//!   (luminance, RGB, perceptual lightness, chroma and hue), darktable's
//!   parametric mask; it passes its input through too.
//! - [`MaskRaster`]: a member that reuses another module's mask (darktable's
//!   raster mask), inverted and scaled.
//! - [`stress`]: a large chained mask network, for stress tests and the
//!   masks bench.

mod blend;
mod combine;
mod luma;
mod parametric;
mod raster;
mod refine;
pub mod stress;

pub use blend::MaskBlend;
pub use combine::{MAX_MEMBERS, MaskCombine, Operator, fold};
pub use luma::{MaskFromLuma, luminance};
pub use parametric::{Channel, MaskParametric, ParametricPlan, Range, jzczhz};
pub use raster::{MaskRaster, RasterPlan};
pub use refine::{MaskBlur, MaskFeather, MaskLevels, tone_curve};

use helios_core::{Params, PortSpec, Value};

/// A one-channel mask port, values 0 to 1.
pub fn mask_port(name: &str) -> PortSpec {
    let mut port = PortSpec::image(name, 1);
    port.layout = Some("y".into());
    port
}

/// A bool parameter (checked against the manifest, so present).
fn flag(params: &Params, name: &str) -> bool {
    params.get(name).and_then(Value::as_bool).unwrap_or(false)
}
