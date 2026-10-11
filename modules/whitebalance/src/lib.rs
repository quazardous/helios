//! White balance, ported from darktable's `temperature` module
//! (`src/iop/temperature.c`), coefficient modes.
//!
//! darktable balances the mosaic, before demosaicing: each photosite is
//! multiplied by the coefficient of its filter color, the coefficients
//! normalized so that green is 1. Here too, with two modes:
//!
//! - **as shot**: the camera's coefficients, from its description (a
//!   declared context input);
//! - **user**: red, green and blue given as parameters.
//!
//! Not ported yet: setting the balance by temperature and tint (it needs
//! the camera's spectral data), the camera's presets, and "camera
//! reference" (D65) mode.

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
                    description: "the camera's coefficients, or the user's".into(),
                    kind: ParamKind::Choice {
                        options: vec![AS_SHOT.into(), USER.into()],
                        default: AS_SHOT.into(),
                    },
                },
                coefficient("red", "red coefficient (user mode)"),
                coefficient("green", "green coefficient (user mode)"),
                coefficient("blue", "blue coefficient (user mode)"),
            ],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![fields::WHITE_BALANCE.into(), fields::CFA.into()],
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
    }

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
