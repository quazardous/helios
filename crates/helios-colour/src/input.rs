//! Input spaces: what a file's tags say about its colours, turned into the
//! conversion from its values to the working space (linear Rec.2020, D65).
//!
//! Matrix and tone curve profiles (nearly every photograph: sRGB, Adobe RGB, Display
//! P3, `ProPhoto`, a camera's JPEG) are converted here, in f64, exactly and
//! the same on every CPU: three tone curves, then one matrix. Profiles that
//! describe their colours with lookup tables go through moxcms's f32
//! transform. moxcms parses every profile. See `docs/icc.md`.

use std::fmt;
use std::sync::Arc;

use moxcms::{
    ColorProfile, DataColorSpace, Layout, Matrix3d, ProfileClass, ProfileText, ToneReprCurve,
    TransformF32Executor, TransformOptions, Xyzd,
};

use crate::{
    ICC_D50, Mat3, OutputSpace, Primaries, apply, bradford, d65_to_d50, inverse, mul,
    try_rgb_to_xyz, xy_to_xyz,
};

/// A tone curve: a file's value to linear light.
#[derive(Clone, Debug, PartialEq)]
pub enum Curve {
    /// The value is linear light.
    Linear,
    /// The curve of an output space ([`OutputSpace::decode`]).
    Space(OutputSpace),
    /// A pure power: linear = value ^ gamma.
    Gamma(f64),
    /// ICC.1's parametric curve in its general form, `[g, a, b, c, d, e,
    /// f]`: (a x + b) ^ g + e from x = d on, c x + f below.
    Parametric([f64; 7]),
    /// Samples over [0, 1], interpolated linearly.
    Table(Vec<f64>),
}

impl Curve {
    /// The linear value of `v`. Negative values (float files only) mirror
    /// the positive ones; a table holds its last value above 1.
    pub fn eval(&self, v: f64) -> f64 {
        if v < 0.0 {
            return -self.eval(-v);
        }
        match self {
            Curve::Linear => v,
            Curve::Space(space) => space.decode(v),
            Curve::Gamma(g) => v.powf(*g),
            Curve::Parametric([g, a, b, c, d, e, f]) => {
                if v >= *d {
                    (a * v + b).max(0.0).powf(*g) + e
                } else {
                    c * v + f
                }
            }
            Curve::Table(t) => match t.len() {
                0 => v,
                1 => t[0],
                n => {
                    let p = v.min(1.0) * (n - 1) as f64;
                    let i = (p.floor() as usize).min(n - 2);
                    let w = p - i as f64;
                    t[i] * (1.0 - w) + t[i + 1] * w
                }
            },
        }
    }

    /// The curve of an ICC tone curve tag.
    fn from_icc(trc: &ToneReprCurve) -> Result<Curve, String> {
        match trc {
            ToneReprCurve::Lut(t) => Ok(match t.len() {
                0 => Curve::Linear,
                // u8Fixed8Number.
                1 => Curve::Gamma(f64::from(t[0]) / 256.0),
                _ => Curve::Table(t.iter().map(|v| f64::from(*v) / 65535.0).collect()),
            }),
            ToneReprCurve::Parametric(p) => {
                let p: Vec<f64> = p.iter().map(|v| f64::from(*v)).collect();
                let below = |a: f64, b: f64| {
                    if a == 0.0 {
                        Err("a parametric curve with a zero slope".to_string())
                    } else {
                        Ok(-b / a)
                    }
                };
                let general = match p[..] {
                    [g] => [g, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    [g, a, b] => [g, a, b, 0.0, below(a, b)?, 0.0, 0.0],
                    [g, a, b, c] => [g, a, b, 0.0, below(a, b)?, c, c],
                    [g, a, b, c, d] => [g, a, b, c, d, 0.0, 0.0],
                    [g, a, b, c, d, e, f] => [g, a, b, c, d, e, f],
                    _ => {
                        return Err(format!("a parametric curve with {} parameters", p.len()));
                    }
                };
                if general.iter().all(|v| v.is_finite()) {
                    Ok(Curve::Parametric(general))
                } else {
                    Err("a parametric curve with a parameter out of range".to_string())
                }
            }
        }
    }
}

#[derive(Clone)]
enum Convert {
    /// Tone curves, then a matrix to the working space.
    Matrix {
        curves: Box<[Curve; 3]>,
        to_working: Mat3,
    },
    /// moxcms's transform to linear Rec.2020, for profiles built on lookup
    /// tables.
    Cms(Arc<TransformF32Executor>),
}

/// The colour space a file's values are in, and how they come to the
/// working space.
#[derive(Clone)]
pub struct InputSpace {
    convert: Convert,
    /// The profile is for grey data.
    gray: bool,
    description: Option<String>,
}

impl fmt::Debug for InputSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("InputSpace");
        match &self.convert {
            Convert::Matrix { curves, to_working } => {
                d.field("curves", curves).field("to_working", to_working)
            }
            Convert::Cms(_) => d.field("convert", &"moxcms transform"),
        };
        d.field("gray", &self.gray)
            .field("description", &self.description)
            .finish()
    }
}

