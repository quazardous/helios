//! The solver: a 3×3 matrix `P` that brings the measured patches onto the
//! chart's reference values, keeping the neutral axis (see
//! `docs/design/calibration.md`, section 5).
//!
//! Notation, as in the design note:
//!
//! - `x_k`: patch `k` measured, in the working space before colour
//!   calibration's adaptation (`M_s c_k`, the camera's white-balanced RGB
//!   through `colorin`'s standard matrix);
//! - `A`: colour calibration's adaptation from the scene's illuminant to
//!   D50, given by the caller (the identity when the light is D50);
//! - `W`: working RGB to XYZ relative to D50 (darktable's working profile
//!   matrix), so that `Lab(v) = lab(W v)` with the D50 white;
//! - `t_k = A⁻¹ W⁻¹ xyz(L_k)`: the reference, in the frame of `x_k`;
//! - `w_k`: the patch's weight, 0 when excluded.
//!
//! Before solving, the `x_k` are scaled by `s` so the neutral patches'
//! luminance matches their references' (the exposure, reported, not kept in
//! the matrix). The neutral direction `n` is the direction of the sum of
//! the kept neutral patches; `P = I + U Nᵀ`, with `N` an orthonormal basis
//! of the plane orthogonal to `n`, keeps `P n = n` exactly and leaves six
//! unknowns. Stage 1 minimises `Σ w_k ‖A P x_k − A t_k‖²`, linear in `U`.
//!
//! [`solve_oriented`] tries the eight ways the corners may have been given
//! (four rotations, mirrored or not) and keeps the one with the lowest mean
//! Delta E 2000.

use helios_photo::color::{IDENTITY, Mat3, apply, inverse, mul};
use helios_photo::compare::delta_e_2000;

use crate::chart::Chart;
use crate::lab::{lab_to_xyz, xyz_to_lab};
use crate::layout::{Layout, Point};

/// Darktable's quality words for a mean Delta E 2000 (`channelmixerrgb.c`),
/// with their upper bounds.
pub const QUALITY: [(f64, &str); 3] = [(1.2, "very good"), (2.3, "good"), (3.4, "passable")];

/// The quality word for a mean Delta E 2000.
pub fn quality(mean: f64) -> &'static str {
    QUALITY
        .iter()
        .find(|(bound, _)| mean <= *bound)
        .map_or("bad", |(_, word)| word)
}

/// What the solver is given.
#[derive(Clone, Copy, Debug)]
pub struct Problem<'a> {
    pub chart: &'static Chart,
    /// `x_k`, one per patch in the chart's order.
    pub measured: &'a [[f64; 3]],
    /// `w_k`, one per patch: 1 by default, 0 for an excluded patch.
    pub weights: &'a [f64],
    /// `A`: the adaptation from the scene's illuminant to D50.
    pub adaptation: Mat3,
    /// `W`: working RGB to XYZ relative to D50.
    pub to_xyz: Mat3,
    /// Whether `P n = n` is imposed (the plan's choice); without it all nine
    /// coefficients are free, as in darktable's checker.
    pub keep_neutral: bool,
}

/// Why a problem has no solution.
#[derive(Clone, Debug, PartialEq)]
pub enum SolveError {
    /// `measured` or `weights` does not have one entry per patch.
    WrongLength,
    /// `A` or `W` cannot be inverted.
    Singular,
    /// No neutral patch is kept, or they measure black: no exposure and no
    /// neutral direction.
    NoNeutral,
    /// Too few kept patches, or patches all of one colour: the normal
    /// equations are singular.
    Underdetermined,
}

/// One patch in the report.
#[derive(Clone, Debug, PartialEq)]
pub struct PatchResult {
    pub name: &'static str,
    /// `s x_k`: the measured value at the solved exposure.
    pub measured: [f64; 3],
    /// The chart's reference, CIE Lab (D50).
    pub reference: [f64; 3],
    /// Delta E 2000 through the standard matrix (`P = I`).
    pub before: f64,
    /// Delta E 2000 through the profile.
    pub after: f64,
    /// `w_k`; a patch at weight 0 was not used and is not in the means.
    pub weight: f64,
}

