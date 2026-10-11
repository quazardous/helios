//! Mask refinement, at element or group scope: darktable's
//! `dt_develop_blend_refine_form_mask` (`src/develop/blend.c`), one step per
//! module. darktable runs feathering, blur, then the tone curve (contrast
//! and brightness); in a network the order is the order of the nodes.
//!
//! Every value of a pixel is computed from the same neighbours in the same
//! order wherever the tile starts, so a tiled render gives the same bits as
//! a whole one.

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamHint, ParamKind, ParamSpec,
    Params, PortSpec, Region, Role,
};

use crate::{flag, luminance, mask_port};

fn refinement(name: &str, description: &str, params: Vec<ParamSpec>) -> Manifest {
    Manifest {
        name: name.into(),
        version: 1,
        description: description.into(),
        params,
        inputs: vec![mask_port("mask")],
        outputs: vec![mask_port("mask")],
        context: vec![],
        needs: Needs::default(),
        kind: ModuleKind::Process,
        family: Family::Mask,
        role: Role::General,
    }
}

/// A Gaussian blur of a mask, its radius (the standard deviation) in
/// full-resolution pixels, as darktable's mask blur: the same look at any
/// render scale.
pub struct MaskBlur;

impl MaskBlur {
    /// Normalized weights from the centre outward, for a sigma in pixels of
    /// the render; `len() - 1` is the reach.
    fn weights(sigma: f64) -> Vec<f32> {
        if sigma < 0.01 {
            return vec![1.0];
        }
        let reach = (3.0 * sigma).ceil() as usize;
        let raw: Vec<f64> = (0..=reach)
            .map(|i| (-(i as f64).powi(2) / (2.0 * sigma * sigma)).exp())
            .collect();
        let total = raw[0] + 2.0 * raw[1..].iter().sum::<f64>();
        raw.iter().map(|w| (w / total) as f32).collect()
    }
}

impl Module for MaskBlur {
    type Plan = Vec<f32>;

    fn manifest(&self) -> Manifest {
        refinement(
            "mask_blur",
            "Gaussian blur of a mask, its radius in full-resolution pixels",
            vec![ParamSpec::float(
                "radius",
                "standard deviation, in full-resolution pixels",
                0.0,
                64.0,
                2.0,
            )],
        )
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        vec![
            ParamHint::new("radius")
                .soft(0.0, 32.0)
                .steps(0.1, 1.0)
                .unit("px"),
        ]
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<Vec<f32>, String> {
        self.prepare_at(params, context, 1.0)
    }

    fn prepare_at(&self, params: &Params, _: &Context, scale: f64) -> Result<Vec<f32>, String> {
        Ok(Self::weights(params.float("radius") * scale))
    }

    fn input_region(&self, w: &Vec<f32>, output: Region, _: u32) -> Region {
        output.grow(w.len() as u32 - 1)
    }

    fn process(
        &self,
        w: &Vec<f32>,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        let reach = w.len() as i64 - 1;
        let rows = Region::new(
            output.x,
            output.y - reach,
            output.width,
            output.height + 2 * reach as u32,
        );
        let across = Image::from_fn(rows, 1, |x, y, _| {
            let mut v = w[0] * input.pixel(x, y)[0];
            for (i, wi) in w.iter().enumerate().skip(1) {
                let i = i as i64;
                v += wi * (input.pixel(x - i, y)[0] + input.pixel(x + i, y)[0]);
            }
            v
        });
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            let mut v = w[0] * across.pixel(x, y)[0];
            for (i, wi) in w.iter().enumerate().skip(1) {
                let i = i as i64;
                v += wi * (across.pixel(x, y - i)[0] + across.pixel(x, y + i)[0]);
            }
            v
        })])
    }
}

