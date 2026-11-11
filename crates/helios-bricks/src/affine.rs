//! `core.affine@1`: the per-pixel affine step of exposure and levels.

use helios_core::{
    BrickFamily, BrickPort, BrickSpec, Contract, Family, GpuKernel, Image, Manifest, Module,
    ModuleKind, Needs, ParamSpec, Params, PortSpec, PortType, Region, Role, Shape, Space,
    Tolerance,
};

/// How far `origin` and `scale` may go: far enough for any derivation
/// (exposure's origin reaches 2^18), near enough that a huge pixel times
/// both stays a finite single-precision number.
const REACH: f64 = 1.0e15;

/// `out = (in - origin) * scale` on the colour channels, the fourth kept.
pub struct Affine;

/// The parameters in the precision of pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AffinePlan {
    pub origin: f32,
    pub scale: f32,
}

impl Module for Affine {
    type Plan = AffinePlan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "core.affine".into(),
            version: 1,
            description: "Shift then scale the colour channels".into(),
            params: vec![
                ParamSpec::float("origin", "the value that maps to zero", -REACH, REACH, 0.0),
                ParamSpec::float("scale", "the factor after the shift", -REACH, REACH, 1.0),
            ],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Tone,
            role: Role::General,
        }
    }

    fn brick(&self) -> Option<BrickSpec> {
        Some(BrickSpec {
            id: "core.affine".into(),
            major: 1,
            family: BrickFamily::Tone,
            shape: Shape::Pointwise,
            // The CPU form; the GPU form is keyed apart, as every GPU result.
            tolerance: Tolerance::Bitwise,
            purpose: "Shift then scale the colour channels of each pixel".into(),
            description: "out = (in - origin) * scale on the first three channels, in single \
                          precision; the fourth channel is passed through untouched."
                .into(),
            use_when: "an exposure, a black level, a levels step: any change that maps one \
                       value to zero and scales the rest"
                .into(),
            avoid_when: "the shift or scale differs per channel (not in this major)".into(),
            keywords: ["exposure", "black", "levels", "gain", "offset"]
                .map(String::from)
                .to_vec(),
            provenance: "darktable's exposure (src/iop/exposure.c), its per-pixel step".into(),
            inputs: vec![BrickPort::new(
                "input",
                PortType::Image {
                    channels: 4,
                    space: Space::Any,
                },
                "the pixels to change",
            )],
            outputs: vec![BrickPort::new(
                "output",
                PortType::Image {
                    channels: 4,
                    space: Space::SameAs("input".into()),
                },
                "the pixels changed, in the input's space",
            )],
        })
    }

    fn provides(&self) -> Vec<Contract> {
        vec![Contract::Brick]
    }

    fn prepare(&self, params: &Params, _: &helios_core::Context) -> Result<AffinePlan, String> {
        Ok(AffinePlan {
            origin: params.float("origin") as f32,
            scale: params.float("scale") as f32,
        })
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: include_str!("affine.wgsl"),
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, plan: &AffinePlan) -> Vec<f32> {
        vec![plan.origin, plan.scale]
    }

    fn process(
        &self,
        plan: &AffinePlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        for pixel in data.as_chunks_mut::<4>().0 {
            for v in &mut pixel[..3] {
                *v = (*v - plan.origin) * plan.scale;
            }
        }
        Ok(vec![Image::new(output, 4, data)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::{Context, Value};

    #[test]
    fn it_shifts_then_scales_and_keeps_the_fourth_channel() {
        let params = Affine
            .manifest()
            .resolve(
                &Params::new()
                    .with("origin", Value::Float(0.1))
                    .with("scale", Value::Float(2.0)),
            )
            .unwrap();
        let plan = Affine.prepare(&params, &Context::new()).unwrap();
        let input = Image::new(Region::sized(1, 1), 4, vec![0.1, 0.6, 1.1, 0.3]);
        let out = Affine
            .process(&plan, &[&input], Region::sized(1, 1))
            .unwrap();
        assert_eq!(out[0].data(), &[0.0, 1.0, 2.0, 0.3]);
    }

    #[test]
    fn contract() {
        helios_testkit::assert_contract(Affine);
    }
}
