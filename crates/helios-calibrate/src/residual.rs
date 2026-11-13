//! The residual table: what the profile's matrix leaves on the chart,
//! corrected by a hue/saturation table after it, kept only when it earns
//! its place (`docs/design/calibration.md`, section 5, "Residual table").
//!
//! The table is the camera look's ([`crate::look::HueSatTable`]: DNG's
//! layout, 72 hues × 5 saturations × 5 values, read in HSV against the
//! neutral `P` keeps, the luminance restored), fitted by the same kernel
//! regression ([`fit_table_with`]) on the kept patches: each patch's
//! `P x_k` toward its reference `t_k`, in the frame of `x` (the working
//! space before colour calibration's adaptation). Two choices differ from
//! the look's, both for 24 patches instead of thousands of pixels:
//!
//! - wide kernels ([`KERNELS`]): a patch speaks for its neighbours in hue,
//!   so the table is smooth between the chart's colours;
//! - no value axis: every value row is the same, so the table depends on
//!   hue and saturation only and does not care about exposure (the shot's
//!   level is not the photo's).
//!
//! **The held-out check.** Each kept patch is left out in turn: the
//! matrix is solved again without it, the table fitted on the others, and
//! the patch's Delta E 2000 is measured through the matrix alone and
//! through the matrix and the table. The table is kept when it lowers the
//! held-out mean by [`MARGIN`] of it and by [`MIN_GAIN`] at least; the
//! table kept is then fitted on every kept patch. Deterministic: the same
//! measurements give the same table, bit for bit.

use helios_photo::color::{apply, inverse, mul};
use helios_photo::compare::delta_e_2000;
use serde::{Deserialize, Serialize};

use crate::lab::{lab_to_xyz, xyz_to_lab};
use crate::look::{HueSatTable, Kernels, fit_table_with};
use crate::solver::{Problem, Solution, solve};

/// The table is kept when it lowers the held-out mean Delta E 2000 by this
/// fraction of it (the camera look's margin for its own table)…
pub const MARGIN: f64 = 0.02;

/// …and by this much at least, so a matrix that already fits (a held-out
/// mean near 0) never gets a table for noise.
pub const MIN_GAIN: f64 = 0.1;

/// The chart's kernels: hue 20°, saturation 0.35, no value axis, a light
/// shrinkage toward the identity.
pub const KERNELS: Kernels = Kernels {
    hue: 20.0,
    saturation: 0.35,
    value: None,
    shrink: 0.1,
};

/// The residual table's fit and its check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Residual {
    /// Whether the table earned its place.
    pub kept: bool,
    /// The held-out mean Delta E 2000 through the matrix alone…
    pub held_out_without: f64,
    /// …and through the matrix and the table.
    pub held_out_with: f64,
    /// Delta E 2000 per patch through the matrix and the table fitted on
    /// every kept patch, in the chart's order (`None` for a patch that was
    /// not kept).
    pub patches: Vec<Option<f64>>,
    /// That table; `None` when no table could be fitted (no patch with
    /// colour).
    pub table: Option<HueSatTable>,
}

impl Residual {
    /// The table to store: the fitted one when it was kept.
    pub fn kept_table(&self) -> Option<&HueSatTable> {
        self.table.as_ref().filter(|_| self.kept)
    }
}

/// The table taking `solution`'s `P x_k` to the references `targets` on
/// the patches it kept.
fn fit_on(solution: &Solution, targets: &[[f64; 3]]) -> Option<HueSatTable> {
    let (scene, wanted): (Vec<[f64; 3]>, Vec<[f64; 3]>) = solution
        .patches
        .iter()
        .enumerate()
        .filter(|(_, p)| p.weight > 0.0)
        .map(|(k, p)| (apply(&solution.matrix, p.measured), targets[k]))
        .unzip();
    fit_table_with(&scene, &wanted, solution.neutral, &KERNELS)
}

/// The residual table of `problem` solved as `solution`, and its held-out
/// check. `None` when `A` or `W` cannot be inverted (the solve would have
/// refused).
pub fn fit(problem: &Problem<'_>, solution: &Solution) -> Option<Residual> {
    let chart = problem.chart;
    let back = mul(&inverse(&problem.adaptation)?, &inverse(&problem.to_xyz)?);
    let targets: Vec<[f64; 3]> = chart
        .patches
        .iter()
        .map(|p| apply(&back, lab_to_xyz(p.lab)))
        .collect();
    let forward = mul(&problem.to_xyz, &problem.adaptation);
    let lab = |v: [f64; 3]| xyz_to_lab(apply(&forward, v));
    let kept: Vec<usize> = (0..chart.len())
        .filter(|&k| problem.weights[k] > 0.0)
        .collect();

    let (mut without, mut with, mut count) = (0.0, 0.0, 0usize);
    for &k in &kept {
        let mut weights = problem.weights.to_vec();
        weights[k] = 0.0;
        let Ok(other) = solve(&Problem {
            weights: &weights,
            ..*problem
        }) else {
            continue;
        };
        let px = apply(&other.matrix, other.patches[k].measured);
        let reference = chart.patches[k].lab;
        let corrected = fit_on(&other, &targets).map_or(px, |t| t.apply(px));
        without += delta_e_2000(lab(px), reference);
        with += delta_e_2000(lab(corrected), reference);
        count += 1;
    }
    let (held_out_without, held_out_with) = if count == 0 {
        (0.0, 0.0)
    } else {
        (without / count as f64, with / count as f64)
    };

    let table = fit_on(solution, &targets);
    let patches = solution
        .patches
        .iter()
        .map(|p| {
            (p.weight > 0.0).then(|| {
                let px = apply(&solution.matrix, p.measured);
                let out = table.as_ref().map_or(px, |t| t.apply(px));
                delta_e_2000(lab(out), p.reference)
            })
        })
        .collect();
    let gain = held_out_without - held_out_with;
    let kept = table.is_some()
        && count > 0
        && gain >= MIN_GAIN
        && held_out_with <= (1.0 - MARGIN) * held_out_without;
    Some(Residual {
        kept,
        held_out_without,
        held_out_with,
        patches,
        table,
    })
}

#[cfg(test)]
mod tests;
