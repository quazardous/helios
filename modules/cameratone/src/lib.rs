//! Camera look, tone half (`docs/design/camera-look.md`, step 3a): the
//! tone curve and the chroma curve a camera look fitted to the raw's
//! embedded JPEG, from scene-linear to display-linear, in the display
//! transform's place.
//!
//! - `tone`: the tone curve's knots from black, scene then display
//!   luminance (`core.curve1d@1`'s `tone`); empty for none.
//! - `chroma`: the chroma curve's 8 ratios over display luminance; empty
//!   for none.
//! - `amount`: 0 to 1, the curves blended toward the identity (each
//!   knot's display luminance `s + a (d - s)`, each chroma ratio
//!   `1 + a (k - 1)`); 1 is the fit as it is. At 0 the tone half only
//!   clamps to the display range, so a user can keep part of the
//!   camera's tone.
//! - `fitter`, `preview`, `held_out`, `baseline`: the fitter's version,
//!   the preview's hash and the fit's held-out mean Delta E 2000 against
//!   the default render's, for provenance and the header; never read at
//!   render. A module does no IO: the fit runs elsewhere and its numbers
//!   are copied here.
//!
//! The pixel step is `core.curve1d@1`'s ([`helios_bricks::curve1d`]), CPU
//! and GPU. The develop template puts the module bypassed right before
//! `sigmoid`; a look turned on turns this module on and `sigmoid` off, so
//! exposure still feeds the camera's curve.

use helios_bricks::curve1d::{self, Curve1dPlan};
use helios_core::{
    Context, Family, GpuKernel, Image, Manifest, Module, ModuleKind, Needs, ParamKind, ParamSpec,
    Params, PortSpec, Region, Role, Value,
};

/// The module's name.
pub const NAME: &str = "cameratone";

/// A preview hash's length at most (a BLAKE3 hash in hex is 64).
const ID_CAPACITY: u32 = 128;

/// The newest fitter version a look may name.
const MAX_FITTER: i64 = 1 << 20;

/// How far a reported Delta E may go.
const SCORE_REACH: f64 = 1.0e3;

pub struct CameraTone;

/// A `tone` parameter (knots, scene then display) blended toward the
/// identity by `amount`: each knot's display luminance `s + a (d - s)`,
/// its scene luminance kept, so the knots stay increasing. No knots stay
/// none.
pub fn blend_tone(tone: &[f64], amount: f64) -> Vec<f64> {
    tone.chunks(2)
        .flat_map(|k| match *k {
            [s, d] => vec![s, s + amount * (d - s)],
            _ => k.to_vec(),
        })
        .collect()
}

/// A `chroma` parameter blended toward the identity by `amount`: each
/// ratio `1 + a (k - 1)`. No curve stays none.
pub fn blend_chroma(chroma: &[f64], amount: f64) -> Vec<f64> {
    chroma.iter().map(|k| 1.0 + amount * (k - 1.0)).collect()
}

fn list(params: &Params, name: &str) -> Vec<f64> {
    params
        .get(name)
        .and_then(Value::as_list)
        .map(<[f64]>::to_vec)
        .unwrap_or_default()
}

impl Module for CameraTone {
    type Plan = Curve1dPlan;

