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
    e.render_at(&g, demosaic, "output", part, 0.5).unwrap();
    let regions: Vec<Region> = events
        .try_iter()
        .filter(|ev| ev.kind == "helios.engine.step_computed")
        .map(|ev| {
            let r = &ev.payload["region"];
            let n = |k: &str| r[k].as_i64().unwrap();
            Region::new(n("x"), n("y"), n("width") as u32, n("height") as u32)
        })
        .collect();
    assert_eq!(regions.len(), 3, "the mosaic, demosaic, the reduction");
    assert_eq!(regions[2], part);
    assert!(regions[1].contains(&part.full_resolution(0.5)));
    assert!(regions.iter().all(|r| r.area() < 60 * 60), "{regions:?}");
}

/// At a quarter scale or less demosaic reads the full-resolution mosaic
/// straight to the asked scale (`DIRECT_REDUCTION`): one step instead of
/// a full demosaic and a reduction, a coloured scene keeping its colour.
/// A render holding its full-size steps (the shown photo's, for the fit
/// and zoom to read) still demosaics at full resolution.
#[test]
fn a_quarter_scale_render_reads_the_mosaic_straight() {
    let color = [0.8f32, 0.5, 0.2];
    let (g, demosaic) = network(color, Region::sized(400, 300));
    let e = Engine::new(Arc::new(Bus::new()));
    let events = e.bus().subscribe();
    let (listener, records) = helios_core::trace::collect();
    let part = Region::new(20, 10, 8, 8);
    let image = e.render_at(&g, demosaic, "output", part, 0.25).unwrap();
    drop(listener);
    let regions: Vec<Region> = events
        .try_iter()
        .filter(|ev| ev.kind == "helios.engine.step_computed")
        .map(|ev| {
            let r = &ev.payload["region"];
            let n = |k: &str| r[k].as_i64().unwrap();
            Region::new(n("x"), n("y"), n("width") as u32, n("height") as u32)
        })
        .collect();
    assert_eq!(regions.len(), 2, "the mosaic, then demosaic at its scale");
    assert_eq!(regions[1], part);
    assert!(regions[0].contains(&part.full_resolution(0.25)));
    assert!(regions[0].area() < 60 * 60, "{regions:?}");
    let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-3);
    assert!(close(mean_rgb(&image), color), "{:?}", mean_rgb(&image));
    let me = std::thread::current().name().map(String::from);
    let names: Vec<&str> = records
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.thread == me)
        .map(|r| r.name)
        .collect();
    assert!(names.contains(&"demosaic.direct"), "{names:?}");
    assert!(!names.contains(&"demosaic"), "{names:?}");

    // The full-size steps are what a hold keeps: computed by the full
    // demosaic, then reduced.
    let whole = Region::sized(100, 75);
    assert!(
        !e.full_size_keys(&g, demosaic, whole, 0.25)
            .unwrap()
            .is_empty()
    );
    let held = e.hold_at(&g, demosaic, "output", whole, 0.25).unwrap();
    assert!(held.bytes() > 0);
}

/// On a smooth scene the straight quarter-scale output is what the full
/// demosaic reduced gives, within a small tolerance, on Bayer and X-Trans.
#[test]
fn a_quarter_scale_render_matches_the_full_render_reduced() {
    for cfa_name in ["RGGB", "GGRGGB/GGBGGR/BRGRBG/GGBGGR/GGRGGB/RBGBRG"] {
        let cfa = Cfa::parse(cfa_name).unwrap();
        let full = Region::sized(240, 160);
        // A gentle gradient, a different one per colour.
        let scene = |x: i64, y: i64| {
            let (u, v) = (x as f32 / 240.0, y as f32 / 160.0);
            [0.2 + 0.6 * u, 0.3 + 0.4 * v, 0.7 - 0.5 * u * v]
        };
        let mosaic = Image::from_fn(full, 1, |x, y, _| {
            let rgb = scene(x, y);
            match cfa.color_at(x, y) {
                Channel::Red => rgb[0],
                Channel::Green => rgb[1],
                Channel::Blue => rgb[2],
            }
        });
        let mut g = Graph::new();
        let src = g.add_source("raw", mosaic);
        let demosaic = g
            .add_module("demosaic", ModuleRef::new(Demosaic), &Params::new())
            .unwrap();
        g.set_context(
            demosaic,
            Context::new().with(fields::CFA, Value::Choice(cfa_name.into())),
        )
        .unwrap();
        g.connect(src, SOURCE_PORT, demosaic, "input").unwrap();
        let e = Engine::new(Arc::new(Bus::new()));
        let reduced = e
            .render(&g, demosaic, "output", full)
            .unwrap()
            .sample_at(Region::sized(60, 40), 0.25);
        let direct = e
            .render_at(&g, demosaic, "output", Region::sized(60, 40), 0.25)
            .unwrap();
        // Away from the edges, where replicated photosites change colour.
        let inner = Region::new(4, 4, 52, 32);
        let worst = (inner.y..inner.bottom())
            .flat_map(|y| (inner.x..inner.right()).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let (a, b) = (direct.pixel(x, y), reduced.pixel(x, y));
                (0..3).map(move |c| (a[c] - b[c]).abs())
            })
            .fold(0.0f32, f32::max);
        assert!(worst < 0.02, "{cfa_name}: worst difference {worst}");
    }
}

/// Opening a small synthetic raw at half scale leaves a trace span per
/// step, in the order the engine runs them: the mosaic read, the
/// demosaic call (its algorithm and megapixels) inside its engine step,
/// then the reduction to the asked scale.
#[test]
fn a_half_scale_render_traces_each_step_in_order() {
    let (g, demosaic) = network([0.8, 0.5, 0.2], Region::sized(64, 48));
    let e = Engine::new(Arc::new(Bus::new()));
    let (listener, records) = helios_core::trace::collect();
    e.render_at(&g, demosaic, "output", Region::sized(32, 24), 0.5)
        .unwrap();
    drop(listener);
    // This test's own spans: a test alongside may trace its renders too.
    let mine = |r: &&helios_core::trace::Record| {
        r.thread == std::thread::current().name().map(String::from)
    };
    let records = records.lock().unwrap();
    let names: Vec<&str> = records.iter().filter(mine).map(|r| r.name).collect();
    assert_eq!(
        names,
        ["engine.source", "demosaic", "engine.step", "engine.reduce"],
        "{records:?}"
    );
    let span = |name: &str| {
        records
            .iter()
            .filter(mine)
            .find(|r| r.name == name)
            .unwrap()
    };
    assert_eq!(span("demosaic").detail, "Rcd 64x48 0.00 MP");
    assert!(
        span("engine.step")
            .detail
            .starts_with("demosaic node=demosaic 64x48")
    );
    assert!(span("engine.reduce").detail.contains("32x24"));
    // The demosaic call lies inside its step.
    assert!(span("demosaic").start >= span("engine.step").start);
    assert!(span("demosaic").end() <= span("engine.step").end());
}
