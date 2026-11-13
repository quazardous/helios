//! Camera colour calibration from a chart (see
//! `docs/design/calibration.md`). The user shoots a colour chart, places
//! four handles on the centres of its four corner patches, and Helios
//! measures every patch and solves a matrix against the chart's reference
//! values.
//!
//! This crate is pure: no IO, no GUI (colour calibration's CAT16 comes from
//! its module's crate, as plain functions). The one pipeline it runs is the
//! raw measurement graph it builds itself ([`raw`]). It holds, so far:
//!
//! - [`chart`]: the charts Helios knows, their reference values (CIE Lab,
//!   D50) with their source, and their layout (grid, patch size);
//! - [`lab`]: CIE Lab with the D50 white, the frame of the reference
//!   values;
//! - [`homography`]: the plane-to-plane projection fitted on four point
//!   pairs;
//! - [`layout`]: a chart placed in an image by its four corner patches,
//!   each patch's centre and sampling quadrilateral, and the pixels inside
//!   it;
//! - [`sampler`]: each patch's robust statistics (trimmed means) and its
//!   checks (clipping, darkness, uniformity), and the chart-wide checks on
//!   the neutral patches, with their thresholds;
//! - [`solver`]: the 3×3 matrix keeping the neutral axis, solved by
//!   weighted least squares then refined on Delta E 2000, the chart's
//!   orientation, and the report;
//! - [`run`]: a chart shot measured, its illuminant estimated and solved
//!   in one call;
//! - [`request`]: `calibrate.chart`'s arguments and answer, serialisable,
//!   and its dry run on a decoded picture;
//! - [`raw`]: a raw shot measured on its sensor data through the raw front
//!   without `colorin`, rendered on the chart's bounding box;
//! - [`apply`]: a profile written into an edit's `chartprofile` node, the
//!   nearest-illuminant pick and a camera's default rule;
//! - [`residual`]: the residual hue/saturation table after the matrix,
//!   kept only when it lowers the held-out Delta E 2000;
//! - [`look`]: a camera look fitted to a raw's embedded JPEG (matrix,
//!   hue/saturation table, tone and chroma curves), with its gates; it
//!   shares the solver's Levenberg-Marquardt.

pub mod apply;
pub mod chart;
pub mod exchange;
pub mod homography;
pub mod lab;
pub mod layout;
pub mod look;
pub mod profile;
pub mod raw;
pub mod request;
pub mod residual;
pub mod run;
pub mod sampler;
pub mod solver;

pub use chart::{Chart, Patch, XRITE_CC24_2000, XRITE_CC24_2014, chart_by_id, charts};
pub use homography::Homography;
pub use layout::{ImageView, Layout, Point};
pub use profile::{PROFILE_FORMAT, Profile, ProfileSource, READ_FORMATS, camera_key};
pub use raw::{
    Measured, RAW_FRONT, RawFront, calibrate_raw, calibrate_raw_with, measure_raw, measured_region,
};
pub use request::{ChartAnswer, ChartArgs, ChartError, calibrate_picture, calibrate_shot};
pub use residual::Residual;
pub use run::{Illuminant, Report, Request, RunError, Shot, calibrate, working_to_xyz};
pub use sampler::{
    ChartCheck, ClipSource, PatchCheck, PatchStats, THRESHOLDS, Thresholds, Warning, check_chart,
};
pub use solver::{
    Measurement, Orientation, Oriented, PatchResult, Problem, Solution, SolveError, Stage, Summary,
    quality, solve, solve_oriented,
};
