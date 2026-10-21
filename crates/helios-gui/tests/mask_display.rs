//! The mask display: the selected mask node, else the module's final
//! mask, shown over the image as a coloured overlay or alone in grey;
//! one transient value behind a command, `M` in the image and a button
//! of the Masks panel; on through edits and undo; rendered by the
//! viewport's engine, from its cache.

use std::time::Duration;

use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use helios_core::{Graph, Image, ModuleRef, NodeId, Params, Region, Value};
use helios_gui::mask_display::{COMMAND, COMMAND_OFF, MODE, Mode};
use helios_gui::{Shell, headless, helios_panels, mask_tree};
use helios_shell::commands::Origin;
use helios_values::{Change, Set};
use serde_json::json;

const W: u32 = 1200;
const H: u32 = 900;

fn module(g: &mut Graph, name: &str, m: ModuleRef, p: &Params) -> NodeId {
    g.add_module(name, m, p).unwrap()
}

/// An exposure through a mask: the group `mask` folds a nested group
/// `inner` (a circle `dot`), a circle `spot` and an ellipse `oval`, all on
/// the row y = 200.
fn graph() -> (Graph, NodeId) {
    use helios_module_masks::{MaskBlend, MaskCircle, MaskCombine, MaskEllipse};
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(W, H), 4, |x, _, c| match c {
            0 => 0.1 + 0.4 * x as f32 / W as f32,
            1 => 0.2,
            2 => 0.3,
            _ => 1.0,
        }),
    );
    let exp = module(
        &mut g,
        "exposure",
        ModuleRef::new(helios_module_exposure::Exposure),
        &Params::new().with("exposure", Value::Float(1.0)),
    );
    g.connect(src, "image", exp, "input").unwrap();
    let mask = module(&mut g, "mask", ModuleRef::new(MaskCombine), &Params::new());
    let inner = module(&mut g, "inner", ModuleRef::new(MaskCombine), &Params::new());
    let at = |x: f64| {
        Params::new()
            .with("x", Value::Float(x))
            .with("y", Value::Float(200.0))
    };
    let circle = |x: f64| at(x).with("radius", Value::Float(60.0));
    let dot = module(&mut g, "dot", ModuleRef::new(MaskCircle), &circle(150.0));
    let spot = module(&mut g, "spot", ModuleRef::new(MaskCircle), &circle(400.0));
    let oval = module(
        &mut g,
        "oval",
        ModuleRef::new(MaskEllipse),
        &at(700.0)
            .with("radius_x", Value::Float(80.0))
            .with("radius_y", Value::Float(40.0)),
    );
    g.connect(dot, "mask", inner, "in0").unwrap();
    for (i, n) in [inner, spot, oval].into_iter().enumerate() {
        g.connect(n, "mask", mask, &format!("in{i}")).unwrap();
    }
    let blend = module(&mut g, "blend", ModuleRef::new(MaskBlend), &Params::new());
    g.connect(src, "image", blend, "input").unwrap();
    g.connect(exp, "output", blend, "processed").unwrap();
    g.connect(mask, "mask", blend, "mask").unwrap();
    (g, blend)
}

fn harness_of(g: Graph, shown: NodeId, port: &str) -> Harness<'static, Shell> {
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, shown, port).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.step();
    h
}

fn harness() -> Harness<'static, Shell> {
    let (g, blend) = graph();
    harness_of(g, blend, "output")
}

/// Runs a command as a person would and answers its result: the
/// shell's commands are queued and run at the next frame, their result
/// in the history.
fn run(h: &mut Harness<'_, Shell>, command: &str, args: serde_json::Value) -> serde_json::Value {
    h.state_mut().run(command, args, Origin::User).unwrap();
    for _ in 0..3 {
        h.step();
    }
    let run = h
        .state()
        .history()
        .into_iter()
        .rev()
        .find(|r| r.id == command)
        .unwrap_or_else(|| panic!("{command} never ran"));
    run.result.unwrap_or_else(|e| panic!("{command}: {e}"))
}

fn mode(h: &Harness<'_, Shell>) -> Mode {
    helios_gui::mask_display::mode(&h.state().values.lock().unwrap())
}

/// The row the viewport shows over the image, and its node's name.
fn shown(h: &Harness<'_, Shell>) -> Option<(String, String)> {
    let v = h.state().viewport();
    let layer = v.mask_layer()?;
    let name = v.graph()?.name(layer.target.node).ok()?.to_string();
    Some((layer.target.key.clone(), name))
}

