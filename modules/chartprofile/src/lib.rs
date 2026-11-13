//! Chart profile: a colour chart calibration applied to a photo
//! (`docs/design/calibration.md`, section 7).
//!
//! It runs right after `colorin`, on the working RGB that `colorin` gives,
//! and turns it into what the profile's matrix gives:
//!
//! - `M_s`, the standard matrix: `colorin`'s, from the camera's
//!   description (a declared context input); the identity for a picture,
//!   which has no camera description.
//! - `M_c`, the profile's matrix (camera RGB to the working space): its
//!   `matrix` parameter, copied from the profile when it is applied. A
//!   module does no IO: the profile is never read at render, and its id
//!   (`profile`) is kept for provenance and the header only.
//! - The plan is `P = M_c M_s⁻¹`, applied by `core.matrix3@1`'s step
//!   ([`helios_bricks::matrix3`]), CPU and GPU.
//!
//! Its context also names the camera (make, model) and its as-shot white
//! balance, never read at render: the module's header shows which camera
//! the photo is of, and the camera's default profile is picked by the
//! light the white balance gives.
//!
//! An all-zero `matrix` (the default) is no profile: the pixels pass
//! unchanged. The template puts the module bypassed after `colorin`, so a
//! render without a profile is unchanged and darktable parity holds.
//!
//! Not built yet: the residual LUT (`lut` in the profile, a later slice).

use helios_bricks::matrix3::{self, Matrix3Plan};
use helios_core::{
    Context, Family, GpuKernel, Image, Manifest, Module, ModuleKind, Needs, ParamKind, ParamSpec,
    Params, PortSpec, Region, Role, Value,
};
use helios_photo::camera::fields;
use helios_photo::color::{self, IDENTITY, Mat3};

/// The module's name.
pub const NAME: &str = "chartprofile";

/// How far a coefficient of `M_c` may go.
const REACH: f64 = 1.0e3;

/// A profile id's length at most (a BLAKE3 hash in hex is 64).
const ID_CAPACITY: u32 = 128;

pub struct ChartProfile;

/// The plan `P = M_c M_s⁻¹`, `M_c` and `M_s` row by row in `f64`, `M_s`
/// given (`colorin`'s matrix, the identity for a picture).
///
/// # Errors
/// `M_s` cannot be inverted.
pub fn correction(m_c: &Mat3, m_s: &Mat3) -> Result<Mat3, String> {
    let inverse = color::inverse(m_s).ok_or("the standard matrix cannot be inverted")?;
    Ok(color::mul(m_c, &inverse))
}

/// `M_s` for a context: `colorin`'s matrix from the camera's
/// description, the identity without one.
///
/// # Errors
/// The camera's matrix is there but unusable.
pub fn standard(context: &Context) -> Result<Mat3, String> {
    let Some(list) = context.get(fields::XYZ_TO_CAMERA).and_then(Value::as_list) else {
        return Ok(IDENTITY);
    };
    let m = <[f64; 9]>::try_from(list).map_err(|_| {
        format!(
            "context field `{}`: the camera's 3 x 3 matrix needed",
            fields::XYZ_TO_CAMERA
        )
    })?;
    color::camera_to_working(&m).ok_or_else(|| "the camera's matrix cannot be inverted".into())
}

/// A matrix row by row, as the `matrix` parameter holds it.
pub fn flat(m: &Mat3) -> Vec<f64> {
    m.iter().flatten().copied().collect()
}

impl Module for ChartProfile {
    type Plan = Matrix3Plan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: NAME.into(),
            version: 1,
            description: "Apply a colour chart calibration profile after input colour".into(),
            params: vec![
                ParamSpec {
                    name: "matrix".into(),
                    description: "the profile's matrix, camera RGB to the working space, row \
                                  by row; all zero for none"
                        .into(),
                    kind: ParamKind::Floats {
                        count: 9,
                        min: -REACH,
                        max: REACH,
                        default: vec![0.0; 9],
                    },
                },
                ParamSpec {
                    name: "profile".into(),
                    description: "the profile's id, for its name and provenance".into(),
                    kind: ParamKind::Text {
                        capacity: ID_CAPACITY,
                        default: String::new(),
                    },
                },
            ],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            // The camera and its as-shot white balance are never read at
            // render: the header names the camera, and the camera's
            // default profile is picked by the as-shot light, from them.
            context: vec![
                fields::XYZ_TO_CAMERA.into(),
                fields::MAKER.into(),
                fields::MODEL.into(),
                fields::WHITE_BALANCE.into(),
            ],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Color,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<Matrix3Plan, String> {
        let m = params
            .get("matrix")
            .and_then(Value::as_list)
            .and_then(|l| <[f64; 9]>::try_from(l).ok())
            .ok_or("`matrix`: 9 numbers, row by row")?;
        if m.iter().all(|v| *v == 0.0) {
            return Ok(matrix3::plan(&matrix3::IDENTITY).expect("9 numbers"));
        }
        let m_c: Mat3 = [0, 1, 2].map(|r| [0, 1, 2].map(|c| m[3 * r + c]));
        let p = correction(&m_c, &standard(context)?)?;
        Ok(matrix3::plan(&flat(&p)).expect("9 numbers"))
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: matrix3::WGSL,
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, plan: &Matrix3Plan) -> Vec<f32> {
        matrix3::uniforms(plan)
    }

    fn process(
        &self,
        plan: &Matrix3Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        matrix3::apply(plan, &mut data);
        Ok(vec![Image::new(output, 4, data)])
    }
}

#[cfg(test)]
mod tests;
