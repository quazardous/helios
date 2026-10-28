//! Colour tagging: what a file says about its colours.
//!
//! Helios works in linear Rec.2020 (D65). A file leaves in an
//! [`OutputSpace`]: its primaries, its transfer curve, and the tags that
//! tell a reader both, an ICC profile ([`OutputSpace::icc_v4`],
//! [`OutputSpace::icc_v2`]) and, for PNG, the cICP code points
//! ([`OutputSpace::cicp`]). The profile is built from the same primaries and
//! curve the pixels are encoded with, so the file and its tag never
//! disagree. See `docs/icc.md` for the choices.

mod icc;

use serde::{Deserialize, Serialize};

/// A 3 x 3 matrix, row by row.
pub type Mat3 = [[f64; 3]; 3];

/// The colour space a file is written in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputSpace {
    /// sRGB (IEC 61966-2-1): what a reader assumes of an untagged file.
    #[default]
    Srgb,
    /// Display P3: the P3 primaries with a D65 white and the sRGB curve.
    DisplayP3,
    /// Adobe RGB (1998): its primaries and a pure gamma of 563/256.
    AdobeRgb,
    /// Rec.2020 (ITU-R BT.2020) with its own transfer curve, the BT.709
    /// camera curve.
    Rec2020,
}

/// CIE xy chromaticities of red, green and blue.
type Primaries = [[f64; 2]; 3];

/// The D65 white, CIE xy.
const D65: [f64; 2] = [0.3127, 0.3290];

/// The ICC profile connection space's white (D50), as ICC.1 fixes it.
pub const ICC_D50: [f64; 3] = [0.9642, 1.0, 0.8249];

/// Bradford cone response.
const BRADFORD: Mat3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// Adobe RGB's gamma, exactly as its specification writes it.
pub const ADOBE_RGB_GAMMA: f64 = 563.0 / 256.0;

/// BT.2020's curve constants (alpha and beta, full precision).
const BT2020_ALPHA: f64 = 1.099_296_826_809_44;
const BT2020_BETA: f64 = 0.018_053_968_510_807;

impl OutputSpace {
    /// Every output space, sRGB first.
    pub const ALL: [OutputSpace; 4] = [
        OutputSpace::Srgb,
        OutputSpace::DisplayP3,
        OutputSpace::AdobeRgb,
        OutputSpace::Rec2020,
    ];

