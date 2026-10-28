//! Gamut mapping: a working-space colour brought inside an output space.
//!
//! The working space (linear Rec.2020) holds colours that sRGB, Display P3
//! or Adobe RGB cannot show, and values below 0 or above 1. Clipping each
//! channel on its own shifts their hue (a saturated orange turns yellow, a
//! blue turns purple) and their lightness. [`GamutMap`] does it the one way
//! Helios does it everywhere a colour leaves the working space:
//!
//! - A colour inside the space is left as it is, exactly: the mapping is
//!   colorimetric where it can be.
//! - A colour outside is moved in Oklab, a perceptual space, along a
//!   straight line toward the neutral axis, keeping its Oklab hue, to the
//!   last point of that line inside the space. The line aims at the grey
//!   of the colour's own lightness, so the lightness stays too; a colour
//!   lighter than white aims at white, one darker than black at black.
//! - What the search leaves outside (a few parts in a billion) is clipped.
//!
//! It is the chroma reduction CSS Color 4 specifies for the web, without
//! its "close enough, clip" shortcut, so that more chroma in gives never
//! less chroma out. See `docs/icc.md`.

use crate::{Mat3, OutputSpace, apply, inverse, mul};

/// Oklab's first matrix, CIE XYZ (D65) to cone responses (Björn Ottosson,
/// "A perceptual color space for image processing", 2020).
const XYZ_TO_LMS: Mat3 = [
    [0.818_933_010_1, 0.361_866_742_4, -0.128_859_713_7],
    [0.032_984_543_6, 0.929_311_871_5, 0.036_145_638_7],
    [0.048_200_301_8, 0.264_366_269_1, 0.633_851_707_0],
];

/// Oklab's first row of the second matrix: cube-rooted cone responses to
/// lightness. Only the lightness is needed: the lines the mapping follows
/// are straight in Oklab, so they are straight in cube-rooted cone space
/// too (the two differ by a matrix), where they are walked.
const LMS_TO_L: [f64; 3] = [0.210_454_255_3, 0.793_617_785_0, -0.004_072_046_8];

/// How far outside [0, 1] a channel may be and still count as inside: the
/// rounding of the matrices, far below a 16-bit step.
const SLACK: f64 = 1e-9;

/// Beyond this, a channel is taken as this: an infinite value is brighter
/// than any white, and the cube roots stay finite.
const HUGE: f64 = 1e12;

/// The search for the gamut's edge: even steps first, then halvings, which
/// leave it within 2^-44 of the line's length.
const STEPS: u32 = 16;
const HALVINGS: u32 = 40;

/// The gamut mapping of one output space, its matrices computed once.
#[derive(Clone, Debug)]
pub struct GamutMap {
    /// Working space to the output space's linear RGB.
    to_space: Mat3,
    /// Working space to cone responses, scaled so that white is (1, 1, 1).
    to_lms: Mat3,
    /// Cone responses back to the output space's linear RGB.
    lms_to_space: Mat3,
}

impl GamutMap {
    /// The mapping into `space`.
    pub fn new(space: OutputSpace) -> Self {
        // The D65 white Helios derives from its xy lands on (1, 1, 1), so
        // that every grey is on Oklab's neutral axis exactly; Ottosson's
        // matrix was fitted to a white rounded differently, a 1e-4 change.
        let white = apply(&XYZ_TO_LMS, crate::xy_to_xyz(crate::D65));
        let scale = [
            [1.0 / white[0], 0.0, 0.0],
            [0.0, 1.0 / white[1], 0.0],
            [0.0, 0.0, 1.0 / white[2]],
        ];
        let xyz_to_lms = mul(&scale, &XYZ_TO_LMS);
        let lms_to_xyz = inverse(&xyz_to_lms).expect("Oklab's matrix is invertible");
        let xyz_to_space = inverse(&space.to_xyz()).expect("primaries are independent");
        GamutMap {
            to_space: space.from_rec2020(),
            to_lms: mul(&xyz_to_lms, &OutputSpace::Rec2020.to_xyz()),
            lms_to_space: mul(&xyz_to_space, &lms_to_xyz),
        }
    }

