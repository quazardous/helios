//! A chart shot measured and solved in one call: what `calibrate.chart`
//! answers on a dry run (see `docs/design/calibration.md`, sections 4, 5
//! and 8).
//!
//! The caller hands the measured image (the raw front's output without
//! `colorin`: white-balanced linear camera RGB, or a decoded picture's
//! linear working RGB), optionally the image clipping is read on, the
//! standard matrix `M_s` (`colorin`'s, the identity for a picture) and the
//! four handles. For each of the eight ways the handles may have been
//! given, [`calibrate`]:
//!
//! 1. lays the chart out and checks every patch ([`check_chart`]);
//! 2. brings the patches into the working space, `x_k = M_s c_k`, a patch
//!    clipped, missing or excluded by the caller at weight 0;
//! 3. estimates the scene's illuminant from the kept neutral patches, the
//!    way darktable's colour checker does from the middle grey
//!    ([`illuminant_from_neutral`]), and builds the adaptation `A` with
//!    colour calibration's own CAT16 ([`Plan::adaptation`]), so the solve
//!    and the render agree;
//! 4. solves `P` ([`solve`]);
//!
//! and keeps the orientation with the lowest mean Delta E 2000 after. The
//! working space is linear Rec.2020, `W` darktable's working profile
//! matrix (Bradford-adapted to D50). Nothing is written: saving the
//! profile is the caller's, on a later slice.

use helios_colour::OutputSpace;
use helios_module_channelmixerrgb::{Plan, close_to_daylight, illuminant_from_neutral};
use helios_photo::color::{Mat3, apply};

use crate::chart::Chart;
use crate::lab::lab_to_xyz;
use crate::layout::{ImageView, Layout, Point};
use crate::sampler::{ChartCheck, ClipSource, THRESHOLDS, Thresholds, check_chart};
use crate::solver::{Orientation, Problem, Solution, SolveError, Stage, solve};

/// The measured shot.
#[derive(Clone, Copy, Debug)]
pub struct Shot<'a> {
    /// The values the solver reads: white-balanced linear camera RGB (or
    /// the picture's linear working RGB), three channels or more (the
    /// first three are read).
    pub image: ImageView<'a>,
    /// Where clipping is read, in the same coordinates; `None` skips the
    /// clipping check.
    pub clip: Option<ClipSource<'a>>,
    /// `M_s`: camera RGB to the working space, `colorin`'s standard
    /// matrix; the identity for a picture.
    pub standard: Mat3,
}

/// What the caller asks.
#[derive(Clone, Debug)]
pub struct Request<'a> {
    pub chart: &'static Chart,
    /// The centres of the four corner patches, in the image's pixels, in
    /// any of the eight orders.
    pub handles: [Point; 4],
    /// Patch names the solver leaves out.
    pub exclude: &'a [String],
    /// `P n = n` (the plan's choice) or all nine coefficients free.
    pub keep_neutral: bool,
    pub stage: Stage,
    pub thresholds: Thresholds,
}

impl<'a> Request<'a> {
    /// The defaults: the neutral kept, stage 2, the sampler's thresholds.
    pub fn new(chart: &'static Chart, handles: [Point; 4], exclude: &'a [String]) -> Self {
        Request {
            chart,
            handles,
            exclude,
            keep_neutral: true,
            stage: Stage::DeltaE2000,
            thresholds: THRESHOLDS,
        }
    }
}

/// The scene's illuminant, estimated from the neutral patches.
#[derive(Clone, Debug, PartialEq)]
pub struct Illuminant {
    /// CIE xy.
    pub xy: [f64; 2],
    /// The correlated colour temperature, in kelvin.
    pub cct: f64,
    /// `d` (near daylight), `bb` (near a black body) or `custom`, as colour
    /// calibration words it.
    pub kind: &'static str,
}

/// The answer: the orientation kept, its layout and checks, the
/// illuminant and adaptation, and the solution.
#[derive(Clone, Debug)]
pub struct Report {
    pub orientation: Orientation,
    pub layout: Layout,
    pub check: ChartCheck,
    pub illuminant: Illuminant,
    /// `A`, from the illuminant to D50, working RGB to working RGB.
    pub adaptation: Mat3,
    /// `W`: working RGB to XYZ (D50).
    pub to_xyz: Mat3,
    pub solution: Solution,
    /// `M_c = P M_s`: camera RGB to the working space, what a profile
    /// stores.
    pub camera_matrix: Mat3,
}

