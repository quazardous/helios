//! `core.curve1d@1`: a tone curve on luminance applied as a luminance
//! ratio (the hue kept), then a chroma curve over the result's luminance
//! (a chroma ratio at constant luminance), the output clamped to [0, 1]:
//! the tone half of a camera look.
//!
//! - The **tone curve** is knots from black, `[scene, display]`, strictly
//!   increasing in scene and never decreasing in display, read linearly
//!   between knots; past the last knot a shoulder rises toward 1 with the
//!   last segment's slope (`d = d_n + room (1 - exp(-slope (s - s_n) /
//!   room))`, `room = 1 - d_n`). A pixel without luminance turns black.
//!   No knots: no tone curve, the pixels' luminance kept.
//! - The **chroma curve** is [`CHROMA_NODES`] ratios evenly over display
//!   luminance 0 to 1, read linearly: `out = d + k(d) (p - d)` per
//!   channel, `d` the pixel's luminance. Empty: none.
//! - With a tone curve or a chroma curve, the colour channels are clamped
//!   to [0, 1] at the end, as a display render is; with neither the
//!   pixels pass unchanged.
//!
//! Luminance is Rec.2020's ([`crate::huesat::LUMA`]). The CPU form
//! ([`apply`]) is the reference, in single precision; the GPU form
//! computes the same formula.

use helios_core::{
    BrickFamily, BrickPort, BrickSpec, Contract, Family, GpuKernel, Image, Manifest, Module,
    ModuleKind, Needs, ParamKind, ParamSpec, Params, PortSpec, PortType, Region, Role, Shape,
    Space, Tolerance,
};

use crate::huesat::LUMA;

/// The tone curve's knots at most.
pub const MAX_KNOTS: usize = 64;

/// The chroma curve's nodes, evenly over display luminance 0 to 1.
pub const CHROMA_NODES: usize = 8;

/// How far a knot's coordinate or a chroma ratio may go.
const REACH: f64 = 1.0e3;

/// The GPU form, for modules that apply a camera look's tone the same way.
pub const WGSL: &str = include_str!("curve1d.wgsl");

/// The tone curve and the chroma curve in the precision of pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve1dPlan {
    /// `[scene, display]` from black; empty for no tone curve.
    pub knots: Vec<[f32; 2]>,
    /// The chroma ratios, when there is a chroma curve.
    pub chroma: Option<[f32; CHROMA_NODES]>,
}

impl Curve1dPlan {
    /// Neither curve: the pixels pass unchanged.
    pub fn is_identity(&self) -> bool {
        self.knots.is_empty() && self.chroma.is_none()
    }
}

/// `core.curve1d@1`.
pub struct Curve1d;

/// The plan of a `tone` parameter (knots, `[scene, display]` pair after
/// pair) and a `chroma` parameter ([`CHROMA_NODES`] ratios, or none).
///
/// # Errors
/// An odd count, one knot only, more than [`MAX_KNOTS`], knots not from
/// black and increasing in scene or decreasing in display, a chroma curve
/// of another length, a number that is not finite.
pub fn plan(tone: &[f64], chroma: &[f64]) -> Result<Curve1dPlan, String> {
    if !tone.iter().chain(chroma).all(|v| v.is_finite()) {
        return Err("`tone`, `chroma`: a number is not finite".into());
    }
    if !tone.len().is_multiple_of(2) || tone.len() == 2 || tone.len() > 2 * MAX_KNOTS {
        return Err(format!(
            "`tone`: 2 to {MAX_KNOTS} knots, scene then display, or none; {} numbers given",
            tone.len()
        ));
    }
    let knots: Vec<[f64; 2]> = tone.as_chunks::<2>().0.to_vec();
    if let Some(first) = knots.first()
        && *first != [0.0, 0.0]
    {
        return Err("`tone`: the first knot is black, [0, 0]".into());
    }
    if knots
        .windows(2)
        .any(|w| w[1][0] <= w[0][0] || w[1][1] < w[0][1])
    {
        return Err("`tone`: knots increase in scene and never decrease in display".into());
    }
    let chroma = match chroma.len() {
        0 => None,
        CHROMA_NODES => Some(std::array::from_fn(|i| chroma[i] as f32)),
        n => {
            return Err(format!(
                "`chroma`: {CHROMA_NODES} ratios, or none; {n} given"
            ));
        }
    };
    Ok(Curve1dPlan {
        knots: knots.iter().map(|k| [k[0] as f32, k[1] as f32]).collect(),
        chroma,
    })
}