/// Feathering: a guided filter (He, Sun and Tang) that makes a mask follow
/// the edges of a guide image, darktable's mask feathering. The guide is
/// the luminance of `guide` (darktable uses the three colour channels);
/// with no guide connected, the mask guides itself, an edge-preserving
/// smoothing. The window's radius is in full-resolution pixels and follows
/// the render's scale (darktable takes the square root of the scale).
pub struct MaskFeather;

/// darktable's guide weight and regularization for scene-referred RGB
/// (feather version 1): the guide is multiplied by 10, `eps` is 0.5².
const GUIDE_WEIGHT: f64 = 10.0;
const EPS: f64 = 0.25;

impl Module for MaskFeather {
    /// The window's radius, in pixels of the render.
    type Plan = u32;

    fn manifest(&self) -> Manifest {
        let mut m = refinement(
            "mask_feather",
            "Guided-filter feathering: the mask follows the edges of a guide image",
            vec![ParamSpec::float(
                "radius",
                "window radius, in full-resolution pixels",
                0.0,
                64.0,
                4.0,
            )],
        );
        m.inputs.push(PortSpec::image("guide", 4).optional());
        m
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        vec![
            ParamHint::new("radius")
                .soft(0.0, 32.0)
                .steps(0.1, 1.0)
                .unit("px"),
        ]
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<u32, String> {
        self.prepare_at(params, context, 1.0)
    }

    fn prepare_at(&self, params: &Params, _: &Context, scale: f64) -> Result<u32, String> {
        Ok((params.float("radius") * scale).round() as u32)
    }

    /// Two box means one after the other: twice the window.
    fn input_region(&self, w: &u32, output: Region, _: u32) -> Region {
        output.grow(2 * w)
    }

    fn process(&self, &w: &u32, inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let (mask, guide) = (inputs[0], inputs[1]);
        if w == 0 {
            return Ok(vec![mask.sample(output)]);
        }
        let outer = output.grow(2 * w);
        let ab = output.grow(w);
        let p: Vec<f64> = mask
            .sample(outer)
            .data()
            .iter()
            .map(|v| f64::from(*v))
            .collect();
        let i: Vec<f64> = if guide.is_absent() {
            p.iter().map(|v| v * GUIDE_WEIGHT).collect()
        } else {
            let g = guide.sample(outer);
            g.data()
                .chunks(4)
                .map(|px| f64::from(luminance(px)) * GUIDE_WEIGHT)
                .collect()
        };
        let ii: Vec<f64> = i.iter().map(|v| v * v).collect();
        let ip: Vec<f64> = i.iter().zip(&p).map(|(a, b)| a * b).collect();
        let mean = |v: &[f64]| box_mean(v, outer, ab, w);
        let (mi, mp, mii, mip) = (mean(&i), mean(&p), mean(&ii), mean(&ip));
        let mut a = Vec::with_capacity(mi.len());
        let mut b = Vec::with_capacity(mi.len());
        for k in 0..mi.len() {
            let ak = (mip[k] - mi[k] * mp[k]) / (mii[k] - mi[k] * mi[k] + EPS);
            a.push(ak);
            b.push(mp[k] - ak * mi[k]);
        }
        let (ma, mb) = (box_mean(&a, ab, output, w), box_mean(&b, ab, output, w));
        let width = outer.width as usize;
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            let k = ((y - output.y) as usize) * output.width as usize + (x - output.x) as usize;
            let at = ((y - outer.y) as usize) * width + (x - outer.x) as usize;
            (ma[k] * i[at] + mb[k]).clamp(0.0, 1.0) as f32
        })])
    }
}