    fn manifest(&self) -> Manifest {
        let score = |name: &str, description: &str| ParamSpec {
            name: name.into(),
            description: description.into(),
            kind: ParamKind::Float {
                min: 0.0,
                max: SCORE_REACH,
                default: 0.0,
            },
        };
        let mut params = curve1d::param_specs();
        params.extend([
            ParamSpec {
                name: "amount".into(),
                description: "how much of the look's tone: 0 none, 1 all".into(),
                kind: ParamKind::Float {
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                },
            },
            ParamSpec {
                name: "fitter".into(),
                description: "the version of the fitter that made the look, for provenance; \
                              0 for none"
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
            score(
                "held_out",
                "the look's mean Delta E 2000 to the camera's JPEG on held-out pixels",
            ),
            score(
                "baseline",
                "the default render's mean Delta E 2000 to the camera's JPEG on the same pixels",
            ),
        ]);
        Manifest {
            name: NAME.into(),
            version: 1,
            description: "Apply a camera look's tone and chroma curves, fitted to the camera's \
                          JPEG, in place of the display transform"
                .into(),
            params,
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Tone,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<Curve1dPlan, String> {
        let amount = params.get("amount").and_then(Value::as_f64).unwrap_or(1.0);
        let (tone, chroma) = (list(params, "tone"), list(params, "chroma"));
        if amount == 1.0 {
            return curve1d::plan(&tone, &chroma);
        }
        curve1d::plan(&blend_tone(&tone, amount), &blend_chroma(&chroma, amount))
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: curve1d::WGSL,
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, plan: &Curve1dPlan) -> Vec<f32> {
        curve1d::uniforms(plan)
    }

    fn process(
        &self,
        plan: &Curve1dPlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        curve1d::apply(plan, &mut data);
        Ok(vec![Image::new(output, 4, data)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(params: Params, data: Vec<f32>) -> Vec<f32> {
        let params = CameraTone.manifest().resolve(&params).unwrap();
        let plan = CameraTone.prepare(&params, &Context::new()).unwrap();
        let w = (data.len() / 4) as u32;
        let input = Image::new(Region::sized(w, 1), 4, data);
        let out = CameraTone
            .process(&plan, &[&input], Region::sized(w, 1))
            .unwrap();
        out[0].data().to_vec()
    }

    #[test]
    fn its_defaults_change_no_pixel() {
        let data = vec![0.25, 0.5, 2.0, 0.3, -0.1, 0.0, 0.4, 1.0];
        assert_eq!(run(Params::new(), data.clone()), data);
    }

    #[test]
    fn it_renders_as_core_curve1d_and_provenance_changes_nothing() {
        let tone = vec![0.0, 0.0, 0.05, 0.12, 0.3, 0.55, 1.2, 0.9];
        let chroma = vec![1.3, 1.2, 1.1, 1.0, 1.0, 0.9, 0.6, 0.3];
        let data = vec![0.25, 0.5, 0.1, 0.3, 2.0, 1.5, 0.4, 1.0];
        let set = Params::new()
            .with("tone", Value::List(tone.clone()))
            .with("chroma", Value::List(chroma.clone()));
        let mut reference = data.clone();
        curve1d::apply(&curve1d::plan(&tone, &chroma).unwrap(), &mut reference);
        assert_eq!(run(set.clone(), data.clone()), reference);
        let noted = set
            .with("fitter", Value::Int(2))
            .with("preview", Value::Text("ab12".into()))
            .with("held_out", Value::Float(2.4))
            .with("baseline", Value::Float(9.8));
        assert_eq!(run(noted, data), reference);
    }

    #[test]
    fn its_amount_blends_the_curves_toward_the_identity_in_one_parameter() {
        let tone = vec![0.0, 0.0, 0.1, 0.3, 0.5, 0.7, 1.0, 0.9];
        let chroma = vec![1.3, 1.2, 1.1, 1.0, 1.0, 0.9, 0.6, 0.3];
        let data = vec![0.05, 0.1, 0.08, 1.0, 0.4, 0.5, 0.3, 1.0];
        let set = Params::new()
            .with("tone", Value::List(tone.clone()))
            .with("chroma", Value::List(chroma.clone()));
        // At 1, the fit as it is.
        let full = run(set.clone().with("amount", Value::Float(1.0)), data.clone());
        assert_eq!(full, run(set.clone(), data.clone()));
        // At 0, the identity: the pixels kept (in the display range).
        let none = run(set.clone().with("amount", Value::Float(0.0)), data.clone());
        for (a, b) in none.iter().zip(&data) {
            assert!((a - b).abs() < 1e-5, "{none:?} against {data:?}");
        }
        // Halfway, the knots halfway, the chroma ratios halfway.
        let half = run(set.with("amount", Value::Float(0.5)), data.clone());
        let mut reference = data;
        curve1d::apply(
            &curve1d::plan(&blend_tone(&tone, 0.5), &blend_chroma(&chroma, 0.5)).unwrap(),
            &mut reference,
        );
        assert_eq!(half, reference);
        assert!((blend_tone(&tone, 0.5)[3] - 0.2).abs() < 1e-12);
        assert!((blend_chroma(&chroma, 0.5)[0] - 1.15).abs() < 1e-12);
        assert!(blend_tone(&[], 0.5).is_empty());
    }

    #[test]
    fn contract() {
        helios_testkit::assert_contract(CameraTone);
    }
}
