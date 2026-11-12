//! Camera colour calibration from a chart (see
//! `docs/design/calibration.md`). The user shoots a colour chart, places
//! four handles on the centres of its four corner patches, and Helios
//! measures every patch and solves a matrix against the chart's reference
//! values.
//!
//! This crate is pure: no IO, no GUI, no pipeline. It holds, so far:
//!
//! - [`chart`]: the charts Helios knows, their reference values (CIE Lab,
//!   D50) with their source, and their layout (grid, patch size);
//! - [`lab`]: CIE Lab with the D50 white, the frame of the reference
//!   values;
//! - [`homography`]: the plane-to-plane projection fitted on four point
//!   pairs;
//! - [`layout`]: a chart placed in an image by its four corner patches,
//!   each patch's centre and sampling quadrilateral, and the pixels inside
//!   it, with their plain mean and variance.
//!
//! The sampler's checks (trimmed means, clipping, uniformity) and the
//! solver come in later slices.

pub mod chart;
pub mod homography;
pub mod lab;
pub mod layout;

pub use chart::{Chart, Patch, XRITE_CC24_2000, XRITE_CC24_2014, chart_by_id, charts};
pub use homography::Homography;
pub use layout::{ImageView, Layout, PatchStats, Point};
