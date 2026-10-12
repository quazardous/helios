//! Drawn shape members: their contract, tiles equal to one render at
//! several scales and tile sizes (bit for bit), tiles away from a shape
//! filled without running it, anti-aliasing, scale invariance, combining
//! with the group operators, and the GPU against the CPU.

use std::f64::consts::PI;
use std::sync::Arc;

use helios_core::{Bus, Context, Engine, Graph, Image, ModuleRef, NodeId, Params, Region, Value};
use helios_module_masks::shape::{GPU_TOLERANCE, path_values};
use helios_module_masks::{
    MaskCircle, MaskCombine, MaskEllipse, MaskGradient, MaskPath, Operator, PathPoint, fold,
};

fn engine() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

fn float(name: &str, v: f64) -> (String, Value) {
    (name.into(), Value::Float(v))
}

fn params(values: Vec<(String, Value)>) -> Params {
    values
        .into_iter()
        .fold(Params::new(), |p, (k, v)| p.with(&k, v))
}

/// A Bézier blob: four corners, two of them with handles.
fn blob() -> Value {
    path_values(&[
        PathPoint {
            before: [40.0, 70.0],
            after: [80.0, 10.0],
            ..PathPoint::corner(60.0, 30.0)
        },
        PathPoint::corner(150.0, 40.0),
        PathPoint {
            before: [170.0, 90.0],
            after: [120.0, 130.0],
            ..PathPoint::corner(140.0, 110.0)
        },
        PathPoint::corner(50.0, 100.0),
    ])
}

/// Every shape, as (label, module, params), over a 200 x 150 image.
fn shapes() -> Vec<(&'static str, ModuleRef, Params)> {
    vec![
        (
            "circle",
            ModuleRef::new(MaskCircle),
            params(vec![
                float("x", 80.3),
                float("y", 60.7),
                float("radius", 30.0),
                float("feather", 12.0),
            ]),
        ),
        (
            "hard circle, inverted",
            ModuleRef::new(MaskCircle),
            params(vec![
                float("x", 120.0),
                float("y", 70.0),
                float("radius", 25.5),
                float("feather", 0.0),
                float("opacity", 0.8),
                ("invert".into(), Value::Bool(true)),
            ]),
        ),
        (
            "ellipse",
            ModuleRef::new(MaskEllipse),
            params(vec![
                float("x", 100.0),
                float("y", 75.0),
                float("radius_x", 50.0),
                float("radius_y", 20.0),
                float("rotation", 30.0),
                float("feather", 10.0),
            ]),
        ),
        (
            "proportional ellipse",
            ModuleRef::new(MaskEllipse),
            params(vec![
                float("x", 60.0),
                float("y", 50.0),
                float("radius_x", 15.0),
                float("radius_y", 35.0),
                float("rotation", -70.0),
                float("feather", 20.0),
                ("proportional".into(), Value::Bool(true)),
            ]),
        ),
        (
            "linear gradient",
            ModuleRef::new(MaskGradient),
            params(vec![
                float("x", 100.0),
                float("y", 75.0),
                float("rotation", 20.0),
                float("feather", 40.0),
            ]),
        ),
        (
            "sigmoid gradient",
            ModuleRef::new(MaskGradient),
            params(vec![
                float("x", 90.0),
                float("y", 60.0),
                float("rotation", 135.0),
                float("feather", 30.0),
                ("profile".into(), Value::Choice("sigmoid".into())),
            ]),
        ),
        (
            "hard gradient",
            ModuleRef::new(MaskGradient),
            params(vec![
                float("x", 90.0),
                float("y", 60.0),
                float("rotation", 95.0),
                float("feather", 0.0),
            ]),
        ),
        (
            "polygon",
            ModuleRef::new(MaskPath),
            params(vec![
                (
                    "points".into(),
                    path_values(&[
                        PathPoint::corner(30.0, 20.0),
                        PathPoint::corner(170.0, 35.0),
                        PathPoint::corner(100.0, 60.0),
                        PathPoint::corner(150.0, 130.0),
                        PathPoint::corner(40.0, 110.0),
                    ]),
                ),
                float("feather", 0.0),
            ]),
        ),
        (
            "Bézier path",
            ModuleRef::new(MaskPath),
            params(vec![("points".into(), blob()), float("feather", 15.0)]),
        ),
    ]
}

const IMAGE: Region = Region::sized(200, 150);

fn shape_graph(module: ModuleRef, params: &Params) -> (Graph, NodeId) {
    let mut g = Graph::new();
    let node = g.add_module("shape", module, params).unwrap();
    (g, node)
}

/// The image's region at `scale`.
fn at(scale: f64) -> Region {
    Region::sized(
        (f64::from(IMAGE.width) * scale).ceil() as u32,
        (f64::from(IMAGE.height) * scale).ceil() as u32,
    )
}

