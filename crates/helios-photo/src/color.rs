//! Color spaces.
//!
//! The working space of the pipe is **linear Rec.2020** (D65), like
//! darktable's scene-referred default: modules receive and produce linear
//! Rec.2020 RGB between the input color step and the output color step.

/// A 3 x 3 matrix, row by row.
pub type Mat3 = [[f64; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Linear Rec.2020 to CIE XYZ, D65.
pub const REC2020_TO_XYZ: Mat3 = [
    [0.636_958_0, 0.144_616_9, 0.168_881_0],
    [0.262_700_2, 0.677_998_1, 0.059_301_7],
    [0.000_000_0, 0.028_072_7, 1.060_985_1],
];

/// Linear sRGB (Rec.709 primaries) to CIE XYZ, D65.
pub const SRGB_TO_XYZ: Mat3 = [
    [0.412_456_4, 0.357_576_1, 0.180_437_5],
    [0.212_672_9, 0.715_152_2, 0.072_175_0],
    [0.019_333_9, 0.119_192_0, 0.950_304_1],
];

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

pub fn apply(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

/// The inverse, or `None` for a singular matrix.
pub fn inverse(m: &Mat3) -> Option<Mat3> {
    let c =
        |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    let det = m[0][0] * c(1, 1, 2, 2) - m[0][1] * c(1, 0, 2, 2) + m[0][2] * c(1, 0, 2, 1);
    if det.abs() < 1e-12 {
        return None;
    }
    let adj = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ];
    Some(adj.map(|row| row.map(|v| v / det)))
}

/// From the camera's XYZ-to-camera matrix (as raw files and DNG record it)
/// to the matrix turning white-balanced camera RGB into linear Rec.2020.
/// Rows are normalized so that camera white maps to working-space white,
/// the way dcraw and darktable do it.
pub fn camera_to_working(xyz_to_camera: &[f64; 9]) -> Option<Mat3> {
    let cam_xyz: Mat3 = [
        [xyz_to_camera[0], xyz_to_camera[1], xyz_to_camera[2]],
        [xyz_to_camera[3], xyz_to_camera[4], xyz_to_camera[5]],
        [xyz_to_camera[6], xyz_to_camera[7], xyz_to_camera[8]],
    ];
    let cam_rgb = mul(&cam_xyz, &REC2020_TO_XYZ);
    // A row summing to zero (an unknown camera's all-zero matrix) cannot be
    // normalized: no matrix, rather than one full of NaN.
    if cam_rgb
        .iter()
        .any(|row| !row.iter().sum::<f64>().is_normal())
    {
        return None;
    }
    let normalized = cam_rgb.map(|row| {
        let sum: f64 = row.iter().sum();
        row.map(|v| v / sum)
    });
    inverse(&normalized).filter(|m| m.iter().flatten().all(|v| v.is_finite()))
}

/// sRGB transfer function: linear to encoded, both in [0, 1].
pub fn srgb_encode(v: f64) -> f64 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

pub fn srgb_decode(v: f64) -> f64 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Mat3, b: &Mat3, tolerance: f64) -> bool {
        (0..3).all(|i| (0..3).all(|j| (a[i][j] - b[i][j]).abs() < tolerance))
    }

    #[test]
    fn inverse_undoes_the_matrix() {
        for m in [REC2020_TO_XYZ, SRGB_TO_XYZ] {
            let inv = inverse(&m).unwrap();
            assert!(close(&mul(&m, &inv), &IDENTITY, 1e-9));
        }
        assert!(inverse(&[[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 0.0, 1.0]]).is_none());
    }

    #[test]
    fn white_is_white_in_every_space() {
        // D65 white: Y = 1 for RGB (1, 1, 1) in both spaces.
        for m in [REC2020_TO_XYZ, SRGB_TO_XYZ] {
            let xyz = apply(&m, [1.0, 1.0, 1.0]);
            assert!(
                (xyz[0] - 0.9505).abs() < 1e-3
                    && (xyz[1] - 1.0).abs() < 1e-6
                    && (xyz[2] - 1.089).abs() < 1e-3,
                "{xyz:?}"
            );
        }
    }

    #[test]
    fn a_camera_that_sees_rec2020_needs_no_conversion() {
        let xyz_to_rec2020 = inverse(&REC2020_TO_XYZ).unwrap();
        let flat: [f64; 9] = std::array::from_fn(|i| xyz_to_rec2020[i / 3][i % 3]);
        assert!(close(&camera_to_working(&flat).unwrap(), &IDENTITY, 1e-9));
    }

    #[test]
    fn camera_white_maps_to_working_white() {
        let m = camera_to_working(&[0.9, -0.3, -0.1, -0.4, 1.2, 0.2, -0.05, 0.15, 0.6]).unwrap();
        let white = apply(&m, [1.0, 1.0, 1.0]);
        assert!(white.iter().all(|v| (v - 1.0).abs() < 1e-9), "{white:?}");
    }

    #[test]
    fn an_unknown_cameras_zero_matrix_gives_none_not_nan() {
        assert_eq!(camera_to_working(&[0.0; 9]), None);
    }

    #[test]
    fn the_srgb_curve_round_trips() {
        for v in [0.0, 0.001, 0.2, 0.5, 1.0] {
            assert!((srgb_decode(srgb_encode(v)) - v).abs() < 1e-12);
        }
        assert!(
            (srgb_encode(0.18) - 0.461).abs() < 1e-3,
            "middle grey encodes near 46 %"
        );
    }
}
