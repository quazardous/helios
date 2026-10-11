//! Masks as a sub-network: operators against hand-computed values (and
//! darktable's formulas), order, nesting, tiles, previews and blending.

use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, Module, ModuleRef, NodeId, Params, Region, Value};
use helios_module_masks::*;

fn engine() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

const R: Region = Region::new(0, 0, 4, 3);

fn flat(g: &mut Graph, name: &str, v: f32) -> NodeId {
    g.add_source(name, Image::from_fn(R, 1, |_, _, _| v))
}

fn op(name: &str) -> Params {
    Params::new().with("operator", Value::Choice(name.into()))
}

/// A group over constant members, connected to `in0`, `in1`...
fn group(g: &mut Graph, params: &Params, members: &[NodeId]) -> NodeId {
    let c = g
        .add_module("group", ModuleRef::new(MaskCombine), params)
        .unwrap();
    for (i, m) in members.iter().enumerate() {
        g.connect(*m, "image", c, &format!("in{i}")).unwrap();
    }
    c
}

fn value(g: &Graph, node: NodeId) -> f32 {
    let image = engine().render(g, node, "mask", R).unwrap();
    let v = image.data()[0];
    assert!(image.data().iter().all(|x| *x == v));
    v
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn each_operator_matches_hand_computed_values() {
    // a = 0.6, b = 0.5.
    let expected = [
        ("maximum", 0.6),
        ("screen", 0.6 + 0.5 - 0.3),
        ("minimum", 0.5),
        ("product", 0.3),
        ("sum", 1.0),
        ("difference", 0.6 * (1.0 - 0.5)),
        // both positive: max((1 - a) b, a (1 - b)) = max(0.2, 0.3)
        ("exclusion", 0.3),
    ];
    for (name, want) in expected {
        let mut g = Graph::new();
        let a = flat(&mut g, "a", 0.6);
        let b = flat(&mut g, "b", 0.5);
        let c = group(&mut g, &op(name), &[a, b]);
        let got = value(&g, c);
        assert!(close(got, want), "{name}: {got}, expected {want}");
    }
}

#[test]
fn operators_follow_darktable_at_the_edges() {
    let f = |o, a: f32, b: f32| fold(o, [(a, 1.0, false), (b, 1.0, false)]).unwrap();
    // Exclusion where one member is 0 is a maximum (classic's else branch).
    assert_eq!(f(Operator::Exclusion, 0.0, 0.5), 0.5);
    // Difference subtracts nothing from where the first member is 0.
    assert_eq!(f(Operator::Difference, 0.0, 0.5), 0.0);
    // Sum saturates, minimum clamps negatives to 0, product of a first
    // member is the member (the seed is 1).
    assert_eq!(f(Operator::Sum, 0.7, 0.7), 1.0);
    assert_eq!(f(Operator::Minimum, -0.2, 0.5), 0.0);
    assert_eq!(fold(Operator::Product, [(0.4, 1.0, false)]), Some(0.4));
    assert_eq!(fold(Operator::Maximum, []), None);
}

#[test]
fn exclusion_depends_on_member_order() {
    let ex = |members: [f32; 3]| fold(Operator::Exclusion, members.map(|v| (v, 1.0, false)));
    // 0.6 -> max(0.4 * 0.5, 0.6 * 0.5) = 0.3 -> max(0.7 * 0.9, 0.3 * 0.1) = 0.63
    assert!(close(ex([0.6, 0.5, 0.9]).unwrap(), 0.63));
    // 0.9 -> max(0.1 * 0.5, 0.9 * 0.5) = 0.45 -> max(0.55 * 0.6, 0.45 * 0.4) = 0.33
    assert!(close(ex([0.9, 0.5, 0.6]).unwrap(), 0.33));
    // The same through the network: the order is the order of the ports.
    let mut g = Graph::new();
    let (a, b, c) = (
        flat(&mut g, "a", 0.6),
        flat(&mut g, "b", 0.5),
        flat(&mut g, "c", 0.9),
    );
    let forward = group(&mut g, &op("exclusion"), &[a, b, c]);
    let backward = group(&mut g, &op("exclusion"), &[c, b, a]);
    assert!(close(value(&g, forward), 0.63));
    assert!(close(value(&g, backward), 0.33));
}

#[test]
fn a_member_is_inverted_before_its_opacity() {
    let mut g = Graph::new();
    let a = flat(&mut g, "a", 0.2);
    let params = op("maximum")
        .with("opacity", Value::List(vec![0.5]))
        .with("invert", Value::Ints(vec![1]));
    let c = group(&mut g, &params, &[a]);
    // (1 - 0.2) * 0.5, not 1 - 0.2 * 0.5
    assert!(close(value(&g, c), 0.4));
}

#[test]
fn a_group_inverts_then_scales_its_fold() {
    let mut g = Graph::new();
    let a = flat(&mut g, "a", 0.6);
    let b = flat(&mut g, "b", 0.3);
    let params = op("maximum")
        .with("group_invert", Value::Bool(true))
        .with("group_opacity", Value::Float(0.5));
    let c = group(&mut g, &params, &[a, b]);
    // fold 0.6, inverted 0.4, scaled 0.2 (scaled first would give 0.7)
    assert!(close(value(&g, c), 0.2));
}

#[test]
fn unconnected_members_are_skipped() {
    let mut g = Graph::new();
    let a = flat(&mut g, "a", 0.6);
    let b = flat(&mut g, "b", 0.5);
    let c = g
        .add_module("group", ModuleRef::new(MaskCombine), &op("difference"))
        .unwrap();
    // in0 and in2 empty: b is the first visible member, a is subtracted.
    g.connect(b, "image", c, "in1").unwrap();
    g.connect(a, "image", c, "in3").unwrap();
    assert!(close(value(&g, c), 0.5 * (1.0 - 0.6)));
    // Minimum and product start from 1, so an empty port must not count as 0.
    g.set_param(c, "operator", Value::Choice("product".into()))
        .unwrap();
    assert!(close(value(&g, c), 0.3));
    // No member at all: the module applies fully, as darktable's
    // "no active mask element".
    let empty = group(&mut g, &op("minimum"), &[]);
    assert_eq!(value(&g, empty), 1.0);
}

#[test]
fn a_group_nests_in_a_group() {
    let mut g = Graph::new();
    let a = flat(&mut g, "a", 0.6);
    let b = flat(&mut g, "b", 0.5);
    let c = flat(&mut g, "c", 0.4);
    let inner = group(
        &mut g,
        &op("screen").with("group_invert", Value::Bool(true)),
        &[a, b],
    );
    let outer = g
        .add_module("outer", ModuleRef::new(MaskCombine), &op("maximum"))
        .unwrap();
    g.connect(inner, "mask", outer, "in0").unwrap();
    g.connect(c, "image", outer, "in1").unwrap();
    // inner: screen 0.8, inverted 0.2; outer: max(0.2, 0.4)
    assert!(close(value(&g, inner), 0.2));
    assert!(close(value(&g, outer), 0.4));
}

#[test]
fn levels_follow_darktables_tone_curve_then_invert_and_opacity() {
    // Neutral settings leave the mask as it is.
    assert_eq!(tone_curve(0.3, 0.0, 0.0), 0.3);
    // Brightness 0.5 at m = 0.5: x = (0 + 0.5) / 0.5 = 1, so 1.
    assert_eq!(tone_curve(0.5, 0.0, 0.5), 1.0);
    // Contrast 1 at m = 0.75: x = 0.5, e = exp(3),
    // 0.5 * (0.5 e / (1 + (e - 1) 0.5)) + 0.5
    let e = 3f32.exp();
    assert!(close(
        tone_curve(0.75, 1.0, 0.0),
        0.5 * (0.5 * e / (1.0 + (e - 1.0) * 0.5)) + 0.5
    ));
    let mut g = Graph::new();
    let a = flat(&mut g, "a", 0.5);
    let levels = g
        .add_module(
            "levels",
            ModuleRef::new(MaskLevels),
            &Params::new()
                .with("brightness", Value::Float(0.5))
                .with("invert", Value::Bool(true))
                .with("opacity", Value::Float(0.5)),
        )
        .unwrap();
    g.connect(a, "image", levels, "mask").unwrap();
    // curve gives 1, inverted 0, scaled 0
    assert_eq!(value(&g, levels), 0.0);
}

#[test]
fn blend_mixes_input_and_output_through_the_mask() {
    let mut g = Graph::new();
    let input = g.add_source("in", Image::from_fn(R, 4, |_, _, c| c as f32 * 0.1));
    let processed = g.add_source("out", Image::from_fn(R, 4, |_, _, _| 1.0));
    let mask = g.add_source("mask", Image::from_fn(R, 1, |x, _, _| x as f32 / 2.0));
    let blend = g
        .add_module(
            "blend",
            ModuleRef::new(MaskBlend),
            &Params::new().with("opacity", Value::Float(0.5)),
        )
        .unwrap();
    g.connect(input, "image", blend, "input").unwrap();
    g.connect(processed, "image", blend, "processed").unwrap();
    g.connect(mask, "image", blend, "mask").unwrap();
    let out = engine().render(&g, blend, "output", R).unwrap();
    for x in 0..4 {
        // the mask clamps to 1 at x = 2 and 3
        let m = (x as f32 / 2.0).min(1.0) * 0.5;
        for c in 0..4 {
            let a = c as f32 * 0.1;
            assert!(close(out.pixel(x, 1)[c], a + (1.0 - a) * m), "x {x} c {c}");
        }
    }
}

#[test]
fn a_module_with_a_mask_output_passes_its_image_through() {
    let mut g = Graph::new();
    let image = Image::from_fn(R, 4, |x, _, _| x as f32 * 0.2);
    let src = g.add_source("src", image.clone());
    let luma = g
        .add_module(
            "luma",
            ModuleRef::new(MaskFromLuma),
            &Params::new()
                .with("low", Value::Float(0.2))
                .with("high", Value::Float(0.6)),
        )
        .unwrap();
    g.connect(src, "image", luma, "input").unwrap();
    let e = engine();
    let out = e.render(&g, luma, "output", R).unwrap();
    assert!(out.same_buffer(&image), "the image is not copied");
    let before = e.counters();
    let mask = e.render(&g, luma, "mask", R).unwrap();
    assert_eq!(
        e.counters().since(&before).computed,
        0,
        "both outputs come from one step"
    );
    // Luminance of a grey is the grey: (x 0.2 - 0.2) / 0.4.
    for x in 0..4 {
        let want = ((x as f32 * 0.2 - 0.2) / 0.4).clamp(0.0, 1.0);
        assert!((mask.pixel(x, 0)[0] - want).abs() < 1e-5);
    }
}

/// A module masked by a refined group of two members, as darktable would
/// render it: the image, a luminance member feathered by the image and a
/// blurred radial member, grouped by screen, refined at group scope, then
/// blended. Returns the graph and the blend node.
fn masked_network(image: Image) -> (Graph, NodeId) {
    let mut g = Graph::new();
    let region = image.region();
    let src = g.add_source("src", image);
    let (cx, cy) = (region.width as f32 * 0.6, region.height as f32 * 0.4);
    let radial = g.add_source(
        "radial",
        Image::from_fn(region, 1, |x, y, _| {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            (1.0 - d / 30.0).clamp(0.0, 1.0)
        }),
    );
    let add =
        |g: &mut Graph, name: &str, m: ModuleRef, p: Params| g.add_module(name, m, &p).unwrap();
    let luma = add(&mut g, "luma", ModuleRef::new(MaskFromLuma), Params::new());
    let feather = add(
        &mut g,
        "feather",
        ModuleRef::new(MaskFeather),
        Params::new().with("radius", Value::Float(4.0)),
    );
    let blur = add(
        &mut g,
        "blur",
        ModuleRef::new(MaskBlur),
        Params::new().with("radius", Value::Float(3.0)),
    );
    let combine = add(&mut g, "group", ModuleRef::new(MaskCombine), op("screen"));
    let group_blur = add(
        &mut g,
        "group_blur",
        ModuleRef::new(MaskBlur),
        Params::new().with("radius", Value::Float(2.0)),
    );
    let levels = add(
        &mut g,
        "levels",
        ModuleRef::new(MaskLevels),
        Params::new()
            .with("contrast", Value::Float(0.3))
            .with("opacity", Value::Float(0.9)),
    );
    let processed = add(
        &mut g,
        "processed",
        ModuleRef::new(MaskFromLuma),
        Params::new(),
    );
    let blend = add(&mut g, "blend", ModuleRef::new(MaskBlend), Params::new());
    g.connect(src, "image", luma, "input").unwrap();
    g.connect(luma, "mask", feather, "mask").unwrap();
    g.connect(src, "image", feather, "guide").unwrap();
    g.connect(radial, "image", blur, "mask").unwrap();
    g.connect(feather, "mask", combine, "in0").unwrap();
    g.connect(blur, "mask", combine, "in1").unwrap();
    g.connect(combine, "mask", group_blur, "mask").unwrap();
    g.connect(group_blur, "mask", levels, "mask").unwrap();
    // The masked module: anything with an image output; here a pass-through
    // brightened by blending against a constant would do as well.
    g.connect(src, "image", processed, "input").unwrap();
    g.connect(src, "image", blend, "input").unwrap();
    g.connect(processed, "output", blend, "processed").unwrap();
    g.connect(levels, "mask", blend, "mask").unwrap();
    (g, blend)
}

/// A smooth image with a soft bright spot.
fn soft(region: Region) -> Image {
    Image::from_fn(region, 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        let (dx, dy) = (x as f32 - 40.0, y as f32 - 30.0);
        0.1 + 0.5 * (-(dx * dx + dy * dy) / 300.0).exp() + 0.001 * x as f32
    })
}

