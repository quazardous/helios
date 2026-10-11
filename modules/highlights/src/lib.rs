//! Highlights, ported from darktable's `highlights` module
//! (`src/iop/highlights.c`), clip mode.
//!
//! On the balanced mosaic, a clipped photosite of a channel with a large
//! coefficient (red, blue) ends above a clipped green one: left alone, blown
//! highlights turn magenta. darktable's clip mode cuts every photosite at
//! `clip` times the smallest white-balance coefficient, so that clipped
//! areas stay neutral. The coefficients are the camera's (a declared context
//! input), normalized to green as the white balance does.
//!
//! Not ported yet: the reconstruction modes (inpaint opposed, segmentation,
//! laplacian), and following a user white balance (the threshold uses the
//! camera's coefficients).

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamSpec, Params, PortSpec,
    Region, Role,
};
use helios_photo::camera::fields;

pub struct Highlights;

/// The level every photosite is cut at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub threshold: f32,
}

impl Module for Highlights {
    type Plan = Plan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "highlights".into(),
            version: 1,
            description: "Keep blown highlights neutral (clip)".into(),
            params: vec![ParamSpec::float(
                "clip",
                "clipping threshold, relative to the sensor's white",
                0.0,
                2.0,
                1.0,
            )],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![fields::WHITE_BALANCE.into()],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Tone,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<Plan, String> {
        let wb = context
            .get(fields::WHITE_BALANCE)
            .and_then(|v| v.as_list())
            .filter(|l| l.len() >= 3 && l[1] > 0.0)
            .ok_or_else(|| {
                format!(
                    "context field `{}`: the camera's coefficients needed",
                    fields::WHITE_BALANCE
                )
            })?;
        let smallest = [wb[0] / wb[1], 1.0, wb[2] / wb[1]]
            .into_iter()
            .fold(f64::INFINITY, f64::min);
        Ok(Plan {
            threshold: (params.float("clip") * smallest) as f32,
        })
    }

    fn process(
        &self,
        plan: &Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let data = inputs[0]
            .data()
            .iter()
            .map(|v| v.min(plan.threshold))
            .collect();
        Ok(vec![Image::new(output, 1, data)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::Value;

    fn context(wb: [f64; 4]) -> Context {
        Context::new().with(fields::WHITE_BALANCE, Value::List(wb.to_vec()))
    }

    #[test]
    fn photosites_are_cut_at_the_smallest_coefficient() {
        // Red 2, green 1, blue 1.5: the smallest is green's, 1.
        let plan = Highlights
            .prepare(
                &Highlights.manifest().resolve(&Params::new()).unwrap(),
                &context([2.0, 1.0, 1.5, 1.0]),
            )
            .unwrap();
        let img = Image::new(Region::sized(3, 1), 1, vec![0.4, 1.5, 2.2]);
        let out = Highlights
            .process(&plan, &[&img], Region::sized(3, 1))
            .unwrap();
        assert_eq!(out[0].data(), &[0.4, 1.0, 1.0]);
    }

    #[test]
    fn a_missing_balance_is_named() {
        let err = Highlights
            .prepare(&Params::new(), &Context::new())
            .unwrap_err();
        assert!(err.contains("photo.white_balance"), "{err}");
    }

    #[test]
    fn it_keeps_the_module_contract() {
        helios_testkit::assert_contract_in(Highlights, &context([2.0, 1.0, 1.5, 1.0]));
    }
}
