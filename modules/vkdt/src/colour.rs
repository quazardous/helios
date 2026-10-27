// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/colour/main-impl.glsl, src/pipe/modules/colour/main.c,
// src/pipe/modules/colour/params, src/pipe/modules/i-raw/main.c (the camera
// matrix), src/pipe/modules/matrices.h. vkdt: copyright 2019 johannes
// hanika, under the BSD-2-Clause licence; its notice is in
// modules/vkdt/LICENSE-vkdt. The saturation works in darktable UCS 2022
// (Aurélien Pierre, MIT licence), see `ucs.rs`.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause AND MIT

//! vkdt's `colour`: camera RGB to linear Rec.2020, white balance,
//! exposure, saturation and highlight clipping, in one kernel.
//!
//! - **matrix**: `image` (the default) uses the camera's matrix, from the
//!   image's context as vkdt's raw loader computes it (`XYZ to Rec.2020`
//!   times the inverse of the camera's `XYZ to camera`, D65); `rec2020`
//!   leaves the colours, `xyz` and `rec709` read CIE XYZ or linear Rec.709.
//! - **white**: the white balance, red, green, blue. All zero (the
//!   default) takes the camera's: its as-shot coefficients through its
//!   matrix, as vkdt does. vkdt balances with a CAT16 adaptation in
//!   Rec.2020, from D65 towards that white, after the matrix.
//! - **clip** and **clipmax**: clip the highlights at the lowest channel of
//!   the camera white `clipmax` once balanced.
//! - **exposure** in stops, then **sat**, a chroma multiplier in darktable
//!   UCS (left out when 1, as in vkdt).
//!
//! **The white balance rule.** vkdt balances in `colour`, on camera RGB
//! the raw loader left unbalanced. A Helios chain that feeds `vkdt_colour`
//! leaves Helios's `whitebalance` (and `colorin`) out, and the camera's
//! matrix and coefficients reach `vkdt_colour` as context inputs, as
//! `colorin` gets its matrix. Balancing twice would tint the image.
//!
//! Not ported yet: the colour lookup table (CLUT) and its temperature
//! anchor, the radial basis function mapping (`mode`, `rbmap`, imported
//! from a colour checker), the colour picker inputs (spot balance and
//! deflicker), the gamut limits (they need vkdt's spectral tables), and
//! input transfer curves other than linear. One deviation: vkdt's
//! saturation divides by zero on black pixels; here black stays black.

use helios_core::{
    Context, Family, GpuKernel, Image, Manifest, Module, ParamKind, ParamSpec, Params, Region,
    Value,
};
use helios_photo::camera::fields;

use crate::ucs::{self, Mat3};

pub struct VkdtColour;

pub const IMAGE: &str = "image";
pub const REC2020: &str = "rec2020";
pub const XYZ: &str = "xyz";
pub const REC709: &str = "rec709";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColourPlan {
    /// The camera matrix, then the CAT16 balance, in one.
    pub matrix: [[f32; 3]; 3],
    /// The highlight clip, when on.
    pub clip: Option<f32>,
    /// `2^exposure`.
    pub gain: f32,
    pub sat: f32,
}

/// The camera's matrix to Rec.2020, as vkdt's raw loader computes it:
/// `XYZ_TO_REC2020 * inverse(xyz_to_camera)`, rows not normalized.
pub fn camera_to_rec2020(xyz_to_camera: &[f64; 9]) -> Option<Mat3> {
    let m: Mat3 = [
        [xyz_to_camera[0], xyz_to_camera[1], xyz_to_camera[2]],
        [xyz_to_camera[3], xyz_to_camera[4], xyz_to_camera[5]],
        [xyz_to_camera[6], xyz_to_camera[7], xyz_to_camera[8]],
    ];
    let inv = helios_photo::color::inverse(&m)?;
    Some(ucs::mul(&ucs::XYZ_TO_REC2020, &inv)).filter(|m| m.iter().flatten().all(|v| v.is_finite()))
}

fn image_matrix(context: &Context) -> Result<Mat3, String> {
    let m = context
        .get(fields::XYZ_TO_CAMERA)
        .and_then(Value::as_list)
        .and_then(|l| <[f64; 9]>::try_from(l).ok())
        .ok_or_else(|| {
            format!(
                "context field `{}`: the camera's 3 x 3 matrix needed (matrix `image`)",
                fields::XYZ_TO_CAMERA
            )
        })?;
    camera_to_rec2020(&m).ok_or_else(|| "the camera's matrix cannot be inverted".into())
}

/// The as-shot white, as vkdt's `commit_params`: the camera's white
/// (`1 / coefficients`) through its matrix, inverted, green 1.
pub fn camera_white(cam: &Mat3, coefficients: [f64; 3]) -> [f64; 3] {
    let w0 = [0, 1, 2].map(|j| (0..3).map(|i| cam[j][i] / coefficients[i]).sum::<f64>());
    [w0[1] / w0[0], 1.0, w0[1] / w0[2]]
}