#[test]
fn a_tiled_render_gives_the_bits_of_a_whole_one() {
    let full = Region::sized(72, 56);
    let (g, _) = masked_network(soft(full));
    let levels = g.find("levels").unwrap();
    let whole = engine().render(&g, levels, "mask", full).unwrap();
    let tiled = engine();
    for tile in full.tiles(17) {
        let part = tiled.render(&g, levels, "mask", tile).unwrap();
        let expected = whole.sample(tile);
        assert!(
            part.data()
                .iter()
                .zip(expected.data())
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "tile {tile} differs"
        );
    }
}

#[test]
fn a_preview_looks_like_the_full_mask_reduced() {
    let full = Region::sized(128, 96);
    // A wider spot than `soft`, so a half-scale window still sees it whole.
    let image = Image::from_fn(full, 4, |x, y, c| {
        if c == 3 {
            return 1.0;
        }
        let (dx, dy) = (x as f32 - 64.0, y as f32 - 48.0);
        0.1 + 0.5 * (-(dx * dx + dy * dy) / 900.0).exp()
    });
    let (g, blend) = masked_network(image);
    let levels = g.find("levels").unwrap();
    let e = engine();
    let half = Region::sized(64, 48);
    for (node, port, tolerance) in [(levels, "mask", 0.05), (blend, "output", 0.05)] {
        let exact = e.render(&g, node, port, full).unwrap();
        let preview = e.render_at(&g, node, port, half, 0.5).unwrap();
        let reduced = exact.sample_at(half, 0.5);
        let worst = preview
            .data()
            .iter()
            .zip(reduced.data())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < tolerance, "{port}: worst difference {worst}");
    }
}

