//! The viewport: a proxy then the exact render, tiles reused on a pan,
//! values driving the view and the graph, and what is on screen matching
//! the engine's render through the display transform.

use std::sync::atomic::Ordering;
use std::time::Duration;

use egui::vec2;
use helios_core::{Graph, Image, ModuleRef, Params, Region};
use helios_gui::viewport::display;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

const W: u32 = 1200;
const H: u32 = 900;

/// A smooth horizontal ramp through an exposure node.
fn shell() -> Shell {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(W, H), 4, |x, y, c| match c {
            0 => 0.05 + 0.4 * x as f32 / W as f32,
            1 => 0.1 + 0.2 * y as f32 / H as f32,
            2 => 0.15,
            _ => 1.0,
        }),
    );
    let exp = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, exp, "output").unwrap();
    shell
}

/// Steps frames until `done`, waiting after each for the render thread to
/// answer what the frame asked. A frame is a quarter of a second of the
/// harness's clock, so the view settles within a few.
fn until(h: &mut egui_kittest::Harness<'_, Shell>, done: impl Fn(&Shell) -> bool) {
    for _ in 0..40 {
        h.step();
        let stats = h.state().viewport_stats();
        // Generous: only a hung render thread reaches it.
        assert!(
            stats.wait_idle(Duration::from_mins(1)),
            "the render thread never answered: {stats:?}"
        );
        h.step();
        if done(h.state()) {
            return;
        }
    }
    panic!(
        "the viewport never got there: {:?}",
        h.state().viewport_stats()
    );
}

fn exacts(s: &Shell) -> u64 {
    s.viewport_stats().exacts.load(Ordering::Relaxed)
}

#[test]
fn a_proxy_comes_first_then_the_exact_render() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    until(&mut h, |s| exacts(s) >= 1);
    let stats = h.state().viewport_stats();
    assert!(stats.proxies.load(Ordering::Relaxed) >= 1);
    // Fitted, the view is smaller than the image: the exact render is at
    // the view's scale, below 1.
    let scene = h.state().viewport_scene().unwrap();
    assert!(scene.region().width < W, "{:?}", scene.region());
}

#[test]
fn what_is_on_screen_is_the_engine_render_through_the_display_transform() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    h.state_mut()
        .run("viewport.zoom.100", json!(null), Origin::User)
        .unwrap();
    until(&mut h, |s| {
        exacts(s) >= 1 && s.viewport_scene().is_some_and(|i| i.region().x > 0)
    });
    let shot = headless::snapshot(&mut h, "viewport.display").unwrap();
    // At 100 %, centred: the view's middle is the image's middle.
    let (cx, cy) = (shot.width() / 2, shot.height() / 2);
    let got = shot.get_pixel(cx, cy).0;
    let x = W as f32 / 2.0;
    let y = H as f32 / 2.0;
    let want = display([0.05 + 0.4 * x / W as f32, 0.1 + 0.2 * y / H as f32, 0.15]);
    for c in 0..3 {
        assert!(
            (i32::from(got[c]) - i32::from(want[c])).abs() <= 1,
            "channel {c}: {got:?} on screen, {want:?} expected"
        );
    }
}

#[test]
fn a_pan_renders_only_the_new_tiles() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    h.state_mut()
        .run("viewport.zoom.100", json!(null), Origin::User)
        .unwrap();
    until(&mut h, |s| {
        exacts(s) >= 1 && s.viewport_scene().is_some_and(|i| i.region().x > 0)
    });
    let rendered = h
        .state()
        .viewport_stats()
        .tiles_rendered
        .load(Ordering::Relaxed);
    let before = exacts(h.state());
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": "view/viewport/center_x", "value": 700.0 }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    until(&mut h, |s| exacts(s) > before);
    let stats = h.state().viewport_stats();
    assert!(stats.tiles_reused.load(Ordering::Relaxed) > 0, "{stats:?}");
    let new = stats.tiles_rendered.load(Ordering::Relaxed) - rendered;
    assert!(new < rendered, "{new} new tiles, {rendered} at first");
}

#[test]
fn an_agent_changing_a_parameter_renders_again() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    h.state_mut()
        .run("viewport.zoom.100", json!(null), Origin::User)
        .unwrap();
    until(&mut h, |s| {
        exacts(s) >= 1 && s.viewport_scene().is_some_and(|i| i.region().x > 0)
    });
    let dark = headless::snapshot(&mut h, "viewport.display").unwrap();
    let before = exacts(h.state());
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": "node/exposure/exposure", "value": 1.0 }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    until(&mut h, |s| exacts(s) > before);
    let bright = headless::snapshot(&mut h, "viewport.display").unwrap();
    let (cx, cy) = (dark.width() / 2, dark.height() / 2);
    assert!(
        bright.get_pixel(cx, cy).0[0] > dark.get_pixel(cx, cy).0[0] + 20,
        "{:?} then {:?}",
        dark.get_pixel(cx, cy),
        bright.get_pixel(cx, cy)
    );
}

#[test]
fn the_scene_is_saved_before_the_display_transform() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    until(&mut h, |s| exacts(s) >= 1);
    let dir = std::env::temp_dir().join(format!("helios-scene-{}", std::process::id()));
    let path = dir.join("scene.png");
    h.state_mut()
        .run(
            "gui.snapshot",
            json!({ "target": "viewport.scene", "path": path }),
            Origin::User,
        )
        .unwrap();
    h.step();
    let png = image::open(&path).unwrap().into_rgba16();
    let scene = h.state().viewport_scene().unwrap();
    let r = scene.region();
    assert_eq!(png.dimensions(), (r.width, r.height));
    // Linear values, not display-encoded.
    let v = scene.pixel(r.x, r.y)[0];
    let got = f32::from(png.get_pixel(0, 0).0[0]) / 65535.0;
    assert!((got - v).abs() < 1e-3, "{got} vs {v}");
    std::fs::remove_dir_all(dir).ok();
}
