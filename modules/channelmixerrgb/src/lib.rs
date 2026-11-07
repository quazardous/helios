//! Colour calibration, ported from darktable's `channelmixerrgb` module
//! (`src/iop/channelmixerrgb.c`, `src/common/chromatic_adaptation.h`,
//! `src/common/illuminants.h`): the CAT16 chromatic adaptation.
//!
//! The pixels are in darktable's pipeline working space, linear Rec.2020
//! (its RGB to XYZ matrix adapted to the ICC D50 white). Each pixel goes to
//! CAT16 LMS, is adapted from the illuminant to D50 (full adaptation, as
//! darktable forces for CAT16), then through the gamut compression and the
//! clipping darktable applies on that path, and back to the working space.
//!
//! The illuminant, with darktable's names and units:
//!
//! - `pipe`: D50, nothing to adapt;
//! - `a`, `e`: the standard illuminants A and E;
//! - `d` (daylight) and `bb` (black body): from `temperature` in kelvin,
//!   with darktable's conversions (daylight below 4000 K falls back to the
//!   black body, as in darktable);
//! - `f` and `led`: a fluorescent (`illum_fluo`) or LED (`illum_led`)
//!   standard illuminant;
//! - `custom`: the chromaticity `x`, `y` as given;
//! - `camera` ("as shot in camera"): found from the camera's as-shot white
//!   balance coefficients and its matrix (both declared context inputs),
//!   with darktable's `find_illuminant_xy_from_as_shot_coeffs` and
//!   `WB_coeffs_to_illuminant_xy`. It is darktable's scene-referred
//!   default, with the white balance in "camera reference" (D65) mode
//!   (`modules/whitebalance`), so that the white balance's D65
//!   coefficients need no correction ratios. When it cannot be found (no
//!   matrix, a coefficient not normal, a chromaticity out of the locus), the
//!   illuminant is D50: nothing is adapted, as darktable's "as set in white
//!   balance" fallback.
//!
//! Not ported yet: the illuminant from the white balance's own
//! coefficients ("as set in white balance") or detected from the image;
//! the other adaptations (Bradford, XYZ,
//! RGB); and the channel mixer itself (the red, green and blue rows,
//! colourfulness, brightness, grey). The plan already carries the mixer
//! as an identity matrix, so it slots in later; a parameter Helios does
//! not know is refused by the manifest.

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamKind, ParamSpec, Params,
    PortSpec, Region, Role, Value,
};
use helios_photo::camera::fields;

pub struct ChannelMixerRgb;

/// The illuminants, by darktable's names (`dt_illuminant_t`).
pub const ILLUMINANTS: [&str; 9] = ["pipe", "a", "d", "e", "f", "led", "bb", "custom", CAMERA];
/// The illuminant found from the camera's as-shot coefficients
/// (`DT_ILLUMINANT_CAMERA`).
pub const CAMERA: &str = "camera";
/// darktable's fluorescent illuminants, F1 to F12 (`dt_illuminant_fluo_t`).
pub const FLUORESCENTS: [&str; 12] = [
    "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12",
];
/// darktable's LED illuminants (`dt_illuminant_led_t`).
pub const LEDS: [&str; 9] = ["b1", "b2", "b3", "b4", "b5", "bh1", "rgb1", "v1", "v2"];
/// The adaptations Helios has (`dt_adaptation_t`).
pub const CAT16: &str = "cat16";

/// darktable's D50 white, CIE xy (`D50xyY`).
const D50_XY: [f32; 2] = [0.34567, 0.35850];

/// darktable's chromaticities of the fluorescent illuminants.
const FLUORESCENT_XY: [[f32; 2]; 12] = [
    [0.31310, 0.33727],
    [0.37208, 0.37529],
    [0.40910, 0.39430],
    [0.44018, 0.40329],
    [0.31379, 0.34531],
    [0.37790, 0.38835],
    [0.31292, 0.32933],
    [0.34588, 0.35875],
    [0.37417, 0.37281],
    [0.34609, 0.35986],
    [0.38052, 0.37713],
    [0.43695, 0.40441],
];

/// darktable's chromaticities of the LED illuminants.
const LED_XY: [[f32; 2]; 9] = [
    [0.4560, 0.4078],
    [0.4357, 0.4012],
    [0.3756, 0.3723],
    [0.3422, 0.3502],
    [0.3118, 0.3236],
    [0.4474, 0.4066],
    [0.4557, 0.4211],
    [0.4560, 0.4548],
    [0.3781, 0.3775],
];

