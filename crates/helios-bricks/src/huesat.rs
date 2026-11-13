//! `core.huesat@1`: a 3×3 matrix on the colour channels of each pixel,
//! then a hue/saturation table in DNG's `ProfileHueSatMap` layout, the
//! luminance kept: the step of a chart profile with its residual table.
//!
//! The table is [`HUES`] hues × [`SATURATIONS`] saturations × [`VALUES`]
//! values, stored value by value, then hue by hue, then saturation by
//! saturation; each node a hue shift in degrees and a saturation scale
//! (DNG's value scale is left out: the luminance is restored after the
//! table, so it would change nothing). Hue, saturation and value are the
//! hexcone HSV of the pixel divided by the table's neutral channel by
//! channel, so the neutral has saturation 0; the value axis reads the
//! scene-linear value as [`value_coordinate`]. The pixel's luminance
//! (Rec.2020's, [`LUMA`]) is restored after the table.
//!
//! The CPU form ([`apply`]) is the reference, in single precision; the
//! GPU form computes the same formula.

use helios_core::{
    BrickFamily, BrickPort, BrickSpec, Contract, Family, GpuKernel, Image, Manifest, Module,
    ModuleKind, Needs, ParamKind, ParamSpec, Params, PortSpec, PortType, Region, Role, Shape,
    Space, Tolerance,
};

use crate::matrix3::{self, Matrix3Plan};

/// Hue divisions, every 5 degrees.
pub const HUES: usize = 72;
/// Saturation divisions, from 0 to 1.
pub const SATURATIONS: usize = 5;
/// Value divisions, from 0 to 1 in [`value_coordinate`].
pub const VALUES: usize = 5;
/// The table's nodes.
pub const NODES: usize = HUES * SATURATIONS * VALUES;
/// The `table` parameter's length: the neutral (3 numbers), then per node
/// its hue shift and saturation scale. An empty `table` is none.
pub const TABLE_LEN: usize = 3 + 2 * NODES;

/// Rec.2020's luminance weights: the working space's.
pub const LUMA: [f32; 3] = [0.262_700_2, 0.677_998_1, 0.059_301_7];

/// How far a number of the table may go.
const REACH: f64 = 1.0e3;

/// How far a matrix coefficient may go, as `core.matrix3`'s.
const MATRIX_REACH: f64 = 1.0e6;

/// The GPU form, for modules that apply a profile the same way.
pub const WGSL: &str = include_str!("huesat.wgsl");

/// A table in the precision of pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// The neutral the table is read against, each channel above 0.
    pub neutral: [f32; 3],
    /// `[hue shift (degrees), saturation scale]` per node, index
    /// `(value × HUES + hue) × SATURATIONS + saturation`.
    pub nodes: Vec<[f32; 2]>,
}

/// The matrix, then the table when there is one.
#[derive(Clone, Debug, PartialEq)]
pub struct HueSatPlan {
    pub matrix: Matrix3Plan,
    pub table: Option<Table>,
}

/// `core.huesat@1`.
pub struct HueSat;

/// The table a `table` parameter holds: `None` when it is empty.
///
/// # Errors
/// Not [`TABLE_LEN`] numbers, a neutral channel not above 0, or a number
/// that is not finite.
pub fn table(flat: &[f64]) -> Result<Option<Table>, String> {
    if flat.is_empty() {
        return Ok(None);
    }
    if flat.len() != TABLE_LEN {
        return Err(format!(
            "`table`: {TABLE_LEN} numbers (the neutral, then a hue shift and a saturation \
             scale per node), or none; {} given",
            flat.len()
        ));
    }
    if !flat.iter().all(|v| v.is_finite()) {
        return Err("`table`: a number is not finite".into());
    }
    let neutral = [flat[0], flat[1], flat[2]];
    if !neutral.iter().all(|&c| c > 0.0) {
        return Err("`table`: the neutral's channels must be above 0".into());
    }
    let nodes = flat[3..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|n| [n[0] as f32, n[1] as f32])
        .collect();
    Ok(Some(Table {
        neutral: neutral.map(|c| c as f32),
        nodes,
    }))
}

