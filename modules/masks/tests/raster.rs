//! Raster masks: a module's mask reused by another, through tiles and a
//! preview scale.

use std::sync::Arc;

use helios_core::{Bus, Engine, Graph, Image, ModuleRef, NodeId, Params, Region, Value};
use helios_module_masks::{
    MaskBlend, MaskBlur, MaskCombine, MaskLevels, MaskParametric, MaskRaster,
};

fn engine() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

/// Module A masked by a blurred red range, and module B masked by A's
/// mask (inverted, at 60 %) and a luminance range, folded by maximum.
struct Network {
    g: Graph,
    /// A's finished mask.
    a_mask: NodeId,
    raster: NodeId,
    /// B's group and blend.
    b_group: NodeId,
    b_blend: NodeId,
}

fn network(image: Image, raster: Params) -> Network {
    let mut g = Graph::new();
    let src = g.add_source("image", image);
    let add =
        |g: &mut Graph, name: &str, m: ModuleRef, p: Params| g.add_module(name, m, &p).unwrap();
    let red = add(
        &mut g,
        "a.red",
        ModuleRef::new(MaskParametric),
        Params::new().with("red", Value::List(vec![0.3, 0.6, 0.1, 0.1])),
    );
    let blur = add(
        &mut g,
        "a.blur",
        ModuleRef::new(MaskBlur),
        Params::new().with("radius", Value::Float(3.0)),
    );
    let a_mask = add(
        &mut g,
        "a.levels",
        ModuleRef::new(MaskLevels),
        Params::new().with("contrast", Value::Float(0.4)),
    );
    let a_blend = add(&mut g, "a.blend", ModuleRef::new(MaskBlend), Params::new());
    g.connect(src, "image", red, "input").unwrap();
    g.connect(red, "mask", blur, "mask").unwrap();
    g.connect(blur, "mask", a_mask, "mask").unwrap();
    g.connect(src, "image", a_blend, "input").unwrap();
    g.connect(red, "output", a_blend, "processed").unwrap();
    g.connect(a_mask, "mask", a_blend, "mask").unwrap();
    let raster_node = add(&mut g, "b.raster", ModuleRef::new(MaskRaster), raster);
    let lum = add(
        &mut g,
        "b.luminance",
        ModuleRef::new(MaskParametric),
        Params::new().with("luminance", Value::List(vec![0.6, 4.0, 0.1, 0.0])),
    );
    let b_group = add(
        &mut g,
        "b.group",
        ModuleRef::new(MaskCombine),
        Params::new(),
    );
    let b_blend = add(&mut g, "b.blend", ModuleRef::new(MaskBlend), Params::new());
    g.connect(a_mask, "mask", raster_node, "mask").unwrap();
    g.connect(a_blend, "output", lum, "input").unwrap();
    g.connect(raster_node, "mask", b_group, "in0").unwrap();
    g.connect(lum, "mask", b_group, "in1").unwrap();
    g.connect(a_blend, "output", b_blend, "input").unwrap();
    g.connect(lum, "output", b_blend, "processed").unwrap();
    g.connect(b_group, "mask", b_blend, "mask").unwrap();
    Network {
        g,
        a_mask,
        raster: raster_node,
        b_group,
        b_blend,
    }
}

fn picture(region: Region) -> Image {
    Image::from_fn(region, 4, |x, y, c| match c {
        0 => x as f32 / region.width as f32,
        1 => y as f32 / region.height as f32,
        2 => 0.3,
        _ => 1.0,
    })
}

fn same_bits(a: &Image, b: &Image) -> bool {
    a.region() == b.region()
        && a.data()
            .iter()
            .zip(b.data())
            .all(|(x, y)| x.to_bits() == y.to_bits())
}