/// Linear RGB under `to_xyz_d50` (XYZ adapted to the ICC D50 white) to the
/// working space.
fn d50_to_working(to_xyz_d50: &Mat3) -> Mat3 {
    let from = inverse(&OutputSpace::Rec2020.to_xyz_d50()).expect("Rec.2020 is invertible");
    mul(&from, to_xyz_d50)
}

impl InputSpace {
    /// One of Helios's output spaces, as it writes it.
    pub fn of(space: OutputSpace) -> InputSpace {
        let to_working = inverse(&space.from_rec2020()).expect("primaries are independent");
        InputSpace {
            convert: Convert::Matrix {
                curves: Box::new([
                    Curve::Space(space),
                    Curve::Space(space),
                    Curve::Space(space),
                ]),
                to_working,
            },
            gray: false,
            description: None,
        }
    }

    /// sRGB: what an untagged file is taken to be.
    pub fn srgb() -> InputSpace {
        InputSpace::of(OutputSpace::Srgb)
    }

    /// RGB with these primaries and white (CIE xy) and one tone curve: a
    /// PNG's cHRM and gAMA chunks.
    pub fn from_chromaticities(
        primaries: Primaries,
        white: [f64; 2],
        curve: Curve,
    ) -> Result<InputSpace, String> {
        let to_xyz = try_rgb_to_xyz(primaries, white)
            .ok_or_else(|| "degenerate chromaticities".to_string())?;
        let to_xyz_d50 = mul(&bradford(xy_to_xyz(white), ICC_D50), &to_xyz);
        Ok(InputSpace {
            convert: Convert::Matrix {
                curves: Box::new([curve.clone(), curve.clone(), curve]),
                to_working: d50_to_working(&to_xyz_d50),
            },
            gray: false,
            description: None,
        })
    }

    /// The space of cICP code points (ITU-T H.273): colour primaries,
    /// transfer characteristics, matrix coefficients, full range. Only RGB
    /// (matrix 0) at full range, with the SDR curves Helios knows; PQ and
    /// HLG wait for HDR support.
    pub fn from_cicp(code_points: [u8; 4]) -> Result<InputSpace, String> {
        let [primaries, transfer, matrix, full_range] = code_points;
        if matrix != 0 {
            return Err(format!(
                "cICP matrix coefficients {matrix}: only RGB (0) is supported"
            ));
        }
        if full_range != 1 {
            return Err("cICP narrow range: only full range is supported".to_string());
        }
        let (primaries, white) = match primaries {
            1 => (OutputSpace::Srgb.primaries(), crate::D65),
            9 => (OutputSpace::Rec2020.primaries(), crate::D65),
            12 => (OutputSpace::DisplayP3.primaries(), crate::D65),
            // DCI-P3: the P3 primaries with the DCI white.
            11 => (OutputSpace::DisplayP3.primaries(), [0.314, 0.351]),
            p => return Err(format!("cICP colour primaries {p} are not supported")),
        };
        let curve = match transfer {
            // H.273 lists the BT.709 curve as 1, 6, 14 and 15.
            1 | 6 | 14 | 15 => Curve::Space(OutputSpace::Rec2020),
            13 => Curve::Space(OutputSpace::Srgb),
            8 => Curve::Linear,
            4 => Curve::Gamma(2.2),
            5 => Curve::Gamma(2.8),
            16 | 18 => {
                return Err(format!(
                    "cICP transfer {transfer} is HDR (PQ or HLG), not supported yet"
                ));
            }
            t => {
                return Err(format!(
                    "cICP transfer characteristics {t} are not supported"
                ));
            }
        };
        InputSpace::from_chromaticities(primaries, white, curve)
    }

    /// The space an embedded ICC profile describes. An error says why the
    /// profile cannot be used; the caller falls back and reports it. A
    /// profile never panics the caller: the parser runs behind
    /// `catch_unwind`.
    pub fn from_icc(bytes: &[u8]) -> Result<InputSpace, String> {
        std::panic::catch_unwind(|| icc(bytes))
            .unwrap_or_else(|_| Err("the ICC profile made its reader fail".to_string()))
    }

