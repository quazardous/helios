//! demosaic in a render at a scale below 1: what the framework's source
//! reduction does to a sensor mosaic.

use std::sync::Arc;

use helios_core::graph::SOURCE_PORT;
use helios_core::{Bus, Context, Engine, Graph, Image, ModuleRef, Params, Region, Value};
use helios_module_demosaic::Demosaic;
use helios_photo::Cfa;
use helios_photo::camera::fields;
use helios_photo::cfa::Channel;

/// A flat scene of `color` seen through an RGGB filter, and demosaic
/// reading it.
fn network(color: [f32; 3], region: Region) -> (Graph, helios_core::NodeId) {
    let cfa = Cfa::parse("RGGB").unwrap();
    let mosaic = Image::from_fn(region, 1, |x, y, _| match cfa.color_at(x, y) {
        Channel::Red => color[0],
        Channel::Green => color[1],
        Channel::Blue => color[2],
    });
    let mut g = Graph::new();
    let src = g.add_source("raw", mosaic);
    let demosaic = g
        .add_module("demosaic", ModuleRef::new(Demosaic), &Params::new())
        .unwrap();
    g.set_context(
        demosaic,
        Context::new().with(fields::CFA, Value::Choice("RGGB".into())),
    )
    .unwrap();
    g.connect(src, SOURCE_PORT, demosaic, "input").unwrap();
    (g, demosaic)
}

/// The mean of each colour channel over an image.
fn mean_rgb(image: &Image) -> [f32; 3] {
    let mut sum = [0.0f64; 3];
    let pixels = image.region().area() as f64;
    for y in image.region().y..image.region().bottom() {
        for x in image.region().x..image.region().right() {
            for (s, v) in sum.iter_mut().zip(image.pixel(x, y)) {
                *s += f64::from(*v);
            }
        }
    }
    sum.map(|s| (s / pixels) as f32)
}

/// demosaic declares full-resolution input: at half scale the engine runs
/// it on the full mosaic and reduces its output, so a coloured scene keeps
/// its colour (box-averaging the mosaic first gave grey: the mean of a
/// red, two green and a blue photosite). The half-scale render is the full
/// render reduced, bit for bit.
#[test]
fn a_half_scale_render_is_the_full_render_reduced() {
    let color = [0.8f32, 0.5, 0.2];
    let full = Region::sized(32, 32);
    let (g, demosaic) = network(color, full);
    let e = Engine::new(Arc::new(Bus::new()));
    let whole = e.render(&g, demosaic, "output", full).unwrap();
    let reduced = whole.sample_at(Region::sized(16, 16), 0.5);
    let before = e.counters();
    let half = e
        .render_at(&g, demosaic, "output", Region::sized(16, 16), 0.5)
        .unwrap();
    assert_eq!(half.data(), reduced.data());
    assert_eq!(
        e.counters().since(&before).computed,
        1,
        "only the reduction: the full-resolution demosaic is cached"
    );
    let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-3);
    // Away from the edges, where replicated photosites change colour.
    let inner = half.sample(Region::new(4, 4, 8, 8));
    assert!(close(mean_rgb(&inner), color), "{:?}", mean_rgb(&inner));
    // A region of a larger image computes only the mosaic it covers.
    let (g, demosaic) = network(color, Region::sized(400, 300));
    let e = Engine::new(Arc::new(Bus::new()));
    let events = e.bus().subscribe();
    let part = Region::new(20, 10, 8, 8);
    e.render_at(&g, demosaic, "output", part, 0.25).unwrap();
    let regions: Vec<Region> = events
        .try_iter()
        .filter(|ev| ev.kind == "engine.step_computed")
        .map(|ev| {
            let r = &ev.payload["region"];
            let n = |k: &str| r[k].as_i64().unwrap();
            Region::new(n("x"), n("y"), n("width") as u32, n("height") as u32)
        })
        .collect();
    assert_eq!(regions.len(), 3, "the mosaic, demosaic, the reduction");
    assert_eq!(regions[2], part);
    assert!(regions[1].contains(&part.full_resolution(0.25)));
    assert!(regions.iter().all(|r| r.area() < 60 * 60), "{regions:?}");
}