/// The mean over a `(2w + 1)²` window of `v` (covering `from`) at every
/// pixel of `to`, which `from` covers grown by `w`. Rows then columns, each
/// sum in the same order whatever the tile.
fn box_mean(v: &[f64], from: Region, to: Region, w: u32) -> Vec<f64> {
    let w = i64::from(w);
    let fw = from.width as usize;
    let at = |x: i64, y: i64| ((y - from.y) as usize) * fw + (x - from.x) as usize;
    let rows = (to.y - w..to.bottom() + w).count();
    let mut across = Vec::with_capacity(rows * to.width as usize);
    for y in to.y - w..to.bottom() + w {
        for x in to.x..to.right() {
            across.push((-w..=w).map(|d| v[at(x + d, y)]).sum::<f64>());
        }
    }
    let tw = to.width as usize;
    let n = ((2 * w + 1) * (2 * w + 1)) as f64;
    let mut out = Vec::with_capacity(to.area());
    for y in 0..to.height as i64 {
        for x in 0..tw {
            let s: f64 = (0..=2 * w).map(|d| across[(y + d) as usize * tw + x]).sum();
            out.push(s / n);
        }
    }
    out
}

/// darktable's mask tone curve (`_develop_blend_process_mask_tone_curve`)
/// with an opacity of 1: brightness shifts the mask, contrast steepens it
/// around its middle. Both 0 leave it unchanged.
pub fn tone_curve(m: f32, contrast: f32, brightness: f32) -> f32 {
    let epsilon = 16.0 * f32::EPSILON;
    let e = (3.0 * contrast).exp();
    let mut x = 2.0 * m - 1.0;
    if 1.0 - brightness <= 0.0 {
        x = if m <= epsilon { -1.0 } else { 1.0 };
    } else if 1.0 + brightness <= 0.0 {
        x = if m >= 1.0 - epsilon { 1.0 } else { -1.0 };
    } else if brightness > 0.0 {
        x = ((x + brightness) / (1.0 - brightness)).min(1.0);
    } else {
        x = ((x + brightness) / (1.0 + brightness)).max(-1.0);
    }
    let c = 0.5 * (x * e / (1.0 + (e - 1.0) * x.abs())) + 0.5;
    if c > 1e-6 { c.clamp(0.0, 1.0) } else { 0.0 }
}

/// Contrast and brightness (darktable's tone curve, skipped as there when
/// both are under 0.01), then inversion, then opacity: the last step of a
/// refined group, which closes it in darktable's order.
pub struct MaskLevels;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelsPlan {
    pub contrast: f32,
    pub brightness: f32,
    pub invert: bool,
    pub opacity: f32,
}

impl Module for MaskLevels {
    type Plan = LevelsPlan;

    fn manifest(&self) -> Manifest {
        refinement(
            "mask_levels",
            "Mask contrast and brightness, then inversion and opacity",
            vec![
                ParamSpec::float(
                    "contrast",
                    "steepens the mask around its middle",
                    -1.0,
                    1.0,
                    0.0,
                ),
                ParamSpec::float("brightness", "shifts the mask up or down", -1.0, 1.0, 0.0),
                ParamSpec {
                    name: "invert".into(),
                    description: "invert the mask, after contrast and brightness".into(),
                    kind: ParamKind::Bool { default: false },
                },
                ParamSpec::float("opacity", "multiplies the mask, last", 0.0, 1.0, 1.0),
            ],
        )
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        vec![
            ParamHint::new("contrast").steps(0.01, 0.1),
            ParamHint::new("brightness").steps(0.01, 0.1),
            ParamHint::new("opacity").unit("%").display(100.0, 0),
        ]
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<LevelsPlan, String> {
        Ok(LevelsPlan {
            contrast: params.float("contrast") as f32,
            brightness: params.float("brightness") as f32,
            invert: flag(params, "invert"),
            opacity: params.float("opacity") as f32,
        })
    }

    fn process(
        &self,
        plan: &LevelsPlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let curve = plan.contrast.abs() >= 0.01 || plan.brightness.abs() >= 0.01;
        let data = inputs[0]
            .data()
            .iter()
            .map(|&m| {
                let m = if curve {
                    tone_curve(m, plan.contrast, plan.brightness)
                } else {
                    m
                };
                let m = if plan.invert { 1.0 - m } else { m };
                m * plan.opacity
            })
            .collect();
        Ok(vec![Image::new(output, 1, data)])
    }
}
