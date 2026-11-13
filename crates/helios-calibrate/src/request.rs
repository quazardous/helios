//! `calibrate.chart`'s arguments and its answer, both serialisable: what a
//! node's control request, its MCP tool, the `helios` command line and the
//! desktop's command share (see `docs/design/calibration.md`, section 8).
//!
//! A raw is measured on its sensor data by [`crate::raw::calibrate_raw`].
//!
//! The library measures only (a dry run, nothing written): a node saves
//! the answer as a profile ([`crate::profile`]) when asked to.
//!
//! The measured image is the caller's. For a decoded picture
//! ([`calibrate_picture`]) it is the picture's linear working RGB, as
//! `helios-load` decodes it, the standard matrix the identity, and clipping
//! read on the same values at [`PICTURE_CLIP_LEVEL`]. The corners are in
//! the picture's pixels as stored, before its EXIF orientation turns it.

use std::fmt;

use helios_photo::color::{IDENTITY, Mat3};
use serde::{Deserialize, Serialize};

use crate::chart::chart_by_id;
use crate::layout::{ImageView, Point};
use crate::run::{Report, Request, RunError, Shot, calibrate};
use crate::sampler::{ClipSource, PICTURE_CLIP_LEVEL, Warning};
use crate::solver::{SolveError, Summary};

/// The chart used when the caller names none.
pub const DEFAULT_CHART: &str = "xrite.cc24.2014";

/// `calibrate.chart`'s arguments, the image aside.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChartArgs {
    /// The centres of the four corner patches, `[x, y]` in the measured
    /// image's pixels, in any order.
    pub corners: [Point; 4],
    /// The chart's id ([`crate::chart::charts`]).
    #[serde(default = "default_chart")]
    pub chart: String,
    /// Patch names the solver leaves out (`B3`).
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Report only, write nothing (the library's only mode: a node saves
    /// the profile when `save` is asked).
    #[serde(default = "yes")]
    pub dry_run: bool,
}

fn default_chart() -> String {
    DEFAULT_CHART.into()
}

fn yes() -> bool {
    true
}

impl ChartArgs {
    /// The defaults around `corners`: the default chart, nothing excluded,
    /// a dry run.
    pub fn new(corners: [Point; 4]) -> Self {
        ChartArgs {
            corners,
            chart: default_chart(),
            exclude: Vec::new(),
            dry_run: true,
        }
    }
}

/// Why `calibrate.chart` gives no answer.
#[derive(Clone, Debug, PartialEq)]
pub enum ChartError {
    /// `dry_run` false here: the library writes nothing, the node saves
    /// (`save`).
    NotDryRun,
    UnknownChart(String),
    UnknownPatch(String),
    /// The image has fewer than three channels, or no pixel.
    BadImage,
    /// No orientation of the handles could be measured and solved.
    Unsolved(Option<SolveError>),
    /// A raw: the chart's patches fall outside the image.
    OutsideImage,
    /// A raw whose camera has no usable colour matrix: no standard matrix.
    NoCameraMatrix,
    /// A raw the measurement graph could not develop, with the reason.
    RawGraph(String),
}

impl fmt::Display for ChartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChartError::NotDryRun => {
                f.write_str("a run that writes needs `save`: the profile is what it writes")
            }
            ChartError::UnknownChart(id) => write!(f, "no chart `{id}`"),
            ChartError::UnknownPatch(name) => write!(f, "the chart has no patch `{name}`"),
            ChartError::BadImage => {
                f.write_str("the image has no pixel or fewer than three channels")
            }
            ChartError::OutsideImage => f.write_str("the chart's patches fall outside the image"),
            ChartError::NoCameraMatrix => {
                f.write_str("the camera has no usable colour matrix to measure the raw against")
            }
            ChartError::RawGraph(reason) => {
                write!(f, "the raw could not be developed for measuring: {reason}")
            }
            ChartError::Unsolved(reason) => {
                f.write_str("the chart could not be measured and solved at these corners")?;
                match reason {
                    Some(reason) => write!(f, " ({})", unsolved_reason(reason)),
                    None => Ok(()),
                }
            }
        }
    }
}

