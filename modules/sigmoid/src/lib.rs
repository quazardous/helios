//! Sigmoid, ported from darktable's `sigmoid` module (`src/iop/sigmoid.c`),
//! per-channel method: the tone mapper that takes scene-referred light (any
//! value from 0 up) to the display's range.
//!
//! The curve is darktable's generalized log-logistic:
//! `white * ((fog + x)^film / (paper_exp + (fog + x)^film))^paper`. Its
//! parameters are solved as darktable's `commit_params` does: middle grey
//! (0.1845) stays middle grey, the slope there is set by `contrast` whatever
//! the `skew`, scene black reaches the display's black target and scene
//! infinity its white target. Each channel goes through the curve, negative
//! values are first desaturated to zero, then the middle channel is moved
//! back toward its original hue at constant energy (`preserve hue`).
//!
//! Not ported yet: the RGB-ratio method, and the primaries adjustments
//! (base primaries, per-primary inset and rotation, purity): this works in
//! the working space's own primaries.

use helios_core::{
    Context, Family, GpuKernel, Image, Manifest, Module, ModuleKind, Needs, ParamHint, ParamSpec,
    Params, PortSpec, Region, Role,
};

pub struct Sigmoid;

const MIDDLE_GREY: f32 = 0.1845;

/// The solved curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub white: f32,
    pub paper_exposure: f32,
    pub film_fog: f32,
    pub film_power: f32,
    pub paper_power: f32,
    pub hue_preservation: f32,
}

fn loglogistic(x: f32, magnitude: f32, paper_exp: f32, fog: f32, film: f32, paper: f32) -> f32 {
    let film_response = (fog + x.max(0.0)).powf(film);
    let v = magnitude * (film_response / (paper_exp + film_response)).powf(paper);
    if v.is_nan() { magnitude } else { v }
}

impl Plan {
    pub fn curve(&self, x: f32) -> f32 {
        loglogistic(
            x,
            self.white,
            self.paper_exposure,
            self.film_fog,
            self.film_power,
            self.paper_power,
        )
    }

    /// darktable's `commit_params`.
    fn solve(contrast: f32, skew: f32, white_pct: f32, black_pct: f32, hue: f32) -> Plan {
        let delta = 1e-6f32;
        let slope = |magnitude, paper_exp, fog, film, paper| {
            (loglogistic(MIDDLE_GREY + delta, magnitude, paper_exp, fog, film, paper)
                - loglogistic(MIDDLE_GREY - delta, magnitude, paper_exp, fog, film, paper))
                / 2.0
                / delta
        };
        // A reference slope with no skew on a normalized display.
        let ref_paper_exposure = MIDDLE_GREY.powf(contrast) * (1.0 / MIDDLE_GREY - 1.0);
        let ref_slope = slope(1.0, ref_paper_exposure, 0.0, contrast, 1.0);
        let paper_power = 5.0f32.powf(-skew);
        // The slope at film power 1 with the skew: what film power reaches the reference.
        let white = 0.01 * white_pct;
        let temp_relation = (white / MIDDLE_GREY).powf(1.0 / paper_power) - 1.0;
        let temp_slope = slope(white, MIDDLE_GREY * temp_relation, 0.0, 1.0, paper_power);
        let film_power = ref_slope / temp_slope;
        let black = 0.01 * black_pct;
        let white_grey = (white / MIDDLE_GREY).powf(1.0 / paper_power) - 1.0;
        let white_black = (black / white).powf(-1.0 / paper_power) - 1.0;
        let film_fog = MIDDLE_GREY * white_grey.powf(1.0 / film_power)
            / (white_black.powf(1.0 / film_power) - white_grey.powf(1.0 / film_power));
        let paper_exposure = (film_fog + MIDDLE_GREY).powf(film_power) * white_grey;
        Plan {
            white,
            paper_exposure,
            film_fog,
            film_power,
            paper_power,
            hue_preservation: (0.01 * hue).clamp(0.0, 1.0),
        }
    }
}

/// Negative values pulled to zero by desaturating toward the pixel's mean.
fn desaturate_negative(p: [f32; 3]) -> [f32; 3] {
    let average = ((p[0] + p[1] + p[2]) / 3.0).max(0.0);
    let min = p[0].min(p[1]).min(p[2]);
    let factor = if min < 0.0 {
        -average / (min - average)
    } else {
        1.0
    };
    p.map(|v| average + factor * (v - average))
}