/// `v > 0`, false for NaN.
fn positive(v: f32) -> bool {
    v > 0.0
}

fn luma(p: [f32; 3]) -> f32 {
    LUMA[0] * p[0] + LUMA[1] * p[1] + LUMA[2] * p[2]
}

/// The display luminance of scene luminance `s` on `knots` (at least 2).
pub fn tone(knots: &[[f32; 2]], s: f32) -> f32 {
    if !positive(s) {
        return 0.0;
    }
    let last = knots[knots.len() - 1];
    if s >= last[0] {
        let before = knots[knots.len() - 2];
        let slope = (last[1] - before[1]) / (last[0] - before[0]);
        let room = 1.0 - last[1];
        if !positive(room) || !positive(slope) {
            return last[1];
        }
        return last[1] + room * (1.0 - (-slope * (s - last[0]) / room).exp());
    }
    let i = knots.partition_point(|k| k[0] <= s).max(1);
    let (a, b) = (knots[i - 1], knots[i]);
    a[1] + (b[1] - a[1]) * (s - a[0]) / (b[0] - a[0])
}

/// The chroma ratio at display luminance `d`.
pub fn chroma(nodes: &[f32; CHROMA_NODES], d: f32) -> f32 {
    let x = d.clamp(0.0, 1.0) * (CHROMA_NODES - 1) as f32;
    let i = (x.floor() as usize).min(CHROMA_NODES - 2);
    let f = x - i as f32;
    nodes[i] * (1.0 - f) + nodes[i + 1] * f
}

/// The plan on one pixel's colour channels.
pub fn pixel(plan: &Curve1dPlan, p: [f32; 3]) -> [f32; 3] {
    if plan.is_identity() {
        return p;
    }
    let mut o = p;
    if !plan.knots.is_empty() {
        let s = luma(p);
        o = if s > 0.0 {
            let ratio = tone(&plan.knots, s) / s;
            p.map(|v| v * ratio)
        } else {
            [0.0; 3]
        };
    }
    if let Some(nodes) = &plan.chroma {
        let d = luma(o);
        let k = chroma(nodes, d);
        o = o.map(|v| d + k * (v - d));
    }
    o.map(|v| v.clamp(0.0, 1.0))
}

/// The plan on the first three channels of 4-channel pixels, the fourth
/// kept: the CPU reference.
pub fn apply(plan: &Curve1dPlan, data: &mut [f32]) {
    if plan.is_identity() {
        return;
    }
    for p in data.as_chunks_mut::<4>().0 {
        let o = pixel(plan, [p[0], p[1], p[2]]);
        p[..3].copy_from_slice(&o);
    }
}

/// The plan as the GPU kernel's uniforms, one `vec4` each: the knot count,
/// 1 when there is a chroma curve (0 without), 1 when either curve is
/// there (0 for none); then the chroma ratios (two `vec4`, zeros without);
/// then two knots per `vec4` (zeros past the count).
pub fn uniforms(plan: &Curve1dPlan) -> Vec<f32> {
    let mut u = vec![
        plan.knots.len() as f32,
        if plan.chroma.is_some() { 1.0 } else { 0.0 },
        if plan.is_identity() { 0.0 } else { 1.0 },
        0.0,
    ];
    u.extend(plan.chroma.unwrap_or([0.0; CHROMA_NODES]));
    u.extend(plan.knots.iter().flatten());
    u.resize(4 + CHROMA_NODES + 2 * MAX_KNOTS, 0.0);
    u
}