#[test]
fn every_shape_keeps_the_module_contract() {
    helios_testkit::assert_contract(MaskCircle);
    helios_testkit::assert_contract(MaskEllipse);
    helios_testkit::assert_contract(MaskGradient);
    helios_testkit::assert_contract(MaskPath);
}

#[test]
fn tiles_equal_one_render_at_every_scale_and_tile_size() {
    for (label, module, p) in shapes() {
        let (g, node) = shape_graph(module, &p);
        for scale in [1.0, 0.5, 0.37, 0.25] {
            let full = at(scale);
            let whole = engine().render_at(&g, node, "mask", full, scale).unwrap();
            assert!(
                whole.data().iter().all(|v| (0.0..=1.0).contains(v)),
                "{label}"
            );
            for tile in [7, 32, 64] {
                let e = engine();
                for t in full.tiles(tile) {
                    let part = e.render_at(&g, node, "mask", t, scale).unwrap();
                    assert_eq!(
                        part.data(),
                        whole.sample(t).data(),
                        "{label} at scale {scale}, tile {t} of {tile}"
                    );
                }
            }
        }
    }
}

#[test]
fn tiles_away_from_a_bounded_shape_are_filled() {
    for (label, module, p) in shapes() {
        let bounded = !label.contains("gradient");
        let (g, node) = shape_graph(module, &p);
        let e = engine();
        let tiles = IMAGE.tiles(16);
        for t in &tiles {
            e.render(&g, node, "mask", *t).unwrap();
        }
        let c = e.counters();
        if bounded {
            assert!(
                c.filled > 0 && c.filled < tiles.len() as u64,
                "{label}: {} of {} filled",
                c.filled,
                tiles.len()
            );
        } else {
            assert_eq!(c.filled, 0, "{label}: a gradient reaches everywhere");
        }
    }
}

#[test]
fn a_shape_is_full_inside_empty_outside_and_soft_between() {
    let (g, node) = shape_graph(
        ModuleRef::new(MaskCircle),
        &params(vec![
            float("x", 100.0),
            float("y", 75.0),
            float("radius", 20.0),
            float("feather", 10.0),
        ]),
    );
    let m = engine().render(&g, node, "mask", IMAGE).unwrap();
    assert_eq!(m.pixel(100, 75)[0], 1.0);
    assert_eq!(m.pixel(110, 75)[0], 1.0);
    assert_eq!(m.pixel(131, 75)[0], 0.0);
    // darktable's falloff: ((t² - l²) / (t² - r²))² at l = 25.5.
    let l: f64 = 25.5;
    let want = ((900.0 - l * l) / (900.0 - 400.0)).powi(2);
    assert!((f64::from(m.pixel(125, 75)[0]) - want).abs() < 1e-3);
    // Monotonic along a ray.
    let row: Vec<f32> = (100..135).map(|x| m.pixel(x, 75)[0]).collect();
    assert!(row.windows(2).all(|w| w[1] <= w[0]), "{row:?}");
}

#[test]
fn a_hard_edge_is_anti_aliased_to_the_shape_s_area() {
    let r = 30.25;
    let (g, node) = shape_graph(
        ModuleRef::new(MaskCircle),
        &params(vec![
            float("x", 100.3),
            float("y", 74.6),
            float("radius", r),
            float("feather", 0.0),
        ]),
    );
    let m = engine().render(&g, node, "mask", IMAGE).unwrap();
    let area: f64 = m.data().iter().map(|v| f64::from(*v)).sum();
    assert!((area / (PI * r * r) - 1.0).abs() < 0.002, "{area}");
    let partial = m.data().iter().filter(|v| **v > 0.0 && **v < 1.0).count();
    assert!(partial > 100, "edge pixels are partly covered: {partial}");

    // A hard polygon's area, a square at fractional coordinates.
    let (g, node) = shape_graph(
        ModuleRef::new(MaskPath),
        &params(vec![
            (
                "points".into(),
                path_values(&[
                    PathPoint::corner(20.3, 30.6),
                    PathPoint::corner(80.3, 30.6),
                    PathPoint::corner(80.3, 70.6),
                    PathPoint::corner(20.3, 70.6),
                ]),
            ),
            float("feather", 0.0),
        ]),
    );
    let m = engine().render(&g, node, "mask", IMAGE).unwrap();
    let area: f64 = m.data().iter().map(|v| f64::from(*v)).sum();
    assert!((area - 2400.0).abs() < 1.0, "{area}");
}

