//! A mask member made from the image: a range of luminance.

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamSpec, Params, PortSpec,
    Region, Role,
};

use crate::mask_port;

/// Luminance of linear Rec.2020 RGB, the working space.
pub fn luminance(rgb: &[f32]) -> f32 {
    0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2]
}

/// A mask rising from 0 at luminance `low` to 1 at `high` (a step at `low`
/// when `high` is not above it). Its `output` is its input unchanged, the
/// same buffer: a module with a second output, `mask`, as a module that
/// makes its own mask would have.
pub struct MaskFromLuma;

impl Module for MaskFromLuma {
    /// `low` and `high`.
    type Plan = (f32, f32);

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "mask_from_luma".into(),
            version: 1,
            description: "A mask from a range of luminance; passes the image through".into(),
            params: vec![
                ParamSpec::float("low", "luminance where the mask starts", 0.0, 4.0, 0.1),
                ParamSpec::float("high", "luminance where the mask is full", 0.0, 4.0, 0.5),
            ],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4), mask_port("mask")],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Mask,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<(f32, f32), String> {
        Ok((params.float("low") as f32, params.float("high") as f32))
    }

    fn process(
        &self,
        &(low, high): &(f32, f32),
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        let mask = Image::from_fn(output, 1, |x, y, _| {
            let l = luminance(input.pixel(x, y));
            if high > low {
                ((l - low) / (high - low)).clamp(0.0, 1.0)
            } else if l >= low {
                1.0
            } else {
                0.0
            }
        });
        Ok(vec![input.sample(output), mask])
    }
}
