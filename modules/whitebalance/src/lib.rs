//! White balance, ported from darktable's `temperature` module
//! (`src/iop/temperature.c`), coefficient modes.
//!
//! darktable balances the mosaic, before demosaicing: each photosite is
//! multiplied by the coefficient of its filter color, the coefficients
//! normalized so that green is 1. Here too, with three modes:
//!
//! - **as shot**: the camera's coefficients, from its description (a
//!   declared context input);
//! - **camera reference**: the coefficients that make a D65 white neutral,
//!   from the camera's matrix (darktable's `_calculate_bogus_daylight_wb`,
//!   its "camera reference" preset). darktable's scene-referred default
//!   balances this way and leaves the scene's illuminant to colour
//!   calibration (`channelmixerrgb`, illuminant "as shot in camera"). A
//!   camera without a usable matrix falls back to as shot, as in darktable;
//! - **user**: red, green and blue given as parameters.
//!
//! Not ported yet: setting the balance by temperature and tint (it needs
//! the camera's spectral data) and the camera's presets.

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamKind, ParamSpec, Params,
    PortSpec, Region, Role, Value,
};
use helios_photo::Cfa;
use helios_photo::camera::fields;
use helios_photo::cfa::Channel;

pub struct WhiteBalance;

pub const AS_SHOT: &str = "as_shot";
pub const USER: &str = "user";
pub const CAMERA_REFERENCE: &str = "camera_reference";

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// Red, green, blue, green normalized to 1.
    pub coeffs: [f32; 3],
    pub cfa: Cfa,
}

fn coefficient(name: &str, about: &str) -> ParamSpec {
    ParamSpec::float(name, about, 0.0, 8.0, 1.0)
}

/// The camera's as-shot coefficients, green normalized to 1.
pub fn as_shot(context: &Context) -> Result<[f64; 3], String> {
    let wb = context
        .get(fields::WHITE_BALANCE)
        .and_then(|v| v.as_list())
        .filter(|l| l.len() >= 3)
        .ok_or_else(|| {
            format!(
                "context field `{}`: the camera's coefficients needed",
                fields::WHITE_BALANCE
            )
        })?;
    if !(wb[1].is_finite() && wb[1] > 0.0) {
        return Err(format!("the camera's green coefficient is {}", wb[1]));
    }
    Ok([wb[0] / wb[1], 1.0, wb[2] / wb[1]])
}

/// darktable's sRGB (D65) to XYZ, as `dt_colorspaces_conversion_matrices_rgb`
/// writes it.
const SRGB_TO_XYZ: [[f64; 3]; 3] = [
    [0.412453, 0.357580, 0.180423],
    [0.212671, 0.715160, 0.072169],
    [0.019334, 0.119193, 0.950227],
];

/// The coefficients that make a D65 white neutral for a camera whose
/// XYZ (D65) to camera matrix is `xyz_to_camera` (row by row), green
/// normalized to 1: darktable's `dt_colorspaces_conversion_matrices_rgb`
/// multipliers, the inverse of each row sum of camera RGB from sRGB.
/// `None` for a matrix that gives no finite, positive coefficients.
pub fn d65_coefficients(xyz_to_camera: &[f64]) -> Option<[f64; 3]> {
    let m: [f64; 9] = xyz_to_camera.try_into().ok()?;
    let mul: [f64; 3] = [0, 1, 2].map(|i| {
        let sum: f64 = (0..3)
            .map(|j| {
                (0..3)
                    .map(|k| m[i * 3 + k] * SRGB_TO_XYZ[k][j])
                    .sum::<f64>()
            })
            .sum();
        1.0 / sum
    });
    let wb = [mul[0] / mul[1], 1.0, mul[2] / mul[1]];
    wb.iter().all(|c| c.is_finite() && *c > 0.0).then_some(wb)
}

/// The camera's D65 coefficients from the context, `None` without a
/// usable matrix.
pub fn camera_reference(context: &Context) -> Option<[f64; 3]> {
    context
        .get(fields::XYZ_TO_CAMERA)
        .and_then(|v| v.as_list())
        .and_then(d65_coefficients)
}

