//! The viewport: egui-pop's viewport fed by the engine, drawn on the GPU.
//! What is on screen is the engine's render through the display
//! transform; a pan renders only the new tiles; the view lives in the
//! value store, moved by commands, agents and drags, never as an undo
//! step; before and after side by side; the readout.

use std::time::Duration;

use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use helios_core::{Graph, Image, ModuleRef, Params, Region};
use helios_gui::engine_source::Stats;
use helios_gui::viewport::{CENTER_X, COMPARE, FIT, ZOOM, display};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

const W: u32 = 1200;
const H: u32 = 900;

/// The ramp's working-space colour at an image pixel.
fn ramp(x: f32, y: f32) -> [f32; 3] {
    [0.05 + 0.4 * x / W as f32, 0.1 + 0.2 * y / H as f32, 0.15]
}

/// A smooth ramp through an exposure node.
fn shell() -> Shell {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(W, H), 4, |x, y, c| match c {
            0..=2 => ramp(x as f32, y as f32)[c as usize],
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
    // The palettes hidden (Tab): the whole image in sight.
    egui_pop::api::palettes::set_hidden(shell.app().handle(), true);
    shell
}

/// Steps frames until every tile wanted is on screen and `done` holds,
/// waiting after each frame for the render threads to answer what it
/// asked. A frame is a quarter of a second of the harness's clock, so the
/// view settles within a few.
fn until(h: &mut Harness<'_, Shell>, done: impl Fn(&Shell) -> bool) {
    for _ in 0..40 {
        h.step();
        let stats = h.state().viewport_stats();
        // Generous: only a hung render thread reaches it.
        assert!(
            stats.wait_idle(Duration::from_mins(1)),
            "the render thread never answered: {stats:?}"
        );
        h.step();
        if h.state().viewport().state().tiles_complete() && done(h.state()) {
            return;
        }
    }
    panic!(
        "the viewport never got there: {:?}",
        h.state().viewport_stats()
    );
}

fn settled(h: &mut Harness<'_, Shell>) {
    until(h, |_| true);
}

fn tiles(s: &Shell) -> u64 {
    Stats::get(&s.viewport_stats().tiles)
}

fn run(h: &mut Harness<'_, Shell>, command: &str, args: serde_json::Value) {
    h.state_mut().run(command, args, Origin::User).unwrap();
}

fn value(s: &Shell, path: &str) -> f64 {
    let store = s.values.lock().unwrap();
    let v = &store.get(path).unwrap().value;
    v.as_f64().or_else(|| v.as_bool().map(f64::from)).unwrap()
}

/// The pixel of a picture at a point of the window.
fn at(shot: &image::RgbaImage, h: &Harness<'_, Shell>, p: Pos2) -> [u8; 4] {
    let r = h.state().target_rect("viewport.display").unwrap();
    let ppp = h.state().pixels_per_point();
    let x = ((p.x - r.min.x) * ppp) as u32;
    let y = ((p.y - r.min.y) * ppp) as u32;
    shot.get_pixel(x, y).0
}

fn assert_close(got: [u8; 4], want: [u8; 3], what: &str) {
    for c in 0..3 {
        assert!(
            (i32::from(got[c]) - i32::from(want[c])).abs() <= 1,
            "{what}, channel {c}: {got:?} on screen, {want:?} expected"
        );
    }
}

#[test]
fn fitted_the_whole_image_shows_at_the_view_scale() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    settled(&mut h);
    assert!(tiles(h.state()) >= 1);
    // Fitted, the view is smaller than the image: the exact tiles are at
    // the view's scale, below 1.
    let scene = h.state().viewport_scene().unwrap();
    assert!(scene.region().width < W, "{:?}", scene.region());
    assert_eq!(value(h.state(), FIT), 1.0);
    // Around the image, the theme's neutral surround.
    let shot = headless::snapshot(&mut h, "viewport.display").unwrap();
    let r = h.state().target_rect("viewport.display").unwrap();
    let m = h.state().viewport().state().mapping().unwrap();
    let image = m.image_rect_to_screen([0.0, 0.0, f64::from(W), f64::from(H)]);
    assert!(image.left() > r.left() + 4.0, "{image:?} in {r:?}");
    let surround = egui_pop::core::theme::Tokens::of(&h.ctx)
        .color("viewport.surround")
        .unwrap();
    let got = at(&shot, &h, egui::pos2(r.left() + 2.0, r.center().y));
    assert_close(got, [surround.r(), surround.g(), surround.b()], "surround");
}

