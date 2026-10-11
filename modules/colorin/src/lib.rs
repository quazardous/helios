//! Input color, ported from darktable's `colorin` module
//! (`src/iop/colorin.c`), standard matrix mode.
//!
//! After white balance and demosaicing, pixels are in the camera's own RGB.
//! darktable turns them into its working space (linear Rec.2020) with the
//! camera's color matrix, rows normalized so that camera white stays white
//! (the dcraw convention). The matrix comes from the camera's description,
//! a declared context input.
//!
//! Not ported yet: ICC input profiles, the "enhanced" embedded matrices of
//! some cameras, and gamut clipping.

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, Params, PortSpec, Region, Role,
};
use helios_photo::camera::fields;
use helios_photo::color::{self, Mat3};

pub struct ColorIn;

impl Module for ColorIn {
    type Plan = [[f32; 3]; 3];

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "colorin".into(),
            version: 1,
            description: "Turn camera RGB into the linear Rec.2020 working space".into(),
            params: vec![],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![fields::XYZ_TO_CAMERA.into()],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Color,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, context: &Context) -> Result<Self::Plan, String> {
        let m = context
            .get(fields::XYZ_TO_CAMERA)
            .and_then(|v| v.as_list())
            .and_then(|l| <[f64; 9]>::try_from(l).ok())
            .ok_or_else(|| {
                format!(
                    "context field `{}`: the camera's 3 x 3 matrix needed",
                    fields::XYZ_TO_CAMERA
                )
            })?;
        let to_working: Mat3 =
            color::camera_to_working(&m).ok_or("the camera's matrix cannot be inverted")?;
        Ok(to_working.map(|row| row.map(|v| v as f32)))
    }

    fn process(
        &self,
        m: &Self::Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        for p in data.as_chunks_mut::<4>().0 {
            let [r, g, b] = [p[0], p[1], p[2]];
            for (i, row) in m.iter().enumerate() {
                p[i] = row[0] * r + row[1] * g + row[2] * b;
            }
        }
        Ok(vec![Image::new(output, 4, data)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::Value;
    /// The Canon EOS 40D's matrix, as its raw files record it.
    fn context() -> Context {
        Context::new().with(
            fields::XYZ_TO_CAMERA,
            Value::List(vec![
                0.6071, -0.0747, -0.0856, -0.7653, 1.5365, 0.2441, -0.2025, 0.2553, 0.7315,
            ]),
        )
    }

    fn apply(rgb: [f32; 3]) -> [f32; 4] {
        let plan = ColorIn.prepare(&Params::new(), &context()).unwrap();
        let img = Image::new(Region::sized(1, 1), 4, vec![rgb[0], rgb[1], rgb[2], 0.5]);
        let out = ColorIn
            .process(&plan, &[&img], Region::sized(1, 1))
            .unwrap();
        out[0].data().try_into().unwrap()
    }

    #[test]
    fn camera_white_stays_white_and_alpha_is_kept() {
        let out = apply([1.0, 1.0, 1.0]);
        for v in &out[..3] {
            assert!((v - 1.0).abs() < 1e-5, "{out:?}");
        }
        assert_eq!(out[3], 0.5);
    }

    #[test]
    fn colors_move_and_the_matrix_round_trips() {
        let m = <[f64; 9]>::try_from(
            context()
                .get(fields::XYZ_TO_CAMERA)
                .unwrap()
                .as_list()
                .unwrap(),
        )
        .unwrap();
        let to_working = color::camera_to_working(&m).unwrap();
        let back = color::inverse(&to_working).unwrap();
        let camera = [0.6, 0.3, 0.2];
        let working = color::apply(&to_working, camera);
        let again = color::apply(&back, working);
        for (a, b) in camera.iter().zip(again) {
            assert!((a - b).abs() < 1e-9);
        }
        let out = apply([0.6, 0.3, 0.2]);
        assert!((f64::from(out[0]) - working[0]).abs() < 1e-5);
        assert_ne!(out[0], 0.6, "a real matrix changes colors");
    }

    #[test]
    fn a_missing_or_singular_matrix_is_named() {
        let err = ColorIn
            .prepare(&Params::new(), &Context::new())
            .unwrap_err();
        assert!(err.contains("photo.xyz_to_camera"), "{err}");
        let zero = Context::new().with(fields::XYZ_TO_CAMERA, Value::List(vec![0.0; 9]));
        assert!(ColorIn.prepare(&Params::new(), &zero).is_err());
    }

    #[test]
    fn it_keeps_the_module_contract() {
        helios_testkit::assert_contract_in(ColorIn, &context());
    }
}