impl Module for WhiteBalance {
    type Plan = Plan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "whitebalance".into(),
            version: 1,
            description: "Balance the colors of the sensor data (on the mosaic)".into(),
            params: vec![
                ParamSpec {
                    name: "mode".into(),
                    description: "the camera's as-shot coefficients, its D65 reference (camera_reference, for colour calibration to adapt), or the user's".into(),
                    kind: ParamKind::Choice {
                        options: vec![AS_SHOT.into(), USER.into(), CAMERA_REFERENCE.into()],
                        default: AS_SHOT.into(),
                    },
                },
                coefficient("red", "red coefficient (user mode)"),
                coefficient("green", "green coefficient (user mode)"),
                coefficient("blue", "blue coefficient (user mode)"),
            ],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![
                fields::WHITE_BALANCE.into(),
                fields::CFA.into(),
                fields::XYZ_TO_CAMERA.into(),
            ],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Color,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<Plan, String> {
        let cfa = context
            .get(fields::CFA)
            .and_then(|v| v.as_choice())
            .ok_or_else(|| format!("context field `{}`: a filter pattern needed", fields::CFA))?;
        let cfa = Cfa::parse(cfa)?;
        let coeffs = match params.get("mode") {
            Some(Value::Choice(m)) if m == USER => {
                let g = params.float("green");
                if g <= 0.0 {
                    return Err("the green coefficient must be above 0".into());
                }
                [params.float("red") / g, 1.0, params.float("blue") / g]
            }
            Some(Value::Choice(m)) if m == CAMERA_REFERENCE => match camera_reference(context) {
                Some(wb) => wb,
                None => as_shot(context)?,
            },
            _ => as_shot(context)?,
        };
        Ok(Plan {
            coeffs: coeffs.map(|c| c as f32),
            cfa,
        })
    }

    fn process(
        &self,
        plan: &Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            let c = match plan.cfa.color_at(x, y) {
                Channel::Red => 0,
                Channel::Green => 1,
                Channel::Blue => 2,
            };
            input.pixel(x, y)[0] * plan.coeffs[c]
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        Context::new()
            .with(fields::WHITE_BALANCE, Value::List(vec![4.0, 2.0, 3.0, 2.0]))
            .with(fields::CFA, Value::Choice("RGGB".into()))
            .with(fields::XYZ_TO_CAMERA, Value::List(NIKON_D750.to_vec()))
    }

    /// The Nikon D750's XYZ to camera matrix (Adobe's, as darktable has it).
    const NIKON_D750: [f64; 9] = [
        0.9020, -0.2890, -0.0715, -0.4535, 1.2436, 0.2348, -0.0934, 0.1919, 0.7086,
    ];

    fn run(params: Params) -> Vec<f32> {
        let m = WhiteBalance.manifest();
        let plan = WhiteBalance
            .prepare(&m.resolve(&params).unwrap(), &context())
            .unwrap();
        let block = Image::new(Region::sized(2, 2), 1, vec![0.25; 4]);
        WhiteBalance
            .process(&plan, &[&block], Region::sized(2, 2))
            .unwrap()[0]
            .data()
            .to_vec()
    }

    #[test]
    fn as_shot_uses_the_cameras_coefficients_green_at_one() {
        // RGGB: red x2, greens x1, blue x1.5.
        assert_eq!(run(Params::new()), [0.5, 0.25, 0.25, 0.375]);
    }

    #[test]
    fn user_mode_takes_the_parameters_normalized_to_green() {
        let p = Params::new()
            .with("mode", Value::Choice(USER.into()))
            .with("red", Value::Float(2.0))
            .with("green", Value::Float(2.0))
            .with("blue", Value::Float(4.0));
        assert_eq!(run(p), [0.25, 0.25, 0.25, 0.5]);
    }

    #[test]
    fn camera_reference_matches_darktables_d65_coefficients() {
        // darktable's own multipliers for this matrix, from the colour
        // calibration module's C harness (`dt_as_shot.c`).
        let wb = d65_coefficients(&NIKON_D750).unwrap();
        for (got, want) in wb.iter().zip([2.177946990998, 1.0, 1.221341278271]) {
            assert!((got - want).abs() < 1e-6, "{wb:?}");
        }
        let p = Params::new().with("mode", Value::Choice(CAMERA_REFERENCE.into()));
        let out = run(p);
        assert!((out[0] / 0.25 - wb[0] as f32).abs() < 1e-5, "{out:?}");
        assert_eq!(out[1], 0.25);
        assert!((out[3] / 0.25 - wb[2] as f32).abs() < 1e-5, "{out:?}");
    }

    #[test]
    fn camera_reference_without_a_matrix_falls_back_to_as_shot() {
        let ctx = Context::new()
            .with(fields::WHITE_BALANCE, Value::List(vec![4.0, 2.0, 3.0, 2.0]))
            .with(fields::CFA, Value::Choice("RGGB".into()))
            .with(fields::XYZ_TO_CAMERA, Value::List(vec![0.0; 9]));
        let p = WhiteBalance
            .manifest()
            .resolve(&Params::new().with("mode", Value::Choice(CAMERA_REFERENCE.into())))
            .unwrap();
        let plan = WhiteBalance.prepare(&p, &ctx).unwrap();
        assert_eq!(plan.coeffs, [2.0, 1.0, 1.5]);
    }

    #[test]
    fn a_missing_balance_is_named() {
        let ctx = Context::new().with(fields::CFA, Value::Choice("RGGB".into()));
        let err = WhiteBalance.prepare(&Params::new(), &ctx).unwrap_err();
        assert!(err.contains("photo.white_balance"), "{err}");
    }

    #[test]
    fn it_keeps_the_module_contract() {
        helios_testkit::assert_contract_in(WhiteBalance, &context());
    }
}