/// The CAT16 adaptation vkdt's `cat16` makes, from Rec.2020 white towards
/// `dst`, as a matrix.
pub fn cat16(dst: [f64; 3]) -> Mat3 {
    let to_cone = ucs::mul(&ucs::CAT16_M, &ucs::REC2020_TO_XYZ);
    let src = ucs::apply(&to_cone, [1.0; 3]);
    let dst = ucs::apply(&to_cone, dst);
    let mut scaled = to_cone;
    for (i, row) in scaled.iter_mut().enumerate() {
        for v in row {
            *v *= dst[i] / src[i];
        }
    }
    ucs::mul(&ucs::mul(&ucs::XYZ_TO_REC2020, &ucs::CAT16_MI), &scaled)
}

/// vkdt's saturation: chroma scaled in darktable UCS, kept within 1.
pub fn saturate(rgb: [f32; 3], sat: f32) -> [f32; 3] {
    let rgb = rgb.map(|v| v.max(0.0));
    if rgb[0] + rgb[1] + rgb[2] <= 0.0 {
        return rgb;
    }
    let mut jch = ucs::xyy_to_jch(ucs::rec2020_to_xyy(rgb), 1.0);
    jch[1] = (jch[1] * sat).clamp(0.0, 1.0);
    ucs::xyy_to_rec2020(ucs::jch_to_xyy(jch, 1.0))
}

/// One pixel, as vkdt's `main-impl.glsl`.
pub fn colour(plan: &ColourPlan, rgb: [f32; 3]) -> [f32; 3] {
    let mut rgb = ucs::apply_f32(&plan.matrix, rgb);
    if let Some(t) = plan.clip {
        rgb = rgb.map(|v| v.min(t));
    }
    rgb = rgb.map(|v| v * plan.gain);
    if plan.sat != 1.0 {
        rgb = saturate(rgb, plan.sat);
    }
    rgb.map(|v| v.clamp(-65535.0, 65535.0))
}

impl Module for VkdtColour {
    type Plan = ColourPlan;