#[test]
fn a_reused_mask_is_inverted_then_scaled() {
    let r = Region::sized(40, 30);
    let n = network(
        picture(r),
        Params::new()
            .with("invert", Value::Bool(true))
            .with("opacity", Value::Float(0.6)),
    );
    let e = engine();
    let a = e.render(&n.g, n.a_mask, "mask", r).unwrap();
    let reused = e.render(&n.g, n.raster, "mask", r).unwrap();
    for (m, v) in a.data().iter().zip(reused.data()) {
        assert!((v - (1.0 - m) * 0.6).abs() < 1e-6);
    }
    // Neutral settings hand the mask on as it is, the same buffer.
    let plain = network(picture(r), Params::new());
    let a = e.render(&plain.g, plain.a_mask, "mask", r).unwrap();
    let reused = e.render(&plain.g, plain.raster, "mask", r).unwrap();
    assert!(reused.same_buffer(&a));
}

#[test]
fn a_module_reuses_another_modules_mask_without_computing_it_again() {
    let r = Region::sized(40, 30);
    let n = network(picture(r), Params::new().with("opacity", Value::Float(0.6)));
    let e = engine();
    // Module A renders first, as a pipeline would.
    e.render(&n.g, n.a_mask, "mask", r).unwrap();
    let before = e.counters();
    e.render(&n.g, n.raster, "mask", r).unwrap();
    assert_eq!(
        e.counters().since(&before).computed,
        1,
        "only the adapter runs: A's mask comes from the cache"
    );
    // B's whole render computes nothing of A's mask either.
    let events = e.bus().subscribe();
    e.render(&n.g, n.b_blend, "output", r).unwrap();
    let mut computed: Vec<String> = events
        .try_iter()
        .filter(|ev| ev.kind == "engine.step_computed")
        .map(|ev| ev.payload["name"].as_str().unwrap().to_string())
        .collect();
    computed.sort();
    // A's output was not rendered yet: its blend runs, and so do the red
    // member and the image over `r` (A's blur read them over a wider
    // region, a different content key). A's mask and the raster member
    // are shared.
    assert_eq!(
        computed,
        [
            "a.blend",
            "a.red",
            "b.blend",
            "b.group",
            "b.luminance",
            "image"
        ]
    );
}

#[test]
fn a_reused_mask_is_consistent_through_tiles_and_previews() {
    let full = Region::sized(64, 48);
    let n = network(
        picture(full),
        Params::new().with("invert", Value::Bool(true)),
    );
    let whole = engine().render(&n.g, n.b_blend, "output", full).unwrap();
    let whole_group = engine().render(&n.g, n.b_group, "mask", full).unwrap();
    let tiled = engine();
    for tile in full.tiles(13) {
        let part = tiled.render(&n.g, n.b_blend, "output", tile).unwrap();
        assert!(same_bits(&part, &whole.sample(tile)), "output tile {tile}");
        let part = tiled.render(&n.g, n.b_group, "mask", tile).unwrap();
        assert!(
            same_bits(&part, &whole_group.sample(tile)),
            "mask tile {tile}"
        );
    }
    // At half scale the adapter reads A's mask at half scale: the same
    // content as A's own preview, shared with it.
    let half = Region::sized(32, 24);
    let e = engine();
    let a_preview = e.render_at(&n.g, n.a_mask, "mask", half, 0.5).unwrap();
    let before = e.counters();
    let reused = e.render_at(&n.g, n.raster, "mask", half, 0.5).unwrap();
    assert_eq!(e.counters().since(&before).computed, 1);
    for (m, v) in a_preview.data().iter().zip(reused.data()) {
        assert_eq!(*v, 1.0 - m);
    }
    // And the preview looks like the full mask reduced.
    let exact = e.render(&n.g, n.raster, "mask", full).unwrap();
    let reduced = exact.sample_at(half, 0.5);
    let worst = reused
        .data()
        .iter()
        .zip(reduced.data())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 0.05, "worst difference {worst}");
}

#[test]
fn a_missing_source_gives_no_mask() {
    let mut g = Graph::new();
    let raster = g
        .add_module("raster", ModuleRef::new(MaskRaster), &Params::new())
        .unwrap();
    let r = Region::sized(4, 3);
    let out = engine().render(&g, raster, "mask", r).unwrap();
    assert!(out.data().iter().all(|v| *v == 0.0));
}

#[test]
fn the_raster_adapter_keeps_the_module_contract() {
    helios_testkit::assert_contract(MaskRaster);
}
