//! A chain of GPU steps shown in the viewport stays on the GPU: the engine
//! computes on the device the viewport draws with (the process's one
//! device), and its results reach the screen as device tiles. Nothing is
//! read back, nothing uploaded again; only the source's pixels, which live
//! on the host, go up once a render. Runs on whatever adapter the
//! headless harness opens (lavapipe in CI).

use std::sync::Arc;
use std::time::Duration;

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::wgpu::WgpuTestRenderer;
use egui_pop::viewport::{View, Viewport, ViewportState};
use helios_core::{Bus, Engine, Graph, Image, ModuleRef, NodeId, Params, Region, Value};
use helios_gui::engine_source::{EngineSource, GpuSlot, Shown, Stats};
use helios_gui::shell::headless::render_state;

const W: u32 = 640;
const H: u32 = 480;

/// A ramp through two exposures: both have a GPU variant.
fn chain() -> (Arc<Graph>, NodeId) {
    let mut g = Graph::new();
    let source = g.add_source(
        "source",
        Image::from_fn(Region::sized(W, H), 4, |x, y, c| match c {
            0 => 0.05 + 0.4 * x as f32 / W as f32,
            1 => 0.1 + 0.2 * y as f32 / H as f32,
            2 => 0.15,
            _ => 1.0,
        }),
    );
    let mut prev = (source, "image");
    for (name, ev) in [("brighter", 1.0), ("darker", -0.5)] {
        let n = g
            .add_module(
                name,
                ModuleRef::new(helios_module_exposure::Exposure),
                &Params::new().with("exposure", Value::Float(ev)),
            )
            .unwrap();
        g.connect(prev.0, prev.1, n, "input").unwrap();
        prev = (n, "output");
    }
    (Arc::new(g), prev.0)
}

struct Scene {
    rs: eframe::egui_wgpu::RenderState,
    installed: bool,
    state: ViewportState,
    view: View,
    source: EngineSource,
    engine: Arc<Engine>,
    stats: Arc<Stats>,
}

/// The chain in a viewport drawn on the process's device: its engine
/// computing there, its source handing device tiles when `on_gpu`.
fn harness(on_gpu: bool) -> Harness<'static, Scene> {
    let rs = render_state();
    let mut state = ViewportState::new();
    state.set_settle(0.0);
    let stats = Arc::<Stats>::default();
    let engine = Engine::new(Arc::new(Bus::new()));
    let slot = GpuSlot::default();
    let engine = if on_gpu {
        let (_, gpu) = helios_gpu::shared::opened().expect("the harness opened the GPU");
        assert!(gpu.handles().device == rs.device, "one device a process");
        slot.set(gpu.clone()).ok();
        Arc::new(engine.with_accelerator(gpu))
    } else {
        Arc::new(engine)
    };
    let source = EngineSource::on_gpu(
        "test-zero-copy",
        engine.clone(),
        state.sink(),
        stats.clone(),
        slot,
    );
    let (g, node) = chain();
    source.show(Shown::new(g, node, "output").unwrap());
    Harness::builder()
        .with_size(vec2(400.0, 300.0))
        .renderer(WgpuTestRenderer::from_render_state(rs.clone()))
        .build_ui_state(
            |ui, s: &mut Scene| {
                if !s.installed {
                    egui_pop::viewport::gpu::install(ui.ctx(), &s.rs);
                    s.installed = true;
                }
                Viewport::new("t")
                    .source(&s.source)
                    .show(ui, &mut s.state, &mut s.view);
            },
            Scene {
                rs,
                installed: false,
                state,
                view: View::fit(),
                source,
                engine,
                stats,
            },
        )
}

/// Steps frames until every tile wanted is on screen, waiting after each
/// for the render thread to answer what the frame asked. Each frame is
/// painted: the GPU path binds (or uploads) its tiles at the paint, and
/// counts them there.
fn settle(h: &mut Harness<'_, Scene>) {
    for _ in 0..40 {
        h.step();
        assert!(
            h.state().stats.wait_idle(Duration::from_mins(1)),
            "the render thread never answered"
        );
        h.step();
        let _ = h.render().expect("a painted frame");
        if h.state().state.tiles_complete() {
            return;
        }
    }
    panic!("the tiles never all came: {:?}", h.state().stats);
}

#[test]
fn a_gpu_chain_reaches_the_screen_without_a_transfer() {
    let mut h = harness(true);
    settle(&mut h);
    let s = h.state();
    let (renders, tiles) = (Stats::get(&s.stats.renders), Stats::get(&s.stats.tiles));
    assert!(tiles > 0, "{:?}", s.stats);
    assert_eq!(Stats::get(&s.stats.device_tiles), tiles, "{:?}", s.stats);
    assert_eq!(Stats::get(&s.stats.failed), 0);
    assert_eq!(s.state.last_error(), None);

    // The engine: every step on the GPU, nothing read back; the source's
    // host pixels uploaded at most once a render.
    let c = s.engine.counters();
    assert!(c.accelerated >= 2, "{c:?}");
    assert_eq!(c.readbacks, 0, "{c:?}");
    assert!(c.uploads <= renders, "{c:?} for {renders} renders");

    // The viewport: every tile bound where it lies, no byte copied.
    let t = s.state.transfers();
    assert_eq!((t.bytes, t.tiles), (0, 0), "{t:?}");
    // A tile replaced before a paint is never bound.
    assert!(t.bound > 0 && t.bound <= tiles, "{t:?} for {tiles} tiles");
    let refused = s
        .state
        .stats()
        .snapshot()
        .into_iter()
        .find(|(name, _)| *name == "refused");
    assert_eq!(refused, Some(("refused", 0)));

    // Drawn again, nothing moves.
    let before = s.engine.counters();
    for _ in 0..3 {
        h.run();
        let _ = h.render().expect("a painted frame");
        let f = h.state().state.frame_transfers();
        assert_eq!((f.bytes, f.tiles), (0, 0), "the last frame copied {f:?}");
    }
    let d = h.state().engine.counters().since(&before);
    assert_eq!((d.uploads, d.readbacks), (0, 0), "{d:?}");
}

#[test]
fn device_tiles_draw_what_the_host_path_draws() {
    let mut gpu = harness(true);
    settle(&mut gpu);
    let mut host = harness(false);
    settle(&mut host);
    assert_eq!(Stats::get(&host.state().stats.device_tiles), 0);
    let (a, b) = (gpu.render().unwrap(), host.render().unwrap());
    assert_eq!(a.dimensions(), b.dimensions());
    // Half-float uploads against f32 buffers, GPU kernels against the CPU
    // reference: a step of 8-bit at most.
    let worst = a
        .pixels()
        .zip(b.pixels())
        .flat_map(|(p, q)| (0..3).map(move |i| p.0[i].abs_diff(q.0[i])))
        .max()
        .unwrap_or(0);
    assert!(worst <= 2, "the pictures differ by {worst}");
}