#[test]
fn what_is_on_screen_is_the_engine_render_through_the_display_transform() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    run(&mut h, "viewport.zoom.100", json!(null));
    until(&mut h, |s| {
        s.viewport_scene().is_some_and(|i| i.region().x > 0)
    });
    assert_eq!(value(h.state(), ZOOM), 1.0);
    assert!(h.state().viewport_gpu().is_some(), "drawn on the GPU path");
    let shot = headless::snapshot(&mut h, "viewport.display").unwrap();
    // At 100 %, the pixel under a point is the drawing's own rule.
    let m = h.state().viewport().state().mapping().unwrap();
    for p in [m.rect.center(), m.rect.center() + vec2(-200.0, 120.0)] {
        let [x, y] = m.pixel_at(p).unwrap();
        let want = display(ramp(x as f32, y as f32));
        assert_close(at(&shot, &h, p), want, &format!("pixel ({x}, {y})"));
    }
}

#[test]
fn a_pan_renders_only_the_new_tiles() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    run(&mut h, "viewport.zoom.100", json!(null));
    until(&mut h, |s| {
        s.viewport_scene().is_some_and(|i| i.region().x > 0)
    });
    let first = tiles(h.state());
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": CENTER_X, "value": 700.0 }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    until(&mut h, |s| tiles(s) > first);
    let new = tiles(h.state()) - first;
    assert!(new < first, "{new} new tiles, {first} at first");
}

#[test]
fn an_agent_changing_a_parameter_renders_again() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    run(&mut h, "viewport.zoom.100", json!(null));
    settled(&mut h);
    let dark = headless::snapshot(&mut h, "viewport.display").unwrap();
    let before = tiles(h.state());
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": "node/exposure/exposure", "value": 1.0 }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    until(&mut h, |s| tiles(s) > before);
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
    settled(&mut h);
    let dir = std::env::temp_dir().join(format!("helios-scene-{}", std::process::id()));
    let path = dir.join("scene.png");
    run(
        &mut h,
        "gui.snapshot",
        json!({ "target": "viewport.scene", "path": path }),
    );
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

#[test]
fn the_zoom_commands_walk_the_ladder_from_100_percent() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    settled(&mut h);
    let zoom = |h: &mut Harness<'_, Shell>, command: &str| {
        run(h, command, json!(null));
        h.step();
        value(h.state(), ZOOM)
    };
    assert_eq!(zoom(&mut h, "viewport.zoom.100"), 1.0);
    assert_eq!(value(h.state(), FIT), 0.0);
    assert_eq!(zoom(&mut h, "viewport.zoom.in"), 2.0);
    assert_eq!(zoom(&mut h, "viewport.zoom.out"), 1.0);
    assert_eq!(zoom(&mut h, "viewport.zoom.out"), 2.0 / 3.0);
    run(&mut h, "viewport.zoom.fit", json!(null));
    h.step();
    assert_eq!(value(h.state(), FIT), 1.0);
    // Where one looks is not an edit.
    assert!(h.state().values.lock().unwrap().undo_steps().is_empty());
}