impl std::error::Error for ChartError {}

/// The solver's reason, in words.
pub fn unsolved_reason(reason: &SolveError) -> &'static str {
    match reason {
        SolveError::WrongLength => "measurements and patches differ in number",
        SolveError::Singular => "the adaptation cannot be inverted",
        SolveError::NoNeutral => "no neutral patch is kept",
        SolveError::Underdetermined => "too few patches are kept",
    }
}

impl From<RunError> for ChartError {
    fn from(e: RunError) -> Self {
        match e {
            RunError::UnknownPatch(name) => ChartError::UnknownPatch(name),
            RunError::BadImage => ChartError::BadImage,
            RunError::Unsolved(reason) => ChartError::Unsolved(reason),
        }
    }
}

/// How the handles were given, relative to the chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrientationAnswer {
    pub quarter_turns: u8,
    pub mirrored: bool,
}

/// The scene's light, estimated from the neutral patches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IlluminantAnswer {
    pub xy: [f64; 2],
    /// Kelvin.
    pub cct: f64,
    /// Colour calibration's word for it: `d`, `bb` or `custom`.
    pub kind: String,
}

/// Mean, max and the worst patches by name, worst first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SummaryAnswer {
    pub mean: f64,
    pub max: f64,
    pub worst: Vec<String>,
}

/// One patch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatchAnswer {
    pub name: String,
    /// The measured value in the working space at the solved exposure.
    pub measured: [f64; 3],
    /// The chart's reference, CIE Lab (D50).
    pub reference: [f64; 3],
    /// Delta E 2000 through the standard matrix.
    pub before: f64,
    /// Delta E 2000 through the solved correction.
    pub after: f64,
    /// `kept`; `excluded` (by name), `clipped` or `missing` (its area
    /// outside the image): left out of the solve and the means.
    pub status: String,
    /// `dark`, `not_uniform`: warnings on a patch still used.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
}

/// A chart-wide warning.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChartWarning {
    /// The neutral `brighter` does not measure lighter than `darker`: the
    /// corners or the orientation may be wrong.
    NeutralOrder { brighter: String, darker: String },
    /// The neutrals' chromaticities spread: mixed light.
    NeutralSpread { value: f64 },
    /// The neutrals' mean chromaticity is that far from neutral.
    NeutralCast { value: f64 },
}

/// `calibrate.chart`'s answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChartAnswer {
    /// True when nothing was written (a profile saved: false).
    pub dry_run: bool,
    pub chart: String,
    pub orientation: OrientationAnswer,
    /// The handles in the chart's corner order (first patch, then around).
    pub corners: [Point; 4],
    pub illuminant: IlluminantAnswer,
    /// The exposure offset found, in EV.
    pub exposure_ev: f64,
    /// darktable's word for the mean Delta E after: `very good`, `good`,
    /// `passable`, `bad`.
    pub quality: String,
    /// Over the kept patches, through the standard matrix.
    pub before: SummaryAnswer,
    /// Over the kept patches, through the correction.
    pub after: SummaryAnswer,
    /// `P`, the correction after the standard matrix, working RGB to
    /// working RGB.
    pub matrix: Mat3,
    /// `M_c = P M_s`: camera RGB (the picture's working RGB) to the
    /// working space, what a profile stores.
    pub camera_matrix: Mat3,
    pub patches: Vec<PatchAnswer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ChartWarning>,
}

fn summary(report: &Report, s: &Summary) -> SummaryAnswer {
    let patches = report.layout.chart.patches;
    SummaryAnswer {
        mean: s.mean,
        max: s.max,
        worst: s.worst.iter().map(|&k| patches[k].name.into()).collect(),
    }
}

