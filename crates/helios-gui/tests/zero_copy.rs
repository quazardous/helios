//! A chain of GPU steps shown in the viewport stays on the GPU: the engine
//! computes on the device the viewport draws with (the process's one
//! device), and its results reach the screen as device tiles. Nothing is
//! read back, nothing uploaded again; only the source's pixels, which live
//! on the host, go up once a render. The mask display stays there too:
//! its overlay is composited on the device by a WGSL kernel, checked here
//! against the CPU composite, the reference, within its declared
//! tolerance. Runs on whatever adapter the headless harness opens
//! (lavapipe in CI).

use std::sync::Arc;
use std::time::Duration;

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::wgpu::WgpuTestRenderer;
use egui_pop::viewport::{View, Viewport, ViewportState};
use helios_core::graph::SOURCE_PORT;
use helios_core::{
    Accelerator, Bus, Engine, Graph, Image, ModuleRef, NodeId, Params, Region, Value,
};
use helios_gui::engine_source::{EngineSource, GpuSlot, Shown, Stats, rgba};
use helios_gui::mask_display::{self, Layer, Mode, Style, Target};
use helios_gui::shell::headless::render_state;

const W: u32 = 640;
const H: u32 = 480;

/// A ramp through two exposures (both have a GPU variant), and a mask
/// beside it: a horizontal ramp of its own source.
fn chain() -> (Arc<Graph>, NodeId, NodeId) {
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
    let mask = g.add_source(
        "mask",
        Image::from_fn(Region::sized(W, H), 1, |x, _, _| x as f32 / W as f32),
    );
    (Arc::new(g), prev.0, mask)
}

/// The mask display of `mask` in `mode`, in the default style.
fn layer(mask: NodeId, mode: Mode) -> Layer {
    Layer {
        mode,
        target: Target {
            key: "mask".into(),
            name: "mask".into(),
            node: mask,
            port: SOURCE_PORT.into(),
        },
        style: Style::default(),
    }
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
    harness_showing(on_gpu, Mode::Off)
}

/// [`harness`], the mask display in `mode` over the chain.
fn harness_showing(on_gpu: bool, mode: Mode) -> Harness<'static, Scene> {
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
    let (g, node, mask) = chain();
    let shown = Shown::new(g, node, "output")
        .unwrap()
        .map(|s| s.with_mask((mode != Mode::Off).then(|| layer(mask, mode))));
    source.show(shown);
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

