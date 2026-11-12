//! A homography: the projective map between two planes, here the chart's
//! plane and the image's. Four point pairs fix it (eight unknowns, the
//! ninth coefficient set to 1).

/// A 3×3 projective map, row-major, applied to `[x, y, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [[f64; 3]; 3]);

impl Homography {
    pub const IDENTITY: Self = Self([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    /// The homography mapping each `from[k]` onto `to[k]`; `None` when the
    /// points are degenerate (three of either set on a line, or repeated).
    pub fn from_points(from: [[f64; 2]; 4], to: [[f64; 2]; 4]) -> Option<Self> {
        // For each pair (x, y) -> (u, v):
        //   h0 x + h1 y + h2 - u h6 x - u h7 y = u
        //   h3 x + h4 y + h5 - v h6 x - v h7 y = v
        let mut a = [[0.0_f64; 9]; 8];
        for k in 0..4 {
            let [x, y] = from[k];
            let [u, v] = to[k];
            a[2 * k] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
            a[2 * k + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
        }
        let h = solve8(a)?;
        let m = Self([[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], 1.0]]);
        // Gaussian elimination accepts a nearly singular system; the
        // determinant tells a projection from a collapse.
        let scale = m.0.iter().flatten().fold(0.0_f64, |s, v| s.max(v.abs()));
        (m.determinant().abs() > 1e-12 * scale.powi(3)).then_some(m)
    }

    /// Where `p` lands; non-finite when it maps to infinity (a point on
    /// the horizon of the projection).
    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        let m = &self.0;
        let [x, y] = p;
        let w = m[2][0] * x + m[2][1] * y + m[2][2];
        [
            (m[0][0] * x + m[0][1] * y + m[0][2]) / w,
            (m[1][0] * x + m[1][1] * y + m[1][2]) / w,
        ]
    }

    pub fn determinant(&self) -> f64 {
        let m = &self.0;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// The inverse map; `None` when singular.
    pub fn inverse(&self) -> Option<Self> {
        let d = self.determinant();
        if d == 0.0 || !d.is_finite() {
            return None;
        }
        let m = &self.0;
        let c = |r0: usize, r1: usize, c0: usize, c1: usize| {
            m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
        };
        // The adjugate (transposed cofactors) over the determinant.
        let adj = [
            [c(1, 2, 1, 2), -c(0, 2, 1, 2), c(0, 1, 1, 2)],
            [-c(1, 2, 0, 2), c(0, 2, 0, 2), -c(0, 1, 0, 2)],
            [c(1, 2, 0, 1), -c(0, 2, 0, 1), c(0, 1, 0, 1)],
        ];
        Some(Self(adj.map(|row| row.map(|v| v / d))))
    }
}

/// Solves the 8×8 system held in `a` (the ninth column is the right-hand
/// side) by Gaussian elimination with partial pivoting.
fn solve8(mut a: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let pivot = (col..8).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, pivot);
        let top = a[col];
        for row in a.iter_mut().skip(col + 1) {
            let f = row[col] / top[col];
            for (v, t) in row.iter_mut().zip(top).skip(col) {
                *v -= f * t;
            }
        }
    }
    let mut x = [0.0; 8];
    for row in (0..8).rev() {
        let s: f64 = (row + 1..8).map(|k| a[row][k] * x[k]).sum();
        x[row] = (a[row][8] - s) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: [[f64; 2]; 4] = [[0.0, 0.0], [5.0, 0.0], [5.0, 3.0], [0.0, 3.0]];
    /// A chart seen in perspective, slightly rotated.
    const QUAD: [[f64; 2]; 4] = [
        [812.5, 640.0],
        [3301.0, 655.5],
        [3290.0, 2310.0],
        [820.0, 2296.5],
    ];

    fn close(a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
        (a[0] - b[0]).abs() < tol && (a[1] - b[1]).abs() < tol
    }

    #[test]
    fn the_four_points_map_exactly() {
        let h = Homography::from_points(SQUARE, QUAD).unwrap();
        for k in 0..4 {
            assert!(close(h.apply(SQUARE[k]), QUAD[k], 1e-8), "{k}");
        }
        let inv = h.inverse().unwrap();
        for k in 0..4 {
            assert!(close(inv.apply(QUAD[k]), SQUARE[k], 1e-10), "{k}");
        }
    }

    #[test]
    fn identity_and_affine_cases() {
        let h = Homography::from_points(SQUARE, SQUARE).unwrap();
        for (a, b) in
            h.0.iter()
                .flatten()
                .zip(Homography::IDENTITY.0.iter().flatten())
        {
            assert!((a - b).abs() < 1e-12);
        }
        // A pure scale and shift is affine: the bottom row stays [0, 0, 1].
        let shifted = SQUARE.map(|[x, y]| [2.0 * x + 10.0, 2.0 * y - 4.0]);
        let h = Homography::from_points(SQUARE, shifted).unwrap();
        assert!(h.0[2][0].abs() < 1e-12 && h.0[2][1].abs() < 1e-12);
        assert!(close(h.apply([2.5, 1.5]), [15.0, -1.0], 1e-12));
    }

    #[test]
    fn lines_stay_lines() {
        let h = Homography::from_points(SQUARE, QUAD).unwrap();
        // The midpoint of a chart edge lands on the image edge between
        // its two corners (though not at its midpoint, in perspective).
        let m = h.apply([2.5, 0.0]);
        let (a, b) = (QUAD[0], QUAD[1]);
        let cross = (b[0] - a[0]) * (m[1] - a[1]) - (b[1] - a[1]) * (m[0] - a[0]);
        assert!(cross.abs() < 1e-6, "{cross}");
    }

    #[test]
    fn degenerate_points_are_refused() {
        let line = [[0.0, 0.0], [1.0, 1.0], [2.0, 2.0], [3.0, 3.0]];
        assert_eq!(Homography::from_points(SQUARE, line), None);
        let repeated = [[0.0, 0.0], [0.0, 0.0], [5.0, 3.0], [0.0, 3.0]];
        assert_eq!(Homography::from_points(SQUARE, repeated), None);
    }
}
