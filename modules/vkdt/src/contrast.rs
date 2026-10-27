// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/contrast/main.c, src/pipe/modules/contrast/combine.comp,
// src/pipe/modules/shared.glsl. vkdt: copyright 2019 johannes hanika, under the
// BSD-2-Clause licence; its notice is in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

//! vkdt's `contrast`: local contrast from a guided filter. The first
//! multi-kernel module ported: it declares the network vkdt's
//! `create_nodes` builds (a guided filter on the luminance, its blurs a
//! cascade at lower resolutions, then `combine`), and the engine plans,
//! caches and tiles each kernel.
//!
//! Its `radius` is a fraction of the image's width, as in vkdt: the
//! network is built for the width of the image at the render's scale, so a
//! preview looks like the full render reduced.

use helios_core::{
    Composite, Context, Expansion, Family, Image, Manifest, Module, ModuleKind, ModuleRef, Needs,
    Network, ParamSpec, Params, PortSpec, Region, Role, Value,
};

use crate::guided::{guided_filter, luminance};

/// `combine`: the input's luminance pushed away from the filtered one by
/// `detail` beyond `thrs`, the colour scaled with it (ratio clamped to 4).
pub struct Combine;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CombinePlan {
    pub detail: f32,
    pub thrs: f32,
}

impl Module for Combine {
    type Plan = CombinePlan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "vkdt_contrast_combine".into(),
            version: 1,
            description:
                "vkdt's contrast, last kernel: detail boosted against the coarse luminance".into(),
            params: vec![
                ParamSpec::float("detail", "the amount of detail enhancement", 0.0, 5.0, 1.0),
                ParamSpec::float(
                    "thrs",
                    "the threshold for detail enhancement",
                    0.0,
                    0.01,
                    0.0,
                ),
            ],
            inputs: vec![PortSpec::image("input", 4), PortSpec::image("coarse", 1)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Detail,
            role: Role::Specialised,
        }
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<CombinePlan, String> {
        Ok(CombinePlan {
            detail: params.float("detail") as f32,
            thrs: params.float("thrs") as f32,
        })
    }

    fn process(
        &self,
        plan: &CombinePlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let (input, coarse) = (inputs[0], inputs[1]);
        let mut data = Vec::with_capacity(output.area() * 4);
        for y in output.y..output.bottom() {
            for x in output.x..output.right() {
                let p = input.pixel(x, y);
                let l0 = luminance(p[0], p[1], p[2]);
                let l1 = coarse.pixel(x, y)[0];
                let d = l0 - l1;
                let lum = l1 + sign(d) * (plan.detail * (d.abs() - plan.thrs).max(0.0));
                let k = (lum / l0.max(0.01)).clamp(0.0, 4.0);
                data.extend([p[0] * k, p[1] * k, p[2] * k, 1.0]);
            }
        }
        Ok(vec![Image::new(output, 4, data)])
    }
}

/// GLSL's `sign`: 0 at 0.
fn sign(v: f32) -> f32 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// vkdt's `contrast` module.
pub struct VkdtContrast;

impl Composite for VkdtContrast {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "vkdt_contrast".into(),
            version: 1,
            description: "vkdt's local contrast, based on a guided filter".into(),
            params: vec![
                ParamSpec::float(
                    "radius",
                    "the guided filter's radius, a fraction of the image's width",
                    0.001,
                    0.1,
                    0.01,
                ),
                ParamSpec::float(
                    "edges",
                    "the guided filter's edge threshold",
                    0.001,
                    10.0,
                    0.03,
                ),
                ParamSpec::float("detail", "the amount of detail enhancement", 0.0, 5.0, 1.0),
                ParamSpec::float(
                    "thrs",
                    "the threshold for detail enhancement",
                    0.0,
                    0.01,
                    0.0,
                ),
            ],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Detail,
            role: Role::General,
        }
    }

    fn expand(&self, params: &Params, _: &Context, at: &Expansion) -> Result<Network, String> {
        let width = at
            .width()
            .ok_or("its radius is a fraction of the image's width: no image feeds it")?;
        let radius = (params.float("radius") * width) as f32;
        let mut n = Network::new(&self.manifest());
        let input = n.input("input").map_err(|e| e.to_string())?;
        let coarse = guided_filter(
            &mut n,
            input,
            radius,
            params.float("edges"),
            width.floor() as u32,
        )?;
        let combine = Params::new()
            .with("detail", Value::Float(params.float("detail")))
            .with("thrs", Value::Float(params.float("thrs")));
        let build = |n: &mut Network| -> Result<(), helios_core::Error> {
            let c = n.add("combine", ModuleRef::new(Combine), &combine)?;
            n.connect(input, c, "input")?;
            n.connect(coarse, c, "coarse")?;
            n.exit(n.output(c, "output")?, "output")
        };
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
        Image::from_fn(Region::sized(128, 96), 4, |x, y, c| {
            if c == 3 {
                return 1.0;
            }
            let edge = if (x / 11 + y / 9) % 2 == 0 { 0.55 } else { 0.2 };
            edge + ((x * 5 + y * 3 + i64::from(c) * 7) % 11) as f32 / 80.0
        })
    }

    fn graph(params: &Params) -> (Graph, helios_core::NodeId) {
        let mut g = Graph::new();
        let src = g.add_source("src", picture());
        let n = g
            .add_module("contrast", ModuleRef::composite(VkdtContrast), params)
            .unwrap();
        g.connect(src, SOURCE_PORT, n, "input").unwrap();
        (g, n)
    }

    #[test]
    fn detail_one_without_threshold_gives_the_input_back() {
        // lum = l1 + (l0 - l1) = l0: the colour is untouched.
        let (g, n) = graph(&Params::new());
        let r = Region::new(10, 10, 40, 30);
        let out = Engine::new(Arc::new(Bus::new()))
            .render(&g, n, "output", r)
            .unwrap();
        let input = picture().sample(r);
        for (a, b) in out.data().iter().zip(input.data()) {
            assert!((a - b).abs() < 1e-5, "{a} {b}");
        }
    }

    #[test]
    fn more_detail_raises_local_contrast_and_tiles_like_one_render() {
        for radius in [0.01, 0.03, 0.06] {
            let params = Params::new()
                .with("radius", Value::Float(radius))
                .with("detail", Value::Float(2.5));
            let (g, n) = graph(&params);
            let region = Region::sized(128, 96);
            let whole = Engine::new(Arc::new(Bus::new()))
                .render(&g, n, "output", region)
                .unwrap();
            let spread = |img: &Image| {
                let l: Vec<f32> = img
                    .data()
                    .chunks(4)
                    .map(|p| luminance(p[0], p[1], p[2]))
                    .collect();
                l.iter().copied().fold(f32::MIN, f32::max)
                    - l.iter().copied().fold(f32::MAX, f32::min)
            };
            assert!(spread(&whole) > spread(&picture()), "radius {radius}");
            let tiled = Engine::new(Arc::new(Bus::new()));
            for tile in region.tiles(37) {
                let part = tiled.render(&g, n, "output", tile).unwrap();
                assert_eq!(
                    part.data(),
                    whole.sample(tile).data(),
                    "radius {radius}, tile {tile}"
                );
            }
        }
    }

    #[test]
    fn it_keeps_the_module_contract_and_its_kernels_too() {
        helios_testkit::assert_contract(Combine);
        helios_testkit::assert_contract_of(&ModuleRef::composite(VkdtContrast), &Context::new());
    }
}