type M3 = [[f32; 3]; 3];

/// CIE XYZ to CAT16 LMS, as darktable writes it.
const XYZ_TO_CAT16_LMS: M3 = [
    [0.401288, 0.650173, -0.051461],
    [-0.250268, 1.204414, 0.045854],
    [-0.002079, 0.048952, 0.953127],
];

/// CAT16 LMS to CIE XYZ, as darktable writes it.
const CAT16_LMS_TO_XYZ: M3 = [
    [1.862068, -1.011255, 0.149187],
    [0.38752, 0.621447, -0.008974],
    [-0.015841, -0.034123, 1.049964],
];

/// D50 in CAT16 LMS, as darktable precomputes it.
const D50_LMS: [f32; 3] = [0.994535, 1.000997, 0.833036];

/// D50 in CIE u'v'.
const D50_UV: [f32; 2] = [0.209_159_15, 0.488_075_32];

/// darktable's smallest norm (2^-16).
const NORM_MIN: f32 = 1.525_878_9e-5;

/// What a render needs, the mixer included for when it is ported.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// Working RGB to CAT16 LMS.
    pub rgb_to_lms: M3,
    /// The illuminant in CAT16 LMS.
    pub illuminant: [f32; 3],
    /// CAT16 LMS through the mixer to XYZ (the mixer is the identity for
    /// now).
    pub mix_to_xyz: M3,
    /// XYZ to working RGB.
    pub xyz_to_rgb: M3,
    /// The gamut compression exponent: `1 / gamut`, 0 for none.
    pub compression: f32,
    pub clip: bool,
}

fn choice(name: &str, description: &str, options: &[&str], default: &str) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        description: description.into(),
        kind: ParamKind::Choice {
            options: options.iter().map(ToString::to_string).collect(),
            default: default.into(),
        },
    }
}

/// darktable's `CCT_to_xy_daylight`: (0, 0) out of 4000 to 25000 K.
pub fn daylight_xy(t: f32) -> [f32; 2] {
    let x = if (4000.0..=7000.0).contains(&t) {
        ((-4.6070e9 / t + 2.9678e6) / t + 0.09911e3) / t + 0.244063
    } else if t > 7000.0 && t <= 25000.0 {
        ((-2.0064e9 / t + 1.9018e6) / t + 0.24748e3) / t + 0.237040
    } else {
        0.0
    };
    let y = (-3.0 * x + 2.87) * x - 0.275;
    [x, y]
}

/// darktable's `CCT_to_xy_blackbody`: (0, 0) out of 1667 to 25000 K.
/// Its constants as darktable writes them.
#[allow(clippy::excessive_precision)]
pub fn blackbody_xy(t: f32) -> [f32; 2] {
    let x = if (1667.0..=4000.0).contains(&t) {
        ((-0.266_123_9e9 / t - 0.234_358_9e6) / t + 0.877_695_6e3) / t + 0.179910
    } else if t > 4000.0 && t <= 25000.0 {
        ((-3.025_846_9e9 / t + 2.107_037_9e6) / t + 0.222_634_7e3) / t + 0.240390
    } else {
        0.0
    };
    let y = if (1667.0..=2222.0).contains(&t) {
        ((-1.106_381_4 * x - 1.348_110_2) * x + 2.185_558_3) * x - 0.202_196_83
    } else if t > 2222.0 && t <= 4000.0 {
        ((-0.954_947_6 * x - 1.374_185_9) * x + 2.091_370_2) * x - 0.167_488_67
    } else if t > 4000.0 && t <= 25000.0 {
        ((3.081_758 * x - 5.873_386_7) * x + 3.751_13) * x - 0.370_014_83
    } else {
        0.0
    };
    [x, y]
}

