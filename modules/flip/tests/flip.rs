//! The flip step in a graph: the extent it gives (`Graph::extent`), the
//! pixels it moves where darktable moves them, the regions it asks of its
//! input, and the camera's orientation as its default.

use std::sync::Arc;

use helios_core::{Bus, Context, Engine, Graph, Image, ModuleRef, NodeId, Params, Region, Value};
use helios_module_flip::Flip;
use helios_photo::camera::fields;

const W: u32 = 5;
const H: u32 = 3;

/// Each pixel names where it comes from: `x * 100 + y`, then 1, 2, 3.
fn source() -> Image {
    Image::from_fn(Region::sized(W, H), 4, |x, y, c| match c {
        0 => (x * 100 + y) as f32,
        c => c as f32,
    })
}

fn graph(orientation: i64, exif: Option<i64>) -> (Graph, NodeId) {
    let mut g = Graph::new();
    let src = g.add_source("raw", source());
    let flip = g
        .add_module(
            "flip",
            ModuleRef::new(Flip),
            &Params::new().with("orientation", Value::Int(orientation)),
        )
        .unwrap();
    g.connect(src, "image", flip, "input").unwrap();
    if let Some(tag) = exif {
        g.set_context(
            flip,
            Context::new().with(fields::ORIENTATION, Value::Int(tag)),
        )
        .unwrap();
    }
    (g, flip)
}

fn render(g: &Graph, node: NodeId, region: Region) -> Image {
    Engine::new(Arc::new(Bus::new()))
        .render(g, node, "output", region)
        .unwrap()
}

/// The input pixel shown at `(u, v)` from the output's top-left corner,
/// as the orientation is named (not through the module's own maps).
fn expected(orientation: u8, u: i64, v: i64) -> (i64, i64) {
    let (w, h) = (i64::from(W), i64::from(H));
    match orientation {
        0 => (u, v),
        1 => (u, h - 1 - v),         // flip vertically
        2 => (w - 1 - u, v),         // mirror
        3 => (w - 1 - u, h - 1 - v), // 180 degrees
        4 => (v, u),                 // transpose
        5 => (v, h - 1 - u),         // 90 degrees clockwise
        6 => (w - 1 - v, u),         // 90 degrees counter-clockwise
        7 => (w - 1 - v, h - 1 - u), // transverse
        _ => unreachable!(),
    }
}

fn check(g: &Graph, node: NodeId, orientation: u8) {
    let extent = g.extent(node).unwrap().unwrap();
    let turned = orientation & 4 != 0;
    let size = if turned { (H, W) } else { (W, H) };
    assert_eq!(
        (extent.width, extent.height),
        size,
        "orientation {orientation}: {extent}"
    );
    let out = render(g, node, extent);
    assert_eq!(out.region(), extent);
    for v in 0..i64::from(extent.height) {
        for u in 0..i64::from(extent.width) {
            let (x, y) = expected(orientation, u, v);
            let px = out.pixel(extent.x + u, extent.y + v);
            assert_eq!(
                px,
                &[(x * 100 + y) as f32, 1.0, 2.0, 3.0],
                "orientation {orientation}, ({u}, {v})"
            );
        }
    }
}

#[test]
fn each_orientation_turns_the_extent_and_moves_the_pixels_as_darktable() {
    for o in 0..8u8 {
        let (g, flip) = graph(i64::from(o), None);
        check(&g, flip, o);
    }
}

#[test]
fn a_region_asked_of_the_output_maps_back_to_the_input() {
    for o in 0..8u8 {
        let (g, flip) = graph(i64::from(o), None);
        let extent = g.extent(flip).unwrap().unwrap();
        let whole = render(&g, flip, extent);
        // Every one-pixel and two-by-two region equals that part of the whole.
        for r in extent.tiles(1).into_iter().chain(extent.tiles(2)) {
            let part = render(&g, flip, r);
            assert_eq!(part.region(), r);
            assert_eq!(
                part.to_vec(),
                whole.crop(r).unwrap().to_vec(),
                "orientation {o}, {r}"
            );
        }
        // What it reads is the input pixels it moves, nothing more.
        let module = ModuleRef::new(Flip);
        let params = Params::new().with("orientation", Value::Int(i64::from(o)));
        let reads = module.reads(&params, &Context::new(), 1.0, extent).unwrap();
        assert_eq!(reads, Region::sized(W, H), "orientation {o}");
    }
}

#[test]
fn the_camera_s_orientation_is_the_default_and_the_user_s_beats_it() {
    // EXIF 6: the camera was turned clockwise; darktable's 5.
    let (g, flip) = graph(-1, Some(6));
    check(&g, flip, 5);
    // EXIF 8: counter-clockwise, darktable's 6.
    let (g, flip) = graph(-1, Some(8));
    check(&g, flip, 6);
    // The user's choice wins over the tag: none, 180 degrees, a mirror.
    for o in [0, 3, 2, 6] {
        let (g, flip) = graph(i64::from(o), Some(6));
        check(&g, flip, o);
    }
    // No camera description (a picture): left as it is.
    let (g, flip) = graph(-1, None);
    check(&g, flip, 0);
}

#[test]
fn a_bypassed_flip_keeps_the_extent() {
    let (mut g, flip) = graph(5, None);
    g.set_bypass(flip, true).unwrap();
    assert_eq!(g.extent(flip).unwrap(), Some(Region::sized(W, H)));
}

#[test]
fn it_keeps_the_module_contract() {
    for tag in [1, 6] {
        let context = Context::new().with(fields::ORIENTATION, Value::Int(tag));
        helios_testkit::assert_contract_in(Flip, &context);
    }
}