/// Mean, max and the worst three of a set of Delta E.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub mean: f64,
    pub max: f64,
    /// Indexes of the worst patches, worst first (at most three).
    pub worst: Vec<usize>,
}

/// The solver's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Solution {
    /// `P`, applied to `x_k` (and so to `colorin`'s output).
    pub matrix: Mat3,
    /// The exposure `s` as an offset in EV (`log2 s`).
    pub exposure_ev: f64,
    /// `n`, normalised: the neutral direction `P` keeps.
    pub neutral: [f64; 3],
    pub patches: Vec<PatchResult>,
    /// Over the kept patches, through the standard matrix.
    pub before: Summary,
    /// Over the kept patches, through the profile.
    pub after: Summary,
    /// Which stage gave `matrix`.
    pub stage: Stage,
}

/// The solver stage a matrix comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Linear weighted least squares in RGB.
    Linear,
}

impl Solution {
    /// The profile's camera matrix `M_c = P M_s`, from `colorin`'s standard
    /// matrix `M_s` (camera RGB to working space).
    pub fn camera_matrix(&self, standard: &Mat3) -> Mat3 {
        mul(&self.matrix, standard)
    }

    /// The quality word for the mean after.
    pub fn quality(&self) -> &'static str {
        quality(self.after.mean)
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalise(v: [f64; 3]) -> Option<[f64; 3]> {
    let norm = dot(v, v).sqrt();
    (norm.is_normal()).then(|| v.map(|c| c / norm))
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// An orthonormal basis of the plane orthogonal to the unit vector `n`.
fn orthogonal_basis(n: [f64; 3]) -> [[f64; 3]; 2] {
    // The axis least aligned with n, made orthogonal to it.
    let axis = (0..3)
        .min_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs()))
        .unwrap_or(0);
    let mut e = [0.0; 3];
    e[axis] = 1.0;
    let along = dot(e, n);
    let a = normalise([0, 1, 2].map(|c| e[c] - along * n[c])).unwrap_or(e);
    [a, cross(n, a)]
}

/// Solves `m u = b` for a small symmetric positive definite `m` by
/// Cholesky; `None` when `m` is not positive definite.
fn cholesky_solve(m: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let mut l = vec![vec![0.0; n]; n];
    // A pivot this small relative to the diagonal's scale means the
    // patches do not pin down every unknown.
    let scale = (0..n).map(|i| m[i][i].abs()).fold(0.0, f64::max);
    for i in 0..n {
        for j in 0..=i {
            let sum: f64 = (0..j).map(|k| l[i][k] * l[j][k]).sum();
            if i == j {
                let pivot = m[i][i] - sum;
                if pivot.is_nan() || pivot <= scale * 1e-14 {
                    return None;
                }
                l[i][i] = pivot.sqrt();
            } else {
                l[i][j] = (m[i][j] - sum) / l[j][j];
            }
        }
    }
    let mut y = vec![0.0; n];
    for i in 0..n {
        let sum: f64 = (0..i).map(|k| l[i][k] * y[k]).sum();
        y[i] = (b[i] - sum) / l[i][i];
    }
    let mut u = vec![0.0; n];
    for i in (0..n).rev() {
        let sum: f64 = (i + 1..n).map(|k| l[k][i] * u[k]).sum();
        u[i] = (y[i] - sum) / l[i][i];
    }
    Some(u)
}

/// Summary of the Delta E of the patches with a positive weight.
fn summarise(patches: &[PatchResult], delta: impl Fn(&PatchResult) -> f64) -> Summary {
    let mut kept: Vec<(usize, f64)> = patches
        .iter()
        .enumerate()
        .filter(|(_, p)| p.weight > 0.0)
        .map(|(k, p)| (k, delta(p)))
        .collect();
    let mean = if kept.is_empty() {
        0.0
    } else {
        kept.iter().map(|(_, d)| d).sum::<f64>() / kept.len() as f64
    };
    kept.sort_by(|a, b| b.1.total_cmp(&a.1));
    Summary {
        mean,
        max: kept.first().map_or(0.0, |(_, d)| *d),
        worst: kept.iter().take(3).map(|(k, _)| *k).collect(),
    }
}