/// Indices of the largest, middle and smallest channels, as darktable orders them.
fn order(p: [f32; 3]) -> (usize, usize, usize) {
    if p[0] >= p[1] {
        if p[1] > p[2] {
            (0, 1, 2)
        } else if p[2] > p[0] {
            (2, 0, 1)
        } else if p[2] > p[1] {
            (0, 2, 1)
        } else {
            (0, 1, 2)
        }
    } else if p[0] >= p[2] {
        (1, 0, 2)
    } else if p[2] > p[1] {
        (2, 1, 0)
    } else {
        (1, 2, 0)
    }
}

/// darktable's `_preserve_hue_and_energy`.
fn preserve_hue(input: [f32; 3], curved: [f32; 3], hue: f32) -> [f32; 3] {
    let (max, mid, min) = order(input);
    let chroma = input[max] - input[min];
    let midscale = if chroma != 0.0 {
        (input[mid] - input[min]) / chroma
    } else {
        0.0
    };
    let full = curved[min] + (curved[max] - curved[min]) * midscale;
    let naive_mid = (1.0 - hue) * curved[mid] + hue * full;
    let energy = curved[0] + curved[1] + curved[2];
    let naive_energy = curved[min] + naive_mid + curved[max];
    let min_plus_mid = input[min] + input[mid];
    let blend = if min_plus_mid != 0.0 {
        2.0 * input[min] / min_plus_mid
    } else {
        0.0
    };
    let target = blend * energy + (1.0 - blend) * naive_energy;
    let mut out = [0.0; 3];
    if naive_mid <= curved[mid] {
        let corrected = ((1.0 - hue) * curved[mid]
            + hue * (midscale * curved[max] + (1.0 - midscale) * (target - curved[max])))
            / (1.0 + hue * (1.0 - midscale));
        out[min] = target - curved[max] - corrected;
        out[mid] = corrected;
        out[max] = curved[max];
    } else {
        let corrected = ((1.0 - hue) * curved[mid]
            + hue * (curved[min] * (1.0 - midscale) + midscale * (target - curved[min])))
            / (1.0 + hue * midscale);
        out[min] = curved[min];
        out[mid] = corrected;
        out[max] = target - curved[min] - corrected;
    }
    out
}