/// A scene-linear HSV value (0 and up) on the table's value axis (0 to 1):
/// `sqrt(v / (1 + v))`.
pub fn value_coordinate(v: f32) -> f32 {
    let v = v.max(0.0);
    (v / (1.0 + v)).sqrt()
}

/// `h` taken into [0, 360].
fn wrap(h: f32) -> f32 {
    h - 360.0 * (h / 360.0).floor()
}

fn luma(p: [f32; 3]) -> f32 {
    LUMA[0] * p[0] + LUMA[1] * p[1] + LUMA[2] * p[2]
}

/// Hexcone HSV: hue in degrees, saturation, value.
fn hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let c = max - min;
    if !(max > 0.0) || !(c > 0.0) {
        return [0.0, 0.0, max];
    }
    let h = if max == r {
        60.0 * ((g - b) / c)
    } else if max == g {
        60.0 * ((b - r) / c + 2.0)
    } else {
        60.0 * ((r - g) / c + 4.0)
    };
    [wrap(h), c / max, max]
}

fn from_hsv([h, s, v]: [f32; 3]) -> [f32; 3] {
    let c = v * s;
    let hp = wrap(h) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

/// The cell of `x` (clamped to [0, 1]) on an axis of `n` divisions: its
/// lower division and the fraction past it.
fn cell(x: f32, n: usize) -> (usize, f32) {
    let x = x.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = (x.floor() as usize).min(n - 2);
    (i, x - i as f32)
}

impl Table {
    /// The hue shift and saturation scale at hue `h` (degrees), saturation
    /// `s` and value coordinate `vc`: trilinear, the hue wrapping round,
    /// saturation and value clamped to the grid.
    pub fn lookup(&self, h: f32, s: f32, vc: f32) -> [f32; 2] {
        let hx = wrap(h) / (360.0 / HUES as f32);
        let h0 = (hx.floor() as usize) % HUES;
        let fh = hx - hx.floor();
        let h1 = (h0 + 1) % HUES;
        let (s0, fs) = cell(s, SATURATIONS);
        let (v0, fv) = cell(vc, VALUES);
        let mut out = [0.0f32; 2];
        for (v, wv) in [(v0, 1.0 - fv), (v0 + 1, fv)] {
            for (hh, wh) in [(h0, 1.0 - fh), (h1, fh)] {
                for (ss, ws) in [(s0, 1.0 - fs), (s0 + 1, fs)] {
                    let w = wv * wh * ws;
                    let n = self.nodes[(v * HUES + hh) * SATURATIONS + ss];
                    out[0] += w * n[0];
                    out[1] += w * n[1];
                }
            }
        }
        out
    }

    /// The table on one pixel, its luminance kept; a pixel without
    /// luminance or colour passes unchanged.
    pub fn pixel(&self, p: [f32; 3]) -> [f32; 3] {
        let n = self.neutral;
        let l = luma(p);
        if !(l > 0.0) {
            return p;
        }
        let [h, s, v] = hsv([p[0] / n[0], p[1] / n[1], p[2] / n[2]]);
        if !(s > 0.0) {
            return p;
        }
        let [dh, scale] = self.lookup(h, s, value_coordinate(v));
        let r = from_hsv([h + dh, s * scale, v]);
        let o = [r[0] * n[0], r[1] * n[1], r[2] * n[2]];
        let lo = luma(o);
        if !(lo > 0.0) {
            return p;
        }
        let k = l / lo;
        o.map(|c| c * k)
    }
}

/// The plan on the first three channels of 4-channel pixels, the fourth
/// kept: the CPU reference. Without a table it is `core.matrix3`'s step,
/// bit for bit.
pub fn apply(plan: &HueSatPlan, data: &mut [f32]) {
    matrix3::apply(&plan.matrix, data);
    if let Some(t) = &plan.table {
        for p in data.as_chunks_mut::<4>().0 {
            let o = t.pixel([p[0], p[1], p[2]]);
            p[..3].copy_from_slice(&o);
        }
    }
}

/// The plan as the GPU kernel's uniforms, one `vec4` each: the matrix's
/// three rows, the neutral with 1 when there is a table (0 without), then
/// two nodes per `vec4` (zeros without a table).
pub fn uniforms(plan: &HueSatPlan) -> Vec<f32> {
    let mut u = matrix3::uniforms(&plan.matrix);
    match &plan.table {
        Some(t) => {
            u.extend(t.neutral);
            u.push(1.0);
            u.extend(t.nodes.iter().flatten());
        }
        None => u.resize(16 + 2 * NODES, 0.0),
    }
    u
}

impl Module for HueSat {
    type Plan = HueSatPlan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "core.huesat".into(),
            version: 1,
            description: "Multiply the colour channels by a 3 x 3 matrix, then apply a hue and \
                          saturation table"
                .into(),
            params: vec![
                ParamSpec {
                    name: "matrix".into(),
                    description: "the matrix, row by row".into(),
                    kind: ParamKind::Floats {
                        count: 9,
                        min: -MATRIX_REACH,
                        max: MATRIX_REACH,
                        default: matrix3::IDENTITY.to_vec(),
                    },
                },
                ParamSpec {
                    name: "table".into(),
                    description: "the neutral, then each node's hue shift (degrees) and \
                                  saturation scale, 72 hues x 5 saturations x 5 values; empty \
                                  for none"
                        .into(),
                    kind: ParamKind::Floats {
                        count: TABLE_LEN as u32,
                        min: -REACH,
                        max: REACH,
                        default: Vec::new(),
                    },
                },
            ],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Color,
            role: Role::General,
        }
    }

    fn brick(&self) -> Option<BrickSpec> {
        Some(BrickSpec {
            id: "core.huesat".into(),
            major: 1,
            family: BrickFamily::Colour,
            shape: Shape::Pointwise,
            tolerance: Tolerance::Relative { max: 1.0e-4 },
            purpose: "Mix the colour channels by a 3 x 3 matrix, then move hues and saturations \
                      by a table"
                .into(),
            description: "out = T(M · in): M given row by row, T a hue/saturation table in DNG's \
                          ProfileHueSatMap layout (72 x 5 x 5) read in HSV against a neutral, \
                          the luminance kept; in single precision, the fourth channel passed \
                          through untouched."
                .into(),
            use_when: "a chart profile with its residual table, a camera look's colour stage: \
                       a correction a matrix alone cannot make, by hue and saturation"
                .into(),
            avoid_when: "a linear mix is enough (core.matrix3), or the lightness must change"
                .into(),
            keywords: [
                "matrix",
                "hue",
                "saturation",
                "table",
                "profile",
                "calibration",
                "dcp",
            ]
            .map(String::from)
            .to_vec(),
            provenance: "Adobe DNG's ProfileHueSatMap, as dcamprof fits it".into(),
            inputs: vec![BrickPort::new(
                "input",
                PortType::Image {
                    channels: 4,
                    space: Space::Rec2020Linear,
                },
                "the pixels to correct",
            )],
            outputs: vec![BrickPort::new(
                "output",
                PortType::Image {
                    channels: 4,
                    space: Space::Rec2020Linear,
                },
                "the pixels corrected",
            )],
        })
    }

    fn provides(&self) -> Vec<Contract> {
        vec![Contract::Brick]
    }

    fn prepare(&self, params: &Params, _: &helios_core::Context) -> Result<HueSatPlan, String> {
        let matrix = params
            .get("matrix")
            .and_then(|v| v.as_list())
            .and_then(matrix3::plan)
            .ok_or("`matrix`: 9 numbers, row by row")?;
        let table = table(params.get("table").and_then(|v| v.as_list()).unwrap_or(&[]))?;
        Ok(HueSatPlan { matrix, table })
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: WGSL,
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, plan: &HueSatPlan) -> Vec<f32> {
        uniforms(plan)
    }

    fn process(
        &self,
        plan: &HueSatPlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        apply(plan, &mut data);
        Ok(vec![Image::new(output, 4, data)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::{Context, Value};

    /// A table turning every hue by `shift` degrees and scaling every
    /// saturation by `scale`, the zero-saturation row the identity.
    pub(crate) fn uniform_table(shift: f64, scale: f64) -> Vec<f64> {
        let mut flat = vec![1.0, 1.0, 1.0];
        for _v in 0..VALUES {
            for _h in 0..HUES {
                for s in 0..SATURATIONS {
                    if s == 0 {
                        flat.extend([0.0, 1.0]);
                    } else {
                        flat.extend([shift, scale]);
                    }
                }
            }
        }
        flat
    }

    fn run(params: Params, data: Vec<f32>) -> Vec<f32> {
        let params = HueSat.manifest().resolve(&params).unwrap();
        let plan = HueSat.prepare(&params, &Context::new()).unwrap();
        let w = (data.len() / 4) as u32;
        let input = Image::new(Region::sized(w, 1), 4, data);
        let out = HueSat
            .process(&plan, &[&input], Region::sized(w, 1))
            .unwrap();
        out[0].data().to_vec()
    }

    #[test]
    fn without_a_table_it_is_the_matrix_bit_for_bit() {
        let m = vec![2.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, -1.0];
        let data = vec![0.25, 0.5, 1.0, 0.3, 0.7, -0.2, 3.0, 1.0];
        let out = run(
            Params::new().with("matrix", Value::List(m.clone())),
            data.clone(),
        );
        let mut reference = data;
        matrix3::apply(&matrix3::plan(&m).unwrap(), &mut reference);
        assert_eq!(out, reference);
    }

    #[test]
    fn the_identity_table_changes_nothing_but_rounding_and_keeps_neutrals() {
        let data = vec![0.2, 0.5, 0.1, 1.0, 0.4, 0.4, 0.4, 0.5];
        let out = run(
            Params::new().with("table", Value::List(uniform_table(0.0, 1.0))),
            data.clone(),
        );
        for (o, d) in out.iter().zip(&data) {
            assert!((o - d).abs() < 1e-6, "{out:?}");
        }
        // The grey is exactly kept.
        assert_eq!(&out[4..], &data[4..]);
    }

    #[test]
    fn a_hue_shift_turns_the_hue_and_keeps_the_luminance() {
        let p = [0.6f32, 0.2, 0.1];
        let out = run(
            Params::new().with("table", Value::List(uniform_table(10.0, 1.0))),
            vec![p[0], p[1], p[2], 1.0],
        );
        let before = hsv(p);
        let after = hsv([out[0], out[1], out[2]]);
        assert!(
            (after[0] - before[0] - 10.0).abs() < 1e-3,
            "{before:?} {after:?}"
        );
        assert!((luma([out[0], out[1], out[2]]) - luma(p)).abs() < 1e-6);
        assert_eq!(out[3], 1.0);
    }

    #[test]
    fn a_bad_table_is_refused() {
        assert!(table(&[1.0; 5]).is_err());
        let mut t = uniform_table(0.0, 1.0);
        t[0] = 0.0;
        assert!(table(&t).is_err());
        assert_eq!(table(&[]), Ok(None));
    }

    #[test]
    fn the_uniforms_have_one_size() {
        let none = HueSatPlan {
            matrix: matrix3::plan(&matrix3::IDENTITY).unwrap(),
            table: None,
        };
        let some = HueSatPlan {
            table: table(&uniform_table(5.0, 1.2)).unwrap(),
            ..none.clone()
        };
        assert_eq!(uniforms(&none).len(), uniforms(&some).len());
        assert_eq!(uniforms(&none).len() % 4, 0);
        // The WGSL's array: one vec4 per four numbers.
        assert!(WGSL.contains(&format!("array<vec4<f32>, {}>", uniforms(&none).len() / 4)));
    }

    #[test]
    fn contract() {
        helios_testkit::assert_contract(HueSat);
    }
}
