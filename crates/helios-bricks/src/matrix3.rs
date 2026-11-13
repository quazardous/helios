//! `core.matrix3@1`: a 3×3 matrix on the colour channels of each pixel,
//! the step of input colour, colour calibration and a chart profile.

use helios_core::{
    BrickFamily, BrickPort, BrickSpec, Contract, Family, GpuKernel, Image, Manifest, Module,
    ModuleKind, Needs, ParamKind, ParamSpec, Params, PortSpec, PortType, Region, Role, Shape,
    Space, Tolerance,
};

/// How far a coefficient may go: far beyond any colour matrix, near enough
/// that a pixel times three of them stays a finite single-precision number.
const REACH: f64 = 1.0e6;

/// The identity, row by row: the parameter's default.
pub const IDENTITY: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

/// The GPU form, for modules that apply a matrix the same way.
pub const WGSL: &str = include_str!("matrix3.wgsl");

/// A matrix in the precision of pixels, row by row.
pub type Matrix3Plan = [[f32; 3]; 3];

/// `out = M · in` on the colour channels, the fourth kept.
pub struct Matrix3;

/// The plan of a matrix given row by row.
pub fn plan(m: &[f64]) -> Option<Matrix3Plan> {
    let m = <&[f64; 9]>::try_from(m).ok()?;
    Some([0, 1, 2].map(|r| [0, 1, 2].map(|c| m[3 * r + c] as f32)))
}

/// `out = M · in` on the first three channels of 4-channel pixels, the
/// fourth kept: the CPU reference, which modules applying a matrix share.
pub fn apply(m: &Matrix3Plan, data: &mut [f32]) {
    for p in data.as_chunks_mut::<4>().0 {
        let [r, g, b] = [p[0], p[1], p[2]];
        for (i, row) in m.iter().enumerate() {
            p[i] = row[0] * r + row[1] * g + row[2] * b;
        }
    }
}

/// The plan as the GPU kernel's uniforms: one row per `vec4`.
pub fn uniforms(m: &Matrix3Plan) -> Vec<f32> {
    m.iter()
        .flat_map(|row| [row[0], row[1], row[2], 0.0])
        .collect()
}

impl Module for Matrix3 {
    type Plan = Matrix3Plan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "core.matrix3".into(),
            version: 1,
            description: "Multiply the colour channels by a 3 x 3 matrix".into(),
            params: vec![ParamSpec {
                name: "matrix".into(),
                description: "the matrix, row by row".into(),
                kind: ParamKind::Floats {
                    count: 9,
                    min: -REACH,
                    max: REACH,
                    default: IDENTITY.to_vec(),
                },
            }],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Color,
            role: Role::General,
        }
    }

    fn brick(&self) -> Option<BrickSpec> {
        Some(BrickSpec {
            id: "core.matrix3".into(),
            major: 1,
            family: BrickFamily::Colour,
            shape: Shape::Pointwise,
            tolerance: Tolerance::Bitwise,
            purpose: "Multiply the colour channels of each pixel by a 3 x 3 matrix".into(),
            description: "out = M · in on the first three channels, M given row by row, in \
                          single precision; the fourth channel is passed through untouched."
                .into(),
            use_when: "a change of colour space, a camera matrix, a chart profile's \
                       correction: any linear mix of the three channels"
                .into(),
            avoid_when: "the change is not linear in the channels (a curve, a gamut map)".into(),
            keywords: ["matrix", "colour", "colorin", "calibration", "mix"]
                .map(String::from)
                .to_vec(),
            provenance: "darktable's colorin and channelmixerrgb, their matrix step".into(),
            inputs: vec![BrickPort::new(
                "input",
                PortType::Image {
                    channels: 4,
                    space: Space::Any,
                },
                "the pixels to mix",
            )],
            outputs: vec![BrickPort::new(
                "output",
                PortType::Image {
                    channels: 4,
                    space: Space::Any,
                },
                "the pixels mixed: the matrix decides their space",
            )],
        })
    }

    fn provides(&self) -> Vec<Contract> {
        vec![Contract::Brick]
    }

    fn prepare(&self, params: &Params, _: &helios_core::Context) -> Result<Matrix3Plan, String> {
        params
            .get("matrix")
            .and_then(|v| v.as_list())
            .and_then(plan)
            .ok_or_else(|| "`matrix`: 9 numbers, row by row".into())
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: WGSL,
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, plan: &Matrix3Plan) -> Vec<f32> {
        uniforms(plan)
    }

    fn process(
        &self,
        plan: &Matrix3Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        apply(plan, &mut data);
        Ok(vec![Image::new(output, 4, data)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::{Context, Value};

    #[test]
    fn it_multiplies_the_colour_channels_and_keeps_the_fourth() {
        let params = Matrix3
            .manifest()
            .resolve(&Params::new().with(
                "matrix",
                Value::List(vec![2.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, -1.0]),
            ))
            .unwrap();
        let plan = Matrix3.prepare(&params, &Context::new()).unwrap();
        let input = Image::new(Region::sized(1, 1), 4, vec![0.25, 0.5, 1.0, 0.3]);
        let out = Matrix3
            .process(&plan, &[&input], Region::sized(1, 1))
            .unwrap();
        assert_eq!(out[0].data(), &[0.5, 0.375, -1.0, 0.3]);
    }

    #[test]
    fn the_default_is_the_identity() {
        let params = Matrix3.manifest().resolve(&Params::new()).unwrap();
        let plan = Matrix3.prepare(&params, &Context::new()).unwrap();
        let data = vec![0.1, 0.2, 0.3, 0.4, 5.0, -1.0, 0.0, 1.0];
        let input = Image::new(Region::sized(2, 1), 4, data.clone());
        let out = Matrix3
            .process(&plan, &[&input], Region::sized(2, 1))
            .unwrap();
        assert_eq!(out[0].data(), data.as_slice());
        assert_eq!(uniforms(&plan).len(), 12);
    }

    #[test]
    fn contract() {
        helios_testkit::assert_contract(Matrix3);
    }
}
