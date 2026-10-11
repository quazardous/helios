//! A Gaussian blur whose radius is a size in the picture, not in pixels of
//! the render: given in full-resolution pixels, it shrinks with the render's
//! scale, so a preview at half resolution looks like the full render
//! reduced. The overlap it reads around a tile follows the same scale.
//!
//! It is the framework's test case for neighbourhood reads (overlap, tiles,
//! scale), and the base of darktable's blurs and local contrast.

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamSpec, Params, PortSpec,
    Region, Role,
};

pub struct Blur;

/// The kernel at the render's scale.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// Normalized weights from the centre outward; `weights.len() - 1` is
    /// the reach in pixels.
    pub weights: Vec<f32>,
}

impl Plan {
    fn reach(&self) -> i64 {
        self.weights.len() as i64 - 1
    }

    fn new(sigma: f64) -> Plan {
        if sigma < 0.01 {
            return Plan { weights: vec![1.0] };
        }
        let reach = (3.0 * sigma).ceil() as usize;
        let raw: Vec<f64> = (0..=reach)
            .map(|i| (-(i as f64).powi(2) / (2.0 * sigma * sigma)).exp())
            .collect();
        let total = raw[0] + 2.0 * raw[1..].iter().sum::<f64>();
        Plan {
            weights: raw.iter().map(|w| (w / total) as f32).collect(),
        }
    }
}

impl Module for Blur {
    type Plan = Plan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "blur".into(),
            version: 1,
            description: "Gaussian blur, its radius in full-resolution pixels".into(),
            params: vec![ParamSpec::float(
                "radius",
                "standard deviation, in full-resolution pixels",
                0.0,
                64.0,
                4.0,
            )],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![],
            // The real reach comes from the plan: see `input_region`.
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Detail,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<Plan, String> {
        self.prepare_at(params, context, 1.0)
    }

    fn prepare_at(&self, params: &Params, _: &Context, scale: f64) -> Result<Plan, String> {
        Ok(Plan::new(params.float("radius") * scale))
    }

    fn input_region(&self, plan: &Plan, output: Region, _: u32) -> Region {
        output.grow(plan.reach() as u32)
    }

    fn process(
        &self,
        plan: &Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        let reach = plan.reach();
        let w = &plan.weights;
        // Rows first, over the output's columns and the rows the column pass reads.
        let rows = Region::new(
            output.x,
            output.y - reach,
            output.width,
            output.height + 2 * reach as u32,
        );
        let across = Image::from_fn(rows, 4, |x, y, c| {
            let c = c as usize;
            let mut v = w[0] * input.pixel(x, y)[c];
            for (i, wi) in w.iter().enumerate().skip(1) {
                let i = i as i64;
                v += wi * (input.pixel(x - i, y)[c] + input.pixel(x + i, y)[c]);
            }
            v
        });
        Ok(vec![Image::from_fn(output, 4, |x, y, c| {
            let c = c as usize;
            let mut v = w[0] * across.pixel(x, y)[c];
            for (i, wi) in w.iter().enumerate().skip(1) {
                let i = i as i64;
                v += wi * (across.pixel(x, y - i)[c] + across.pixel(x, y + i)[c]);
            }
            v
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::{Bus, Engine, Graph, ModuleRef, Value};
    use std::sync::Arc;

    fn network(radius: f64, image: Image) -> (Graph, helios_core::NodeId) {
        let mut g = Graph::new();
        let src = g.add_source("src", image);
        let blur = g
            .add_module(
                "blur",
                ModuleRef::new(Blur),
                &Params::new().with("radius", Value::Float(radius)),
            )
            .unwrap();
        g.connect(src, "image", blur, "input").unwrap();
        (g, blur)
    }

    #[test]
    fn a_flat_image_stays_flat_and_weights_sum_to_one() {
        let p = Plan::new(3.0);
        let sum = p.weights[0] + 2.0 * p.weights[1..].iter().sum::<f32>();
        assert!((sum - 1.0).abs() < 1e-5);
        let r = Region::sized(6, 6);
        let flat = Image::from_fn(r.grow(p.reach() as u32), 4, |_, _, _| 0.3);
        let out = Blur.process(&p, &[&flat], r).unwrap();
        assert!(out[0].data().iter().all(|v| (v - 0.3).abs() < 1e-5));
    }

    #[test]
    fn the_reach_follows_the_scale() {
        let params = Blur
            .manifest()
            .resolve(&Params::new().with("radius", Value::Float(8.0)))
            .unwrap();
        let full = Blur.prepare_at(&params, &Context::new(), 1.0).unwrap();
        let quarter = Blur.prepare_at(&params, &Context::new(), 0.25).unwrap();
        assert_eq!((full.reach(), quarter.reach()), (24, 6));
        let r = Region::sized(10, 10);
        assert_eq!(Blur.input_region(&quarter, r, 0), r.grow(6));
    }

    #[test]
    fn half_scale_looks_like_the_full_render_reduced() {
        // A smooth gradient with a soft spot: blurred at full resolution then
        // reduced, against reduced first (by the engine) then blurred.
        let full = Region::sized(128, 96);
        let image = Image::from_fn(full, 4, |x, y, c| {
            if c == 3 {
                return 1.0;
            }
            let (dx, dy) = (x as f32 - 60.0, y as f32 - 40.0);
            0.2 + 0.6 * (-(dx * dx + dy * dy) / 400.0).exp() + 0.002 * x as f32
        });
        let (g, blur) = network(6.0, image);
        let e = Engine::new(Arc::new(Bus::new()));
        let exact = e.render(&g, blur, "output", full).unwrap();
        let half = Region::sized(64, 48);
        let preview = e.render_at(&g, blur, "output", half, 0.5).unwrap();
        let reduced = exact.sample_at(half, 0.5);
        let worst = preview
            .data()
            .iter()
            .zip(reduced.data())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.01, "worst difference {worst}");
        assert_ne!(
            e.key_at(&g, blur, half, 0.5).unwrap(),
            e.key_at(&g, blur, half, 1.0).unwrap(),
            "a preview is cached apart from the exact render"
        );
    }

    #[test]
    fn it_keeps_the_module_contract() {
        helios_testkit::assert_contract(Blur);
    }
}
