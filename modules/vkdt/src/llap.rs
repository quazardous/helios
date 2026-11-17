// Ported from vkdt (https://github.com/hanatos/vkdt, commit afc3425):
// src/pipe/modules/llap/main.c, src/pipe/modules/llap/llap.glsl,
// src/pipe/modules/llap/config.h, src/pipe/modules/llap/curve.comp,
// src/pipe/modules/llap/reduce.comp, src/pipe/modules/llap/assemble.comp,
// src/pipe/modules/llap/colour.comp, src/pipe/modules/llap/params,
// src/pipe/modules/shared.glsl. vkdt: copyright 2019 johannes hanika, under
// the BSD-2-Clause licence; its notice is in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

//! vkdt's `llap`: local contrast, shadows and highlights by a local
//! Laplacian pyramid. The luminance goes through ten tone curves, each
//! centred on a grey level `g` (`curve`); the eleven images (the ten
//! curves and the plain luminance) are reduced level by level, halving the
//! resolution until a side reaches one pixel or twelve levels are built
//! (`reduce`); from the coarsest level up, each finer level adds the
//! Laplacian of the curve whose grey levels bracket the pixel's luminance
//! (`assemble`); the result scales the colour (`colour`).
//!
//! vkdt's eleven-layer array images are eleven channels of one image
//! here. vkdt's `assemble` reads its finer level and its coarser one in a
//! single kernel; a Helios kernel reads all its inputs over one region,
//! so the upsampling of the coarser level is a kernel of its own
//! (`expand`, all eleven layers), and `assemble` reads two images of the
//! finer level. The pyramid's levels and their sizes come from the
//! image's extent at the render's scale, as vkdt builds them from its
//! size: lookups mirror at each level's edges as vkdt's sampler does.
//!
//! The kernels run on the CPU.

use helios_core::{
    Composite, Context, Error, Expansion, Family, Image, Manifest, Module, ModuleKind, ModuleRef,
    Needs, Network, ParamKind, ParamSpec, Params, PortSpec, Region, Role, Value, Wire,
};

use crate::guided::luminance;
use crate::sample::{Domain, sample_semisoft, sample_soft, semisoft_reach, soft_reach};

/// vkdt's `NUM_GAMMA`: the tone curves, one per grey level.
pub const NUM_GAMMA: usize = 10;
/// The layers of a pyramid level: the curves, then the luminance.
pub const LAYERS: u32 = NUM_GAMMA as u32 + 1;
/// vkdt's `max_nl`: the most levels a pyramid has.
pub const MAX_LEVELS: usize = 12;

/// `gamma_from_i`: grey level `i`, evenly spread over `0..=1`.
pub fn gamma_from_i(i: usize) -> f32 {
    i as f32 / (NUM_GAMMA as f32 - 1.0)
}

/// `gamma_hi_from_v`: the first grey level above `v` (from 1, at most
/// `NUM_GAMMA - 1`).
pub fn gamma_hi_from_v(v: f32) -> usize {
    let mut hi = 1;
    while hi < NUM_GAMMA - 1 && gamma_from_i(hi) <= v {
        hi += 1;
    }
    hi
}

/// The tone curve's parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurvePlan {
    pub sigma: f32,
    pub shadows: f32,
    pub highlights: f32,
    pub clarity: f32,
}

/// vkdt's `curve`: `x` around grey level `g`, linear with slope `shadows`
/// (above `g`) or `highlights` (below) beyond `2 sigma`, a quadratic
/// Bézier blend inside, plus `clarity` times a Gaussian bump. vkdt's
/// assignment of shadows above and highlights below is kept.
pub fn curve(x: f32, g: f32, p: &CurvePlan) -> f32 {
    let c = x - g;
    let ssigma = if c > 0.0 { p.sigma } else { -p.sigma };
    let shadhi = if c > 0.0 { p.shadows } else { p.highlights };
    let mut val = if c.abs() > 2.0 * p.sigma {
        g + ssigma + shadhi * (c - ssigma)
    } else {
        let t = (c / (2.0 * ssigma)).clamp(0.0, 1.0);
        let t2 = t * t;
        let mt = 1.0 - t;
        g + ssigma * 2.0 * mt * t + t2 * (ssigma + ssigma * shadhi)
    };
    val += p.clarity * c * (-c * c / (2.0 * p.sigma * p.sigma / 3.0)).exp();
    val
}