    /// A working-space colour (linear Rec.2020) as linear RGB of the
    /// output space, every channel in [0, 1]. Never NaN: a NaN channel
    /// counts as 0, an infinite one as very large.
    pub fn map(&self, rgb: [f64; 3]) -> [f64; 3] {
        let rgb = rgb.map(|v| {
            if v.is_nan() {
                0.0
            } else {
                v.clamp(-HUGE, HUGE)
            }
        });
        let out = apply(&self.to_space, rgb);
        if inside(out) {
            return out.map(|v| v.clamp(0.0, 1.0));
        }
        let target = apply(&self.to_lms, rgb).map(f64::cbrt);
        let lightness: f64 = LMS_TO_L.iter().zip(target).map(|(k, v)| k * v).sum();
        // White's lightness: the row's sum, 1 to eight decimals.
        let white: f64 = LMS_TO_L.iter().sum();
        let grey = (lightness / white).clamp(0.0, 1.0);
        let at = |t: f64| {
            let lms = target.map(|v| (grey + t * (v - grey)).powi(3));
            apply(&self.lms_to_space, lms)
        };
        // The last point inside on the line from the grey (t = 0) to the
        // colour (t = 1): first the step where it leaves, so a line that
        // would come back in further on cannot fool the search, then
        // halving.
        let mut low = 0.0;
        let mut high = 1.0;
        for step in 1..=STEPS {
            let t = f64::from(step) / f64::from(STEPS);
            if !inside(at(t)) {
                high = t;
                break;
            }
            low = t;
        }
        for _ in 0..HALVINGS {
            let middle = 0.5 * (low + high);
            if inside(at(middle)) {
                low = middle;
            } else {
                high = middle;
            }
        }
        at(low).map(|v| v.clamp(0.0, 1.0))
    }
}

fn inside(rgb: [f64; 3]) -> bool {
    rgb.iter().all(|v| (-SLACK..=1.0 + SLACK).contains(v))
}