#[test]
fn feathering_follows_the_guide_and_keeps_a_flat_mask() {
    // A flat mask stays flat whatever the guide.
    let r = Region::sized(12, 8);
    let mask = Image::from_fn(r.grow(8), 1, |_, _, _| 0.7);
    let guide = helios_testkit::noise(r.grow(8), 4, 3);
    let out = MaskFeather.process(&4, &[&mask, &guide], r).unwrap();
    assert!(out[0].data().iter().all(|v| (v - 0.7).abs() < 1e-5));
    // A step mask on a step guide keeps its edge; on a flat guide it is
    // smoothed across it.
    let step = |x: i64| if x < 6 { 0.0 } else { 1.0 };
    let mask = Image::from_fn(r.grow(8), 1, |x, _, _| step(x));
    let edge = Image::from_fn(r.grow(8), 4, |x, _, _| step(x));
    let plain = Image::from_fn(r.grow(8), 4, |_, _, _| 0.5);
    let kept = MaskFeather.process(&4, &[&mask, &edge], r).unwrap();
    let smoothed = MaskFeather.process(&4, &[&mask, &plain], r).unwrap();
    let at = |i: &Image| i.pixel(5, 3)[0];
    assert!(at(&kept[0]) < 0.05, "edge kept: {}", at(&kept[0]));
    assert!(
        at(&smoothed[0]) > 0.2,
        "edge smoothed: {}",
        at(&smoothed[0])
    );
}

#[test]
fn every_module_keeps_the_module_contract() {
    helios_testkit::assert_contract(MaskCombine);
    helios_testkit::assert_contract(MaskBlend);
    helios_testkit::assert_contract(MaskBlur);
    helios_testkit::assert_contract(MaskFeather);
    helios_testkit::assert_contract(MaskLevels);
    helios_testkit::assert_contract(MaskFromLuma);
}