    /// A short English name, for reports.
    pub fn name(self) -> &'static str {
        match self {
            OutputSpace::Srgb => "sRGB",
            OutputSpace::DisplayP3 => "Display P3",
            OutputSpace::AdobeRgb => "Adobe RGB (1998)",
            OutputSpace::Rec2020 => "Rec.2020",
        }
    }

    fn primaries(self) -> Primaries {
        match self {
            OutputSpace::Srgb => [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
            OutputSpace::DisplayP3 => [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]],
            OutputSpace::AdobeRgb => [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]],
            OutputSpace::Rec2020 => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]],
        }
    }

    /// Linear RGB in this space to CIE XYZ, D65 white at Y = 1.
    pub fn to_xyz(self) -> Mat3 {
        rgb_to_xyz(self.primaries(), D65)
    }

    /// Linear RGB in this space to CIE XYZ adapted to the ICC D50 white
    /// (Bradford): the columns are the profile's colorants.
    pub fn to_xyz_d50(self) -> Mat3 {
        mul(&d65_to_d50(), &self.to_xyz())
    }

    /// Linear Rec.2020 (the working space) to linear RGB in this space.
    pub fn from_rec2020(self) -> Mat3 {
        if self == OutputSpace::Rec2020 {
            return IDENTITY;
        }
        let from_xyz = inverse(&self.to_xyz()).expect("primaries are independent");
        mul(&from_xyz, &OutputSpace::Rec2020.to_xyz())
    }

    /// The transfer curve: linear light to the file's value, in [0, 1].
    pub fn encode(self, v: f64) -> f64 {
        match self {
            OutputSpace::Srgb | OutputSpace::DisplayP3 => srgb_encode(v),
            OutputSpace::AdobeRgb => v.max(0.0).powf(1.0 / ADOBE_RGB_GAMMA),
            OutputSpace::Rec2020 => {
                if v < BT2020_BETA {
                    4.5 * v
                } else {
                    BT2020_ALPHA * v.powf(0.45) - (BT2020_ALPHA - 1.0)
                }
            }
        }
    }

    /// The inverse of [`OutputSpace::encode`]: the file's value to linear
    /// light.
    pub fn decode(self, v: f64) -> f64 {
        match self {
            OutputSpace::Srgb | OutputSpace::DisplayP3 => srgb_decode(v),
            OutputSpace::AdobeRgb => v.max(0.0).powf(ADOBE_RGB_GAMMA),
            OutputSpace::Rec2020 => {
                if v < 4.5 * BT2020_BETA {
                    v / 4.5
                } else {
                    ((v + BT2020_ALPHA - 1.0) / BT2020_ALPHA).powf(1.0 / 0.45)
                }
            }
        }
    }

    /// The cICP code points (ITU-T H.273) of this space: colour primaries,
    /// transfer characteristics, matrix coefficients (0, RGB) and full
    /// range (1). Adobe RGB has no code: `None`, the ICC profile alone
    /// tags it.
    pub fn cicp(self) -> Option<[u8; 4]> {
        // Primaries: 1 BT.709 (sRGB's), 12 SMPTE EG 432 (P3, D65), 9
        // BT.2020. Transfer: 13 the sRGB curve; 1 the BT.709 curve, which
        // H.273 also lists as 6, 14 and 15 (BT.2020 is the same function)
        // and which readers know best as 1.
        match self {
            OutputSpace::Srgb => Some([1, 13, 0, 1]),
            OutputSpace::DisplayP3 => Some([12, 13, 0, 1]),
            OutputSpace::AdobeRgb => None,
            OutputSpace::Rec2020 => Some([9, 1, 0, 1]),
        }
    }

    /// The ICC v4 profile of this space (for PNG and TIFF). Its bytes are
    /// the same on every run.
    pub fn icc_v4(self) -> Vec<u8> {
        icc::v4(self)
    }

    /// The ICC v2 profile of this space, for readers that predate v4 (it
    /// goes into JPEG files). Its bytes are the same on every run.
    pub fn icc_v2(self) -> Vec<u8> {
        icc::v2(self)
    }
}

/// sRGB transfer curve: linear to encoded.
pub fn srgb_encode(v: f64) -> f64 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB transfer curve: encoded to linear.
pub fn srgb_decode(v: f64) -> f64 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

fn xy_to_xyz([x, y]: [f64; 2]) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// The RGB-to-XYZ matrix of a set of primaries and a white point: each
/// primary scaled so that RGB (1, 1, 1) lands on the white at Y = 1.
fn rgb_to_xyz(primaries: Primaries, white: [f64; 2]) -> Mat3 {
    let p = primaries.map(xy_to_xyz);
    let columns: Mat3 = [
        [p[0][0], p[1][0], p[2][0]],
        [p[0][1], p[1][1], p[2][1]],
        [p[0][2], p[1][2], p[2][2]],
    ];
    let inv = inverse(&columns).expect("primaries are independent");
    let s = apply(&inv, xy_to_xyz(white));
    columns.map(|row| [row[0] * s[0], row[1] * s[1], row[2] * s[2]])
}