impl OutputSpace {
    /// The gamut mapping into this space ([`GamutMap`]).
    pub fn gamut_map(self) -> GamutMap {
        GamutMap::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Oklab's second matrix: cube-rooted cone responses to L, a, b.
    const LMS_TO_LAB: Mat3 = [
        LMS_TO_L,
        [1.977_998_495_1, -2.428_592_205_0, 0.450_593_709_9],
        [0.025_904_037_1, 0.782_771_766_2, -0.808_675_766_0],
    ];

    /// Oklab (L, a, b) of a linear colour of `space`, from the published
    /// matrices (white within 1e-4 of the neutral axis).
    fn oklab(space: OutputSpace, rgb: [f64; 3]) -> [f64; 3] {
        let lms = apply(&XYZ_TO_LMS, apply(&space.to_xyz(), rgb)).map(f64::cbrt);
        apply(&LMS_TO_LAB, lms)
    }

    fn hue(lab: [f64; 3]) -> f64 {
        lab[2].atan2(lab[1])
    }

    fn chroma(lab: [f64; 3]) -> f64 {
        lab[1].hypot(lab[2])
    }

    /// The angle between two hues, in radians, in [0, pi].
    fn hue_gap(a: f64, b: f64) -> f64 {
        let d = (a - b).rem_euclid(std::f64::consts::TAU);
        d.min(std::f64::consts::TAU - d)
    }

    /// Working-space colours: greys, the Rec.2020 primaries and
    /// secondaries at several levels, and colours on a coarse grid that
    /// reaches below 0 and above 1.
    fn colours() -> Vec<[f64; 3]> {
        let mut out = Vec::new();
        let levels = [-0.2, 0.0, 0.05, 0.2, 0.5, 0.8, 1.0, 1.3];
        for r in levels {
            for g in levels {
                for b in levels {
                    out.push([r, g, b]);
                }
            }
        }
        out
    }

    fn is_inside(rgb: [f64; 3]) -> bool {
        rgb.iter().all(|v| (0.0..=1.0).contains(v))
    }

    #[test]
    fn colours_inside_are_left_as_they_are() {
        for space in OutputSpace::ALL {
            let map = space.gamut_map();
            let m = space.from_rec2020();
            for rgb in colours() {
                let plain = apply(&m, rgb);
                if is_inside(plain) {
                    let got = map.map(rgb);
                    assert!(
                        got.iter().zip(plain).all(|(g, p)| (g - p).abs() < 1e-12),
                        "{space:?}, {rgb:?}: {got:?} for {plain:?}"
                    );
                }
            }
        }
        // Into Rec.2020 itself, every colour of the working cube is inside.
        let map = OutputSpace::Rec2020.gamut_map();
        for rgb in [[0.3, 0.2, 0.1], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] {
            assert_eq!(map.map(rgb), rgb);
        }
    }

    #[test]
    fn colours_outside_land_inside() {
        for space in OutputSpace::ALL {
            let map = space.gamut_map();
            for rgb in colours() {
                let got = map.map(rgb);
                assert!(is_inside(got), "{space:?}, {rgb:?}: {got:?}");
            }
        }
    }

    #[test]
    fn hue_and_lightness_are_kept() {
        // Out-of-gamut colours of lightness inside (0, 1) keep their Oklab
        // hue within 0.01 rad (about half a degree; the published matrices
        // put white 1e-4 off the axis) and their lightness within 1e-4, as
        // long as some chroma is left to carry a hue.
        for space in [
            OutputSpace::Srgb,
            OutputSpace::DisplayP3,
            OutputSpace::AdobeRgb,
        ] {
            let map = space.gamut_map();
            let m = space.from_rec2020();
            let mut checked = 0;
            for rgb in colours() {
                let plain = apply(&m, rgb);
                if is_inside(plain) {
                    continue;
                }
                let before = oklab(OutputSpace::Rec2020, rgb);
                if !(0.05..0.95).contains(&before[0]) {
                    continue;
                }
                let after = oklab(space, map.map(rgb));
                assert!(
                    (after[0] - before[0]).abs() < 1e-4,
                    "{space:?}, {rgb:?}: lightness {} for {}",
                    after[0],
                    before[0]
                );
                if chroma(after) > 0.02 {
                    let gap = hue_gap(hue(after), hue(before));
                    assert!(gap < 0.01, "{space:?}, {rgb:?}: hue off by {gap} rad");
                    checked += 1;
                }
            }
            assert!(checked > 20, "{space:?}: only {checked} colours checked");
        }
    }

    #[test]
    fn more_saturation_never_gives_less() {
        // Ramps of Oklab chroma at a fixed lightness and hue, from grey to
        // well past every output space: the output's chroma never goes
        // down (within 1e-5: the ramps are built with the published
        // matrices, whose white is 1e-4 off the mapping's), and the ramp
        // ends with colour.
        let lab_to_lms = inverse(&LMS_TO_LAB).unwrap();
        let lms_to_working = mul(
            &inverse(&OutputSpace::Rec2020.to_xyz()).unwrap(),
            &inverse(&XYZ_TO_LMS).unwrap(),
        );
        for space in [
            OutputSpace::Srgb,
            OutputSpace::DisplayP3,
            OutputSpace::AdobeRgb,
        ] {
            let map = space.gamut_map();
            for hue_step in 0..12 {
                let h = f64::from(hue_step) * std::f64::consts::TAU / 12.0;
                for lightness in [0.3, 0.5, 0.7] {
                    let mut last = 0.0;
                    for step in 0..=200 {
                        let c = 0.4 * f64::from(step) / 200.0;
                        let lab = [lightness, c * h.cos(), c * h.sin()];
                        let lms = apply(&lab_to_lms, lab).map(|v| v.powi(3));
                        let out = chroma(oklab(space, map.map(apply(&lms_to_working, lms))));
                        assert!(
                            out >= last - 1e-5,
                            "{space:?}, L {lightness}, hue {h}, chroma {c}: {out} after {last}"
                        );
                        last = out;
                    }
                    assert!(last > 0.03, "{space:?}, L {lightness}, hue {h}: {last}");
                }
            }
        }
    }

    #[test]
    fn extreme_values_give_finite_colours() {
        let extremes = [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            -0.0,
            1e30,
            -1e30,
            1e-30,
        ];
        for space in OutputSpace::ALL {
            let map = space.gamut_map();
            for r in extremes {
                for g in extremes {
                    for b in [f64::NAN, 0.0, 0.5, f64::INFINITY, -1e30] {
                        let got = map.map([r, g, b]);
                        assert!(
                            got.iter().all(|v| v.is_finite()) && is_inside(got),
                            "{space:?}, {:?}: {got:?}",
                            [r, g, b]
                        );
                    }
                }
            }
            // Brighter than white is white; darker than black is black.
            let white = map.map([1e6, 1e6, 1e6]);
            assert!(white.iter().all(|v| (v - 1.0).abs() < 1e-9), "{white:?}");
            assert_eq!(map.map([-1.0, -1.0, -1.0]), [0.0, 0.0, 0.0]);
            assert_eq!(map.map([f64::NAN; 3]), [0.0, 0.0, 0.0]);
        }
    }

    #[test]
    fn rec2020_red_in_srgb_stays_red() {
        // Clipping each channel gives (1, 0, 0) with its hue and lightness
        // off; the mapping keeps the hue, so green and blue rise a little.
        let map = OutputSpace::Srgb.gamut_map();
        let got = map.map([1.0, 0.0, 0.0]);
        assert!(got[0] > 0.9 && got[1] < 0.2 && got[2] < 0.2, "{got:?}");
        let before = oklab(OutputSpace::Rec2020, [1.0, 0.0, 0.0]);
        let after = oklab(OutputSpace::Srgb, got);
        assert!(hue_gap(hue(after), hue(before)) < 0.01);
        assert!((after[0] - before[0]).abs() < 1e-4);
    }
}
