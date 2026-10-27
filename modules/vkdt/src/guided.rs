// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/shared/blur.comp, src/pipe/modules/shared/blurs.comp,
// src/pipe/modules/shared/blurh.comp, src/pipe/modules/shared/blurv.comp,
// src/pipe/modules/shared/blur_head.glsl, src/pipe/modules/shared/guided1.comp,
// src/pipe/modules/shared/guided2.comp, src/pipe/modules/shared/guided3.comp,
// src/pipe/modules/shared/resample.comp, src/pipe/modules/shared.glsl,
// src/pipe/modules/api.h. vkdt: copyright 2019 johannes hanika, under the
// BSD-2-Clause licence; its notice is in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

//! vkdt's shared kernels for blurs and guided filters (`pipe/modules/shared`
//! and `dt_api_blur`, `dt_api_guided_filter` in `pipe/modules/api.h`), each
//! a module of its own, so that a multi-kernel module wires them into its
//! network: the engine then plans, caches and tiles every one.
//!
//! vkdt works in texture coordinates over images of a given size; here a
//! kernel works in absolute pixel coordinates (a pixel's centre at its
//! integer position), so any region of it can be computed. A blur that
//! halves the resolution maps output pixel `o` to input position
//! `2 o + 0.5`, which is vkdt's mapping when the image's sides divide by
//! two at every level; vkdt rounds odd sides up, and then differs from it
//! by a fraction of a pixel.
//!
//! The kernels reading one image have a GPU variant (WGSL translated from
//! the same GLSL); `vkdt_guided3`, reading two, runs on the CPU.

use helios_core::{
    Context, Error, Family, GpuKernel, Image, Manifest, Module, ModuleKind, ModuleRef, Needs,
    Network, ParamKind, ParamSpec, Params, PortSpec, Region, Role, Value, Wire,
};

/// vkdt's `luminance_rec2020`: the Y row of Rec.2020 to XYZ.
pub fn luminance(r: f32, g: f32, b: f32) -> f32 {
    2.627_002e-1 * r + 6.779_981e-1 * g + 5.930_171_6e-2 * b
}

/// Channel `c` of `image` at pixel `(x, y)`, edges held.
pub(crate) fn texel(image: &Image, x: i64, y: i64, c: usize) -> f32 {
    let r = image.region();
    let x = x.clamp(r.x, r.right() - 1);
    let y = y.clamp(r.y, r.bottom() - 1);
    image.pixel(x, y)[c]
}

/// Channel `c` of `image` at position `(px, py)` (pixel centres at whole
/// numbers), bilinear, edges held.
pub(crate) fn bilinear(image: &Image, px: f32, py: f32, c: usize) -> f32 {
    let (x0, y0) = (px.floor(), py.floor());
    let (fx, fy) = (px - x0, py - y0);
    let (ix, iy) = (x0 as i64, y0 as i64);
    let mix = |a: f32, b: f32, t: f32| a * (1.0 - t) + b * t;
    let top = mix(texel(image, ix, iy, c), texel(image, ix + 1, iy, c), fx);
    let bottom = mix(
        texel(image, ix, iy + 1, c),
        texel(image, ix + 1, iy + 1, c),
        fx,
    );
    mix(top, bottom, fy)
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
        family: Family::Detail,
        role: Role::Specialised,
    }
}

fn flag(name: &str, description: &str) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        description: description.into(),
        kind: ParamKind::Bool { default: false },
    }
}

fn kernel(wgsl: &'static str) -> GpuKernel {
    GpuKernel {
        wgsl,
        entry: "main",
    }
}

/// `guided1`: luminance and its square (clamped to 10), from RGBA.
pub struct Guided1;

impl Module for Guided1 {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        kernel_manifest(
            "vkdt_guided1",
            "a guided filter's first kernel: luminance I and I*I",
            vec![],
            vec![PortSpec::image("input", 4)],
            2,
        )
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        Ok(vec![Image::from_fn(output, 2, |x, y, c| {
            let p = input.pixel(x, y);
            let i = luminance(p[0], p[1], p[2]);
            if c == 0 { i } else { (i * i).clamp(0.0, 10.0) }
        })])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(kernel(include_str!("wgsl/guided1.wgsl")))
    }
}

/// `guided2`: from the blurred `(I, I*I)`, the linear model `(a, b)` with
/// `a = var / (var + edges^2 / 1000)` and `b = mean - a * mean`.
pub struct Guided2;

impl Module for Guided2 {
    /// `edges^2 / 1000`.
    type Plan = f32;