#[test]
fn a_preview_looks_like_the_full_render_reduced() {
    for (label, module, p) in shapes() {
        if label.starts_with("hard") || label == "polygon" {
            continue; // a hard edge is a sub-pixel matter at any scale
        }
        let (g, node) = shape_graph(module, &p);
        let e = engine();
        let full = e.render(&g, node, "mask", IMAGE).unwrap();
        let preview = e.render_at(&g, node, "mask", at(0.5), 0.5).unwrap();
        let reduced = full.sample_at(at(0.5), 0.5);
        let worst = preview
            .data()
            .iter()
            .zip(reduced.data())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.03, "{label}: {worst}");
    }
}

#[test]
fn shapes_combine_with_the_group_operators() {
    let all = shapes();
    let members = [&all[0], &all[2], &all[8]];
    for op in ["maximum", "minimum", "difference", "exclusion"] {
        let mut g = Graph::new();
        let group = g
            .add_module(
                "group",
                ModuleRef::new(MaskCombine),
                &Params::new()
                    .with("operator", Value::Choice(op.into()))
                    .with("invert", Value::Ints(vec![0, 1, 0])),
            )
            .unwrap();
        let mut nodes = Vec::new();
        for (i, (label, module, p)) in members.iter().enumerate() {
            let n = g.add_module(label, module.clone(), p).unwrap();
            g.connect(n, "mask", group, &format!("in{i}")).unwrap();
            nodes.push(n);
        }
        let e = engine();
        let out = e.render(&g, group, "mask", IMAGE).unwrap();
        let alone: Vec<Image> = nodes
            .iter()
            .map(|n| e.render(&g, *n, "mask", IMAGE).unwrap())
            .collect();
        let operator = Operator::from_name(op).unwrap();
        for (x, y) in [(80, 60), (100, 75), (60, 40), (150, 120), (10, 10)] {
            let want = fold(
                operator,
                alone
                    .iter()
                    .enumerate()
                    .map(|(i, m)| (m.pixel(x, y)[0], 1.0, i == 1)),
            )
            .unwrap();
            assert_eq!(out.pixel(x, y)[0], want, "{op} at {x},{y}");
        }
        // Tiled, some members filled: the same.
        let tiled = engine();
        for t in IMAGE.tiles(32) {
            let part = tiled.render(&g, group, "mask", t).unwrap();
            assert_eq!(part.data(), out.sample(t).data(), "{op}, tile {t}");
        }
    }
}

#[test]
fn a_path_rendered_by_rows_equals_its_value_everywhere() {
    use helios_core::Module;
    for (label, _, p) in shapes() {
        if !matches!(label, "polygon" | "Bézier path") {
            continue;
        }
        let manifest = MaskPath.manifest();
        let mut resolved = manifest.defaults();
        for (k, v) in p.iter() {
            resolved.set(k, v.clone());
        }
        for scale in [1.0, 0.5, 0.3] {
            let plan = MaskPath
                .prepare_at(&resolved, &Context::new(), scale)
                .unwrap();
            let region = at(scale);
            let image = &MaskPath.process(&plan, &[], region).unwrap()[0];
            for y in (region.y..region.bottom()).step_by(3) {
                for x in (region.x..region.right()).step_by(2) {
                    let (u, v) = ((x as f64 + 0.5) / scale, (y as f64 + 0.5) / scale);
                    let want = plan.common.member.apply(plan.value(u, v));
                    assert_eq!(
                        image.pixel(x, y)[0],
                        want,
                        "{label} at {x},{y}, scale {scale}"
                    );
                }
            }
        }
    }
}

#[test]
fn every_shape_on_the_gpu_matches_the_cpu() {
    let _gpu = helios_testkit::gpu_lock();
    let gpu = match helios_gpu::shared::gpu() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    for (label, module, p) in shapes() {
        for (scale, region) in [
            (1.0, IMAGE),
            (1.0, Region::new(-20, 90, 70, 80)),
            (0.5, at(0.5)),
            (0.75, Region::new(100, 70, 60, 50)),
        ] {
            let gpu_job = module.gpu_job(&p, &Context::new(), scale, region);
            assert!(gpu_job.unwrap().is_some(), "{label} has a kernel");
            let (g, node) = shape_graph(module.clone(), &p);
            let cpu = engine().render_at(&g, node, "mask", region, scale).unwrap();
            let on_gpu = Engine::new(Arc::new(Bus::new())).with_accelerator(gpu.clone());
            let out = on_gpu.render_at(&g, node, "mask", region, scale).unwrap();
            let c = helios_gpu::compare(&out, &cpu).unwrap();
            assert!(
                c.max_abs <= GPU_TOLERANCE,
                "{label} at scale {scale} over {region}: {c:?}"
            );
            let n = on_gpu.counters();
            assert_eq!(n.fallbacks, 0, "{label}: {:?}", on_gpu.fallback_reason());
        }
    }
}
