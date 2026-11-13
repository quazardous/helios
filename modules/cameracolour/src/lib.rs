//! Camera look, colour half (`docs/design/camera-look.md`, step 3a): the
//! matrix and the hue/saturation table a camera look fitted to the raw's
//! embedded JPEG, applied right after `colorin`, where a chart profile
//! acts.
//!
//! - `matrix`: working RGB to working RGB, keeping the neutral, row by
//!   row (the identity by default).
//! - `lut`: the hue/saturation table after the matrix, in DNG's layout
//!   (`core.huesat@1`'s `table`); empty for none.
//! - `amount`: 0 to 1, the matrix and the table blended toward the
//!   identity (`I + a (P - I)`, hue shifts times `a`, saturation scales
//!   `1 + a (s - 1)`); 1 is the fit as it is.
//! - `fitter`, `preview`: the fitter's version and the preview's hash,
//!   for provenance and the header; never read at render. A module does
//!   no IO: the fit runs elsewhere and its numbers are copied here.
//!
//! The pixel step is `core.huesat@1`'s ([`helios_bricks::huesat`]), CPU
//! and GPU. The templates put the module bypassed after `colorin` (and
//! after `chartprofile`), so a render without a look is unchanged; with a
//! chart profile on, the colour half stays off (the chart rules the
//! colour), which is the edit's business, not the render's.

use helios_bricks::huesat::{self, HueSatPlan};
use helios_bricks::matrix3;
use helios_core::{
    Context, Family, GpuKernel, Image, Manifest, Module, ModuleKind, Needs, ParamKind, ParamSpec,
    Params, PortSpec, Region, Role, Value,
};

/// The module's name.
pub const NAME: &str = "cameracolour";

/// How far a matrix coefficient may go (the fit bounds them by 8).
const REACH: f64 = 1.0e3;

/// How far a number of the table may go.
const LUT_REACH: f64 = 1.0e3;

/// A preview hash's length at most (a BLAKE3 hash in hex is 64).
const ID_CAPACITY: u32 = 128;

/// The newest fitter version a look may name.
const MAX_FITTER: i64 = 1 << 20;

pub struct CameraColour;

/// `matrix` blended toward the identity by `amount`.
pub fn blend_matrix(m: &[f64; 9], amount: f64) -> [f64; 9] {
    std::array::from_fn(|i| matrix3::IDENTITY[i] + amount * (m[i] - matrix3::IDENTITY[i]))
}

/// A `lut` parameter blended toward the identity by `amount`: hue shifts
/// times `amount`, saturation scales `1 + amount (s - 1)`, the neutral
/// kept. An empty table stays empty.
pub fn blend_lut(lut: &[f64], amount: f64) -> Vec<f64> {
    lut.iter()
        .enumerate()
        .map(|(i, &v)| match i {
            0..3 => v,
            _ if (i - 3).is_multiple_of(2) => amount * v,
            _ => 1.0 + amount * (v - 1.0),
        })
        .collect()
}

