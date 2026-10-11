//! Modules ported from vkdt (`src/pipe/modules/` of a vkdt checkout,
//! BSD-2): each a Rust CPU reference written from vkdt's GLSL, and a WGSL
//! variant translated from the same shader, checked against each other and
//! against `vkdt-cli` (`helios-parity`).
//!
//! - `vkdt_exposure`: `rgb * 2^ev`, alpha set to 1;
//! - `vkdt_usm`: unsharp mask. vkdt blurs with a "flower" of five bilinear
//!   taps (the centre, and four at (±1.2, ±0.4) and (∓0.4, ±1.2)), then
//!   boosts the difference to the centre by `amount` where it is under
//!   `thrs`. It reads two pixels around each output pixel.
//!
//! vkdt samples its textures with mirrored repeat at the image's edges;
//! here the framework replicates edge pixels, so the outer two pixels of
//! `vkdt_usm` may differ from vkdt's.

use helios_core::{
    Context, Family, GpuKernel, Image, Manifest, Module, ModuleKind, Needs, ParamSpec, Params,
    PortSpec, Region, Role,
};

fn manifest(
    name: &str,
    description: &str,
    params: Vec<ParamSpec>,
    overlap: u32,
    family: Family,
) -> Manifest {
    Manifest {
        name: name.into(),
        version: 1,
        description: description.into(),
        params,
        inputs: vec![PortSpec::image("input", 4)],
        outputs: vec![PortSpec::image("output", 4)],
        context: vec![],
        needs: Needs { overlap },
        kind: ModuleKind::Process,
        family,
        role: Role::General,
    }
}

pub struct VkdtExposure;

impl Module for VkdtExposure {
    /// The gain, 2^ev.
    type Plan = f32;

    fn manifest(&self) -> Manifest {
        manifest(
            "vkdt_exposure",
            "vkdt's exposure: scale the light by a number of stops",
            vec![ParamSpec::float(
                "exposure",
                "the exposure correction value in stops",
                -10.0,
                10.0,
                0.0,
            )],
            0,
            Family::Tone,
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<f32, String> {
        Ok((params.float("exposure") as f32).exp2())
    }

    fn process(&self, gain: &f32, inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        for pixel in data.as_chunks_mut::<4>().0 {
            for v in &mut pixel[..3] {
                *v *= gain;
            }
            pixel[3] = 1.0;
        }
        Ok(vec![Image::new(output, 4, data)])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: include_str!("exposure.wgsl"),
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, gain: &f32) -> Vec<f32> {
        vec![*gain]
    }
}

pub struct VkdtUsm;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsmPlan {
    pub amount: f32,
    pub thrs: f32,
}

/// The flower's side taps, in pixels.
const TAPS: [(f32, f32); 4] = [(1.2, 0.4), (-1.2, -0.4), (-0.4, 1.2), (0.4, -1.2)];
/// The centre's weight in the flower.
const CENTRE: f32 = 36.0 / 256.0;

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}

/// Bilinear at a position in pixel centres of `image` (image coordinates),
/// edges held.
fn bilinear(image: &Image, px: f32, py: f32, c: usize) -> f32 {
    let r = image.region();
    let at = |x: i64, y: i64| {
        let x = x.clamp(r.x, r.x + i64::from(r.width) - 1);
        let y = y.clamp(r.y, r.y + i64::from(r.height) - 1);
        image.pixel(x, y)[c]
    };
    let (x0, y0) = (px.floor(), py.floor());
    let (fx, fy) = (px - x0, py - y0);
    let (ix, iy) = (x0 as i64, y0 as i64);
    let top = mix(at(ix, iy), at(ix + 1, iy), fx);
    let bottom = mix(at(ix, iy + 1), at(ix + 1, iy + 1), fx);
    mix(top, bottom, fy)
}

impl Module for VkdtUsm {
    type Plan = UsmPlan;

    fn manifest(&self) -> Manifest {
        manifest(
            "vkdt_usm",
            "vkdt's unsharp mask: sharpen at the pixel level",
            vec![
                ParamSpec::float("amount", "how much to increase sharpness", 0.0, 3.0, 2.0),
                ParamSpec::float("thrs", "the threshold to not boost noise", 0.0, 0.3, 0.2),
            ],
            2,
            Family::Detail,
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<UsmPlan, String> {
        Ok(UsmPlan {
            amount: params.float("amount") as f32,
            thrs: params.float("thrs") as f32,
        })
    }

    fn process(
        &self,
        plan: &UsmPlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        let side = (1.0 - CENTRE) / 4.0;
        Ok(vec![Image::from_fn(output, 4, |x, y, c| {
            if c == 3 {
                return 1.0;
            }
            let c = c as usize;
            let centre = input.pixel(x, y)[c];
            let (fx, fy) = (x as f32, y as f32);
            let mut blur = CENTRE * centre;
            for (dx, dy) in TAPS {
                blur += side * bilinear(input, fx + dx, fy + dy, c);
            }
            let d = centre - blur;
            let gain = if d < plan.thrs { plan.amount } else { 1.0 };
            blur + gain * d
        })])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: include_str!("usm.wgsl"),
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, plan: &UsmPlan) -> Vec<f32> {
        vec![plan.amount, plan.thrs]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::Value;

    #[test]
    fn exposure_scales_by_two_to_the_stops_and_sets_alpha() {
        let params = Params::new().with("exposure", Value::Float(1.0));
        let gain = VkdtExposure
            .prepare(
                &VkdtExposure.manifest().resolve(&params).unwrap(),
                &Context::new(),
            )
            .unwrap();
        let input = Image::new(Region::sized(1, 1), 4, vec![0.1, 0.2, 0.4, 0.5]);
        let out = VkdtExposure
            .process(&gain, &[&input], Region::sized(1, 1))
            .unwrap();
        assert_eq!(out[0].data(), &[0.2, 0.4, 0.8, 1.0]);
    }

    #[test]
    fn usm_leaves_a_flat_image_alone_and_sharpens_an_edge() {
        let plan = UsmPlan {
            amount: 2.0,
            thrs: 0.2,
        };
        let out = Region::sized(8, 4);
        let flat = Image::from_fn(out.grow(2), 4, |_, _, _| 0.5);
        let result = VkdtUsm.process(&plan, &[&flat], out).unwrap();
        for p in result[0].data().as_chunks::<4>().0 {
            assert!((p[0] - 0.5).abs() < 1e-6 && p[3] == 1.0);
        }
        // A step: the bright side gets brighter next to the edge.
        let step = Image::from_fn(out.grow(2), 4, |x, _, _| if x < 4 { 0.2 } else { 0.6 });
        let result = VkdtUsm.process(&plan, &[&step], out).unwrap();
        assert!(
            result[0].pixel(4, 1)[0] > 0.6,
            "{:?}",
            result[0].pixel(4, 1)
        );
        assert!(
            result[0].pixel(3, 1)[0] < 0.2,
            "{:?}",
            result[0].pixel(3, 1)
        );
    }

    #[test]
    fn both_keep_the_module_contract() {
        helios_testkit::assert_contract(VkdtExposure);
        helios_testkit::assert_contract(VkdtUsm);
    }
}