fn put(h: &mut Harness<'_, Shell>, path: &str, value: Value) {
    h.state()
        .values
        .lock()
        .unwrap()
        .set(Set::new(path, Change::To(value), Origin::User))
        .unwrap();
    for _ in 0..3 {
        h.step();
    }
}

/// Steps frames until every tile wanted is on screen, waiting after each
/// frame for the render threads to answer what it asked.
fn settled(h: &mut Harness<'_, Shell>) {
    for _ in 0..40 {
        h.step();
        let stats = h.state().viewport_stats();
        // Generous: only a hung render thread reaches it.
        assert!(
            stats.wait_idle(Duration::from_mins(1)),
            "the render thread never answered: {stats:?}"
        );
        h.step();
        if h.state().viewport().state().tiles_complete() {
            return;
        }
    }
    panic!("the viewport never settled");
}

/// The colour on screen at an image pixel.
fn on_screen(h: &mut Harness<'_, Shell>, x: f64, y: f64) -> [u8; 3] {
    let shot = headless::snapshot(h, "viewport.display").unwrap();
    let r = h.state().target_rect("viewport.display").unwrap();
    let p = h
        .state()
        .viewport()
        .state()
        .mapping()
        .unwrap()
        .image_to_screen([x, y]);
    let ppp = h.state().pixels_per_point();
    let px = shot
        .get_pixel(
            ((p.x - r.min.x) * ppp) as u32,
            ((p.y - r.min.y) * ppp) as u32,
        )
        .0;
    [px[0], px[1], px[2]]
}

fn chroma(c: [u8; 3]) -> u8 {
    c.iter().max().unwrap() - c.iter().min().unwrap()
}

fn click(h: &mut Harness<'_, Shell>, at: Pos2) {
    h.hover_at(at);
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    for _ in 0..3 {
        h.step();
    }
}

#[test]
fn nothing_selected_shows_the_module_s_final_mask() {
    let mut h = harness();
    assert_eq!(mode(&h), Mode::Off);
    assert_eq!(shown(&h), None);
    let out = run(&mut h, COMMAND, json!(null));
    assert_eq!(out, json!({ "mode": "overlay", "mask": "mask" }));
    // The root: what the blend reads.
    assert_eq!(shown(&h), Some(("mask".into(), "mask".into())));
}

#[test]
fn a_selected_member_or_group_is_shown_alone() {
    let mut h = harness();
    run(&mut h, COMMAND, json!(null));
    run(&mut h, "mask.select", json!({ "shape": "spot" }));
    assert_eq!(shown(&h), Some(("spot".into(), "spot".into())));
    // A nested group: its own output, as its parent reads it.
    put(
        &mut h,
        helios_gui::params::SELECTION,
        Value::Text("inner".into()),
    );
    run(&mut h, "mask.select", json!({ "shape": "" }));
    assert_eq!(shown(&h), Some(("inner".into(), "inner".into())));
}

#[test]
fn the_command_cycles_and_sets_and_is_never_an_undo_step() {
    let mut h = harness();
    let modes: Vec<serde_json::Value> = (0..3)
        .map(|_| run(&mut h, COMMAND, json!(null))["mode"].clone())
        .collect();
    assert_eq!(modes, [json!("overlay"), json!("grey"), json!("off")]);
    assert_eq!(shown(&h), None);
    run(&mut h, COMMAND, json!({ "mode": "grey" }));
    assert_eq!(mode(&h), Mode::Grey);
    assert!(shown(&h).is_some());
    run(&mut h, COMMAND_OFF, json!(null));
    assert_eq!(mode(&h), Mode::Off);
    assert!(
        h.state_mut()
            .run(COMMAND, json!({ "mode": "loud" }), Origin::User)
            .is_err()
    );
    let store = h.state().values.lock().unwrap();
    assert!(store.undo_steps().is_empty(), "{:?}", store.undo_steps());
    assert!(store.get(MODE).unwrap().descriptor.transient);
}

#[test]
fn m_with_the_image_focused_cycles_the_display() {
    let mut h = harness();
    run(&mut h, "helios.viewport.focus", json!(null));
    h.key_press(Key::M);
    h.step();
    h.step();
    assert_eq!(mode(&h), Mode::Overlay);
    h.key_press(Key::M);
    h.step();
    h.step();
    assert_eq!(mode(&h), Mode::Grey);
    egui_pop_test::assert_keymap_clean(h.state().app());
}

#[test]
fn the_masks_panel_button_cycles_the_display() {
    let mut h = harness();
    run(&mut h, "helios.masks.focus", json!(null));
    let panel = h.state().target_rect(mask_tree::PANEL).unwrap();
    let button = h
        .query_all_by_label("Show mask")
        .map(|n| n.rect())
        .find(|r| panel.contains(r.center()))
        .expect("the Masks panel's Show mask button");
    click(&mut h, button.center());
    assert_eq!(mode(&h), Mode::Overlay);
    assert_eq!(shown(&h), Some(("mask".into(), "mask".into())));
}