    fn manifest(&self) -> Manifest {
        let mut m = crate::manifest(
            "vkdt_colour",
            "vkdt's colour: camera matrix, white balance, exposure and saturation",
            vec![
                ParamSpec::float(
                    "exposure",
                    "the exposure correction, in stops",
                    -15.0,
                    15.0,
                    0.0,
                ),
                ParamSpec::float("sat", "the saturation, a chroma multiplier", 0.0, 3.0, 1.0),
                ParamSpec {
                    name: "matrix".into(),
                    description: "the colours of the input: the camera's, or a known space".into(),
                    kind: ParamKind::Choice {
                        options: vec![IMAGE.into(), REC2020.into(), XYZ.into(), REC709.into()],
                        default: IMAGE.into(),
                    },
                },
                ParamSpec {
                    name: "white".into(),
                    description: "the white balance, red, green, blue; all zero for the camera's"
                        .into(),
                    kind: ParamKind::Floats {
                        count: 3,
                        min: 0.0,
                        max: 8.0,
                        default: vec![0.0; 3],
                    },
                },
                ParamSpec {
                    name: "clip".into(),
                    description: "clip the highlights".into(),
                    kind: ParamKind::Bool { default: false },
                },
                ParamSpec::float(
                    "clipmax",
                    "where the highlights clip, in camera white",
                    0.0,
                    2.0,
                    1.0,
                ),
            ],
            0,
            Family::Color,
        );
        m.context = vec![fields::XYZ_TO_CAMERA.into(), fields::WHITE_BALANCE.into()];
        m
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<ColourPlan, String> {
        let choice = params.get("matrix").and_then(Value::as_choice);
        let cam = match choice {
            Some(REC2020) => ucs::IDENTITY,
            Some(XYZ) => ucs::XYZ_TO_REC2020,
            Some(REC709) => ucs::REC709_TO_REC2020,
            _ => image_matrix(context)?,
        };
        let list = params.get("white").and_then(Value::as_list).unwrap_or(&[]);
        let mut white = [0, 1, 2].map(|i| list.get(i).copied().unwrap_or(0.0));
        if white == [0.0; 3] {
            // vkdt takes the image's matrix here, whatever `matrix` says.
            let image = image_matrix(context)?;
            let coefficients = as_shot(context)?;
            white = camera_white(&image, coefficients);
        }
        if !white.iter().all(|v| v.is_finite() && *v != 0.0) {
            white = [1.0; 3];
        }
        let dst = [white[0] / white[1], 1.0, white[2] / white[1]];
        let matrix = ucs::mul(&cat16(dst), &cam);
        let clip = (params.get("clip").and_then(Value::as_bool) == Some(true)).then(|| {
            let c = params.float("clipmax");
            let v = ucs::apply(&matrix, [c; 3]);
            v[0].min(v[1]).min(v[2]) as f32
        });
        Ok(ColourPlan {
            matrix: matrix.map(|r| r.map(|v| v as f32)),
            clip,
            gain: 2f32.powf(params.float("exposure") as f32),
            sat: params.float("sat") as f32,
        })
    }

    fn process(
        &self,
        plan: &ColourPlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        for p in data.as_chunks_mut::<4>().0 {
            let out = colour(plan, [p[0], p[1], p[2]]);
            p[..3].copy_from_slice(&out);
            p[3] = 1.0;
        }
        Ok(vec![Image::new(output, 4, data)])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: concat!(
                include_str!("wgsl/ucs.wgsl"),
                include_str!("wgsl/colour.wgsl")
            ),
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, plan: &ColourPlan) -> Vec<f32> {
        let mut u = Vec::with_capacity(16);
        for row in plan.matrix {
            u.extend_from_slice(&row);
            u.push(0.0);
        }
        u.extend_from_slice(&[
            plan.clip.unwrap_or(0.0),
            if plan.clip.is_some() { 1.0 } else { 0.0 },
            plan.gain,
            plan.sat,
        ]);
        u
    }
}

/// The camera's as-shot coefficients, green 1 (vkdt's raw loader
/// normalizes them the same way).
fn as_shot(context: &Context) -> Result<[f64; 3], String> {
    let wb = context
        .get(fields::WHITE_BALANCE)
        .and_then(Value::as_list)
        .filter(|l| l.len() >= 3)
        .ok_or_else(|| {
            format!(
                "context field `{}`: the camera's coefficients needed (white all zero)",
                fields::WHITE_BALANCE
            )
        })?;
    if !(wb[1].is_finite() && wb[1] > 0.0) {
        return Err(format!("the camera's green coefficient is {}", wb[1]));
    }
    Ok([wb[0] / wb[1], 1.0, wb[2] / wb[1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Canon EOS 40D's matrix and as-shot coefficients.
    fn context() -> Context {
        Context::new()
            .with(
                fields::XYZ_TO_CAMERA,
                Value::List(vec![
                    0.6071, -0.0747, -0.0856, -0.7653, 1.5365, 0.2441, -0.2025, 0.2553, 0.7315,
                ]),
            )
            .with(fields::WHITE_BALANCE, Value::List(vec![2.1, 1.0, 1.4, 1.0]))
    }

    fn plan(params: &Params, context: &Context) -> Result<ColourPlan, String> {
        VkdtColour.prepare(&VkdtColour.manifest().resolve(params).unwrap(), context)
    }

    #[test]
    fn rec2020_with_a_neutral_white_leaves_the_image_alone() {
        let p = plan(
            &Params::new()
                .with("matrix", Value::Choice(REC2020.into()))
                .with("white", Value::List(vec![1.0, 1.0, 1.0])),
            &Context::new(),
        )
        .unwrap();
        for rgb in [[0.2, 0.5, 0.7], [1.5, 0.1, 0.0]] {
            let out = colour(&p, rgb);
            for c in 0..3 {
                assert!((out[c] - rgb[c]).abs() < 1e-5, "{rgb:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn the_as_shot_balance_makes_the_cameras_white_neutral() {
        let p = plan(&Params::new(), &context()).unwrap();
        // A white surface under the scene's light, in camera RGB.
        let white = colour(&p, [1.0 / 2.1, 1.0, 1.0 / 1.4]);
        // vkdt's balance adapts in a cone space, not exactly neutral: close.
        assert!(
            (white[0] / white[1] - 1.0).abs() < 0.05 && (white[2] / white[1] - 1.0).abs() < 0.05,
            "{white:?}"
        );
    }

    #[test]
    fn exposure_saturation_and_clip() {
        let base = Params::new()
            .with("matrix", Value::Choice(REC2020.into()))
            .with("white", Value::List(vec![1.0, 1.0, 1.0]));
        let p = plan(
            &base.clone().with("exposure", Value::Float(1.0)),
            &Context::new(),
        )
        .unwrap();
        assert!((colour(&p, [0.25; 3])[0] - 0.5).abs() < 1e-5);
        let p = plan(
            &base.clone().with("sat", Value::Float(0.0)),
            &Context::new(),
        )
        .unwrap();
        let grey = colour(&p, [0.6, 0.3, 0.2]);
        assert!(
            (grey[0] - grey[1]).abs() < 1e-3 && (grey[1] - grey[2]).abs() < 1e-3,
            "{grey:?}"
        );
        assert_eq!(colour(&p, [0.0; 3]), [0.0; 3]);
        let p = plan(
            &base
                .with("clip", Value::Bool(true))
                .with("clipmax", Value::Float(0.8)),
            &Context::new(),
        )
        .unwrap();
        assert!((colour(&p, [1.5, 0.5, 0.9])[0] - 0.8).abs() < 1e-5);
    }

    #[test]
    fn missing_context_is_named() {
        let err = plan(&Params::new(), &Context::new()).unwrap_err();
        assert!(err.contains("photo.xyz_to_camera"), "{err}");
        let only_matrix = Context::new().with(
            fields::XYZ_TO_CAMERA,
            context().get(fields::XYZ_TO_CAMERA).unwrap().clone(),
        );
        let err = plan(&Params::new(), &only_matrix).unwrap_err();
        assert!(err.contains("photo.white_balance"), "{err}");
    }

    #[test]
    fn it_keeps_the_module_contract() {
        helios_testkit::assert_contract_in(VkdtColour, &context());
    }
}