/// The illuminant's chromaticity, as darktable's `illuminant_to_xy` finds
/// it (`custom` takes `x` and `y`; `camera` reads the context, D50 when it
/// finds nothing).
pub fn illuminant_xy(params: &Params, context: &Context) -> Result<[f32; 2], String> {
    let pick = |name: &str| params.get(name).and_then(Value::as_choice).unwrap_or("");
    let t = params.float("temperature") as f32;
    let index = |list: &[&str], name: &str| {
        list.iter()
            .position(|o| *o == pick(name))
            .ok_or_else(|| format!("`{name}`: unknown value `{}`", pick(name)))
    };
    let xy = match pick("illuminant") {
        "pipe" => D50_XY,
        "e" => [1.0 / 3.0; 2],
        "a" => [0.44757, 0.40745],
        "f" => FLUORESCENT_XY[index(&FLUORESCENTS, "illum_fluo")?],
        "led" => LED_XY[index(&LEDS, "illum_led")?],
        // Daylight below 4000 K is the black body's, as in darktable.
        "d" => Some(daylight_xy(t))
            .filter(|xy| xy[0] != 0.0 && xy[1] != 0.0)
            .unwrap_or_else(|| blackbody_xy(t)),
        "bb" => blackbody_xy(t),
        "custom" => [params.float("x") as f32, params.float("y") as f32],
        CAMERA => camera_illuminant_xy(context).unwrap_or(D50_XY),
        other => return Err(format!("illuminant `{other}` is not supported")),
    };
    if xy[0] == 0.0 || xy[1] == 0.0 || !xy.iter().all(|v| v.is_finite()) {
        return Err(format!(
            "no illuminant chromaticity for {} at {t} K (x {}, y {})",
            pick("illuminant"),
            xy[0],
            xy[1]
        ));
    }
    Ok(xy)
}

