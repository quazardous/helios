//! The AI object member: its answer's encoding, its upsampling to the
//! render at every scale and tile, and its place in a group.

use std::sync::Arc;

use helios_core::*;
use helios_module_masks::object::{self, MAX_SIDE, decode_mask, encode_mask};
use helios_module_masks::{MaskCombine, MaskObject};

const IMAGE: Region = Region {
    x: 0,
    y: 0,
    width: 64,
    height: 48,
};

/// A 16×12 mask: the left half 1, the right half 0.
fn half() -> Vec<u8> {
    (0..12)
        .flat_map(|_| (0..16).map(|x| if x < 8 { 255 } else { 0 }))
        .collect()
}

fn network() -> (Graph, NodeId) {
    let mut g = Graph::new();
    let src = g.add_source("src", Image::from_fn(IMAGE, 4, |_, _, _| 0.5));
    let obj = g
        .add_module("sky", ModuleRef::new(MaskObject), &Params::new())
        .unwrap();
    g.connect(src, graph::SOURCE_PORT, obj, "input").unwrap();
    (g, obj)
}

#[test]
fn a_mask_round_trips_through_its_answer() {
    let values = half();
    let text = encode_mask(16, 12, &values).unwrap();
    assert_eq!(decode_mask(&text).unwrap(), (16, 12, values));
    assert!(encode_mask(MAX_SIDE + 1, 1, &[0; MAX_SIDE as usize + 1]).is_err());
    assert!(encode_mask(2, 2, &[0; 3]).is_err());
    assert!(decode_mask("not base64 !").is_err());
    assert!(decode_mask("aGVsbG8=").is_err(), "base64, but not a PNG");
}

#[test]
fn the_member_waits_for_its_answer_then_upsamples_it() {
    let (mut g, obj) = network();
    let e = Engine::new(Arc::new(Bus::new()));
    assert!(matches!(
        e.render(&g, obj, "mask", IMAGE),
        Err(Error::Pending { .. })
    ));
    let answer = object::answer(16, 12, &half(), IMAGE, 0.9).unwrap();
    g.answer(obj, &answer).unwrap();
    let mask = e.render(&g, obj, "mask", IMAGE).unwrap();
    assert_eq!(mask.pixel(0, 0)[0], 1.0);
    assert_eq!(mask.pixel(63, 47)[0], 0.0);
    // Across the edge, a ramp over one mask pixel (4 image pixels).
    let row: Vec<f32> = (28..36).map(|x| mask.pixel(x, 20)[0]).collect();
    assert!(row.windows(2).all(|w| w[0] >= w[1]), "{row:?}");
    assert!(row[0] == 1.0 && row[7] == 0.0, "{row:?}");
}

#[test]
fn outside_its_area_the_mask_is_zero() {
    let (mut g, obj) = network();
    let area = Region::new(16, 8, 32, 24);
    g.answer(obj, &object::answer(1, 1, &[255], area, 1.0).unwrap())
        .unwrap();
    let e = Engine::new(Arc::new(Bus::new()));
    let mask = e.render(&g, obj, "mask", IMAGE).unwrap();
    assert_eq!(mask.pixel(15, 20)[0], 0.0);
    assert_eq!(mask.pixel(16, 20)[0], 1.0);
    assert_eq!(mask.pixel(47, 31)[0], 1.0);
    assert_eq!(mask.pixel(48, 31)[0], 0.0);
    assert_eq!(mask.pixel(30, 32)[0], 0.0);
}

#[test]
fn tiles_and_scales_see_the_same_mask() {
    let (mut g, obj) = network();
    g.answer(obj, &object::answer(16, 12, &half(), IMAGE, 0.5).unwrap())
        .unwrap();
    let e = Engine::new(Arc::new(Bus::new()));
    for scale in [1.0, 0.5, 0.37] {
        let w = (f64::from(IMAGE.width) * scale).round() as u32;
        let h = (f64::from(IMAGE.height) * scale).round() as u32;
        let full = Region::sized(w, h);
        let whole = e.render_at(&g, obj, "mask", full, scale).unwrap();
        for ty in (0..h).step_by(7) {
            for tx in (0..w).step_by(7) {
                let tile = Region::new(i64::from(tx), i64::from(ty), 7.min(w - tx), 7.min(h - ty));
                let part = e.render_at(&g, obj, "mask", tile, scale).unwrap();
                assert_eq!(part.data(), whole.sample(tile).data(), "{scale} {tile}");
            }
        }
    }
}

#[test]
fn it_plugs_into_a_group_like_any_member() {
    let (mut g, obj) = network();
    let group = g
        .add_module("group", ModuleRef::new(MaskCombine), &Params::new())
        .unwrap();
    g.connect(obj, "mask", group, "in0").unwrap();
    g.answer(obj, &object::answer(16, 12, &half(), IMAGE, 0.9).unwrap())
        .unwrap();
    let e = Engine::new(Arc::new(Bus::new()));
    let mask = e.render(&g, group, "mask", IMAGE).unwrap();
    assert_eq!(mask.pixel(2, 2)[0], 1.0);
    assert_eq!(mask.pixel(60, 2)[0], 0.0);
}

#[test]
fn its_question_is_set_its_answer_is_not() {
    let (mut g, obj) = network();
    g.set_param(obj, "kind", Value::Choice("sky".into()))
        .unwrap();
    g.set_param(obj, "prompt", Value::Text("the red car".into()))
        .unwrap();
    assert!(
        g.set_param(obj, "mask", Value::Text(String::new()))
            .is_err()
    );
    assert_eq!(
        MaskObject.question(),
        ["kind", "prompt", "resolver"].map(String::from)
    );
}