/// Stage 1: the weighted linear least squares, under the neutral
/// constraint when the problem asks for it.
pub fn solve(problem: &Problem<'_>) -> Result<Solution, SolveError> {
    let chart = problem.chart;
    if problem.measured.len() != chart.len() || problem.weights.len() != chart.len() {
        return Err(SolveError::WrongLength);
    }
    let a = problem.adaptation;
    let w = problem.to_xyz;
    let a_inv = inverse(&a).ok_or(SolveError::Singular)?;
    let w_inv = inverse(&w).ok_or(SolveError::Singular)?;
    let weight = |k: usize| problem.weights[k].max(0.0);

    // The references in the frame of x: t = A⁻¹ W⁻¹ xyz(L).
    let back = mul(&a_inv, &w_inv);
    let targets: Vec<[f64; 3]> = chart
        .patches
        .iter()
        .map(|p| apply(&back, lab_to_xyz(p.lab)))
        .collect();

    // Exposure: the kept neutrals' luminance (Y of W A x) against their
    // references'.
    let forward = mul(&w, &a);
    let luminance = |v: [f64; 3]| apply(&forward, v)[1];
    let neutrals: Vec<usize> = chart
        .neutrals
        .iter()
        .copied()
        .filter(|&k| weight(k) > 0.0)
        .collect();
    let measured_y: f64 = neutrals
        .iter()
        .map(|&k| luminance(problem.measured[k]))
        .sum();
    let reference_y: f64 = neutrals
        .iter()
        .map(|&k| lab_to_xyz(chart.patches[k].lab)[1])
        .sum();
    let s = reference_y / measured_y;
    if neutrals.is_empty() || !s.is_normal() || s < 0.0 {
        return Err(SolveError::NoNeutral);
    }
    let x: Vec<[f64; 3]> = problem.measured.iter().map(|v| v.map(|c| c * s)).collect();

    let sum = neutrals
        .iter()
        .fold([0.0; 3], |acc, &k| [0, 1, 2].map(|c| acc[c] + x[k][c]));
    let neutral = normalise(sum).ok_or(SolveError::NoNeutral)?;

    // P = P0 + U Bᵀ: the constrained P0 = I, B = N (six unknowns); the free
    // P0 = 0, B = I (nine).
    let (base, basis): (Mat3, Vec<[f64; 3]>) = if problem.keep_neutral {
        (IDENTITY, orthogonal_basis(neutral).to_vec())
    } else {
        (
            [[0.0; 3]; 3],
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        )
    };
    let m = basis.len();
    let unknowns = 3 * m;

    // Residual A (P0 x + U y − t), y = Bᵀ x: the column for U[i][j] is
    // A[:, i] y[j]; the target is A (t − P0 x).
    let mut normal = vec![vec![0.0; unknowns]; unknowns];
    let mut rhs = vec![0.0; unknowns];
    for k in 0..chart.len() {
        let wk = weight(k);
        if wk == 0.0 {
            continue;
        }
        let y: Vec<f64> = basis.iter().map(|b| dot(*b, x[k])).collect();
        let p0x = apply(&base, x[k]);
        let target = apply(&a, [0, 1, 2].map(|c| targets[k][c] - p0x[c]));
        let column = |u: usize| -> [f64; 3] {
            let (i, j) = (u / m, u % m);
            [0, 1, 2].map(|r| a[r][i] * y[j])
        };
        let columns: Vec<[f64; 3]> = (0..unknowns).map(column).collect();
        for p in 0..unknowns {
            rhs[p] += wk * dot(columns[p], target);
            for (cell, column) in normal[p].iter_mut().zip(&columns) {
                *cell += wk * dot(columns[p], *column);
            }
        }
    }
    let u = cholesky_solve(&normal, &rhs).ok_or(SolveError::Underdetermined)?;

    let mut matrix = base;
    for (i, row) in matrix.iter_mut().enumerate() {
        for (j, b) in basis.iter().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell += u[i * m + j] * b[c];
            }
        }
    }

    let lab = |v: [f64; 3]| xyz_to_lab(apply(&forward, v));
    let patches: Vec<PatchResult> = chart
        .patches
        .iter()
        .enumerate()
        .map(|(k, p)| PatchResult {
            name: p.name,
            measured: x[k],
            reference: p.lab,
            before: delta_e_2000(lab(x[k]), p.lab),
            after: delta_e_2000(lab(apply(&matrix, x[k])), p.lab),
            weight: weight(k),
        })
        .collect();
    Ok(Solution {
        matrix,
        exposure_ev: s.log2(),
        neutral,
        before: summarise(&patches, |p| p.before),
        after: summarise(&patches, |p| p.after),
        patches,
        stage: Stage::Linear,
    })
}