    /// Whether the profile describes grey data (one channel).
    pub fn is_gray(&self) -> bool {
        self.gray
    }

    /// The profile's own description, when it has one.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Converts pixels in place, RGBA in [0, 1] (alpha untouched), from
    /// this space to the working space. `bits` is the depth of the file's
    /// samples when they are integers (8 or 16): the curves are then
    /// tabulated per code value. Grey files come as RGBA with R = G = B.
    /// An error leaves the pixels as they were.
    pub fn to_working(&self, rgba: &mut [f32], bits: Option<u32>) -> Result<(), String> {
        match &self.convert {
            Convert::Matrix { curves, to_working } => {
                let tables = bits.filter(|b| *b == 8 || *b == 16).map(|b| {
                    let max = f64::from((1u32 << b) - 1);
                    let table: Vec<[f64; 3]> = (0..=(max as u32))
                        .map(|i| {
                            let v = f64::from(i) / max;
                            [curves[0].eval(v), curves[1].eval(v), curves[2].eval(v)]
                        })
                        .collect();
                    (table, max as f32)
                });
                for px in rgba.chunks_exact_mut(4) {
                    let linear = [0, 1, 2].map(|c| match &tables {
                        Some((table, max)) => {
                            table[(px[c].clamp(0.0, 1.0) * max).round() as usize][c]
                        }
                        None => curves[c].eval(f64::from(px[c])),
                    });
                    let out = apply(to_working, linear);
                    for c in 0..3 {
                        px[c] = out[c] as f32;
                    }
                }
                Ok(())
            }
            Convert::Cms(transform) => {
                let src: Vec<f32> = rgba
                    .chunks_exact(4)
                    .flat_map(|p| [p[0], p[1], p[2]])
                    .collect();
                let mut dst = vec![0.0f32; src.len()];
                transform
                    .transform(&src, &mut dst)
                    .map_err(|e| format!("the ICC transform failed: {e}"))?;
                for (px, rgb) in rgba.chunks_exact_mut(4).zip(dst.chunks_exact(3)) {
                    px[..3].copy_from_slice(rgb);
                }
                Ok(())
            }
        }
    }
}

fn text(t: Option<&ProfileText>) -> Option<String> {
    let s = match t? {
        ProfileText::PlainString(s) => s.clone(),
        ProfileText::Localizable(l) => l.first()?.value.clone(),
        ProfileText::Description(d) => d.ascii_string.clone(),
    };
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!s.is_empty()).then(|| s.to_string())
}

fn xyzd([x, y, z]: [f64; 3]) -> Xyzd {
    Xyzd { x, y, z }
}

/// Linear Rec.2020 as an ICC profile: where moxcms's transforms land.
fn working_profile() -> ColorProfile {
    let mut p = ColorProfile::new_bt2020();
    let m = OutputSpace::Rec2020.to_xyz_d50();
    p.red_colorant = xyzd([m[0][0], m[1][0], m[2][0]]);
    p.green_colorant = xyzd([m[0][1], m[1][1], m[2][1]]);
    p.blue_colorant = xyzd([m[0][2], m[1][2], m[2][2]]);
    p.white_point = xyzd(ICC_D50);
    p.media_white_point = Some(xyzd(ICC_D50));
    p.chromatic_adaptation = Some(Matrix3d { v: d65_to_d50() });
    // An empty table is the identity curve.
    let linear = ToneReprCurve::Lut(Vec::new());
    p.red_trc = Some(linear.clone());
    p.green_trc = Some(linear.clone());
    p.blue_trc = Some(linear);
    p.cicp = None;
    p
}