fn float(name: &str, description: &str, min: f64, max: f64, default: f64) -> ParamSpec {
    ParamSpec::float(name, description, min, max, default)
}

fn int(name: &str, description: &str, min: i64, max: i64, default: i64) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        description: description.into(),
        kind: ParamKind::Int { min, max, default },
    }
}

fn curve_params() -> Vec<ParamSpec> {
    vec![
        float(
            "sigma",
            "the width of the tone curves around each grey level",
            0.01,
            1.0,
            0.12,
        ),
        float(
            "shadows",
            "the slope above each grey level: above 1 brightens the shadows' detail",
            0.0,
            10.0,
            1.0,
        ),
        float(
            "highlights",
            "the slope below each grey level: above 1 darkens the highlights' detail",
            0.0,
            10.0,
            1.0,
        ),
        float(
            "clarity",
            "the local contrast in the midtones",
            -1.0,
            3.0,
            0.0,
        ),
    ]
}

fn kernel_manifest(
    name: &str,
    description: &str,
    params: Vec<ParamSpec>,
    inputs: Vec<PortSpec>,
    output: u32,
) -> Manifest {
    Manifest {
        name: name.into(),
        version: 1,
        description: description.into(),
        params,
        inputs,
        outputs: vec![PortSpec::image("output", output)],
        context: vec![],
        needs: Needs::default(),
        kind: ModuleKind::Process,
        family: Family::Tone,
        role: Role::Specialised,
    }
}

/// A level's domain, from the parameters `x`, `y`, `width`, `height`.
fn domain_params() -> Vec<ParamSpec> {
    const SIDE: i64 = 1 << 20;
    vec![
        int("x", "where the level starts, column", -SIDE, SIDE, 0),
        int("y", "where the level starts, row", -SIDE, SIDE, 0),
        int("width", "the level's width", 1, SIDE, 64),
        int("height", "the level's height", 1, SIDE, 64),
    ]
}

fn domain_of(params: &Params) -> Domain {
    let get = |name| params.get(name).and_then(Value::as_i64).unwrap_or(0);
    Domain::new(
        get("x"),
        get("y"),
        get("width").max(1) as u32,
        get("height").max(1) as u32,
    )
}

fn with_domain(params: Params, d: &Domain) -> Params {
    params
        .with("x", Value::Int(d.x))
        .with("y", Value::Int(d.y))
        .with("width", Value::Int(i64::from(d.width)))
        .with("height", Value::Int(i64::from(d.height)))
}

/// `curve`: the luminance through the ten tone curves, and the luminance
/// itself, as eleven channels.
pub struct Curve;

impl Module for Curve {
    type Plan = CurvePlan;