impl Module for CameraColour {
    type Plan = HueSatPlan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: NAME.into(),
            version: 1,
            description: "Apply a camera look's colours, fitted to the camera's JPEG, after \
                          input colour"
                .into(),
            params: vec![
                ParamSpec {
                    name: "matrix".into(),
                    description: "the look's matrix, working RGB to working RGB, row by row".into(),
                    kind: ParamKind::Floats {
                        count: 9,
                        min: -REACH,
                        max: REACH,
                        default: matrix3::IDENTITY.to_vec(),
                    },
                },
                ParamSpec {
                    name: "lut".into(),
                    description: "the look's hue and saturation table: its neutral, then each \
                                  node's hue shift (degrees) and saturation scale, 72 hues x 5 \
                                  saturations x 5 values; empty for none"
                        .into(),
                    kind: ParamKind::Floats {
                        count: huesat::TABLE_LEN as u32,
                        min: -LUT_REACH,
                        max: LUT_REACH,
                        default: Vec::new(),
                    },
                },
                ParamSpec {
                    name: "amount".into(),
                    description: "how much of the look's colours: 0 none, 1 all".into(),
                    kind: ParamKind::Float {
                        min: 0.0,
                        max: 1.0,
                        default: 1.0,
                    },
                },
                ParamSpec {
                    name: "fitter".into(),
                    description: "the version of the fitter that made the look, for \
                                  provenance; 0 for none"
                        .into(),
                    kind: ParamKind::Int {
                        min: 0,
                        max: MAX_FITTER,
                        default: 0,
                    },
                },
                ParamSpec {
                    name: "preview".into(),
                    description: "the hash of the camera's JPEG the look was fitted to, for \
                                  provenance"
                        .into(),
                    kind: ParamKind::Text {
                        capacity: ID_CAPACITY,
                        default: String::new(),
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

    fn prepare(&self, params: &Params, _: &Context) -> Result<HueSatPlan, String> {
        let m = params
            .get("matrix")
            .and_then(Value::as_list)
            .and_then(|l| <[f64; 9]>::try_from(l).ok())
            .ok_or("`matrix`: 9 numbers, row by row")?;
        let amount = params.get("amount").and_then(Value::as_f64).unwrap_or(1.0);
        let lut = params.get("lut").and_then(Value::as_list).unwrap_or(&[]);
        let (m, lut) = if amount == 1.0 {
            (m, lut.to_vec())
        } else {
            (blend_matrix(&m, amount), blend_lut(lut, amount))
        };
        Ok(HueSatPlan {
            matrix: matrix3::plan(&m).expect("9 numbers"),
            table: huesat::table(&lut).map_err(|e| e.replace("`table`", "`lut`"))?,
        })
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: huesat::WGSL,
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, plan: &HueSatPlan) -> Vec<f32> {
        huesat::uniforms(plan)
    }

    fn process(
        &self,
        plan: &HueSatPlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        huesat::apply(plan, &mut data);
        Ok(vec![Image::new(output, 4, data)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(params: Params, data: Vec<f32>) -> Vec<f32> {
        let params = CameraColour.manifest().resolve(&params).unwrap();
        let plan = CameraColour.prepare(&params, &Context::new()).unwrap();
        let w = (data.len() / 4) as u32;
        let input = Image::new(Region::sized(w, 1), 4, data);
        let out = CameraColour
            .process(&plan, &[&input], Region::sized(w, 1))
            .unwrap();
        out[0].data().to_vec()
    }

    /// A table turning every coloured node by `shift` degrees and scaling
    /// its saturation by `scale`.
    fn uniform_lut(shift: f64, scale: f64) -> Vec<f64> {
        let mut flat = vec![1.0, 1.0, 1.0];
        for i in 0..huesat::NODES {
            if i.is_multiple_of(huesat::SATURATIONS) {
                flat.extend([0.0, 1.0]);
            } else {
                flat.extend([shift, scale]);
            }
        }
        flat
    }

    const BEND: [f64; 9] = [1.1, -0.06, -0.04, -0.03, 1.05, -0.02, 0.01, -0.08, 1.07];

    #[test]
    fn its_defaults_change_no_pixel() {
        let data = vec![0.25, 0.5, 2.0, 0.3, -0.1, 0.0, 0.4, 1.0];
        assert_eq!(run(Params::new(), data.clone()), data);
    }

    #[test]
    fn it_renders_as_core_huesat() {
        let lut = uniform_lut(8.0, 1.2);
        let data = vec![0.25, 0.5, 0.1, 0.3, 0.6, 0.2, 0.4, 1.0];
        let params = Params::new()
            .with("matrix", Value::List(BEND.to_vec()))
            .with("lut", Value::List(lut.clone()));
        let mut reference = data.clone();
        huesat::apply(
            &HueSatPlan {
                matrix: matrix3::plan(&BEND).unwrap(),
                table: huesat::table(&lut).unwrap(),
            },
            &mut reference,
        );
        assert_eq!(run(params, data), reference);
    }

    #[test]
    fn amount_zero_is_the_identity_and_a_half_is_between() {
        let data = vec![0.6, 0.3, 0.1, 1.0];
        let with = |a: f64| {
            run(
                Params::new()
                    .with("matrix", Value::List(BEND.to_vec()))
                    .with("lut", Value::List(uniform_lut(8.0, 1.2)))
                    .with("amount", Value::Float(a)),
                data.clone(),
            )
        };
        let (none, half, all) = (with(0.0), with(0.5), with(1.0));
        for c in 0..3 {
            assert!((none[c] - data[c]).abs() < 1e-6, "{none:?}");
            let (lo, hi) = (data[c].min(all[c]), data[c].max(all[c]));
            assert!(half[c] > lo - 1e-6 && half[c] < hi + 1e-6, "{half:?}");
        }
        assert_ne!(half, all);
    }

    #[test]
    fn a_bad_lut_is_refused_by_its_name() {
        let params = CameraColour
            .manifest()
            .resolve(&Params::new().with("lut", Value::List(vec![1.0; 5])))
            .unwrap();
        let err = CameraColour.prepare(&params, &Context::new()).unwrap_err();
        assert!(err.contains("`lut`"), "{err}");
    }

    #[test]
    fn contract() {
        helios_testkit::assert_contract(CameraColour);
    }
}