fn press(h: &mut Harness<'_, Shell>, pos: Pos2, pressed: bool) {
    h.input_mut().events.push(Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

#[test]
fn a_drag_pans_as_one_gesture_holding_the_view_and_is_no_undo_step() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    run(&mut h, "viewport.zoom.100", json!(null));
    settled(&mut h);
    let x0 = value(h.state(), CENTER_X);
    let committed = h.state().values.lock().unwrap().committed();
    let c = h.state().target_rect("viewport.display").unwrap().center();
    h.hover_at(c);
    press(&mut h, c, true);
    h.step();
    for i in 1..=4 {
        h.hover_at(c + vec2(10.0 * i as f32, 0.0));
        h.step();
    }
    // The drag holds the view: an agent's write is refused meanwhile.
    let refused = h.state_mut().run(
        "param.set",
        json!({ "path": CENTER_X, "value": 10.0 }),
        Origin::Agent("a1".into()),
    );
    assert!(refused.is_err(), "{refused:?}");
    press(&mut h, c + vec2(40.0, 0.0), false);
    h.step();
    h.step();
    // The image followed the pointer: 40 points right is 40 image pixels
    // at 100 % on a 1x window.
    let ppp = f64::from(h.state().pixels_per_point());
    assert_eq!(value(h.state(), CENTER_X), x0 - 40.0 * ppp);
    let store = h.state().values.lock().unwrap();
    assert!(store.undo_steps().is_empty());
    assert_eq!(store.committed(), committed);
    assert!(store.open_gesture().is_none());
}

#[test]
fn before_and_after_side_by_side() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    settled(&mut h);
    let plain = headless::snapshot(&mut h, "viewport.display").unwrap();
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": "node/exposure/exposure", "value": 2.0 }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    let before = tiles(h.state());
    until(&mut h, |s| tiles(s) > before);
    let edited = headless::snapshot(&mut h, "viewport.display").unwrap();
    run(&mut h, "viewport.compare", json!(null));
    assert_eq!(value(h.state(), COMPARE), 0.0, "applied at the next frame");
    h.step();
    assert_eq!(value(h.state(), COMPARE), 1.0);
    settled(&mut h);
    let split = headless::snapshot(&mut h, "viewport.display").unwrap();
    let r = h.state().target_rect("viewport.display").unwrap();
    // "Before" left of the split, in the middle; "after" right of it.
    let left = r.center() - vec2(r.width() / 4.0, 0.0);
    let right = r.center() + vec2(r.width() / 4.0, 0.0);
    let px = |shot, p| at(shot, &h, p);
    assert_close(px(&split, left), rgb(px(&plain, left)), "before");
    assert_close(px(&split, right), rgb(px(&edited, right)), "after");
    assert!(px(&edited, left)[0] > px(&plain, left)[0] + 20);
}

fn rgb(p: [u8; 4]) -> [u8; 3] {
    [p[0], p[1], p[2]]
}

#[test]
fn the_readout_gives_the_working_space_values_under_the_pointer() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    run(&mut h, "viewport.zoom.100", json!(null));
    settled(&mut h);
    let c = h.state().target_rect("viewport.display").unwrap().center();
    h.hover_at(c);
    for _ in 0..10 {
        h.step();
        h.state().viewport_stats().wait_idle(Duration::from_mins(1));
        if h.state().viewport().readout().is_some() {
            break;
        }
    }
    let r = h.state().viewport().readout().cloned().expect("a readout");
    let want = ramp(r.pixel[0] as f32, r.pixel[1] as f32);
    for (i, (name, v)) in r.values.iter().enumerate() {
        assert_eq!(name, ["R", "G", "B"][i]);
        assert!((v - want[i]).abs() < 1e-5, "{r:?} vs {want:?}");
    }
}

#[test]
fn undo_through_the_host_skips_the_view_and_takes_back_the_edit() {
    use egui::{Key, Modifiers};
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    settled(&mut h);
    run(
        &mut h,
        "param.set",
        json!({ "path": "node/exposure/exposure", "value": 1.0 }),
    );
    run(&mut h, "viewport.zoom.100", json!(null));
    h.step();
    run(&mut h, "viewport.zoom.in", json!(null));
    h.step();
    assert_eq!(value(h.state(), ZOOM), 2.0);
    // Edit > Undo names the edit, not the zoom.
    let handle = h.state().app().handle();
    let undo = handle.command(&egui_pop::api::app::CMD_UNDO).unwrap();
    assert_eq!(
        handle.command_title(&undo),
        "Undo set node/exposure/exposure"
    );
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.step();
    h.step();
    assert_eq!(value(h.state(), "node/exposure/exposure"), 0.0);
    // The view stays where it was.
    assert_eq!(value(h.state(), ZOOM), 2.0);
    assert_eq!(value(h.state(), FIT), 0.0);
}