    fn manifest(&self) -> Manifest {
        kernel_manifest(
            "vkdt_guided2",
            "a guided filter's second kernel: the linear model (a, b)",
            vec![ParamSpec::float(
                "edges",
                "the edge threshold (epsilon)",
                0.001,
                10.0,
                0.03,
            )],
            vec![PortSpec::image("input", 2)],
            2,
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<f32, String> {
        let e = params.float("edges") as f32;
        Ok(e * e * 1e-3)
    }

    fn process(&self, eps: &f32, inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        let mut data = Vec::with_capacity(output.area() * 2);
        for y in output.y..output.bottom() {
            for x in output.x..output.right() {
                let p = input.pixel(x, y);
                let var = p[1] - p[0] * p[0];
                let a = var / (var + eps);
                data.extend([a, p[0] - a * p[0]]);
            }
        }
        Ok(vec![Image::new(output, 2, data)])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(kernel(include_str!("wgsl/guided2.wgsl")))
    }

    fn gpu_uniforms(&self, eps: &f32) -> Vec<f32> {
        vec![*eps]
    }
}

/// `guided3` (grey 0): the filtered luminance `a * I + b`, `I` the
/// input's luminance, `(a, b)` the blurred model.
pub struct Guided3;

impl Module for Guided3 {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        kernel_manifest(
            "vkdt_guided3",
            "a guided filter's last kernel: the filtered luminance a * I + b",
            vec![],
            vec![PortSpec::image("input", 4), PortSpec::image("ab", 2)],
            1,
        )
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let (input, ab) = (inputs[0], inputs[1]);
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            let p = input.pixel(x, y);
            let m = ab.pixel(x, y);
            m[0] * luminance(p[0], p[1], p[2]) + m[1]
        })])
    }
}

/// `blur`: vkdt's 5x5 a-trous kernel (weights 1 4 6 4 1 / 16) over two
/// channels, sampled bilinearly; with `down`, at half the resolution.
pub struct Blur;

/// The a-trous weights.
const ATROUS: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

impl Module for Blur {
    /// Halves the resolution.
    type Plan = bool;