#[test]
fn a_mask_overlay_on_a_gpu_chain_stays_on_the_device() {
    let mut h = harness_showing(true, Mode::Overlay);
    settle(&mut h);
    let s = h.state();
    let (renders, tiles) = (Stats::get(&s.stats.renders), Stats::get(&s.stats.tiles));
    assert!(tiles > 0, "{:?}", s.stats);
    // Every tile handed on the device, its overlay laid there.
    assert_eq!(Stats::get(&s.stats.device_tiles), tiles, "{:?}", s.stats);
    assert_eq!(Stats::get(&s.stats.device_masks), tiles, "{:?}", s.stats);
    assert_eq!(Stats::get(&s.stats.failed), 0);
    assert_eq!(s.state.last_error(), None);

    // Nothing read back; the image's source and the mask's, both on the
    // host, go up at most once each a render.
    let c = s.engine.counters();
    assert_eq!(c.readbacks, 0, "{c:?}");
    assert!(c.uploads <= 2 * renders, "{c:?} for {renders} renders");

    // The viewport binds the composite where it lies, copying nothing.
    let t = s.state.transfers();
    assert_eq!((t.bytes, t.tiles), (0, 0), "{t:?}");
    assert!(t.bound > 0 && t.bound <= tiles, "{t:?} for {tiles} tiles");

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
fn the_mask_display_on_the_device_draws_what_the_host_path_draws() {
    for mode in [Mode::Overlay, Mode::Grey] {
        let mut gpu = harness_showing(true, mode);
        settle(&mut gpu);
        assert!(Stats::get(&gpu.state().stats.device_masks) > 0, "{mode:?}");
        let mut host = harness_showing(false, mode);
        settle(&mut host);
        assert_eq!(Stats::get(&host.state().stats.device_masks), 0);
        let (a, b) = (gpu.render().unwrap(), host.render().unwrap());
        assert_eq!(a.dimensions(), b.dimensions());
        let worst = a
            .pixels()
            .zip(b.pixels())
            .flat_map(|(p, q)| (0..3).map(move |i| p.0[i].abs_diff(q.0[i])))
            .max()
            .unwrap_or(0);
        assert!(worst <= 2, "{mode:?}: the pictures differ by {worst}");
    }
}

/// Deterministic values in `[lo, hi]`, a different stream by `seed`.
fn values(region: Region, channels: u32, seed: u64, (lo, hi): (f32, f32)) -> Image {
    Image::from_fn(region, channels, |x, y, c| {
        let mut z = seed
            ^ (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (y as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
            ^ u64::from(c).wrapping_mul(0x1656_67b1_9e37_79f9);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        lo + (hi - lo) * ((z >> 40) as f32 / (1u64 << 24) as f32)
    })
}

/// Backend contract: the WGSL composite against the CPU one, the
/// reference, on every mode, styles at their extremes, images of one to
/// four channels with values past the display's range, and masks past
/// `[0, 1]`, within `mask_display::GPU_TOLERANCE`.
#[test]
fn the_mask_composite_on_the_gpu_matches_the_cpu_reference() {
    // The process's one device, as the window draws with.
    let _ = render_state();
    let (_, gpu) = helios_gpu::shared::opened().expect("the harness opened the GPU");
    // Odd sizes off the origin: the last workgroup runs past the pixels.
    let region = Region::new(5, -3, 37, 23);
    let styles = [
        Style::default(),
        Style {
            color: [0.1, 0.8, 0.3],
            opacity: 1.0,
            desaturate: 0.0,
        },
        Style {
            color: [1.0, 0.0, 1.0],
            opacity: 0.25,
            desaturate: 0.5,
        },
    ];
    for channels in 1..=4 {
        let image = values(region, channels, u64::from(channels), (-0.1, 1.4));
        for mask_channels in [1, 2] {
            let mask = values(region, mask_channels, 99, (-0.25, 1.25));
            let (on_image, on_mask) = (gpu.upload(&image).unwrap(), gpu.upload(&mask).unwrap());
            for mode in [Mode::Overlay, Mode::Grey] {
                for style in &styles {
                    let mut want = rgba(&image);
                    mask_display::composite(&mut want, &mask, mode, style);
                    let want = Image::new(region, 4, want);
                    let laid =
                        mask_display::composite_on_device(&gpu, &on_image, &on_mask, mode, style)
                            .unwrap();
                    assert_eq!((laid.region(), laid.channels()), (region, 4));
                    let got = gpu.download(&laid).unwrap();
                    let c = helios_gpu::compare(&got, &want).unwrap();
                    assert!(
                        c.max_rel <= mask_display::GPU_TOLERANCE,
                        "{channels} channels, a mask of {mask_channels}, {mode:?}, {style:?}: {c:?}"
                    );
                }
            }
        }
    }
    // Off lays nothing: an error, never a copy.
    let (a, b) = (
        gpu.upload(&values(region, 3, 1, (0.0, 1.0))).unwrap(),
        gpu.upload(&values(region, 1, 2, (0.0, 1.0))).unwrap(),
    );
    assert!(mask_display::composite_on_device(&gpu, &a, &b, Mode::Off, &Style::default()).is_err());
    // Images of two regions are refused.
    let other = gpu
        .upload(&values(Region::new(6, -3, 37, 23), 1, 3, (0.0, 1.0)))
        .unwrap();
    assert!(
        mask_display::composite_on_device(&gpu, &a, &other, Mode::Grey, &Style::default()).is_err()
    );
}