fn params(p: &Params, name: &str) -> Vec<f64> {
    p.get(name)
        .and_then(|v| v.as_list())
        .map(<[f64]>::to_vec)
        .unwrap_or_default()
}

/// The `tone` and `chroma` parameters' specs, shared with modules that
/// apply a camera look's tone.
pub fn param_specs() -> Vec<ParamSpec> {
    vec![
        ParamSpec {
            name: "tone".into(),
            description: "the tone curve's knots from black, scene then display luminance, \
                          pair after pair; empty for none"
                .into(),
            kind: ParamKind::Floats {
                count: 2 * MAX_KNOTS as u32,
                min: 0.0,
                max: REACH,
                default: Vec::new(),
            },
        },
        ParamSpec {
            name: "chroma".into(),
            description: "the chroma curve: 8 chroma ratios evenly over display luminance 0 \
                          to 1; empty for none"
                .into(),
            kind: ParamKind::Floats {
                count: CHROMA_NODES as u32,
                min: 0.0,
                max: REACH,
                default: Vec::new(),
            },
        },
    ]
}

impl Module for Curve1d {
    type Plan = Curve1dPlan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "core.curve1d".into(),
            version: 1,
            description: "Map luminance through a tone curve, the hue kept, then scale chroma \
                          by a curve over luminance"
                .into(),
            params: param_specs(),
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
            id: "core.curve1d".into(),
            major: 1,
            family: BrickFamily::Tone,
            shape: Shape::Pointwise,
            tolerance: Tolerance::Relative { max: 1.0e-4 },
            purpose: "Map scene luminance to display through a curve, then scale chroma by \
                      display luminance"
                .into(),
            description: "out = clamp(c(T(in))): T multiplies the pixel by tone(Y) / Y, Y its \
                          Rec.2020 luminance and tone a piecewise-linear curve from black with \
                          a shoulder toward 1; c moves each channel from the luminance d by a \
                          ratio read on 8 nodes over d; in single precision, the fourth \
                          channel passed through untouched."
                .into(),
            use_when: "a camera look's tone half: a fitted tone curve and the camera's chroma \
                       falloff, in a display transform's place"
                .into(),
            avoid_when: "a parametric display transform is wanted (sigmoid), or the curve must \
                         act per channel"
                .into(),
            keywords: ["curve", "tone", "luminance", "chroma", "saturation", "look"]
                .map(String::from)
                .to_vec(),
            provenance: "LightCraft's camera-preview tone and chroma curves, as Helios fits \
                         them"
                .into(),
            inputs: vec![BrickPort::new(
                "input",
                PortType::Image {
                    channels: 4,
                    space: Space::Rec2020Linear,
                },
                "scene-linear pixels",
            )],
            outputs: vec![BrickPort::new(
                "output",
                PortType::Image {
                    channels: 4,
                    space: Space::Rec2020Linear,
                },
                "display-linear pixels, in [0, 1]",
            )],
        })
    }

    fn provides(&self) -> Vec<Contract> {
        vec![Contract::Brick]
    }

    fn prepare(&self, params: &Params, _: &helios_core::Context) -> Result<Curve1dPlan, String> {
        plan(
            &self::params(params, "tone"),
            &self::params(params, "chroma"),
        )
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: WGSL,
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, plan: &Curve1dPlan) -> Vec<f32> {
        uniforms(plan)
    }

    fn process(
        &self,
        plan: &Curve1dPlan,
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

    fn run(params: Params, data: Vec<f32>) -> Vec<f32> {
        let params = Curve1d.manifest().resolve(&params).unwrap();
        let plan = Curve1d.prepare(&params, &Context::new()).unwrap();
        let w = (data.len() / 4) as u32;
        let input = Image::new(Region::sized(w, 1), 4, data);
        let out = Curve1d
            .process(&plan, &[&input], Region::sized(w, 1))
            .unwrap();
        out[0].data().to_vec()
    }

    const KNOTS: [f64; 8] = [0.0, 0.0, 0.1, 0.2, 0.4, 0.6, 0.8, 0.85];

    #[test]
    fn no_curve_changes_nothing() {
        let data = vec![0.25, 0.5, 2.0, 0.3, -0.1, 0.0, 0.4, 1.0];
        assert_eq!(run(Params::new(), data.clone()), data);
    }

    #[test]
    fn the_tone_curve_maps_luminance_and_keeps_the_hue() {
        let p = [0.3f32, 0.1, 0.05];
        let out = run(
            Params::new().with("tone", Value::List(KNOTS.to_vec())),
            vec![p[0], p[1], p[2], 0.5],
        );
        let (s, d) = (luma(p), luma([out[0], out[1], out[2]]));
        let knots: Vec<[f32; 2]> = KNOTS
            .as_chunks::<2>()
            .0
            .iter()
            .map(|k| k.map(|v| v as f32))
            .collect();
        assert!((d - tone(&knots, s)).abs() < 1e-6, "{s} -> {d}");
        // The same ratio on every channel: the hue kept.
        for c in 0..3 {
            assert!((out[c] / p[c] - d / s).abs() < 1e-5, "{out:?}");
        }
        assert_eq!(out[3], 0.5);
    }

    #[test]
    fn the_shoulder_rises_toward_one_and_stays_continuous() {
        let knots: Vec<[f32; 2]> = KNOTS
            .as_chunks::<2>()
            .0
            .iter()
            .map(|k| k.map(|v| v as f32))
            .collect();
        let at = tone(&knots, 0.8);
        assert!((at - 0.85).abs() < 1e-6);
        assert!((tone(&knots, 0.800_01) - at).abs() < 1e-4);
        let far = tone(&knots, 100.0);
        assert!(far > 0.99 && far <= 1.0, "{far}");
        assert_eq!(tone(&knots, 0.0), 0.0);
        assert_eq!(tone(&knots, f32::NAN), 0.0);
    }

    #[test]
    fn the_chroma_curve_scales_chroma_at_constant_luminance() {
        let p = [0.6f32, 0.3, 0.2];
        let out = run(
            Params::new().with("chroma", Value::List(vec![0.5; CHROMA_NODES])),
            vec![p[0], p[1], p[2], 1.0],
        );
        let d = luma(p);
        for c in 0..3 {
            assert!((out[c] - (d + 0.5 * (p[c] - d))).abs() < 1e-6, "{out:?}");
        }
    }

    #[test]
    fn bad_curves_are_refused() {
        assert!(plan(&[0.0, 0.0], &[]).is_err());
        assert!(plan(&[0.0, 0.0, 0.5], &[]).is_err());
        assert!(plan(&[0.1, 0.0, 0.5, 0.5], &[]).is_err());
        assert!(plan(&[0.0, 0.0, 0.5, 0.5, 0.4, 0.6], &[]).is_err());
        assert!(plan(&[0.0, 0.0, 0.5, 0.5, 0.6, 0.4], &[]).is_err());
        assert!(plan(&[], &[1.0; 3]).is_err());
        assert!(plan(&[0.0, 0.0, f64::NAN, 0.5], &[]).is_err());
        assert_eq!(plan(&[], &[]), Ok(Curve1dPlan::default()));
    }

    #[test]
    fn the_uniforms_have_one_size() {
        let none = Curve1dPlan::default();
        let some = plan(&KNOTS, &[1.0; CHROMA_NODES]).unwrap();
        assert_eq!(uniforms(&none).len(), uniforms(&some).len());
        assert_eq!(uniforms(&none).len() % 4, 0);
        assert_eq!(uniforms(&none)[2], 0.0);
        assert_eq!(&uniforms(&some)[..3], &[4.0, 1.0, 1.0]);
        // The WGSL's array: one vec4 per four numbers.
        assert!(WGSL.contains(&format!("array<vec4<f32>, {}>", uniforms(&none).len() / 4)));
    }

    #[test]
    fn contract() {
        helios_testkit::assert_contract(Curve1d);
    }
}