impl Module for Sigmoid {
    type Plan = Plan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "sigmoid".into(),
            version: 1,
            description: "Map scene light to the display's range (tone mapper)".into(),
            params: vec![
                ParamSpec::float("contrast", "contrast around middle grey", 0.1, 10.0, 1.5),
                ParamSpec::float(
                    "skew",
                    "skews the contrast toward shadows or highlights",
                    -1.0,
                    1.0,
                    0.0,
                ),
                ParamSpec::float("white_target", "display white, in %", 20.0, 1600.0, 100.0),
                ParamSpec::float("black_target", "display black, in %", 0.0, 15.0, 0.0152),
                ParamSpec::float(
                    "preserve_hue",
                    "how much of the hue to keep, in %",
                    0.0,
                    100.0,
                    100.0,
                ),
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

    fn param_hints(&self) -> Vec<ParamHint> {
        vec![
            ParamHint::new("contrast")
                .soft(0.5, 3.0)
                .steps(0.01, 0.1)
                .display(1.0, 2),
            ParamHint::new("skew").steps(0.01, 0.1).display(1.0, 2),
            ParamHint::new("white_target")
                .soft(50.0, 400.0)
                .steps(1.0, 10.0)
                .unit("%")
                .display(1.0, 1),
            ParamHint::new("preserve_hue")
                .steps(1.0, 10.0)
                .unit("%")
                .display(1.0, 0),
        ]
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<Plan, String> {
        let f = |n: &str| params.float(n) as f32;
        let plan = Plan::solve(
            f("contrast"),
            f("skew"),
            f("white_target"),
            f("black_target"),
            f("preserve_hue"),
        );
        let solved = [
            plan.paper_exposure,
            plan.film_fog,
            plan.film_power,
            plan.paper_power,
        ];
        if solved.iter().all(|v| v.is_finite()) {
            Ok(plan)
        } else {
            Err(format!("no curve fits these targets: {plan:?}"))
        }
    }

    fn gpu_kernel(&self) -> Option<GpuKernel> {
        Some(GpuKernel {
            wgsl: include_str!("sigmoid.wgsl"),
            entry: "main",
        })
    }

    fn gpu_uniforms(&self, p: &Plan) -> Vec<f32> {
        vec![
            p.white,
            p.paper_exposure,
            p.film_fog,
            p.film_power,
            p.paper_power,
            p.hue_preservation,
        ]
    }

    fn process(
        &self,
        plan: &Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let mut data = inputs[0].data().to_vec();
        for p in data.as_chunks_mut::<4>().0 {
            let input = desaturate_negative([p[0], p[1], p[2]]);
            let curved = input.map(|v| plan.curve(v));
            let out = preserve_hue(input, curved, plan.hue_preservation);
            p[..3].copy_from_slice(&out);
        }
        Ok(vec![Image::new(output, 4, data)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::Value;

    fn plan(params: Params) -> Plan {
        Sigmoid
            .prepare(
                &Sigmoid.manifest().resolve(&params).unwrap(),
                &Context::new(),
            )
            .unwrap()
    }

    #[test]
    fn middle_grey_stays_and_the_ends_reach_the_targets() {
        for skew in [-0.5, 0.0, 0.5] {
            let p = plan(Params::new().with("skew", Value::Float(skew)));
            assert!(
                (p.curve(MIDDLE_GREY) - MIDDLE_GREY).abs() < 1e-4,
                "skew {skew}: {}",
                p.curve(MIDDLE_GREY)
            );
            assert!(
                (p.curve(0.0) - 0.000152).abs() < 1e-5,
                "black {}",
                p.curve(0.0)
            );
            assert!(p.curve(1e6) <= 1.0 && p.curve(1e6) > 0.99);
        }
    }

    #[test]
    fn the_curve_rises_and_contrast_steepens_it_at_grey() {
        let p = plan(Params::new());
        let mut last = -1.0;
        for i in 0..200 {
            let v = p.curve(i as f32 * 0.05);
            assert!(v >= last, "not monotonic at {i}");
            last = v;
        }
        let soft = plan(Params::new().with("contrast", Value::Float(1.0)));
        let hard = plan(Params::new().with("contrast", Value::Float(2.5)));
        let slope = |p: &Plan| p.curve(MIDDLE_GREY * 1.1) - p.curve(MIDDLE_GREY * 0.9);
        assert!(slope(&hard) > slope(&soft));
    }

    #[test]
    fn hue_is_kept_and_greys_stay_grey() {
        let p = plan(Params::new());
        let run = |rgb: [f32; 3]| {
            let img = Image::new(Region::sized(1, 1), 4, vec![rgb[0], rgb[1], rgb[2], 0.7]);
            let out = Sigmoid.process(&p, &[&img], Region::sized(1, 1)).unwrap();
            <[f32; 4]>::try_from(out[0].data()).unwrap()
        };
        let grey = run([2.0, 2.0, 2.0]);
        // Equal in exact arithmetic; darktable's energy-preserving steps leave
        // a rounding error in floats.
        assert!(
            (grey[0] - grey[1]).abs() < 1e-6 && (grey[1] - grey[2]).abs() < 1e-6,
            "{grey:?}"
        );
        assert_eq!(grey[3], 0.7);
        // An orange keeps the order and the relative place of its middle channel.
        let (r, g, b) = (1.2f32, 0.6, 0.2);
        let o = run([r, g, b]);
        assert!(o[0] > o[1] && o[1] > o[2], "{o:?}");
        let before = (g - b) / (r - b);
        let after = (o[1] - o[2]) / (o[0] - o[2]);
        assert!((before - after).abs() < 1e-3, "{before} vs {after}");
        // Negative values are desaturated, never passed on.
        assert!(run([0.5, -0.1, 0.2])[..3].iter().all(|v| *v >= 0.0));
    }

    #[test]
    fn it_keeps_the_module_contract() {
        helios_testkit::assert_contract(Sigmoid);
    }
}