/// The answer to a dry run from `report`, `exclude` the names the caller
/// left out.
pub fn answer(report: &Report, exclude: &[String]) -> ChartAnswer {
    let chart = report.layout.chart;
    let name = |k: usize| chart.patches[k].name.to_string();
    let patches = report
        .solution
        .patches
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let check = report.check.patches[k].as_ref();
            let status = match check {
                None => "missing",
                Some(c) if c.clipped => "clipped",
                Some(_) if exclude.iter().any(|n| n == p.name) => "excluded",
                Some(_) => "kept",
            };
            let mut flags = Vec::new();
            if check.is_some_and(|c| c.dark) {
                flags.push("dark".to_string());
            }
            if check.is_some_and(|c| c.not_uniform) {
                flags.push("not_uniform".to_string());
            }
            PatchAnswer {
                name: p.name.into(),
                measured: p.measured,
                reference: p.reference,
                before: p.before,
                after: p.after,
                status: status.into(),
                flags,
            }
        })
        .collect();
    let warnings = report
        .check
        .warnings()
        .into_iter()
        .filter_map(|w| match w {
            Warning::NeutralOrder { brighter, darker } => Some(ChartWarning::NeutralOrder {
                brighter: name(brighter),
                darker: name(darker),
            }),
            Warning::NeutralSpread(value) => Some(ChartWarning::NeutralSpread { value }),
            Warning::NeutralCast(value) => Some(ChartWarning::NeutralCast { value }),
            // Per patch: in each patch's status and flags.
            Warning::Missing(_)
            | Warning::Clipped(_)
            | Warning::Dark(_)
            | Warning::NotUniform(_) => None,
        })
        .collect();
    ChartAnswer {
        dry_run: true,
        chart: chart.id.into(),
        orientation: OrientationAnswer {
            quarter_turns: report.orientation.quarter_turns,
            mirrored: report.orientation.mirrored,
        },
        corners: report.layout.corners,
        illuminant: IlluminantAnswer {
            xy: report.illuminant.xy,
            cct: report.illuminant.cct,
            kind: report.illuminant.kind.into(),
        },
        exposure_ev: report.solution.exposure_ev,
        quality: report.solution.quality().into(),
        before: summary(report, &report.solution.before),
        after: summary(report, &report.solution.after),
        matrix: report.solution.matrix,
        camera_matrix: report.camera_matrix,
        patches,
        warnings,
    }
}

/// Measures `shot` as `args` asks and answers the dry run.
///
/// # Errors
/// A save asked for, an unknown chart or patch, a bad image, or a chart
/// that cannot be solved at these corners.
pub fn calibrate_shot(args: &ChartArgs, shot: &Shot<'_>) -> Result<ChartAnswer, ChartError> {
    if !args.dry_run {
        return Err(ChartError::NotDryRun);
    }
    let chart =
        chart_by_id(&args.chart).ok_or_else(|| ChartError::UnknownChart(args.chart.clone()))?;
    let request = Request::new(chart, args.corners, &args.exclude);
    let report = calibrate(shot, &request)?;
    Ok(answer(&report, &args.exclude))
}

/// A decoded picture's dry run: `data` its linear working RGB, `channels`
/// per pixel (three or more, the first three read), row by row.
///
/// # Errors
/// As [`calibrate_shot`].
pub fn calibrate_picture(
    args: &ChartArgs,
    data: &[f32],
    width: usize,
    height: usize,
    channels: usize,
) -> Result<ChartAnswer, ChartError> {
    if channels < 3 || width == 0 || height == 0 || data.len() < width * height * channels {
        return Err(ChartError::BadImage);
    }
    let image = ImageView {
        data,
        width,
        height,
        channels,
    };
    let shot = Shot {
        image,
        clip: Some(ClipSource {
            image,
            level: PICTURE_CLIP_LEVEL,
        }),
        standard: IDENTITY,
    };
    calibrate_shot(args, &shot)
}

#[cfg(test)]
mod tests;