/// How the chart was seen, relative to the order the handles were given
/// in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Orientation {
    /// The chart's first corner patch is at handle `quarter_turns` (0 to 3):
    /// the handles were given that many places ahead around the chart.
    pub quarter_turns: u8,
    /// The handles go around the chart the other way (a chart seen in a
    /// mirror, or the handles placed counter-clockwise).
    pub mirrored: bool,
}

impl Orientation {
    /// The eight orientations, the upright one first.
    pub const ALL: [Orientation; 8] = {
        let mut all = [Orientation {
            quarter_turns: 0,
            mirrored: false,
        }; 8];
        let mut i = 0;
        while i < 8 {
            all[i] = Orientation {
                quarter_turns: (i % 4) as u8,
                mirrored: i >= 4,
            };
            i += 1;
        }
        all
    };

    /// The handles reordered into the chart's corner order
    /// ([`Chart::corner_centres`]).
    pub fn corners(self, handles: [Point; 4]) -> [Point; 4] {
        let turns = usize::from(self.quarter_turns);
        std::array::from_fn(|c| {
            let at = if self.mirrored {
                (turns + 4 - c) % 4
            } else {
                (turns + c) % 4
            };
            handles[at]
        })
    }

    /// The inverse: the handles as given, from the chart's corner order.
    pub fn handles(self, corners: [Point; 4]) -> [Point; 4] {
        let reordered = self.corners([[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]]);
        let mut out = corners;
        for (c, at) in reordered.iter().enumerate() {
            out[at[0] as usize] = corners[c];
        }
        out
    }
}

/// What the caller measures for one placement of the chart: `x_k` and
/// `w_k`, one per patch.
#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub measured: Vec<[f64; 3]>,
    pub weights: Vec<f64>,
}

/// The best orientation and its solution.
#[derive(Clone, Debug)]
pub struct Oriented {
    pub orientation: Orientation,
    pub layout: Layout,
    pub solution: Solution,
}

