// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/grade/main.comp, src/pipe/modules/grade/params. vkdt:
// copyright 2019 johannes hanika, under the BSD-2-Clause licence; its notice
// is in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

//! vkdt's `grade`: lift, gamma, gain and offset (the ACES CDL formula), in
//! one kernel. Each of the four is four numbers, as in vkdt: red, green and
//! blue, and a fourth added to all three.
//!
//! - **classic**: `rgb * gain`, then `rgb * (1 - lift) + lift`, then
//!   `max(rgb, 0)^(1 / gamma)`, then `+ offset`, on every pixel alike;
//! - **zoned log**: the same, each term weighted by the pixel's zone, from
//!   its luminance on a log scale: lift in the shadows, gamma in the
//!   midtones, gain in the highlights, split at `sh_pivot` and `hi_pivot`.
//!
//! Alpha is set to 1, as vkdt does. One deviation: vkdt's `smoothstep`
//! with equal edges (`hi_pivot` clamped to 1) is undefined in GLSL; here
//! it is a step at the edge.

use helios_core::{
    Context, Family, GpuKernel, Image, Module, ParamKind, ParamSpec, Params, Region, Value,
};

pub struct VkdtGrade;

pub const CLASSIC: &str = "classic";
pub const ZONED_LOG: &str = "zoned_log";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradePlan {
    /// Each `.rgb + .a`, gamma kept above 1e-6, gain above 0.
    pub lift: [f32; 3],
    pub gamma: [f32; 3],
    pub gain: [f32; 3],
    pub offset: [f32; 3],
    pub zoned: bool,
    /// The pivots, clamped as vkdt clamps them.
    pub sh_pivot: f32,
    pub hi_pivot: f32,
}

fn four(name: &str, description: &str, min: f64, max: f64, rgb: f64) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        description: description.into(),
        kind: ParamKind::Floats {
            count: 4,
            min,
            max,
            default: vec![rgb, rgb, rgb, 0.0],
        },
    }
}

/// `.rgb + .a` of a four-number parameter, missing values from `default`.
fn combined(params: &Params, name: &str, default: f64) -> [f32; 3] {
    let list = params.get(name).and_then(Value::as_list).unwrap_or(&[]);
    let at = |i: usize| {
        list.get(i)
            .copied()
            .unwrap_or(if i < 3 { default } else { 0.0 })
    };
    [0, 1, 2].map(|c| (at(c) + at(3)) as f32)
}

/// GLSL's `smoothstep`; with `e1 <= e0`, a step at `e0`.
pub(crate) fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One pixel, as vkdt's `main.comp`.
pub fn grade(plan: &GradePlan, rgb: [f32; 3]) -> [f32; 3] {
    let (mut gain, mut lift, mut gamma) = (plan.gain, plan.lift, plan.gamma);
    if plan.zoned {
        let l = 0.2126 * rgb[0].max(0.0) + 0.7152 * rgb[1].max(0.0) + 0.0722 * rgb[2].max(0.0);
        let l = (0.67 + l.max(1e-6).log2() * 0.11).clamp(0.0, 1.0);
        let w_s = 1.0 - smoothstep(0.0, plan.sh_pivot, l);
        let w_h = smoothstep(plan.hi_pivot, 1.0, l);
        let w_m = 1.0 - w_s - w_h;
        for c in 0..3 {
            gain[c] = 1.0 + (gain[c] - 1.0) * w_h;
            lift[c] *= w_s;
            gamma[c] = 1.0 + (gamma[c] - 1.0) * w_m;
        }
    }
    let mut out = [0.0; 3];
    for c in 0..3 {
        let v = gain[c] * rgb[c];
        let v = v * (1.0 - lift[c]) + lift[c];
        out[c] = v.max(0.0).powf(1.0 / gamma[c]) + plan.offset[c];
    }
    out
}

impl Module for VkdtGrade {
    type Plan = GradePlan;