/// Bradford adaptation from D65 to the ICC D50.
pub fn d65_to_d50() -> Mat3 {
    let src = apply(&BRADFORD, xy_to_xyz(D65));
    let dst = apply(&BRADFORD, ICC_D50);
    let scale = [
        [dst[0] / src[0], 0.0, 0.0],
        [0.0, dst[1] / src[1], 0.0],
        [0.0, 0.0, dst[2] / src[2]],
    ];
    let back = inverse(&BRADFORD).expect("Bradford is invertible");
    mul(&back, &mul(&scale, &BRADFORD))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Mat3, b: &Mat3, tolerance: f64) -> bool {
        a.iter()
            .flatten()
            .zip(b.iter().flatten())
            .all(|(x, y)| (x - y).abs() < tolerance)
    }

    #[test]
    fn matrices_match_the_published_ones() {
        // sRGB and Rec.2020: references of the same derivation, given to
        // 8 decimals, with the primaries and the D65 white both taken as
        // the xy chromaticities IEC 61966-2-1 and ITU-R BT.2020 state; 1e-6
        // is well above their rounding. The often-quoted sRGB matrix
        // (0.4124564, ..., which Helios's photo crate has) is not one: it
        // takes the white as XYZ rounded to five decimals (0.95047, 1,
        // 1.08883), not from its xy, and differs by up to 2.3e-4.
        let srgb: Mat3 = [
            [0.412_390_80, 0.357_584_34, 0.180_480_79],
            [0.212_639_01, 0.715_168_68, 0.072_192_32],
            [0.019_330_82, 0.119_194_78, 0.950_532_15],
        ];
        let rec2020: Mat3 = [
            [0.636_958_05, 0.144_616_90, 0.168_880_98],
            [0.262_700_21, 0.677_998_07, 0.059_301_72],
            [0.000_000_00, 0.028_072_69, 1.060_985_06],
        ];
        assert!(close(&OutputSpace::Srgb.to_xyz(), &srgb, 1e-6));
        assert!(close(&OutputSpace::Rec2020.to_xyz(), &rec2020, 1e-6));
        // Adobe RGB (1998) specification, section 4.3.4.3, to five
        // decimals: the tolerance is one unit of the last one.
        let adobe: Mat3 = [
            [0.576_67, 0.185_56, 0.188_23],
            [0.297_34, 0.627_36, 0.075_29],
            [0.027_03, 0.070_69, 0.991_34],
        ];
        assert!(close(&OutputSpace::AdobeRgb.to_xyz(), &adobe, 1e-5));
    }

    #[test]
    fn white_stays_white() {
        for space in OutputSpace::ALL {
            let white = apply(&space.from_rec2020(), [1.0, 1.0, 1.0]);
            assert!(white.iter().all(|v| (v - 1.0).abs() < 1e-9), "{space:?}");
            let d50 = apply(&space.to_xyz_d50(), [1.0, 1.0, 1.0]);
            assert!(
                d50.iter().zip(ICC_D50).all(|(a, b)| (a - b).abs() < 1e-9),
                "{space:?}: {d50:?}"
            );
        }
    }

    #[test]
    fn curves_invert() {
        for space in OutputSpace::ALL {
            for i in 0..=100 {
                let v = f64::from(i) / 100.0;
                let back = space.decode(space.encode(v));
                assert!((back - v).abs() < 1e-9, "{space:?} at {v}: {back}");
            }
            assert_eq!(space.encode(0.0), 0.0);
            assert!((space.encode(1.0) - 1.0).abs() < 1e-9);
        }
        // BT.2020: the linear toe and the power segment meet.
        let s = OutputSpace::Rec2020;
        assert!((s.encode(BT2020_BETA) - 4.5 * BT2020_BETA).abs() < 1e-6);
    }

    #[test]
    fn spaces_have_stable_names() {
        let names: Vec<String> = OutputSpace::ALL
            .iter()
            .map(|s| serde_json::to_string(s).unwrap())
            .collect();
        assert_eq!(
            names,
            [
                r#""srgb""#,
                r#""display-p3""#,
                r#""adobe-rgb""#,
                r#""rec2020""#
            ]
        );
        let back: OutputSpace = serde_json::from_str(r#""adobe-rgb""#).unwrap();
        assert_eq!(back, OutputSpace::AdobeRgb);
    }
}