/// Tries every orientation of `handles`: for each, `measure` samples the
/// patches of the layout it gives (or `None`, when the layout leaves the
/// image), the problem is solved, and the lowest mean Delta E 2000 after
/// wins (the upright order on a tie). `problem`'s `measured` and `weights`
/// are ignored. `None` when no orientation could be solved.
pub fn solve_oriented(
    problem: &Problem<'_>,
    handles: [Point; 4],
    mut measure: impl FnMut(&Layout) -> Option<Measurement>,
) -> Option<Oriented> {
    let mut best: Option<Oriented> = None;
    for orientation in Orientation::ALL {
        let Some(layout) = Layout::new(problem.chart, orientation.corners(handles)) else {
            continue;
        };
        let Some(measurement) = measure(&layout) else {
            continue;
        };
        let attempt = Problem {
            measured: &measurement.measured,
            weights: &measurement.weights,
            ..*problem
        };
        let Ok(solution) = solve(&attempt) else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|b| solution.after.mean < b.solution.after.mean)
        {
            best = Some(Oriented {
                orientation,
                layout,
                solution,
            });
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::XRITE_CC24_2014;
    use helios_photo::color::{REC2020_TO_XYZ, camera_to_working};

    const CHART: &Chart = &XRITE_CC24_2014;

    /// A working RGB to XYZ (D50) matrix for the tests: Rec.2020's, its
    /// rows scaled so white lands on D50 (a crude adaptation, enough for a
    /// synthetic chart).
    fn to_xyz() -> Mat3 {
        let white = apply(&REC2020_TO_XYZ, [1.0; 3]);
        std::array::from_fn(|r| REC2020_TO_XYZ[r].map(|v| v * crate::lab::D50[r] / white[r]))
    }

    /// An adaptation that is not the identity: a mild cast, as from a warm
    /// light, with a little cross-talk.
    const ADAPTATION: Mat3 = [[0.92, 0.05, 0.01], [0.02, 1.0, -0.01], [0.0, 0.04, 1.18]];

    /// XYZ to camera RGB of a real body (dcraw's table, Canon EOS 5D Mark
    /// II), and of another one used as the "standard" matrix.
    const CAMERA: [f64; 9] = [
        0.4716, 0.0603, -0.0830, -0.7798, 1.5474, 0.2480, -0.1496, 0.1937, 0.6651,
    ];

    /// The references in the frame of x: `t_k = A⁻¹ W⁻¹ xyz(L_k)`.
    fn targets(adaptation: &Mat3) -> Vec<[f64; 3]> {
        let back = mul(&inverse(adaptation).unwrap(), &inverse(&to_xyz()).unwrap());
        CHART
            .patches
            .iter()
            .map(|p| apply(&back, lab_to_xyz(p.lab)))
            .collect()
    }

    /// A true correction keeping the references' neutral direction: what
    /// the solver should find back.
    fn true_correction(targets: &[[f64; 3]]) -> Mat3 {
        let sum = CHART.neutrals.iter().fold([0.0; 3], |acc, &k| {
            [0, 1, 2].map(|c| acc[c] + targets[k][c])
        });
        let n = normalise(sum).unwrap();
        let basis = orthogonal_basis(n);
        let u = [[0.08, -0.05], [-0.03, 0.11], [0.06, 0.04]];
        let mut p = IDENTITY;
        for (i, row) in p.iter_mut().enumerate() {
            for (j, b) in basis.iter().enumerate() {
                for (c, cell) in row.iter_mut().enumerate() {
                    *cell += u[i][j] * b[c];
                }
            }
        }
        p
    }

    fn problem<'a>(measured: &'a [[f64; 3]], weights: &'a [f64]) -> Problem<'a> {
        Problem {
            chart: CHART,
            measured,
            weights,
            adaptation: ADAPTATION,
            to_xyz: to_xyz(),
            keep_neutral: true,
        }
    }

    /// Test 3 on every solve: `P n = n`.
    fn assert_neutral_kept(solution: &Solution) {
        let n = solution.neutral;
        let pn = apply(&solution.matrix, n);
        for c in 0..3 {
            assert!((pn[c] - n[c]).abs() < 1e-12, "P n = {pn:?}, n = {n:?}");
        }
    }

    /// The synthetic shot: the true camera matrix `M_true`, the standard
    /// matrix `M_s = P_true⁻¹ M_true` (off from it but keeping the
    /// neutral), each patch's camera RGB `c_k = M_true⁻¹ t_k` at an
    /// exposure `gain`, and `x_k = M_s c_k`.
    struct Shot {
        truth: Mat3,
        standard: Mat3,
        camera: Vec<[f64; 3]>,
    }

    fn shot(gain: f64) -> Shot {
        let targets = targets(&ADAPTATION);
        let truth = camera_to_working(&CAMERA).unwrap();
        let standard = mul(&inverse(&true_correction(&targets)).unwrap(), &truth);
        let truth_inv = inverse(&truth).unwrap();
        let camera = targets
            .iter()
            .map(|t| apply(&truth_inv, *t).map(|c| c * gain))
            .collect();
        Shot {
            truth,
            standard,
            camera,
        }
    }

    impl Shot {
        fn measured(&self) -> Vec<[f64; 3]> {
            self.camera
                .iter()
                .map(|c| apply(&self.standard, *c))
                .collect()
        }
    }

    #[test]
    fn recovers_a_known_matrix() {
        let shot = shot(0.7);
        let measured = shot.measured();
        let weights = vec![1.0; CHART.len()];
        let solution = solve(&problem(&measured, &weights)).unwrap();
        assert_neutral_kept(&solution);
        assert!((solution.exposure_ev - (1.0 / 0.7_f64).log2()).abs() < 1e-9);
        let found = solution.camera_matrix(&shot.standard);
        for r in 0..3 {
            for c in 0..3 {
                assert!(
                    (found[r][c] - shot.truth[r][c]).abs() < 1e-6,
                    "{found:?} vs {:?}",
                    shot.truth
                );
            }
        }
        for p in &solution.patches {
            assert!(p.after < 1e-4, "{} {}", p.name, p.after);
        }
        assert!(solution.before.mean > 1.0, "{:?}", solution.before);
        assert!(solution.after.max < 1e-4);
        assert_eq!(solution.after.worst.len(), 3);
        assert_eq!(solution.quality(), "very good");
        assert_eq!(solution.stage, Stage::Linear);
    }

    #[test]
    fn with_noise_stays_close() {
        let shot = shot(1.0);
        // A deterministic generator: a small LCG, then Box-Muller.
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut uniform = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let mut gauss = || {
            let (u, v) = (uniform(), uniform());
            (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
        };
        let noisy: Vec<[f64; 3]> = shot
            .camera
            .iter()
            .map(|c| c.map(|v| v * (1.0 + 0.005 * gauss())))
            .map(|c| apply(&shot.standard, c))
            .collect();
        let weights = vec![1.0; CHART.len()];
        let solution = solve(&problem(&noisy, &weights)).unwrap();
        assert_neutral_kept(&solution);
        assert!(solution.after.mean < 0.5, "{:?}", solution.after);
        assert!(solution.after.max < 1.5, "{:?}", solution.after);
        let found = solution.camera_matrix(&shot.standard);
        for r in 0..3 {
            for c in 0..3 {
                assert!((found[r][c] - shot.truth[r][c]).abs() < 0.01, "{found:?}");
            }
        }
    }

    #[test]
    fn excluded_patches_change_nothing() {
        let shot = shot(1.0);
        let measured = shot.measured();
        let mut weights = vec![1.0; CHART.len()];
        // B3 (moderate red) and D5 (a neutral) excluded.
        weights[8] = 0.0;
        weights[22] = 0.0;
        let reference = solve(&problem(&measured, &weights)).unwrap();
        assert_neutral_kept(&reference);
        for wild in [[0.0; 3], [5.0, -3.0, 1e6], [f64::MAX / 4.0, 0.0, 0.0]] {
            let mut moved = measured.clone();
            moved[8] = wild;
            moved[22] = wild;
            let solution = solve(&problem(&moved, &weights)).unwrap();
            assert_eq!(solution.matrix, reference.matrix);
            assert_eq!(solution.exposure_ev, reference.exposure_ev);
            assert_eq!(solution.after.mean, reference.after.mean);
            assert!(!solution.after.worst.contains(&8));
            assert!(!solution.after.worst.contains(&22));
        }
    }

    #[test]
    fn without_the_constraint_all_nine_are_free() {
        // A correction that does not keep the neutral: only the free solve
        // finds it back exactly.
        let targets = targets(&ADAPTATION);
        let skew: Mat3 = [[1.05, 0.02, -0.01], [0.0, 0.97, 0.03], [0.01, -0.02, 1.1]];
        let skew_inv = inverse(&skew).unwrap();
        let measured: Vec<[f64; 3]> = targets.iter().map(|t| apply(&skew_inv, *t)).collect();
        let weights = vec![1.0; CHART.len()];
        let mut free = problem(&measured, &weights);
        free.keep_neutral = false;
        let solution = solve(&free).unwrap();
        // The exposure scale is folded into x: P s = skew.
        let s = solution.exposure_ev.exp2();
        for (found, expected) in solution.matrix.iter().flatten().zip(skew.iter().flatten()) {
            assert!((found * s - expected).abs() < 1e-9);
        }
        assert!(solution.after.max < 1e-4);
        let kept = solve(&problem(&measured, &weights)).unwrap();
        assert_neutral_kept(&kept);
        assert!(kept.after.mean > solution.after.mean);
    }

    #[test]
    fn refuses_what_it_cannot_solve() {
        let shot = shot(1.0);
        let measured = shot.measured();
        let all = vec![1.0; CHART.len()];
        assert_eq!(
            solve(&problem(&measured[..10], &all)).unwrap_err(),
            SolveError::WrongLength
        );
        let mut no_neutral = all.clone();
        for &k in CHART.neutrals {
            no_neutral[k] = 0.0;
        }
        assert_eq!(
            solve(&problem(&measured, &no_neutral)).unwrap_err(),
            SolveError::NoNeutral
        );
        // Only the neutrals, measured exactly grey: nothing pins the colours
        // down.
        let mut only_neutrals = vec![0.0; CHART.len()];
        let mut grey = measured.clone();
        for &k in CHART.neutrals {
            only_neutrals[k] = 1.0;
            grey[k] = [measured[k][1]; 3];
        }
        assert_eq!(
            solve(&problem(&grey, &only_neutrals)).unwrap_err(),
            SolveError::Underdetermined
        );
        let mut singular = problem(&measured, &all);
        singular.adaptation = [[0.0; 3]; 3];
        assert_eq!(solve(&singular).unwrap_err(), SolveError::Singular);
    }

    #[test]
    fn quality_words_follow_darktable() {
        assert_eq!(quality(0.5), "very good");
        assert_eq!(quality(1.2), "very good");
        assert_eq!(quality(2.0), "good");
        assert_eq!(quality(3.0), "passable");
        assert_eq!(quality(3.5), "bad");
    }

    #[test]
    fn orientations_are_eight_and_invert() {
        let handles = [[1.0, 2.0], [3.0, 4.0], [5.0, 6.0], [7.0, 8.0]];
        let mut seen = Vec::new();
        for o in Orientation::ALL {
            let corners = o.corners(handles);
            assert!(!seen.contains(&corners), "{o:?}");
            seen.push(corners);
            assert_eq!(o.handles(corners), handles, "{o:?}");
        }
        assert_eq!(Orientation::ALL[0].corners(handles), handles);
    }

    #[test]
    fn finds_the_orientation_back() {
        let shot = shot(1.0);
        let measured = shot.measured();
        let weights = vec![1.0; CHART.len()];
        // The chart upright in the image, slightly in perspective.
        let truth = [[100.0, 80.0], [610.0, 95.0], [600.0, 410.0], [110.0, 400.0]];
        let upright = Layout::new(CHART, truth).unwrap();
        // The camera sees, at any point, the patch whose centre is nearest.
        let measure = |layout: &Layout| -> Option<Measurement> {
            let measured = (0..CHART.len())
                .map(|k| {
                    let at = layout.centre(k);
                    let nearest = (0..CHART.len())
                        .min_by(|&a, &b| {
                            let d = |q: usize| {
                                let c = upright.centre(q);
                                (c[0] - at[0]).hypot(c[1] - at[1])
                            };
                            d(a).total_cmp(&d(b))
                        })
                        .unwrap();
                    measured[nearest]
                })
                .collect();
            Some(Measurement {
                measured,
                weights: weights.clone(),
            })
        };
        let base = problem(&[], &[]);
        for given in Orientation::ALL {
            let handles = given.handles(truth);
            let found = solve_oriented(&base, handles, measure).unwrap();
            assert_eq!(found.orientation, given);
            assert_eq!(found.layout.corners, truth);
            assert_neutral_kept(&found.solution);
            assert!(found.solution.after.max < 1e-4, "{given:?}");
        }
    }
}