    fn manifest(&self) -> helios_core::Manifest {
        let mut params = vec![
            four(
                "lift",
                "blend towards a colour in the shadows: red, green, blue, and all three",
                -1.0,
                1.0,
                0.0,
            ),
            four(
                "gamma",
                "power for the midtones, above 1 brightens: red, green, blue, and all three",
                -1.0,
                4.0,
                1.0,
            ),
            four(
                "gain",
                "multiply, applied first: red, green, blue, and all three",
                -1.0,
                4.0,
                1.0,
            ),
            four(
                "offset",
                "add, applied last: red, green, blue, and all three",
                -1.0,
                1.0,
                0.0,
            ),
        ];
        params.push(ParamSpec {
            name: "mode".into(),
            description: "apply to all tones alike, or by zone".into(),
            kind: ParamKind::Choice {
                options: vec![CLASSIC.into(), ZONED_LOG.into()],
                default: CLASSIC.into(),
            },
        });
        params.push(ParamSpec::float(
            "sh_pivot",
            "where shadows turn into midtones (zoned log)",
            0.0,
            1.0,
            0.3,
        ));
        params.push(ParamSpec::float(
            "hi_pivot",
            "where midtones turn into highlights (zoned log)",
            0.0,
            1.0,
            0.4,
        ));
        crate::manifest(
            "vkdt_grade",
            "vkdt's grade: lift, gamma, gain and offset",
            params,
            0,
            Family::Color,
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<GradePlan, String> {
        let sh_pivot = (params.float("sh_pivot") as f32).clamp(1e-3, 1.0 - 1e-3);
        Ok(GradePlan {
            lift: combined(params, "lift", 0.0),
            gamma: combined(params, "gamma", 1.0).map(|v| v.max(1e-6)),
            gain: combined(params, "gain", 1.0).map(|v| v.max(0.0)),
            offset: combined(params, "offset", 0.0),
            zoned: params.get("mode").and_then(Value::as_choice) == Some(ZONED_LOG),
            sh_pivot,
            hi_pivot: (params.float("hi_pivot") as f32).clamp(sh_pivot + 1e-3, 1.0),
        })
    }

    fn process(
        &self,
        plan: &GradePlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        for p in data.as_chunks_mut::<4>().0 {
            let out = grade(plan, [p[0], p[1], p[2]]);
            p[..3].copy_from_slice(&out);
            p[3] = 1.0;
        }
        Ok(vec![Image::new(output, 4, data)])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: include_str!("wgsl/grade.wgsl"),
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, plan: &GradePlan) -> Vec<f32> {
        let mut u = Vec::with_capacity(16);
        let tail = [
            if plan.zoned { 1.0 } else { 0.0 },
            plan.sh_pivot,
            plan.hi_pivot,
            0.0,
        ];
        for (rgb, last) in [plan.lift, plan.gamma, plan.gain, plan.offset]
            .into_iter()
            .zip(tail)
        {
            u.extend_from_slice(&rgb);
            u.push(last);
        }
        u
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(params: &Params) -> GradePlan {
        VkdtGrade
            .prepare(
                &VkdtGrade.manifest().resolve(params).unwrap(),
                &Context::new(),
            )
            .unwrap()
    }

    #[test]
    fn the_defaults_leave_the_image_alone() {
        let p = plan(&Params::new());
        for rgb in [[0.1, 0.5, 0.9], [0.0, 0.2, 1.5]] {
            let out = grade(&p, rgb);
            for c in 0..3 {
                assert!((out[c] - rgb[c]).abs() < 1e-6, "{rgb:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn classic_applies_gain_lift_gamma_then_offset() {
        let p = plan(
            &Params::new()
                .with("lift", Value::List(vec![0.1, 0.0, 0.0, 0.0]))
                .with("gamma", Value::List(vec![1.0, 2.0, 1.0, 0.0]))
                .with("gain", Value::List(vec![1.0, 1.0, 1.0, 1.0]))
                .with("offset", Value::List(vec![0.0, 0.0, 0.0, 0.05])),
        );
        let out = grade(&p, [0.25, 0.25, 0.25]);
        // gain 2 everywhere; lift 0.1 on red; gamma 2 on green.
        let expect = [0.5 * 0.9 + 0.1 + 0.05, 0.5f32.powf(0.5) + 0.05, 0.5 + 0.05];
        for c in 0..3 {
            assert!((out[c] - expect[c]).abs() < 1e-6, "{out:?} vs {expect:?}");
        }
    }

    #[test]
    fn zoned_log_lifts_shadows_and_leaves_highlights_unlifted() {
        let p = plan(
            &Params::new()
                .with("lift", Value::List(vec![0.0, 0.0, 0.0, 0.1]))
                .with("mode", Value::Choice(ZONED_LOG.into())),
        );
        // A deep shadow is lifted almost fully, white not at all.
        let dark = grade(&p, [0.001; 3]);
        assert!(dark[0] > 0.09, "{dark:?}");
        let white = grade(&p, [1.0; 3]);
        assert!((white[0] - 1.0).abs() < 1e-6, "{white:?}");
    }

    #[test]
    fn it_keeps_the_module_contract() {
        helios_testkit::assert_contract(VkdtGrade);
    }
}
