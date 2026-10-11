//! Applying a module through a mask: darktable's blending in normal mode.

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamHint, ParamSpec, Params,
    PortSpec, Region, Role,
};

use crate::mask_port;

/// `input + (processed - input) * mask * opacity`, on every channel: where
/// the mask is 0 the module's input shows, where it is 1 its output.
pub struct MaskBlend;

impl Module for MaskBlend {
    /// The global opacity.
    type Plan = f32;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "mask_blend".into(),
            version: 1,
            description: "Mixes a module's input and output through a mask".into(),
            params: vec![ParamSpec::float(
                "opacity",
                "multiplies the mask",
                0.0,
                1.0,
                1.0,
            )],
            inputs: vec![
                PortSpec::image("input", 4),
                PortSpec::image("processed", 4),
                mask_port("mask"),
            ],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Mask,
            role: Role::General,
        }
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        vec![ParamHint::new("opacity").unit("%").display(100.0, 0)]
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<f32, String> {
        Ok(params.float("opacity") as f32)
    }

    fn process(
        &self,
        opacity: &f32,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let (input, processed, mask) = (inputs[0], inputs[1], inputs[2]);
        Ok(vec![Image::from_fn(output, 4, |x, y, c| {
            let c = c as usize;
            let m = mask.pixel(x, y)[0].clamp(0.0, 1.0) * opacity;
            let a = input.pixel(x, y)[c];
            a + (processed.pixel(x, y)[c] - a) * m
        })])
    }
}