    fn manifest(&self) -> Manifest {
        kernel_manifest(
            "vkdt_blur",
            "vkdt's 5x5 blur, at the same or half the resolution",
            vec![flag("down", "output at half the resolution")],
            vec![PortSpec::image("input", 2)],
            2,
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<bool, String> {
        Ok(params.get("down").and_then(Value::as_bool) == Some(true))
    }

    fn input_region(&self, down: &bool, r: Region, _: u32) -> Region {
        if *down {
            // Output pixel o reads 2o - 2 to 2o + 3.
            Region::new(2 * r.x - 2, 2 * r.y - 2, 2 * r.width + 4, 2 * r.height + 4)
        } else {
            r.grow(2)
        }
    }

    fn process(
        &self,
        down: &bool,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        let mut data = Vec::with_capacity(output.area() * 2);
        for y in output.y..output.bottom() {
            for x in output.x..output.right() {
                let (cx, cy) = if *down {
                    (2.0 * x as f32 + 0.5, 2.0 * y as f32 + 0.5)
                } else {
                    (x as f32, y as f32)
                };
                let mut sum = [0.0f32; 2];
                for (j, wy) in ATROUS.iter().enumerate() {
                    for (i, wx) in ATROUS.iter().enumerate() {
                        let (px, py) = (cx + i as f32 - 2.0, cy + j as f32 - 2.0);
                        for (c, s) in sum.iter_mut().enumerate() {
                            *s += wx * wy * bilinear(input, px, py, c);
                        }
                    }
                }
                data.extend(sum);
            }
        }
        Ok(vec![Image::new(output, 2, data)])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(kernel(include_str!("wgsl/blur.wgsl")))
    }

    fn gpu_uniforms(&self, down: &bool) -> Vec<f32> {
        vec![if *down { 1.0 } else { 0.0 }]
    }
}

/// `blurs`: a small blur for a small sigma, four bilinear taps over a 3x3
/// support, at `±exp(-1 / (2 sigma^2))` from the centre.
pub struct BlurSmall;

impl Module for BlurSmall {
    /// The taps' offset.
    type Plan = f32;

    fn manifest(&self) -> Manifest {
        kernel_manifest(
            "vkdt_blurs",
            "vkdt's small blur (sigma up to 0.85)",
            vec![ParamSpec::float(
                "sigma",
                "the blur's sigma in pixels",
                0.0,
                1.0,
                0.5,
            )],
            vec![PortSpec::image("input", 2)],
            2,
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<f32, String> {
        let sigma = params.float("sigma") as f32;
        Ok((-1.0 / (2.0 * sigma * sigma)).exp())
    }

    fn input_region(&self, _: &f32, r: Region, _: u32) -> Region {
        r.grow(1)
    }

    fn process(&self, off: &f32, inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        let off = *off;
        Ok(vec![Image::from_fn(output, 2, |x, y, c| {
            let (x, y, c) = (x as f32, y as f32, c as usize);
            (bilinear(input, x - off, y - off, c)
                + bilinear(input, x - off, y + off, c)
                + bilinear(input, x + off, y - off, c)
                + bilinear(input, x + off, y + off, c))
                / 4.0
        })])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(kernel(include_str!("wgsl/blurs.wgsl")))
    }

    fn gpu_uniforms(&self, off: &f32) -> Vec<f32> {
        vec![*off]
    }
}

/// `blurh` and `blurv`: one pass of a separable Gaussian, `floor(radius)`
/// taps on each side, sigma a third of the radius.
pub struct BlurPass;

/// A pass's taps on each side and their weights (the centre's first).
#[derive(Clone, Debug, PartialEq)]
pub struct PassPlan {
    pub vertical: bool,
    pub weights: Vec<f32>,
}

impl Module for BlurPass {
    type Plan = PassPlan;

    fn manifest(&self) -> Manifest {
        kernel_manifest(
            "vkdt_blurpass",
            "one pass of vkdt's separable Gaussian blur (blurh, blurv)",
            vec![
                ParamSpec::float(
                    "radius",
                    "the blur's radius in pixels (3 sigma)",
                    0.0,
                    64.0,
                    3.0,
                ),
                flag("vertical", "along columns rather than rows"),
            ],
            vec![PortSpec::image("input", 2)],
            2,
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<PassPlan, String> {
        let radius = params.float("radius") as f32;
        let taps = radius.floor() as usize;
        let sigma = radius / 3.0;
        let a = 0.5 / (sigma * sigma);
        Ok(PassPlan {
            vertical: params.get("vertical").and_then(Value::as_bool) == Some(true),
            // The centre weighs 1 whatever the radius (a radius of 0 is a
            // copy).
            weights: std::iter::once(1.0)
                .chain((1..=taps).map(|k| (-a * (k * k) as f32).exp()))
                .collect(),
        })
    }

    fn input_region(&self, plan: &PassPlan, r: Region, _: u32) -> Region {
        let taps = plan.weights.len() as i64 - 1;
        let n = 2 * (plan.weights.len() as u32 - 1);
        if plan.vertical {
            Region::new(r.x, r.y - taps, r.width, r.height + n)
        } else {
            Region::new(r.x - taps, r.y, r.width + n, r.height)
        }
    }

    fn process(
        &self,
        plan: &PassPlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        let total: f32 = plan.weights[0] + 2.0 * plan.weights[1..].iter().sum::<f32>();
        let (dx, dy) = if plan.vertical { (0, 1) } else { (1, 0) };
        Ok(vec![Image::from_fn(output, 2, |x, y, c| {
            let c = c as usize;
            let mut sum = plan.weights[0] * input.pixel(x, y)[c];
            for (k, w) in plan.weights.iter().enumerate().skip(1) {
                let k = k as i64;
                sum += w
                    * (input.pixel(x + k * dx, y + k * dy)[c]
                        + input.pixel(x - k * dx, y - k * dy)[c]);
            }
            sum / total
        })])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(kernel(include_str!("wgsl/blurpass.wgsl")))
    }

    fn gpu_uniforms(&self, plan: &PassPlan) -> Vec<f32> {
        let mut u = vec![
            if plan.vertical { 1.0 } else { 0.0 },
            plan.weights.len() as f32 - 1.0,
        ];
        u.extend(&plan.weights);
        // The kernel declares 17 vec4: 2 + 65 weights at the largest radius.
        u.resize(68, 0.0);
        u
    }
}

/// `resample`: back to `2^levels` times the resolution with a Catmull-Rom
/// kernel (4x4 taps).
pub struct Resample;

impl Module for Resample {
    /// The factor.
    type Plan = u32;

    fn manifest(&self) -> Manifest {
        kernel_manifest(
            "vkdt_resample",
            "vkdt's Catmull-Rom upsampling, by a power of two",
            vec![ParamSpec {
                name: "levels".into(),
                description: "the resolution doubles this many times".into(),
                kind: ParamKind::Int {
                    min: 0,
                    max: 10,
                    default: 1,
                },
            }],
            vec![PortSpec::image("input", 2)],
            2,
        )
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<u32, String> {
        let levels = params.get("levels").and_then(Value::as_i64).unwrap_or(1);
        Ok(1 << levels)
    }

    fn input_region(&self, factor: &u32, r: Region, _: u32) -> Region {
        let f = i64::from(*factor);
        let first = |o: i64| catmull_first(o, f);
        let (x0, y0) = (first(r.x) - 1, first(r.y) - 1);
        let (x1, y1) = (first(r.right() - 1) + 2, first(r.bottom() - 1) + 2);
        Region::new(x0, y0, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32)
    }

    fn process(
        &self,
        factor: &u32,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        let f = i64::from(*factor);
        let mut data = Vec::with_capacity(output.area() * 2);
        for y in output.y..output.bottom() {
            let (ty, wy) = catmull(y, f);
            for x in output.x..output.right() {
                let (tx, wx) = catmull(x, f);
                let mut sum = [0.0f32; 2];
                for (j, wj) in wy.iter().enumerate() {
                    for (i, wi) in wx.iter().enumerate() {
                        let p = input.pixel(tx + i as i64 - 1, ty + j as i64 - 1);
                        sum[0] += wi * wj * p[0];
                        sum[1] += wi * wj * p[1];
                    }
                }
                data.extend(sum);
            }
        }
        Ok(vec![Image::new(output, 2, data)])
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(kernel(include_str!("wgsl/resample.wgsl")))
    }

    fn gpu_uniforms(&self, factor: &u32) -> Vec<f32> {
        vec![*factor as f32]
    }
}

/// The second of the four input pixels output pixel `o` reads, upsampling
/// by `f`.
fn catmull_first(o: i64, f: i64) -> i64 {
    // ((o + 0.5) / f - 0.5).floor(), exactly: (2o + 1 - f) / 2f.
    (2 * o + 1 - f).div_euclid(2 * f)
}

/// The second input pixel output pixel `o` reads, and the four weights.
fn catmull(o: i64, f: i64) -> (i64, [f32; 4]) {
    let t = catmull_first(o, f);
    let s = (o as f32 + 0.5) / f as f32 - 0.5 - t as f32;
    (
        t,
        [
            s * (-0.5 + s * (1.0 - 0.5 * s)),
            1.0 + s * s * (-2.5 + 1.5 * s),
            s * (0.5 + s * (2.0 - 1.5 * s)),
            s * s * (-0.5 + 0.5 * s),
        ],
    )
}

fn module_error(e: Error) -> String {
    e.to_string()
}

/// Adds vkdt's blur of `radius` pixels (twice its sigma) to `n`, reading
/// `from` (two channels), as `dt_api_blur` chooses it for an image `width`
/// pixels wide: a small blur below 2 pixels, one 5x5 blur at 2, else a
/// cascade of 5x5 blurs halving the resolution (a separable pass first
/// when the cascade alone falls short), upsampled back. Gives the blur's
/// output.
pub fn blur(
    n: &mut Network,
    from: Wire,
    radius: f32,
    width: u32,
    name: &str,
) -> Result<Wire, String> {
    let add = |n: &mut Network, label: String, module: ModuleRef, params: Params, from: Wire| {
        let node = n
            .chain(&label, module, &params, from)
            .map_err(module_error)?;
        n.output(node, "output").map_err(module_error)
    };
    let small = |n: &mut Network, sigma: f32| {
        add(
            n,
            format!("{name}.blurs"),
            ModuleRef::new(BlurSmall),
            Params::new().with("sigma", Value::Float(f64::from(sigma))),
            from,
        )
    };
    if radius < 2.0 {
        return small(n, 0.5 * radius);
    }
    if radius == 2.0 {
        return add(
            n,
            format!("{name}.blur"),
            ModuleRef::new(Blur),
            Params::new(),
            from,
        );
    }
    let radius = radius.clamp(1.0, (width / 2).max(1) as f32);
    let req = radius * radius * 0.25;
    let mut sig2 = 0.0f32;
    let mut mul = [0u32; 10];
    let mut it = 0;
    while it < 10 {
        let sig = (1u32 << it) as f32;
        if sig2 + sig * sig > req {
            break;
        }
        sig2 += sig * sig;
        mul[it] = 1;
        it += 1;
    }
    for j in (1..=it.min(9)).rev() {
        let sig = (1u32 << j) as f32;
        for _ in 0..10 {
            if sig2 + sig * sig < req {
                sig2 += sig * sig;
                mul[j] += 1;
            } else {
                break;
            }
        }
    }
    let rem = (req - sig2).max(0.0).sqrt();
    if it == 0 {
        return small(n, 0.5 * radius);
    }
    let mut at = from;
    if rem != 0.0 && it < 2 {
        let r = f64::from(3.0 * rem);
        for vertical in [false, true] {
            at = add(
                n,
                format!("{name}.blur{}", if vertical { "v" } else { "h" }),
                ModuleRef::new(BlurPass),
                Params::new()
                    .with("radius", Value::Float(r))
                    .with("vertical", Value::Bool(vertical)),
                at,
            )?;
        }
    }
    // mul[it] may hold extra passes at the first level the cascade did not
    // reach (`dt_api_blur_sub` only runs the first `it` levels).
    for (level, &times) in mul.iter().enumerate().take(it) {
        for j in 0..times {
            at = add(
                n,
                format!("{name}.blur{level}.{j}"),
                ModuleRef::new(Blur),
                Params::new().with("down", Value::Bool(j + 1 == times)),
                at,
            )?;
        }
    }
    add(
        n,
        format!("{name}.resample"),
        ModuleRef::new(Resample),
        Params::new().with("levels", Value::Int(it as i64)),
        at,
    )
}

/// Adds vkdt's guided filter without a guide image (`dt_api_guided_filter`,
/// the guide being the input itself) to `n`, reading the RGBA `input`:
/// `radius` in pixels, `edges` its edge threshold, for an image `width`
/// pixels wide. Gives the filtered luminance (one channel).
pub fn guided_filter(
    n: &mut Network,
    input: Wire,
    radius: f32,
    edges: f64,
    width: u32,
) -> Result<Wire, String> {
    let g1 = n
        .chain("guided1", ModuleRef::new(Guided1), &Params::new(), input)
        .map_err(module_error)?;
    let g1 = n.output(g1, "output").map_err(module_error)?;
    let mean = blur(n, g1, radius, width, "mean")?;
    let g2 = n
        .chain(
            "guided2",
            ModuleRef::new(Guided2),
            &Params::new().with("edges", Value::Float(edges)),
            mean,
        )
        .map_err(module_error)?;
    let g2 = n.output(g2, "output").map_err(module_error)?;
    let ab = blur(n, g2, radius, width, "ab")?;
    let g3 = n
        .add("guided3", ModuleRef::new(Guided3), &Params::new())
        .map_err(module_error)?;
    n.connect(input, g3, "input").map_err(module_error)?;
    n.connect(ab, g3, "ab").map_err(module_error)?;
    n.output(g3, "output").map_err(module_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catmull_rom_reads_four_pixels_around_and_keeps_a_constant() {
        for f in [2, 4, 8] {
            for o in -9..20 {
                let (t, w) = catmull(o, f);
                let centre = (o as f32 + 0.5) / f as f32 - 0.5;
                assert!(t as f32 <= centre && centre < t as f32 + 1.0, "{o} {f}");
                assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn the_blur_cascade_is_vkdts() {
        let names = |radius: f32| {
            let mut n = Network::with_ports(
                vec![PortSpec::image("input", 2)],
                vec![PortSpec::image("output", 2)],
            );
            let i = n.input("input").unwrap();
            blur(&mut n, i, radius, 256, "b").unwrap();
            n.nodes().iter().map(|n| n.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(names(1.5), ["b.blurs"]);
        assert_eq!(names(2.0), ["b.blur"]);
        // 2.56 pixels: sigma^2 1.64 = 1 (level 0) + a separable rest.
        assert_eq!(
            names(2.56),
            ["b.blurh", "b.blurv", "b.blur0.0", "b.resample"]
        );
        // 7.68 pixels: sigma^2 14.7 = 1 + 3 * 4, levels 0 and 1.
        assert_eq!(
            names(7.68),
            [
                "b.blur0.0",
                "b.blur1.0",
                "b.blur1.1",
                "b.blur1.2",
                "b.resample"
            ]
        );
    }

    #[test]
    fn every_kernel_keeps_the_module_contract() {
        helios_testkit::assert_contract(Guided1);
        helios_testkit::assert_contract(Guided2);
        helios_testkit::assert_contract(Guided3);
        helios_testkit::assert_contract(Blur);
        helios_testkit::assert_contract(BlurSmall);
        helios_testkit::assert_contract(BlurPass);
        helios_testkit::assert_contract(Resample);
    }
}
