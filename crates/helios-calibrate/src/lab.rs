//! CIE Lab with the D50 white: the frame of the charts' reference values.
//!
//! `helios_photo::compare::lab` reads linear Rec.2020 with the D65 white,
//! for comparing renders; the charts are measured under D50, so their
//! Lab goes to and from XYZ relative to D50.

/// The D50 white in XYZ, Y = 1, as ICC profiles write it (the same value
/// as `helios_colour::ICC_D50`).
pub const D50: [f64; 3] = [0.9642, 1.0, 0.8249];

const EPSILON: f64 = 216.0 / 24389.0;
const KAPPA: f64 = 24389.0 / 27.0;

/// XYZ (D50-relative, Y = 1 for white) to CIE Lab.
pub fn xyz_to_lab(xyz: [f64; 3]) -> [f64; 3] {
    let f = |t: f64| {
        if t > EPSILON {
            t.cbrt()
        } else {
            (KAPPA * t + 16.0) / 116.0
        }
    };
    let fx = f(xyz[0] / D50[0]);
    let fy = f(xyz[1] / D50[1]);
    let fz = f(xyz[2] / D50[2]);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIE Lab to XYZ (D50-relative, Y = 1 for white).
pub fn lab_to_xyz(lab: [f64; 3]) -> [f64; 3] {
    let [l, a, b] = lab;
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let finv = |f: f64| {
        let t = f * f * f;
        if t > EPSILON {
            t
        } else {
            (116.0 * f - 16.0) / KAPPA
        }
    };
    let y = if l > KAPPA * EPSILON {
        fy * fy * fy
    } else {
        l / KAPPA
    };
    [finv(fx) * D50[0], y * D50[1], finv(fz) * D50[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::charts;

    #[test]
    fn white_is_l_100_neutral() {
        let lab = xyz_to_lab(D50);
        assert!((lab[0] - 100.0).abs() < 1e-12);
        assert!(lab[1].abs() < 1e-12 && lab[2].abs() < 1e-12);
        let xyz = lab_to_xyz([100.0, 0.0, 0.0]);
        for c in 0..3 {
            assert!((xyz[c] - D50[c]).abs() < 1e-12);
        }
    }

    #[test]
    fn round_trips_every_reference_and_the_dark_branch() {
        let dark = [[2.0, 0.5, -0.5], [0.5, 0.0, 0.0], [7.0, -3.0, 4.0]];
        let references = charts()
            .into_iter()
            .flat_map(|c| c.patches.iter().map(|p| p.lab));
        for lab in references.chain(dark) {
            let back = xyz_to_lab(lab_to_xyz(lab));
            for c in 0..3 {
                assert!((back[c] - lab[c]).abs() < 1e-9, "{lab:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn middle_grey_is_near_eighteen_percent() {
        // D4 of the 2014 chart, L = 50.76: Y close to 0.19.
        let y = lab_to_xyz([50.76, -0.13, 0.14])[1];
        assert!((y - 0.19).abs() < 0.005, "{y}");
    }
}