fn apply(m: &M3, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn mul(a: &M3, b: &M3) -> M3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

/// An illuminant's chromaticity in CAT16 LMS (`illuminant_xy_to_XYZ`, then
/// `convert_any_XYZ_to_LMS`).
pub fn illuminant_lms([x, y]: [f32; 2]) -> [f32; 3] {
    apply(&XYZ_TO_CAT16_LMS, [x / y, 1.0, (1.0 - x - y) / y])
}

/// Linear Rec.2020 to XYZ adapted to the ICC D50, darktable's working
/// profile matrix.
pub fn rec2020_to_xyz_d50() -> M3 {
    helios_colour::OutputSpace::Rec2020
        .to_xyz_d50()
        .map(|r| r.map(|v| v as f32))
}

fn invert(m: &M3) -> Option<M3> {
    let m64 = m.map(|r| r.map(f64::from));
    let det = m64[0][0] * (m64[1][1] * m64[2][2] - m64[1][2] * m64[2][1])
        - m64[0][1] * (m64[1][0] * m64[2][2] - m64[1][2] * m64[2][0])
        + m64[0][2] * (m64[1][0] * m64[2][1] - m64[1][1] * m64[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let c = |i: usize, j: usize| {
        let (a, b) = ((i + 1) % 3, (i + 2) % 3);
        let (p, q) = ((j + 1) % 3, (j + 2) % 3);
        m64[a][p] * m64[b][q] - m64[a][q] * m64[b][p]
    };
    let mut out = [[0.0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (c(j, i) / det) as f32;
        }
    }
    Some(out)
}

impl Plan {
    /// The plan for an illuminant `xy` in a working space whose RGB to XYZ
    /// (D50) matrix is `rgb_to_xyz`.
    pub fn new(xy: [f32; 2], rgb_to_xyz: &M3, gamut: f32, clip: bool) -> Result<Plan, String> {
        let identity: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        Ok(Plan {
            rgb_to_lms: mul(&XYZ_TO_CAT16_LMS, rgb_to_xyz),
            illuminant: illuminant_lms(xy),
            mix_to_xyz: mul(&CAT16_LMS_TO_XYZ, &identity),
            xyz_to_rgb: invert(rgb_to_xyz).ok_or("the working space's matrix is singular")?,
            compression: if gamut == 0.0 { 0.0 } else { 1.0 / gamut },
            clip,
        })
    }

    /// One pixel, darktable's `_loop_switch` on its CAT16 path.
    pub fn pixel(&self, rgb: [f32; 3]) -> [f32; 3] {
        let min = if self.clip { 0.0 } else { f32::MIN };
        let clipneg = |v: [f32; 3]| {
            if self.clip { v.map(|c| c.max(0.0)) } else { v }
        };
        let lms = apply(&self.rgb_to_lms, rgb.map(|c| c.max(min)));
        let adapted = [0, 1, 2].map(|c| lms[c] * D50_LMS[c] / self.illuminant[c]);
        let xyz = clipneg(apply(&self.mix_to_xyz, adapted));
        let xyz = gamut_mapping(xyz, self.compression, self.clip);
        let lms = clipneg(apply(&XYZ_TO_CAT16_LMS, xyz));
        // Colourfulness and brightness would apply here, in LMS.
        let lms = clipneg(lms);
        let xyz = clipneg(apply(&CAT16_LMS_TO_XYZ, lms));
        clipneg(apply(&self.xyz_to_rgb, xyz))
    }
}

/// darktable's `_gamut_mapping`: compress the chromaticity towards D50 in
/// u'v' by `Y * distance^compression`, never past D50.
pub fn gamut_mapping(xyz: [f32; 3], compression: f32, clip: bool) -> [f32; 3] {
    let sum = xyz[0] + xyz[1] + xyz[2];
    let y_lum = xyz[1];
    let (x, y) = if sum > 0.0 {
        (xyz[0] / sum, xyz[1] / sum)
    } else {
        (D50_XY[0], D50_XY[1])
    };
    let den = -2.0 * x + 12.0 * y + 3.0;
    let mut uv = [4.0 * x / den, 9.0 * y / den];
    let delta = [D50_UV[0] - uv[0], D50_UV[1] - uv[1]];
    let distance = y_lum * (delta[0] * delta[0] + delta[1] * delta[1]);
    let correction = if compression == 0.0 {
        0.0
    } else {
        distance.powf(compression)
    };
    for c in 0..2 {
        let tmp = correction.mul_add(delta[c], uv[c]);
        uv[c] = if uv[c] > D50_UV[c] {
            tmp.max(D50_UV[c])
        } else {
            tmp.min(D50_UV[c])
        };
    }
    let den = 6.0 * uv[0] - 16.0 * uv[1] + 12.0;
    let mut xy = [9.0 * uv[0] / den, 4.0 * uv[1] / den];
    if clip {
        xy = xy.map(|v| v.max(0.0));
    }
    xy[1] = xy[1].max(NORM_MIN);
    let scale = xy[0] + xy[1];
    if scale >= 1.0 {
        xy = xy.map(|v| v / scale);
    }
    if xy[1] == 0.0 {
        return [0.0; 3];
    }
    [
        y_lum * xy[0] / xy[1],
        y_lum,
        y_lum * (1.0 - xy[0] - xy[1]) / xy[1],
    ]
}

/// darktable's `matrice_pseudoinverse` for a 3 x 3 matrix, in single
/// precision as darktable computes it: through the normal equations. Its
/// loops kept as darktable writes them, for the same rounding.
#[allow(clippy::needless_range_loop)]
fn pseudoinverse(m: &M3) -> M3 {
    let mut work = [[0.0f32; 6]; 3];
    for i in 0..3 {
        for j in 0..6 {
            work[i][j] = if j == i + 3 { 1.0 } else { 0.0 };
        }
        for j in 0..3 {
            for row in m {
                work[i][j] += row[i] * row[j];
            }
        }
    }
    for i in 0..3 {
        let num = work[i][i];
        for v in &mut work[i] {
            *v /= num;
        }
        for k in 0..3 {
            if k == i {
                continue;
            }
            let num = work[k][i];
            for j in 0..6 {
                work[k][j] -= work[i][j] * num;
            }
        }
    }
    let mut out = [[0.0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            for k in 0..3 {
                *cell += work[j][k + 3] * m[i][k];
            }
        }
    }
    out
}

/// XYZ to Bradford LMS, as darktable writes it.
const XYZ_TO_BRADFORD_LMS: M3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// Bradford LMS to XYZ, as darktable writes it.
const BRADFORD_LMS_TO_XYZ: M3 = [
    [0.9870, -0.1471, 0.1600],
    [0.4323, 0.5184, 0.0493],
    [-0.0085, 0.0400, 0.9685],
];

/// D65 in Bradford LMS, as darktable precomputes it.
const BRADFORD_D65: [f32; 3] = [0.941238, 1.040633, 1.088932];

/// D50 in Bradford LMS, as darktable precomputes it.
const BRADFORD_D50: [f32; 3] = [0.996078, 1.020646, 0.818155];

/// darktable's `WB_coeffs_to_illuminant_xy`: the chromaticity of camera
/// white seen through coefficients `wb`, the matrix's D65 white taken to
/// D50 (linear Bradford). `cam_to_xyz` is as darktable's pseudo-inverse
/// gives it (indexed `[camera channel][XYZ]`).
pub fn wb_coeffs_to_illuminant_xy(cam_to_xyz: &M3, wb: [f32; 3]) -> [f32; 2] {
    let xyz = [0, 1, 2]
        .map(|c| cam_to_xyz[0][c] / wb[0] + cam_to_xyz[1][c] / wb[1] + cam_to_xyz[2][c] / wb[2]);
    let lms = apply(&XYZ_TO_BRADFORD_LMS, xyz);
    let lms = [0, 1, 2].map(|c| BRADFORD_D50[c] * (lms[c] / BRADFORD_D65[c]));
    let mut xyz = apply(&BRADFORD_LMS_TO_XYZ, lms);
    xyz[0] /= xyz[1];
    xyz[2] /= xyz[1];
    xyz[1] /= xyz[1];
    let sum = xyz[0] + xyz[1] + xyz[2];
    [xyz[0] / sum, xyz[1] / sum]
}

/// darktable's `_is_usable_illuminant_xy`: inside the triangle, and every
/// CAT16 cone response positive.
fn usable([x, y]: [f32; 2]) -> bool {
    let z = 1.0 - x - y;
    if !(x > 0.0 && y > 0.0 && z > 0.0) {
        return false;
    }
    illuminant_lms([x, y]).iter().all(|v| *v > 0.0)
}

/// The illuminant "as shot in camera": darktable's
/// `find_illuminant_xy_from_as_shot_coeffs` with no correction ratios (the
/// white balance in camera reference mode), for a camera whose XYZ (D65)
/// to camera matrix is `xyz_to_camera` (row by row) and as-shot
/// coefficients `wb` (any scale). `None` where darktable finds none.
pub fn as_shot_illuminant_xy(xyz_to_camera: &[f64], wb: &[f64]) -> Option<[f32; 2]> {
    let wb: [f32; 3] = [wb.first()?, wb.get(1)?, wb.get(2)?].map(|c| *c as f32);
    if !wb.iter().all(|c| c.is_normal()) || xyz_to_camera.len() < 9 {
        return None;
    }
    let m: M3 = [0, 1, 2].map(|i| [0, 1, 2].map(|j| xyz_to_camera[i * 3 + j] as f32));
    let cam_to_xyz = pseudoinverse(&m);
    if !cam_to_xyz.iter().flatten().all(|v| v.is_finite()) {
        return None;
    }
    Some(wb_coeffs_to_illuminant_xy(&cam_to_xyz, wb)).filter(|xy| usable(*xy))
}

/// The illuminant "as shot in camera" from the context: the camera's
/// as-shot coefficients and its matrix.
pub fn camera_illuminant_xy(context: &Context) -> Option<[f32; 2]> {
    let list = |field: &str| context.get(field).and_then(|v| v.as_list());
    as_shot_illuminant_xy(list(fields::XYZ_TO_CAMERA)?, list(fields::WHITE_BALANCE)?)
}

/// darktable's `xy_to_CCT` (after Lee's paper): valid in 3000 to 50000 K.
/// Its constants as darktable writes them.
#[allow(clippy::excessive_precision)]
pub fn xy_to_cct([x, y]: [f32; 2]) -> f32 {
    let n = (x - 0.3366) / (y - 0.1735);
    -949.86315
        + 6253.80338 * (-n / 0.92159).exp()
        + 28.70599 * (-n / 0.20039).exp()
        + 0.00004 * (-n / 0.07125).exp()
}

/// darktable's `CCT_reverse_lookup`: the temperature on the daylight
/// (above 4000 K) or black body locus closest to `xy`.
pub fn cct_reverse_lookup([x, y]: [f32; 2]) -> f32 {
    const SAMPLES: usize = 1 << 16;
    let (t_min, t_max) = (1667.0f32, 25000.0f32);
    let mut best = (f32::MAX, 0.0f32);
    for i in 0..SAMPLES {
        let step = (i as f32 / (SAMPLES - 1) as f32).powf(4.0);
        let t = t_min + step * (t_max - t_min);
        let [xb, yb] = if t >= 4000.0 {
            daylight_xy(t)
        } else {
            blackbody_xy(t)
        };
        let r = ((xb - x) * (xb - x) + (yb - y) * (yb - y)).sqrt();
        if r < best.0 {
            best = (r, t);
        }
    }
    best.1
}

/// darktable's `xy_to_uv`, CIE 1960 (its one double-precision term kept).
fn xy_to_uv([x, y]: [f32; 2]) -> [f32; 2] {
    let denom = 12.0 * y - 1.882 * x + 2.9088;
    let u = (f64::from(5.5932f32 * x) + 1.9116 * f64::from(y)) as f32;
    [u / denom, 7.8972 * y / denom]
}

/// darktable's `_check_if_close_to_daylight`: the illuminant's correlated
/// temperature, and `d` or `bb` when the daylight or black body locus at
/// that temperature is within 0.005 in CIE 1960 uv, `custom` otherwise.
pub fn close_to_daylight(xy: [f32; 2]) -> (f32, &'static str) {
    let mut t = xy_to_cct(xy);
    if t < 3000.0 && t > 1667.0 {
        t = cct_reverse_lookup(xy);
    }
    let uv = xy_to_uv(xy);
    let delta = |test: [f32; 2]| {
        let test = xy_to_uv(test);
        ((test[0] - uv[0]).powi(2) + (test[1] - uv[1]).powi(2)).sqrt()
    };
    let daylight = Some(daylight_xy(t))
        .filter(|xy| xy[0] != 0.0 && xy[1] != 0.0)
        .unwrap_or_else(|| blackbody_xy(t));
    let (delta_daylight, delta_bb) = (delta(daylight), delta(blackbody_xy(t)));
    let kind = if delta_bb < 0.005 || delta_daylight < 0.005 {
        if delta_bb < delta_daylight { "bb" } else { "d" }
    } else {
        "custom"
    };
    (t, kind)
}

impl Module for ChannelMixerRgb {
    type Plan = Plan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "channelmixerrgb".into(),
            version: 1,
            description:
                "Colour calibration: adapt the colours from the scene's illuminant to D50 (CAT16)"
                    .into(),
            params: vec![
                choice(
                    "illuminant",
                    "the scene's illuminant: D50 (pipe), a standard one, daylight or black body at a temperature, custom x and y, or as shot in camera (camera)",
                    &ILLUMINANTS,
                    "d",
                ),
                choice(
                    "illum_fluo",
                    "the fluorescent illuminant (illuminant f)",
                    &FLUORESCENTS,
                    "f3",
                ),
                choice(
                    "illum_led",
                    "the LED illuminant (illuminant led)",
                    &LEDS,
                    "b5",
                ),
                choice("adaptation", "the chromatic adaptation", &[CAT16], CAT16),
                ParamSpec::float(
                    "x",
                    "the illuminant's CIE x (custom illuminant)",
                    0.0,
                    1.0,
                    0.333,
                ),
                ParamSpec::float(
                    "y",
                    "the illuminant's CIE y (custom illuminant)",
                    0.0,
                    1.0,
                    0.333,
                ),
                ParamSpec::float(
                    "temperature",
                    "the illuminant's temperature in kelvin (daylight and black body)",
                    1667.0,
                    25000.0,
                    5003.0,
                ),
                ParamSpec::float("gamut", "gamut compression: 0 for none", 0.0, 12.0, 1.0),
                ParamSpec {
                    name: "clip".into(),
                    description: "clip negative RGB from gamut".into(),
                    kind: ParamKind::Bool { default: true },
                },
            ],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![fields::WHITE_BALANCE.into(), fields::XYZ_TO_CAMERA.into()],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Color,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<Plan, String> {
        match params.get("adaptation").and_then(Value::as_choice) {
            Some(CAT16) | None => {}
            Some(other) => return Err(format!("adaptation `{other}` is not supported")),
        }
        let xy = illuminant_xy(params, context)?;
        let clip = params.get("clip").and_then(Value::as_bool).unwrap_or(true);
        Plan::new(
            xy,
            &rec2020_to_xyz_d50(),
            params.float("gamut") as f32,
            clip,
        )
    }

    fn process(
        &self,
        plan: &Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        for p in data.as_chunks_mut::<4>().0 {
            let out = plan.pixel([p[0], p[1], p[2]]);
            p[..3].copy_from_slice(&out);
        }
        Ok(vec![Image::new(output, 4, data)])
    }
}

#[cfg(test)]
mod tests;