    fn manifest(&self) -> Manifest {
        kernel_manifest(
            "vkdt_llap_curve",
            "vkdt's llap, first kernel: the luminance through ten tone curves",
            curve_params(),
            vec![PortSpec::image("input", 4)],
            LAYERS,
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<CurvePlan, String> {
        Ok(CurvePlan {
            sigma: params.float("sigma") as f32,
            shadows: params.float("shadows") as f32,
            highlights: params.float("highlights") as f32,
            clarity: params.float("clarity") as f32,
        })
    }

    fn process(
        &self,
        plan: &CurvePlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        let mut data = Vec::with_capacity(output.area() * LAYERS as usize);
        for y in output.y..output.bottom() {
            for x in output.x..output.right() {
                let p = input.pixel(x, y);
                // Clamped: NaN and infinities would spread over the pyramid.
                let [r, g, b] = [p[0], p[1], p[2]].map(|v| v.clamp(-1000.0, 1000.0));
                let l = luminance(r, g, b);
                data.extend((0..NUM_GAMMA).map(|i| curve(l, gamma_from_i(i), plan)));
                data.push(l);
            }
        }
        Ok(vec![Image::new(output, LAYERS, data)])
    }
}

/// `reduce`: a pyramid level at half the resolution, each of its eleven
/// layers through `sample_semisoft` (a 1 2 1 filter) centred on input
/// pixel `2 o`. Its parameters are the finer level's domain.
pub struct Reduce;

impl Module for Reduce {
    type Plan = Domain;

    fn manifest(&self) -> Manifest {
        kernel_manifest(
            "vkdt_llap_reduce",
            "vkdt's llap: a pyramid level at half the resolution",
            domain_params(),
            vec![PortSpec::image("input", LAYERS)],
            LAYERS,
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<Domain, String> {
        Ok(domain_of(params))
    }

    fn input_region(&self, d: &Domain, r: Region, _: u32) -> Region {
        if r.is_empty() {
            return Region::new(d.x, d.y, 1, 1);
        }
        let span = |a: i64, b: i64| {
            (
                semisoft_reach(2.0 * a as f32).0,
                semisoft_reach(2.0 * (b - 1) as f32).1,
            )
        };
        let (x0, x1) = span(r.x, r.right());
        let (y0, y1) = span(r.y, r.bottom());
        d.reads(x0, x1, y0, y1)
    }

    fn process(&self, d: &Domain, inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        let mut data = Vec::with_capacity(output.area() * LAYERS as usize);
        for y in output.y..output.bottom() {
            for x in output.x..output.right() {
                let (px, py) = (2.0 * x as f32, 2.0 * y as f32);
                data.extend((0..LAYERS as usize).map(|c| sample_semisoft(input, d, px, py, c)));
            }
        }
        Ok(vec![Image::new(output, LAYERS, data)])
    }
}

/// Where `expand` reads and writes: the finer level's origin, the coarser
/// level's domain (starting at 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpandPlan {
    pub fine_x: i64,
    pub fine_y: i64,
    pub coarse: Domain,
}

/// `expand`: vkdt's `gauss_expand` (`sample_soft` at `o / 2`) of a coarser
/// level onto the finer one: the ten curves' layers, then the coarser
/// output (`coarse`) or, at the coarsest level where there is none, the
/// luminance layer. Its parameters are the finer level's origin (`x`,
/// `y`) and the coarser level's size (`width`, `height`).
pub struct Expand;

impl Module for Expand {
    type Plan = ExpandPlan;

    fn manifest(&self) -> Manifest {
        kernel_manifest(
            "vkdt_llap_expand",
            "vkdt's llap: a coarser level upsampled onto the finer one",
            domain_params(),
            vec![
                PortSpec::image("currhi", LAYERS),
                PortSpec::image("coarse", 1).optional(),
            ],
            LAYERS,
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<ExpandPlan, String> {
        let d = domain_of(params);
        Ok(ExpandPlan {
            fine_x: d.x,
            fine_y: d.y,
            coarse: Domain::new(0, 0, d.width, d.height),
        })
    }

    fn input_region(&self, p: &ExpandPlan, r: Region, _: u32) -> Region {
        if r.is_empty() {
            return Region::new(0, 0, 1, 1);
        }
        let (x0, x1) = (
            soft_reach(0.5 * (r.x - p.fine_x) as f32).0,
            soft_reach(0.5 * (r.right() - 1 - p.fine_x) as f32).1,
        );
        let (y0, y1) = (
            soft_reach(0.5 * (r.y - p.fine_y) as f32).0,
            soft_reach(0.5 * (r.bottom() - 1 - p.fine_y) as f32).1,
        );
        p.coarse.reads(x0, x1, y0, y1)
    }

    fn process(
        &self,
        p: &ExpandPlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let (currhi, coarse) = (inputs[0], inputs.get(1).filter(|c| !c.is_absent()));
        let d = &p.coarse;
        let mut data = Vec::with_capacity(output.area() * LAYERS as usize);
        for y in output.y..output.bottom() {
            let py = 0.5 * (y - p.fine_y) as f32;
            for x in output.x..output.right() {
                let px = 0.5 * (x - p.fine_x) as f32;
                data.extend((0..NUM_GAMMA).map(|c| sample_soft(currhi, d, px, py, c)));
                data.push(match coarse {
                    Some(coarse) => sample_soft(coarse, d, px, py, 0),
                    None => sample_soft(currhi, d, px, py, NUM_GAMMA),
                });
            }
        }
        Ok(vec![Image::new(output, LAYERS, data)])
    }
}

/// `assemble`: the upsampled coarser output plus the Laplacian (a level
/// minus its upsampled coarser level) of the two curves whose grey levels
/// bracket the pixel's luminance, blended linearly between them.
pub struct Assemble;

impl Module for Assemble {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        kernel_manifest(
            "vkdt_llap_assemble",
            "vkdt's llap: one level of the output pyramid",
            vec![],
            vec![
                PortSpec::image("currlo", LAYERS),
                PortSpec::image("expanded", LAYERS),
            ],
            1,
        )
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let (fine, up) = (inputs[0], inputs[1]);
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            let (f, e) = (fine.pixel(x, y), up.pixel(x, y));
            let v = f[NUM_GAMMA];
            let hi = gamma_hi_from_v(v);
            let lo = hi - 1;
            let (g_lo, g_hi) = (gamma_from_i(lo), gamma_from_i(hi));
            let a = ((v - g_lo) / (g_hi - g_lo)).clamp(0.0, 1.0);
            let l0 = f[lo] - e[lo];
            let l1 = f[hi] - e[hi];
            e[NUM_GAMMA] + l0 * (1.0 - a) + l1 * a
        })])
    }
}

