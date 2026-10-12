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

/// The risk the darktable pitfalls inventory inferred, shown: at half
/// scale the framework box-averages the mosaic before demosaic, so each
/// value mixes a red, two green and a blue photosite, and demosaic reads
/// that mix as a mosaic. A coloured scene comes out grey.
#[test]
fn a_half_scale_render_mixes_photosites_before_demosaic() {
    let color = [0.8f32, 0.5, 0.2];
    let full = Region::sized(32, 32);
    let (g, demosaic) = network(color, full);
    let e = Engine::new(Arc::new(Bus::new()));
    let whole = e.render(&g, demosaic, "output", full).unwrap();
    let reduced = whole.sample_at(Region::sized(16, 16), 0.5);
    let half = e
        .render_at(&g, demosaic, "output", Region::sized(16, 16), 0.5)
        .unwrap();
    let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-3);
    // Away from the edges, where replicated photosites change colour.
    let inner = Region::new(4, 4, 8, 8);
    let (reduced, half) = (reduced.sample(inner), half.sample(inner));
    // The full render reduced keeps the scene's colour.
    assert!(close(mean_rgb(&reduced), color), "{:?}", mean_rgb(&reduced));
    // The half-scale render is the mean of the four photosites, grey.
    let mix = (color[0] + 2.0 * color[1] + color[2]) / 4.0;
    assert!(
        close(mean_rgb(&half), [mix; 3]),
        "{:?} against {mix}",
        mean_rgb(&half)
    );
}