#[test]
fn the_overlay_colours_the_mask_and_grey_shows_it_alone() {
    let mut h = harness();
    run(&mut h, COMMAND, json!({ "mode": "overlay" }));
    settled(&mut h);
    // Inside the spot, the theme's colour; far from every shape, the
    // image turned grey.
    let inside = on_screen(&mut h, 400.0, 200.0);
    let outside = on_screen(&mut h, 1000.0, 700.0);
    assert!(chroma(inside) > 30, "{inside:?}");
    assert!(chroma(outside) <= 2, "{outside:?}");
    run(&mut h, COMMAND, json!({ "mode": "grey" }));
    settled(&mut h);
    let inside = on_screen(&mut h, 400.0, 200.0);
    let outside = on_screen(&mut h, 1000.0, 700.0);
    assert!(inside.iter().all(|&c| c >= 250), "{inside:?}");
    assert!(outside.iter().all(|&c| c <= 5), "{outside:?}");
}

#[test]
fn it_stays_on_through_a_drag_a_parameter_change_and_undo() {
    let mut h = harness();
    run(&mut h, COMMAND, json!({ "mode": "grey" }));
    run(&mut h, "mask.select", json!({ "shape": "spot" }));
    settled(&mut h);
    // Off the centre: the selected spot's handles are drawn there.
    assert!(on_screen(&mut h, 385.0, 185.0).iter().all(|&c| c >= 250));
    // A handle dragged by a command: the spot moves, the display follows.
    run(
        &mut h,
        "mask.handle.move",
        json!({ "shape": "spot", "handle": "centre", "x": 400.0, "y": 600.0 }),
    );
    settled(&mut h);
    assert_eq!(mode(&h), Mode::Grey);
    assert_eq!(shown(&h), Some(("spot".into(), "spot".into())));
    assert!(on_screen(&mut h, 385.0, 585.0).iter().all(|&c| c >= 250));
    assert!(on_screen(&mut h, 385.0, 185.0).iter().all(|&c| c <= 5));
    // A parameter changed in the store.
    put(&mut h, "node/spot/radius", Value::Float(10.0));
    settled(&mut h);
    assert!(on_screen(&mut h, 440.0, 600.0).iter().all(|&c| c <= 5));
    // Undo: the radius back, still shown.
    h.state()
        .values
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    settled(&mut h);
    assert_eq!(mode(&h), Mode::Grey);
    assert!(on_screen(&mut h, 440.0, 600.0).iter().all(|&c| c >= 250));
}

#[test]
fn the_mask_comes_from_the_viewport_s_engine_and_its_cache() {
    let mut h = harness();
    settled(&mut h);
    let engine = h.state().viewport().engine().clone();
    let before = engine.counters();
    // On: the viewport's engine is asked for the mask's tiles too.
    run(&mut h, COMMAND, json!({ "mode": "grey" }));
    settled(&mut h);
    let after = engine.counters();
    assert!(after.requests > before.requests, "{before:?} {after:?}");
    // What the render thread does for a union: the image, then the mask
    // over the same region and scale, which the image's render left in
    // the cache (the blend read it): nothing more is computed.
    let shown = h.state().viewport().shown().unwrap();
    let target = h.state().viewport().mask_layer().unwrap().target.clone();
    let region = Region::new(64, 32, 512, 256);
    engine
        .render_at(&shown.graph, shown.node, &shown.port, region, 0.5)
        .unwrap();
    let image = engine.counters();
    let mask = engine
        .render_at(&shown.graph, target.node, &target.port, region, 0.5)
        .unwrap();
    assert_eq!(mask.channels(), 1);
    assert_eq!(
        engine.counters().computed,
        image.computed,
        "the mask came from the cache"
    );
}

#[test]
fn a_module_without_a_mask_shows_nothing_and_the_mode_stays() {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(W, H), 4, |_, _, _| 0.2),
    );
    let exp = module(
        &mut g,
        "exposure",
        ModuleRef::new(helios_module_exposure::Exposure),
        &Params::new(),
    );
    g.connect(src, "image", exp, "input").unwrap();
    let mut h = harness_of(g, exp, "output");
    let out = run(&mut h, COMMAND, json!(null));
    assert_eq!(out, json!({ "mode": "overlay", "mask": null }));
    assert_eq!(shown(&h), None);
    assert_eq!(mode(&h), Mode::Overlay);
}