/// `colour`: the input's colour scaled by the new luminance over its own,
/// darker values rolled off exponentially; alpha set to 1.
pub struct LlapColour;

impl Module for LlapColour {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        kernel_manifest(
            "vkdt_llap_colour",
            "vkdt's llap, last kernel: the colour scaled to the new luminance",
            vec![],
            vec![PortSpec::image("input", 4), PortSpec::image("lum", 1)],
            4,
        )
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let (input, lum) = (inputs[0], inputs[1]);
        let mut data = Vec::with_capacity(output.area() * 4);
        for y in output.y..output.bottom() {
            for x in output.x..output.right() {
                let p = input.pixel(x, y);
                let mut l = lum.pixel(x, y)[0];
                let yo = luminance(p[0], p[1], p[2]).max(1e-8);
                // The black roll-off (c = 1).
                if l < yo {
                    l = yo * (l - yo).exp();
                }
                let k = l / yo;
                data.extend([
                    (p[0] * k).max(0.0),
                    (p[1] * k).max(0.0),
                    (p[2] * k).max(0.0),
                    1.0,
                ]);
            }
        }
        Ok(vec![Image::new(output, 4, data)])
    }
}

/// The domains of the pyramid's levels, finest first, as vkdt's
/// `create_nodes` sizes them: halving until a side of the next level
/// would be one pixel, at most [`MAX_LEVELS`].
pub fn levels(base: Domain) -> Vec<Domain> {
    let mut levels = vec![base];
    let mut next = base.half();
    for _ in 1..MAX_LEVELS {
        levels.push(next);
        next = next.half();
        if next.width <= 1 || next.height <= 1 {
            break;
        }
    }
    levels
}

/// The image's extent at the render's scale: the finest level.
fn base(at: &Expansion) -> Option<Domain> {
    let e = at.extent?;
    let s = at.scale;
    let x0 = (e.x as f64 * s).floor() as i64;
    let y0 = (e.y as f64 * s).floor() as i64;
    let x1 = (e.right() as f64 * s).ceil() as i64;
    let y1 = (e.bottom() as f64 * s).ceil() as i64;
    Some(Domain::new(
        x0,
        y0,
        (x1 - x0).max(1) as u32,
        (y1 - y0).max(1) as u32,
    ))
}

