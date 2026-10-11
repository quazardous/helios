//! Exposure, ported from darktable's `exposure` module (`src/iop/exposure.c`),
//! manual mode.
//!
//! darktable computes `out = (in - black) * scale` with
//! `white = 2^-exposure` and `scale = 1 / (white - black)`, pulling `black`
//! just below `white` when it would reach it. The same is done here.
//!
//! With `compensate_bias`, the camera's exposure bias (from its description,
//! a declared context input) is taken back, as darktable's "compensate
//! camera exposure": a shot at +1 EV gets -1 EV.
//!
//! Not ported yet: the deflicker mode (an analysis input: the raw
//! histogram) and highlight preservation.
//!
//! One difference: darktable also scales the fourth channel, which its
//! pixelpipe carries but does not use; here it is passed through untouched.

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamKind, ParamSpec, Params,
    PortSpec, Region, Role, Value,
};
use helios_photo::camera::fields::EXPOSURE_BIAS;

pub struct Exposure;

/// What `prepare` computes from the parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub black: f32,
    pub scale: f32,
}

impl Module for Exposure {
    type Plan = Plan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "exposure".into(),
            version: 1,
            description: "Scale the light by a number of stops and set the black level".into(),
            params: vec![
                ParamSpec::float("exposure", "exposure adjustment, in EV", -18.0, 18.0, 0.0),
                ParamSpec::float("black", "black level correction", -1.0, 1.0, 0.0),
                ParamSpec {
                    name: "compensate_bias".into(),
                    description: "take back the camera's exposure bias".into(),
                    kind: ParamKind::Bool { default: false },
                },
            ],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![EXPOSURE_BIAS.into()],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Tone,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<Plan, String> {
        let mut exposure = params.float("exposure");
        if params.get("compensate_bias") == Some(&Value::Bool(true)) {
            let bias = context
                .get(EXPOSURE_BIAS)
                .and_then(|v| v.as_f64())
                .ok_or_else(|| {
                    format!("context field `{EXPOSURE_BIAS}`: the camera's bias needed")
                })?;
            exposure -= bias;
        }
        let white = (-exposure as f32).exp2();
        let mut black = params.float("black") as f32;
        if black >= white {
            black = white - 0.01;
        }
        Ok(Plan {
            black,
            scale: 1.0 / (white - black),
        })
    }

    fn process(
        &self,
        plan: &Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        for pixel in data.as_chunks_mut::<4>().0 {
            for v in &mut pixel[..3] {
                *v = (*v - plan.black) * plan.scale;
            }
        }
        Ok(vec![Image::new(output, 4, data)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::Value;

    fn plan(exposure: f64, black: f64) -> Plan {
        let params = Params::new()
            .with("exposure", Value::Float(exposure))
            .with("black", Value::Float(black));
        Exposure
            .prepare(
                &Exposure.manifest().resolve(&params).unwrap(),
                &Context::new(),
            )
            .unwrap()
    }

    fn apply(plan: &Plan, rgba: [f32; 4]) -> Vec<f32> {
        let input = Image::new(Region::sized(1, 1), 4, rgba.to_vec());
        Exposure
            .process(plan, &[&input], Region::sized(1, 1))
            .unwrap()[0]
            .data()
            .to_vec()
    }

    #[test]
    fn defaults_are_the_identity() {
        assert_eq!(
            apply(&plan(0.0, 0.0), [0.1, 0.5, 2.0, 0.7]),
            [0.1, 0.5, 2.0, 0.7]
        );
    }

    #[test]
    fn one_stop_doubles_the_light_and_keeps_the_fourth_channel() {
        assert_eq!(
            apply(&plan(1.0, 0.0), [0.1, 0.5, 2.0, 0.7]),
            [0.2, 1.0, 4.0, 0.7]
        );
        assert_eq!(
            apply(&plan(-2.0, 0.0), [0.4, 1.0, 2.0, 0.7]),
            [0.1, 0.25, 0.5, 0.7]
        );
    }

    #[test]
    fn black_maps_to_zero_and_white_to_one() {
        let p = plan(0.0, 0.1);
        let out = apply(&p, [0.1, 1.0, 0.55, 0.0]);
        assert_eq!(out[0], 0.0);
        assert!((out[1] - 1.0).abs() < 1e-6);
        assert!((out[2] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_cameras_bias_is_taken_back_when_asked() {
        let params = Exposure
            .manifest()
            .resolve(&Params::new().with("compensate_bias", Value::Bool(true)))
            .unwrap();
        let shot_over = Context::new().with(EXPOSURE_BIAS, Value::Float(1.0));
        let p = Exposure.prepare(&params, &shot_over).unwrap();
        assert_eq!(apply(&p, [0.4, 0.4, 0.4, 1.0]), [0.2, 0.2, 0.2, 1.0]);
        let err = Exposure.prepare(&params, &Context::new()).unwrap_err();
        assert!(err.contains(EXPOSURE_BIAS), "{err}");
        // Off by default: the bias changes nothing.
        let off = Exposure.manifest().resolve(&Params::new()).unwrap();
        assert_eq!(Exposure.prepare(&off, &shot_over).unwrap(), plan(0.0, 0.0));
    }

    #[test]
    fn black_is_kept_below_white_like_darktable() {
        // exposure 1 EV puts white at 0.5; a black of 0.8 is pulled to 0.49.
        let p = plan(1.0, 0.8);
        assert!((p.black - 0.49).abs() < 1e-6);
        assert!(p.scale.is_finite() && p.scale > 0.0);
    }

    #[test]
    fn contract() {
        helios_testkit::assert_contract(Exposure);
    }
}
