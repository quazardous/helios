//! A photo's opening asks one module needing full resolution (the
//! demosaic) at three scales at once: the viewport's proxy, the
//! preview and the fitted frame. Each reduced ask maps back to a
//! full-resolution region a few pixels past the image, a little
//! different at each scale; they must share one full-size computation,
//! and each must read the same pixels as it would alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use helios_core::*;

/// Needs full resolution, like the demosaic; its output past the image
/// depends on the coordinates (it is not the edge replicated), so a
/// reduce reading a clamped region instead would not be bit-exact.
struct FullSize {
    calls: Arc<AtomicUsize>,
}

impl Module for FullSize {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "full_size".into(),
            version: 1,
            description: "a demosaic stand-in".into(),
            params: vec![],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs {
                overlap: 1,
                full_resolution: true,
                ..Needs::default()
            },
            kind: ModuleKind::Process,
            family: Family::Detail,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let input = inputs[0];
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            (input.pixel(x - 1, y)[0] + input.pixel(x + 1, y)[0]) * 0.5
                + x as f32 * 1e-3
                + y as f32 * 1e-2
        })])
    }
}

/// The Nikon Z6 II's size: proxy, preview and fit map back to
/// 6048×4032, 6048×4039 and 6050×4024.
const W: u32 = 6048;
const H: u32 = 4024;

fn graph(calls: &Arc<AtomicUsize>) -> (Graph, NodeId) {
    let mut g = Graph::new();
    let src = g.add_source(
        "src",
        Image::from_fn(Region::sized(W, H), 1, |x, y, _| {
            ((x * 7 + y * 13) % 97) as f32
        }),
    );
    let node = g
        .add_module(
            "demosaic",
            ModuleRef::new(FullSize {
                calls: calls.clone(),
            }),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", node, "input").unwrap();
    (g, node)
}

/// The region a scale's render asks for, as the viewport and the preview
/// size it: the image at that scale, rounded up.
fn at(s: f64) -> Region {
    Region::sized(
        ((f64::from(W) * s).ceil() as u32).max(1),
        ((f64::from(H) * s).ceil() as u32).max(1),
    )
}

/// The open's three scales: the fit in 1920×1080, a proxy at a power of
/// two below a quarter of it, the 256 px preview.
fn scales() -> [f64; 3] {
    let fit = (1920.0 / f64::from(W)).min(1080.0 / f64::from(H));
    let mut proxy = 1.0;
    while proxy > fit / 4.0 {
        proxy /= 2.0;
    }
    [proxy, 256.0 / f64::from(W.max(H)), fit]
}

#[test]
fn the_three_asks_of_an_open_map_to_different_full_size_regions() {
    let regions: Vec<Region> = scales().iter().map(|&s| at(s).full_resolution(s)).collect();
    assert_eq!(
        regions,
        [
            Region::sized(6048, 4032),
            Region::sized(6048, 4039),
            Region::sized(6050, 4024)
        ]
    );
}

#[test]
fn an_open_runs_one_full_size_computation_for_proxy_preview_and_fit() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (g, node) = graph(&calls);
    let g = Arc::new(g);
    let engine = Arc::new(Engine::new(Arc::new(Bus::new())));
    let [proxy, preview, fit] = scales();
    // As the desktop asks: the viewport's proxy then its fit, and the
    // preview at the same moment.
    let (e, gv) = (engine.clone(), g.clone());
    let view = thread::spawn(move || {
        let p = e.render_at(&gv, node, "output", at(proxy), proxy).unwrap();
        let f = e.render_at(&gv, node, "output", at(fit), fit).unwrap();
        (p, f)
    });
    let (e, gp) = (engine.clone(), g.clone());
    let pre = thread::spawn(move || {
        e.render_at(&gp, node, "output", at(preview), preview)
            .unwrap()
    });
    let shown = pre.join().unwrap();
    let (p, f) = view.join().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1, "full-size computations");

    // Each reads what it reads alone: the full-size pixels its own
    // region covers, past the image included, reduced.
    for (s, image) in [(proxy, p), (preview, shown), (fit, f)] {
        let r = at(s);
        let full = Engine::new(Arc::new(Bus::new()))
            .render(&g, node, "output", r.full_resolution(s))
            .unwrap();
        assert!(
            image.data() == full.sample_at(r, s).data(),
            "at {s}: not bit-exact"
        );
    }
}