/// vkdt's `llap` module.
pub struct VkdtLlap;

impl Composite for VkdtLlap {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "vkdt_llap".into(),
            version: 1,
            description: "vkdt's local Laplacian: local contrast, shadows and highlights".into(),
            params: curve_params(),
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Tone,
            role: Role::General,
        }
    }

    fn expand(&self, params: &Params, _: &Context, at: &Expansion) -> Result<Network, String> {
        let base =
            base(at).ok_or("its pyramid is built for the image's size: no image feeds it")?;
        let curves = curve_params().iter().fold(Params::new(), |p, s| {
            p.with(&s.name, Value::Float(params.float(&s.name)))
        });
        let levels = levels(base);
        let build = |n: &mut Network| -> Result<(), Error> {
            let input = n.input("input")?;
            let c = n.chain("curve", ModuleRef::new(Curve), &curves, input)?;
            let mut pyramid: Vec<Wire> = vec![n.output(c, "output")?];
            for l in 1..levels.len() {
                let r = n.chain(
                    &format!("reduce{l}"),
                    ModuleRef::new(Reduce),
                    &with_domain(Params::new(), &levels[l - 1]),
                    pyramid[l - 1],
                )?;
                pyramid.push(n.output(r, "output")?);
            }
            let mut coarse: Option<Wire> = None;
            for l in (1..levels.len()).rev() {
                let (fine, size) = (levels[l - 1], levels[l]);
                let at = Domain::new(fine.x, fine.y, size.width, size.height);
                let e = n.add(
                    &format!("expand{l}"),
                    ModuleRef::new(Expand),
                    &with_domain(Params::new(), &at),
                )?;
                n.connect(pyramid[l], e, "currhi")?;
                if let Some(c) = coarse {
                    n.connect(c, e, "coarse")?;
                }
                let a = n.add(
                    &format!("assemble{l}"),
                    ModuleRef::new(Assemble),
                    &Params::new(),
                )?;
                n.connect(pyramid[l - 1], a, "currlo")?;
                n.connect(n.output(e, "output")?, a, "expanded")?;
                coarse = Some(n.output(a, "output")?);
            }
            let lum = coarse.expect("a pyramid has two levels or more");
            let col = n.add("colour", ModuleRef::new(LlapColour), &Params::new())?;
            n.connect(input, col, "input")?;
            n.connect(lum, col, "lum")?;
            n.exit(n.output(col, "output")?, "output")
        };
        let mut n = Network::new(&self.manifest());
        build(&mut n).map_err(|e| e.to_string())?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use helios_core::graph::SOURCE_PORT;
    use helios_core::{Bus, Engine, Graph};

    use super::*;

    fn picture() -> Image {
        Image::from_fn(Region::sized(90, 70), 4, |x, y, c| {
            if c == 3 {
                return 1.0;
            }
            let edge = if (x / 13 + y / 11) % 2 == 0 {
                0.6
            } else {
                0.15
            };
            edge + ((x * 5 + y * 3 + i64::from(c) * 7) % 11) as f32 / 90.0
        })
    }

    fn render(params: &Params, region: Region) -> Image {
        let mut g = Graph::new();
        let src = g.add_source("src", picture());
        let n = g
            .add_module("llap", ModuleRef::composite(VkdtLlap), params)
            .unwrap();
        g.connect(src, SOURCE_PORT, n, "input").unwrap();
        Engine::new(Arc::new(Bus::new()))
            .render(&g, n, "output", region)
            .unwrap()
    }

    #[test]
    fn grey_levels_and_their_brackets_are_vkdts() {
        assert_eq!(gamma_from_i(0), 0.0);
        assert_eq!(gamma_from_i(9), 1.0);
        assert_eq!(gamma_hi_from_v(-1.0), 1);
        assert_eq!(gamma_hi_from_v(0.05), 1);
        assert_eq!(gamma_hi_from_v(0.5), 5);
        assert_eq!(gamma_hi_from_v(0.99), 9);
        assert_eq!(gamma_hi_from_v(7.0), 9);
        assert_eq!(gamma_hi_from_v(f32::NAN), 1);
    }

    #[test]
    fn the_curve_is_the_identity_at_slopes_one_and_bends_otherwise() {
        let neutral = CurvePlan {
            sigma: 0.12,
            shadows: 1.0,
            highlights: 1.0,
            clarity: 0.0,
        };
        for x in [-0.3, 0.0, 0.1, 0.33, 0.5, 0.9, 2.0] {
            for i in 0..NUM_GAMMA {
                let g = gamma_from_i(i);
                assert!((curve(x, g, &neutral) - x).abs() < 1e-6, "{x} {g}");
            }
        }
        let steep = CurvePlan {
            shadows: 2.0,
            ..neutral
        };
        // Beyond 2 sigma above g: g + sigma + 2 (x - g - sigma).
        assert!((curve(0.9, 0.5, &steep) - (0.5 + 0.12 + 2.0 * 0.28)).abs() < 1e-6);
        // Below g, the highlights' slope (1): unchanged.
        assert!((curve(0.1, 0.5, &steep) - 0.1).abs() < 1e-6);
        let clear = CurvePlan {
            clarity: 1.0,
            ..neutral
        };
        assert!(curve(0.55, 0.5, &clear) > 0.55);
        assert!(curve(0.45, 0.5, &clear) < 0.45);
    }

    #[test]
    fn the_pyramid_halves_until_a_side_reaches_one_pixel() {
        let sizes = |w, h| {
            levels(Domain::new(0, 0, w, h))
                .iter()
                .map(|d| (d.width, d.height))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sizes(64, 48),
            [(64, 48), (32, 24), (16, 12), (8, 6), (4, 3), (2, 2)]
        );
        assert_eq!(sizes(3, 3), [(3, 3), (2, 2)]);
        assert_eq!(sizes(1, 1), [(1, 1), (1, 1)]);
        assert_eq!(sizes(1 << 14, 1 << 14).len(), MAX_LEVELS);
    }

    #[test]
    fn the_defaults_give_the_input_back() {
        // Slopes 1 and no clarity: every curve is the identity, the
        // pyramid gives the luminance back and the colour is kept.
        let r = Region::sized(90, 70);
        let out = render(&Params::new(), r);
        for (a, b) in out.data().iter().zip(picture().data()) {
            assert!((a - b).abs() < 1e-5, "{a} {b}");
        }
    }

    #[test]
    fn clarity_raises_local_contrast_and_tiles_like_one_render() {
        let params = Params::new()
            .with("clarity", Value::Float(1.0))
            .with("shadows", Value::Float(1.5));
        let region = Region::sized(90, 70);
        let whole = render(&params, region);
        let spread = |img: &Image| {
            let l: Vec<f32> = img
                .data()
                .chunks(4)
                .map(|p| luminance(p[0], p[1], p[2]))
                .collect();
            l.iter().copied().fold(f32::MIN, f32::max) - l.iter().copied().fold(f32::MAX, f32::min)
        };
        assert!(spread(&whole) > spread(&picture()));
        for tile in region.tiles(29) {
            let part = render(&params, tile);
            assert_eq!(part.data(), whole.sample(tile).data(), "tile {tile}");
        }
    }

    #[test]
    fn it_keeps_the_module_contract_and_its_kernels_too() {
        helios_testkit::assert_contract(Curve);
        helios_testkit::assert_contract(Reduce);
        helios_testkit::assert_contract(Expand);
        helios_testkit::assert_contract(Assemble);
        helios_testkit::assert_contract(LlapColour);
        helios_testkit::assert_contract_of(&ModuleRef::composite(VkdtLlap), &Context::new());
    }
}