/// Why a shot gives no report.
#[derive(Clone, Debug, PartialEq)]
pub enum RunError {
    /// An excluded name is not a patch of the chart.
    UnknownPatch(String),
    /// The image has fewer than three channels, or no pixel.
    BadImage,
    /// No orientation could be measured and solved; the last reason seen
    /// (`None` when every layout left the image or had no illuminant).
    Unsolved(Option<SolveError>),
}

/// The working space's RGB to XYZ (D50): linear Rec.2020, as colour
/// calibration uses it.
pub fn working_to_xyz() -> Mat3 {
    OutputSpace::Rec2020.to_xyz_d50()
}

fn to_f32(m: &Mat3) -> [[f32; 3]; 3] {
    m.map(|r| r.map(|v| v as f32))
}

fn to_f64(m: &[[f32; 3]; 3]) -> Mat3 {
    m.map(|r| r.map(f64::from))
}

/// The illuminant estimated from the kept neutrals of `measured` (working
/// RGB) against their references, and colour calibration's adaptation from
/// it.
fn adaptation(
    chart: &Chart,
    measured: &[[f64; 3]],
    weights: &[f64],
    to_xyz: &Mat3,
) -> Option<(Illuminant, Mat3)> {
    let mut seen = [0.0; 3];
    let mut truth = [0.0; 3];
    for &k in chart.neutrals.iter().filter(|&&k| weights[k] > 0.0) {
        let xyz = apply(to_xyz, measured[k]);
        let reference = lab_to_xyz(chart.patches[k].lab);
        for c in 0..3 {
            seen[c] += xyz[c];
            truth[c] += reference[c];
        }
    }
    let xy = illuminant_from_neutral(seen.map(|v| v as f32), truth.map(|v| v as f32))?;
    let plan = Plan::new(xy, &to_f32(to_xyz), 0.0, false).ok()?;
    let (cct, kind) = close_to_daylight(xy);
    Some((
        Illuminant {
            xy: xy.map(f64::from),
            cct: f64::from(cct),
            kind,
        },
        to_f64(&plan.adaptation()),
    ))
}

/// Measures, checks and solves the chart in `shot` (see the module's
/// documentation).
pub fn calibrate(shot: &Shot<'_>, request: &Request<'_>) -> Result<Report, RunError> {
    let chart = request.chart;
    let image = &shot.image;
    if image.channels < 3 || image.width == 0 || image.height == 0 {
        return Err(RunError::BadImage);
    }
    let excluded: Vec<usize> = request
        .exclude
        .iter()
        .map(|name| {
            chart
                .patch(name)
                .map(|(k, _)| k)
                .ok_or_else(|| RunError::UnknownPatch(name.clone()))
        })
        .collect::<Result<_, _>>()?;
    let to_xyz = working_to_xyz();

    let mut best: Option<Report> = None;
    let mut last_error = None;
    for orientation in Orientation::ALL {
        let Some(layout) = Layout::new(chart, orientation.corners(request.handles)) else {
            continue;
        };
        let check = check_chart(&layout, image, shot.clip.as_ref(), &request.thresholds);
        let mut measured = vec![[0.0; 3]; chart.len()];
        let mut weights = vec![0.0; chart.len()];
        for (k, patch) in check.patches.iter().enumerate() {
            let Some(patch) = patch else { continue };
            let m = &patch.stats.mean;
            measured[k] = apply(&shot.standard, [m[0], m[1], m[2]]);
            if !patch.excluded() && !excluded.contains(&k) {
                weights[k] = 1.0;
            }
        }
        let Some((illuminant, a)) = adaptation(chart, &measured, &weights, &to_xyz) else {
            continue;
        };
        let problem = Problem {
            chart,
            measured: &measured,
            weights: &weights,
            adaptation: a,
            to_xyz,
            keep_neutral: request.keep_neutral,
            stage: request.stage,
        };
        let solution = match solve(&problem) {
            Ok(solution) => solution,
            Err(e) => {
                last_error = Some(e);
                continue;
            }
        };
        if best
            .as_ref()
            .is_none_or(|b| solution.after.mean < b.solution.after.mean)
        {
            let camera_matrix = helios_photo::color::mul(&solution.matrix, &shot.standard);
            best = Some(Report {
                orientation,
                layout,
                check,
                illuminant,
                adaptation: a,
                to_xyz,
                solution,
                camera_matrix,
            });
        }
    }
    best.ok_or(RunError::Unsolved(last_error))
}

#[cfg(test)]
pub(crate) mod tests;
