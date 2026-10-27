// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/filmcurv/main.comp, src/pipe/modules/filmcurv/params.glsl,
// src/pipe/modules/filmcurv/params, src/pipe/modules/shared.glsl
// (adjust_colour_dng). vkdt: copyright 2019 johannes hanika, under the
// BSD-2-Clause licence; its notice is in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

//! vkdt's `filmcurv`: a film-like tone curve compressing scene-referred
//! light for display, the cumulative Weibull distribution
//! `1 - exp(-(x / lambda)^k)`, with `light` as `1 / lambda` and `contrast`
//! as `k`. `bias` is added to every channel first.
//!
//! The curve goes through each channel; the colour is then rebuilt by one
//! of vkdt's modes. Two are ported:
//!
//! - **hsl** (vkdt's default): the curve on each channel, then the middle
//!   channel set back between the other two in the proportion it had
//!   (the saturation-preserving step of the DNG specification);
//! - **per channel**: the curve on each channel, nothing more.
//!
//! Not ported yet: vkdt's darktable UCS, Munsell, `AgX` and luminance-ratio
//! modes, and its `dspy` output (a plot of the curve for the GUI). Alpha is
//! set to 1, as vkdt does.

use helios_core::{
    Context, Family, GpuKernel, Image, Manifest, Module, ParamKind, ParamSpec, Params, Region,
    Value,
};

pub struct VkdtFilmcurv;

pub const HSL: &str = "hsl";
pub const PER_CHANNEL: &str = "per_channel";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilmcurvPlan {
    /// `1 / lambda`, at least 5e-3.
    pub il: f32,
    /// `k`, at least 1e-4.
    pub k: f32,
    pub bias: f32,
    /// The DNG hue-preserving step (hsl), else per channel.
    pub hsl: bool,
}

/// The cumulative Weibull distribution, as vkdt's `weibull_cdf`.
pub fn weibull_cdf(x: f32, il: f32, k: f32) -> f32 {
    1.0 - (-(x.max(1e-7) * il).powf(k)).exp()
}

/// vkdt's `adjust_colour_dng`: the middle channel of `col1` set between
/// the other two in the proportion it has in `col0`, so the hue (in the
/// HSV sense) stays.
pub fn adjust_colour_dng(mut col0: [f32; 3], mut col1: [f32; 3]) -> [f32; 3] {
    let swap = |c0: &mut [f32; 3], c1: &mut [f32; 3], a: usize, b: usize| {
        c0.swap(a, b);
        c1.swap(a, b);
    };
    // A bubble sort, red the largest, blue the smallest.
    let fx = col0[2] > col0[1];
    if fx {
        swap(&mut col0, &mut col1, 2, 1);
    }
    let fy = col0[1] > col0[0];
    if fy {
        swap(&mut col0, &mut col1, 0, 1);
    }
    let fz = col0[2] > col0[1];
    if fz {
        swap(&mut col0, &mut col1, 2, 1);
    }
    let t = (col0[1] - col0[2] + 1e-6) / (col0[0] - col0[2] + 1e-6);
    col1[1] = col1[2] * (1.0 - t) + col1[0] * t;
    if fz {
        col1.swap(2, 1);
    }
    if fy {
        col1.swap(0, 1);
    }
    if fx {
        col1.swap(2, 1);
    }
    col1
}

/// One pixel, as vkdt's `main.comp`.
pub fn filmcurv(plan: &FilmcurvPlan, rgb: [f32; 3]) -> [f32; 3] {
    let col0 = rgb.map(|v| v + plan.bias);
    let col1 = col0.map(|v| weibull_cdf(v, plan.il, plan.k));
    if plan.hsl {
        adjust_colour_dng(col0, col1)
    } else {
        col1
    }
}

impl Module for VkdtFilmcurv {
    type Plan = FilmcurvPlan;

    fn manifest(&self) -> Manifest {
        crate::manifest(
            "vkdt_filmcurv",
            "vkdt's film curve: compress scene light for display",
            vec![
                ParamSpec::float(
                    "light",
                    "the overall brightness: the Weibull curve's 1 / lambda",
                    0.0,
                    10.0,
                    3.0,
                ),
                ParamSpec::float(
                    "contrast",
                    "the contrast: the Weibull curve's k",
                    0.0,
                    2.0,
                    1.2,
                ),
                ParamSpec::float(
                    "bias",
                    "added to every channel first, to lift the blacks",
                    0.0,
                    0.02,
                    0.0,
                ),
                ParamSpec {
                    name: "colour".into(),
                    description: "how the colour is rebuilt after the curve".into(),
                    kind: ParamKind::Choice {
                        options: vec![HSL.into(), PER_CHANNEL.into()],
                        default: HSL.into(),
                    },
                },
            ],
            0,
            Family::Tone,
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<FilmcurvPlan, String> {
        Ok(FilmcurvPlan {
            il: (params.float("light") as f32).max(5e-3),
            k: (params.float("contrast") as f32).max(1e-4),
            bias: params.float("bias") as f32,
            hsl: params.get("colour").and_then(Value::as_choice) != Some(PER_CHANNEL),
        })
    }

    fn process(
        &self,
        plan: &FilmcurvPlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        for p in data.as_chunks_mut::<4>().0 {
            let out = filmcurv(plan, [p[0], p[1], p[2]]);
            p[..3].copy_from_slice(&out);
            p[3] = 1.0;
        }
        Ok(vec![Image::new(output, 4, data)])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: include_str!("wgsl/filmcurv.wgsl"),
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, plan: &FilmcurvPlan) -> Vec<f32> {
        vec![plan.il, plan.k, plan.bias, if plan.hsl { 1.0 } else { 0.0 }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(params: &Params) -> FilmcurvPlan {
        VkdtFilmcurv
            .prepare(
                &VkdtFilmcurv.manifest().resolve(params).unwrap(),
                &Context::new(),
            )
            .unwrap()
    }

    #[test]
    fn the_curve_is_monotonic_and_at_most_one() {
        let p = plan(&Params::new());
        let mut last = 0.0;
        for i in 0..200 {
            let v = filmcurv(&p, [i as f32 / 50.0; 3])[0];
            assert!(v >= last && v <= 1.0, "{i}: {v}");
            last = v;
        }
        // Grey stays grey.
        let g = filmcurv(&p, [0.18; 3]);
        assert!((g[0] - g[1]).abs() < 1e-7 && (g[1] - g[2]).abs() < 1e-7);
    }

    #[test]
    fn hsl_keeps_the_middle_channel_in_proportion() {
        let p = plan(&Params::new());
        let rgb = [0.6, 0.3, 0.1];
        let out = filmcurv(&p, rgb);
        let t_in = (rgb[1] - rgb[2]) / (rgb[0] - rgb[2]);
        let t_out = (out[1] - out[2]) / (out[0] - out[2]);
        assert!((t_in - t_out).abs() < 1e-4, "{t_in} {t_out}");
        // In any channel order.
        let swapped = filmcurv(&p, [0.1, 0.6, 0.3]);
        assert!((swapped[0] - out[2]).abs() < 1e-6);
        assert!((swapped[1] - out[0]).abs() < 1e-6);
        assert!((swapped[2] - out[1]).abs() < 1e-6);
        // Per channel leaves the middle channel to the curve.
        let per = plan(&Params::new().with("colour", Value::Choice(PER_CHANNEL.into())));
        let out = filmcurv(&per, rgb);
        assert_eq!(out[1], weibull_cdf(0.3, 3.0, 1.2));
    }

    #[test]
    fn it_keeps_the_module_contract() {
        helios_testkit::assert_contract(VkdtFilmcurv);
    }
}