fn icc(bytes: &[u8]) -> Result<InputSpace, String> {
    let profile =
        ColorProfile::new_from_slice(bytes).map_err(|e| format!("unreadable ICC profile: {e}"))?;
    let description = text(profile.description.as_ref());
    match profile.profile_class {
        ProfileClass::DeviceLink | ProfileClass::Abstract | ProfileClass::Named => {
            return Err(format!(
                "a {:?} ICC profile, not one that describes an image",
                profile.profile_class
            ));
        }
        _ => {}
    }
    let has_lut =
        profile.lut_a_to_b_perceptual.is_some() || profile.lut_a_to_b_colorimetric.is_some();
    match profile.color_space {
        DataColorSpace::Gray => {
            let trc = profile
                .gray_trc
                .as_ref()
                .ok_or_else(|| "a grey ICC profile without a tone curve".to_string())?;
            let curve = Curve::from_icc(trc)?;
            // Grey is the D50 white of the connection space scaled: the
            // working space's white, scaled.
            Ok(InputSpace {
                convert: Convert::Matrix {
                    curves: Box::new([curve.clone(), curve.clone(), curve]),
                    to_working: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                },
                gray: true,
                description,
            })
        }
        DataColorSpace::Rgb if has_lut => {
            // ICC.1: the A2B tables take precedence over the matrix/TRC tags.
            let options = TransformOptions {
                prefer_fixed_point: false,
                ..TransformOptions::default()
            };
            let transform = profile
                .create_transform_f32(Layout::Rgb, &working_profile(), Layout::Rgb, options)
                .map_err(|e| format!("the ICC profile's tables cannot be used: {e}"))?;
            Ok(InputSpace {
                convert: Convert::Cms(transform),
                gray: false,
                description,
            })
        }
        DataColorSpace::Rgb => {
            if profile.pcs != DataColorSpace::Xyz {
                return Err("a matrix ICC profile whose connection space is not XYZ".to_string());
            }
            let (Some(r), Some(g), Some(b)) = (
                profile.red_trc.as_ref(),
                profile.green_trc.as_ref(),
                profile.blue_trc.as_ref(),
            ) else {
                return Err("an RGB ICC profile without tone curves or tables".to_string());
            };
            let curves = [
                Curve::from_icc(r)?,
                Curve::from_icc(g)?,
                Curve::from_icc(b)?,
            ];
            let (r, g, b) = (
                profile.red_colorant,
                profile.green_colorant,
                profile.blue_colorant,
            );
            let to_xyz_d50: Mat3 = [[r.x, g.x, b.x], [r.y, g.y, b.y], [r.z, g.z, b.z]];
            let white = apply(&to_xyz_d50, [1.0, 1.0, 1.0]);
            let sane = to_xyz_d50.iter().flatten().all(|v| v.is_finite())
                && (0.5..2.0).contains(&white[1])
                && inverse(&to_xyz_d50).is_some();
            if !sane {
                return Err("the ICC profile's colorants do not make a colour space".to_string());
            }
            Ok(InputSpace {
                convert: Convert::Matrix {
                    curves: Box::new(curves),
                    to_working: d50_to_working(&to_xyz_d50),
                },
                gray: false,
                description,
            })
        }
        other => Err(format!(
            "an ICC profile for {other:?} data: only RGB and grey are supported"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(space: &InputSpace, rgb: [f32; 3], bits: Option<u32>) -> [f32; 3] {
        let mut px = [rgb[0], rgb[1], rgb[2], 1.0];
        space.to_working(&mut px, bits).unwrap();
        assert_eq!(px[3], 1.0, "alpha untouched");
        [px[0], px[1], px[2]]
    }

    fn close(a: [f32; 3], b: [f32; 3], tolerance: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < tolerance)
    }

    /// Encoded values of a few colours in `space`, and the working-space
    /// colour each stands for.
    fn samples(space: OutputSpace) -> Vec<([f32; 3], [f32; 3])> {
        let to = space.from_rec2020();
        [
            [0.18, 0.18, 0.18],
            [1.0, 1.0, 1.0],
            [0.3, 0.2, 0.1],
            [0.05, 0.1, 0.2],
        ]
        .into_iter()
        .map(|working| {
            let encoded = apply(&to, working).map(|v| space.encode(v) as f32);
            (encoded, working.map(|v| v as f32))
        })
        .collect()
    }

    #[test]
    fn output_spaces_read_back_exactly() {
        for space in OutputSpace::ALL {
            let input = InputSpace::of(space);
            for (encoded, working) in samples(space) {
                let got = convert(&input, encoded, None);
                assert!(
                    close(got, working, 1e-5),
                    "{space:?}: {got:?} for {working:?}"
                );
            }
        }
    }

    #[test]
    fn helios_profiles_read_like_their_spaces() {
        for space in OutputSpace::ALL {
            for (version, bytes) in [(4, space.icc_v4()), (2, space.icc_v2())] {
                let input = InputSpace::from_icc(&bytes)
                    .unwrap_or_else(|e| panic!("{space:?} v{version}: {e}"));
                assert!(!input.is_gray());
                assert_eq!(
                    input.description(),
                    Some(format!("Helios {}", space.name()).as_str())
                );
                for (encoded, working) in samples(space) {
                    // s15Fixed16 colorants; the v2 tables interpolate.
                    for bits in [None, Some(16)] {
                        let got = convert(&input, encoded, bits);
                        assert!(
                            close(got, working, 5e-4),
                            "{space:?} v{version} {bits:?}: {got:?} for {working:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn cicp_code_points_name_the_spaces() {
        for space in OutputSpace::ALL {
            let Some(cicp) = space.cicp() else { continue };
            let input = InputSpace::from_cicp(cicp).unwrap();
            for (encoded, working) in samples(space) {
                let got = convert(&input, encoded, Some(8));
                // 8-bit tables: the value is rounded to a code value.
                let want = convert(&InputSpace::of(space), encoded, Some(8));
                assert!(close(got, want, 1e-5), "{space:?}: {got:?} for {want:?}");
                assert!(close(got, working, 2e-2), "{space:?}");
            }
        }
        for unsupported in [
            [9, 16, 0, 1],
            [9, 18, 0, 1],
            [1, 13, 1, 1],
            [1, 13, 0, 0],
            [2, 13, 0, 1],
        ] {
            let err = InputSpace::from_cicp(unsupported).unwrap_err();
            assert!(err.contains("cICP"), "{err}");
        }
    }

    #[test]
    fn gamma_and_chromaticities() {
        // Linear light with the sRGB primaries: grey stays grey.
        let input = InputSpace::from_chromaticities(
            OutputSpace::Srgb.primaries(),
            crate::D65,
            Curve::Gamma(1.0),
        )
        .unwrap();
        assert!(close(convert(&input, [0.5; 3], None), [0.5; 3], 1e-6));
        let degenerate = [[0.3, 0.3], [0.3, 0.3], [0.3, 0.3]];
        assert!(InputSpace::from_chromaticities(degenerate, crate::D65, Curve::Linear).is_err());
    }

    #[test]
    fn parametric_curves_follow_icc() {
        // sRGB as ICC function 3.
        let srgb = Curve::from_icc(&ToneReprCurve::Parametric(vec![
            2.4,
            1.0 / 1.055,
            0.055 / 1.055,
            1.0 / 12.92,
            0.04045,
        ]))
        .unwrap();
        for i in 0..=10 {
            let v = f64::from(i) / 10.0;
            assert!((srgb.eval(v) - crate::srgb_decode(v)).abs() < 1e-6, "{v}");
        }
        assert!((srgb.eval(-0.5) + crate::srgb_decode(0.5)).abs() < 1e-6);
        assert!(Curve::from_icc(&ToneReprCurve::Parametric(vec![2.2, 0.0, 0.1])).is_err());
        assert!(Curve::from_icc(&ToneReprCurve::Parametric(vec![2.2, 1.0])).is_err());
        assert_eq!(
            Curve::from_icc(&ToneReprCurve::Lut(vec![563])).unwrap(),
            Curve::Gamma(563.0 / 256.0)
        );
    }

    #[test]
    fn grey_profiles_give_neutral_grey() {
        let bytes = ColorProfile::new_gray_with_gamma(2.2).encode().unwrap();
        let input = InputSpace::from_icc(&bytes).unwrap();
        assert!(input.is_gray());
        let got = convert(&input, [0.5; 3], None);
        let want = 0.5f32.powf(2.2);
        assert!(close(got, [want; 3], 2e-3), "{got:?}");
    }

    #[test]
    fn broken_profiles_are_refused_with_a_reason() {
        let mut truncated = OutputSpace::DisplayP3.icc_v4();
        truncated.truncate(200);
        let mut no_colorants = OutputSpace::Srgb.icc_v4();
        let mut parsed = ColorProfile::new_from_slice(&no_colorants).unwrap();
        parsed.red_colorant = Xyzd::default();
        parsed.green_colorant = Xyzd::default();
        parsed.blue_colorant = Xyzd::default();
        no_colorants = parsed.encode().unwrap();
        let cmyk = {
            let mut p = ColorProfile::new_srgb();
            p.color_space = DataColorSpace::Cmyk;
            p.encode().ok()
        };
        let garbage = b"definitely not an ICC profile, but long enough to look like one at a glance, with more bytes than a header holds: 0123456789".to_vec();
        let mut cases = vec![Vec::new(), garbage, truncated, no_colorants];
        cases.extend(cmyk);
        for bytes in cases {
            let err = InputSpace::from_icc(&bytes).unwrap_err();
            assert!(!err.is_empty());
        }
    }
}
