//! rawprepare, ported from darktable's `rawprepare` module
//! (`src/iop/rawprepare.c`): the first step on a raw mosaic.
//!
//! darktable computes `out = (in - black) / (white - black)` per photosite,
//! the black level chosen by the photosite's position in a 2 x 2 block
//! (`(row & 1) * 2 + (col & 1)`), so that each color of a Bayer pattern has
//! its own. The levels come from the camera's description, a declared
//! context input: the module never reads a file.
//!
//! Not ported yet: the user's overrides of the levels, the flat-field (gain
//! map) correction of DNG files, and the crop (done by the loader).

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, Params, PortSpec, Region, Role,
};
use helios_photo::camera::fields;

pub struct RawPrepare;

/// The levels of the 2 x 2 block, row by row, and the white level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub black: [f32; 4],
    pub white: f32,
}

impl Module for RawPrepare {
    type Plan = Plan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "rawprepare".into(),
            version: 1,
            description: "Subtract the sensor's black level and scale its data to [0, 1]".into(),
            params: vec![],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![fields::BLACK.into(), fields::WHITE.into()],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Io,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, context: &Context) -> Result<Plan, String> {
        let black = context
            .get(fields::BLACK)
            .and_then(|v| v.as_list())
            .and_then(|l| <[f64; 4]>::try_from(l).ok())
            .ok_or_else(|| {
                format!(
                    "context field `{}`: four black levels needed",
                    fields::BLACK
                )
            })?;
        let white = context
            .get(fields::WHITE)
            .and_then(helios_core::Value::as_f64)
            .ok_or_else(|| format!("context field `{}`: a white level needed", fields::WHITE))?;
        if black.iter().any(|b| *b >= white) {
            return Err(format!(
                "black levels {black:?} reach the white level {white}"
            ));
        }
        Ok(Plan {
            black: black.map(|b| b as f32),
            white: white as f32,
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
            let black = plan.black[((y & 1) * 2 + (x & 1)) as usize];
            (input.pixel(x, y)[0] - black) / (plan.white - black)
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::Value;

    fn context(black: [f64; 4], white: f64) -> Context {
        Context::new()
            .with(fields::BLACK, Value::List(black.to_vec()))
            .with(fields::WHITE, Value::Float(white))
    }

    #[test]
    fn each_photosite_loses_its_own_black_and_white_becomes_one() {
        let plan = RawPrepare
            .prepare(
                &Params::new(),
                &context([100.0, 110.0, 120.0, 130.0], 1100.0),
            )
            .unwrap();
        // A 2 x 2 block, every photosite at its black, then at white.
        let block = |v: [f32; 4]| Image::new(Region::sized(2, 2), 1, v.to_vec());
        let run = |img: &Image| {
            RawPrepare
                .process(&plan, &[img], Region::sized(2, 2))
                .unwrap()[0]
                .data()
                .to_vec()
        };
        assert_eq!(run(&block([100.0, 110.0, 120.0, 130.0])), [0.0; 4]);
        assert_eq!(run(&block([1100.0; 4])), [1.0; 4]);
    }

    #[test]
    fn the_block_follows_absolute_coordinates() {
        // A region starting on an odd column sees the levels shifted.
        let plan = RawPrepare
            .prepare(&Params::new(), &context([0.0, 100.0, 0.0, 100.0], 1100.0))
            .unwrap();
        let r = Region::new(1, 0, 2, 1);
        let img = Image::new(r, 1, vec![600.0, 600.0]);
        let out = RawPrepare.process(&plan, &[&img], r).unwrap();
        // x = 1 takes black 100: (600 - 100) / 1000; x = 2 takes 0: 600 / 1100.
        assert_eq!(out[0].data(), &[0.5, 600.0 / 1100.0]);
    }

    #[test]
    fn missing_or_impossible_levels_are_named() {
        let err = RawPrepare
            .prepare(&Params::new(), &Context::new())
            .unwrap_err();
        assert!(err.contains("photo.black"), "{err}");
        let err = RawPrepare
            .prepare(&Params::new(), &context([2000.0; 4], 1000.0))
            .unwrap_err();
        assert!(err.contains("reach the white level"), "{err}");
    }

    #[test]
    fn it_keeps_the_module_contract() {
        helios_testkit::assert_contract_in(RawPrepare, &context([0.05, 0.06, 0.05, 0.06], 0.95));
    }
}
