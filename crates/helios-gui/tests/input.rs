//! Scenarios driven by raw input events (pointer, wheel, keys, as a
//! windowing system sends them), never by widget calls, checking the
//! interface's grammar (docs/gui.md) where panels meet:
//!
//! - Helios starts on Photoshop's pointer profile: the wheel over a slider
//!   scrolls the panel and the slider takes it only once focused (a
//!   click); on darktable's profile, once chosen, the wheel over the
//!   image zooms and over a slider changes the value (Ctrl+Alt: the panel
//!   scrolls), and with the wheel over sliders guarded (a setting) a
//!   slider takes it only with the focus or Alt held;
//! - a drag belongs to where it started, wherever it ends;
//! - a double click resets a slider, never resets on the canvas, and
//!   fits the image when the setting says so (darktable's leaves the
//!   darkroom, `tests/contexts.rs`);
//! - a held key repeats a step, never a command, and a held arrow is one
//!   undo step;
//! - nothing but the user moves the keyboard focus, and a field typed in
//!   lets go with Escape;
//! - the first run offers darktable's profile once, without blocking the
//!   window.

use egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, TouchPhase, Vec2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};
use helios_gui::masks::SHAPE;
use helios_gui::viewport::{CENTER_X, CENTER_Y, FIT, ZOOM};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

const EV: &str = "node/exposure/exposure";
const W: u32 = 1200;
const H: u32 = 900;

/// A ramp, an exposure through a mask with no shape yet: the viewport
/// and a Parameters panel long enough to scroll in a short window.
fn shell() -> Shell {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(W, H), 4, |x, y, c| match c {
            0..=2 => 0.05 + 0.3 * (x + y) as f32 / (W + H) as f32,
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
    let mask = g
        .add_module(
            "mask",
            ModuleRef::new(helios_module_masks::MaskCombine),
            &Params::new(),
        )
        .unwrap();
    let blend = g
        .add_module(
            "blend",
            ModuleRef::new(helios_module_masks::MaskBlend),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", blend, "input").unwrap();
    g.connect(exp, "output", blend, "processed").unwrap();
    g.connect(mask, "mask", blend, "mask").unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, blend, "output").unwrap();
    shell
}

/// The window, its Parameters panel shown: short enough that the panel
/// scrolls.
fn harness() -> Harness<'static, Shell> {
    let mut h = headless::harness(shell(), vec2(1280.0, 480.0));
    h.step();
    h.state_mut()
        .run("helios.params.focus", json!(null), Origin::User)
        .unwrap();
    settle(&mut h);
    h
}

/// A few frames, the clock moving a quarter second each: smooth scrolling
/// and queued commands are done after it.
fn settle(h: &mut Harness<'_, Shell>) {
    for _ in 0..8 {
        h.step();
    }
}

fn f(h: &Harness<'_, Shell>, path: &str) -> f64 {
    let store = h.state().values.lock().unwrap();
    let v = &store.get(path).unwrap().value;
    v.as_f64().or_else(|| v.as_bool().map(f64::from)).unwrap()
}

fn text(h: &Harness<'_, Shell>, path: &str) -> String {
    match &h.state().values.lock().unwrap().get(path).unwrap().value {
        Value::Text(t) | Value::Choice(t) => t.clone(),
        v => panic!("{path}: {v:?}"),
    }
}

fn steps(h: &Harness<'_, Shell>) -> Vec<String> {
    let store = h.state().values.lock().unwrap();
    store.undo_steps().iter().map(|s| s.label.clone()).collect()
}

fn view(h: &Harness<'_, Shell>) -> [f64; 3] {
    [f(h, ZOOM), f(h, CENTER_X), f(h, CENTER_Y)]
}

/// The zoom on screen (the stored one is stale while fitted).
fn shown_zoom(h: &Harness<'_, Shell>) -> f64 {
    h.state().viewport().state().mapping().unwrap().zoom
}

fn rect(h: &Harness<'_, Shell>, target: &str) -> egui::Rect {
    h.state().target_rect(target).unwrap()
}

fn slider(h: &Harness<'_, Shell>, path: &str) -> egui::Rect {
    rect(h, &format!("param:{path}"))
}

/// The image's centre on screen.
fn image(h: &Harness<'_, Shell>) -> Pos2 {
    rect(h, "viewport.display").center()
}

/// One of egui-pop's settings, a choice, written as the user would in
/// the settings window (`input.slider_wheel`...).
fn setting(h: &mut Harness<'_, Shell>, id: &str, choice: &str) {
    use egui_pop::model::values::{Change, Set, Value as PopValue};
    let store = h.state().app().handle().store();
    store
        .lock()
        .unwrap()
        .set(
            Set::new(
                &format!("settings/{id}"),
                Change::To(PopValue::Choice(choice.into())),
                egui_pop::model::commands::Origin::User,
            )
            .untracked(),
        )
        .unwrap();
    settle(h);
}

/// The pointer profile, chosen as the user would in the settings window
/// (`darktable`...).
fn profile(h: &mut Harness<'_, Shell>, id: &str) {
    egui_pop::api::pointer::set_profile(h.state().app().handle(), id).unwrap();
    settle(h);
    assert_eq!(
        egui_pop::api::pointer::profile_id(h.state().app().handle()),
        id
    );
}

fn modifiers(h: &mut Harness<'_, Shell>, m: Modifiers) {
    h.input_mut().events.push(Event::ModifiersChanged(m));
}

fn button(h: &mut Harness<'_, Shell>, b: PointerButton, at: Pos2, pressed: bool, m: Modifiers) {
    h.input_mut().events.push(Event::PointerButton {
        pos: at,
        button: b,
        pressed,
        modifiers: m,
    });
}

fn click(h: &mut Harness<'_, Shell>, at: Pos2) {
    h.hover_at(at);
    h.step();
    button(h, PointerButton::Primary, at, true, Modifiers::NONE);
    button(h, PointerButton::Primary, at, false, Modifiers::NONE);
    h.step();
    h.step();
}

/// Wheel notches (lines) or a touchpad's stream (points) at `at`.
fn wheel(h: &mut Harness<'_, Shell>, at: Pos2, unit: MouseWheelUnit, delta: Vec2, m: Modifiers) {
    modifiers(h, m);
    h.hover_at(at);
    h.step();
    modifiers(h, m);
    h.input_mut().events.push(Event::MouseWheel {
        unit,
        delta,
        phase: TouchPhase::Move,
        modifiers: m,
    });
    h.step();
    for _ in 0..6 {
        modifiers(h, m);
        h.step();
    }
    modifiers(h, Modifiers::NONE);
    h.step();
}

/// A key pressed, held for `repeats` repeats, released.
fn hold(h: &mut Harness<'_, Shell>, key: Key, repeats: usize) {
    let event = |pressed, repeat| Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat,
        modifiers: Modifiers::NONE,
    };
    h.input_mut().events.push(event(true, false));
    h.step();
    for _ in 0..repeats {
        h.input_mut().events.push(event(true, true));
        h.step();
    }
    h.input_mut().events.push(event(false, false));
    h.step();
    h.step();
}

/// Two clicks `gap` seconds apart at `a` then `b` (the harness's clock
/// moves only when told).
fn two_clicks(h: &mut Harness<'_, Shell>, a: Pos2, b: Pos2, gap: f64) {
    h.hover_at(a);
    h.step();
    let mut t = h.ctx.input(|i| i.time) + 1.0;
    for at in [a, b] {
        h.input_mut().events.push(Event::PointerMoved(at));
        for down in [true, false] {
            t += gap / 2.0;
            h.input_mut().time = Some(t);
            button(h, PointerButton::Primary, at, down, Modifiers::NONE);
            h.step();
        }
    }
    settle(h);
}

// --- The wheel ---

#[test]
fn darktable_s_wheel_over_a_slider_changes_it_and_ctrl_alt_leaves_it() {
    let mut h = harness();
    profile(&mut h, "darktable");
    let ev = slider(&h, EV);
    // A notch up over the slider, without the focus: a step up, the
    // panel still.
    wheel(
        &mut h,
        ev.center(),
        MouseWheelUnit::Line,
        vec2(0.0, 1.0),
        Modifiers::NONE,
    );
    assert!(f(&h, EV) > 0.0, "{}", f(&h, EV));
    assert_eq!(slider(&h, EV), ev, "the panel scrolled");
    // Ctrl+Alt and the wheel leave it to the panel: the value stays.
    // (Ctrl as egui-winit reports it off macOS: `command` with `ctrl`,
    // which the profile's `Ctrl` matches.)
    let up = f(&h, EV);
    wheel(
        &mut h,
        ev.center(),
        MouseWheelUnit::Line,
        vec2(0.0, -2.0),
        Modifiers::COMMAND | Modifiers::ALT,
    );
    assert_eq!(f(&h, EV), up, "Ctrl+Alt and the wheel changed the value");
}

/// Helios's default profile (Photoshop's), nothing saved: scrolling past
/// sliders never changes a value.
#[test]
fn the_wheel_over_a_slider_scrolls_the_panel_by_default_and_never_changes_the_value() {
    let mut h = harness();
    assert_eq!(
        egui_pop::api::pointer::profile_id(h.state().app().handle()),
        helios_gui::pointer_choice::DEFAULT_PROFILE
    );
    let ev = slider(&h, EV);
    let before = (f(&h, EV), view(&h));
    // Notches, then a touchpad's stream of small pixel deltas.
    wheel(
        &mut h,
        ev.center(),
        MouseWheelUnit::Line,
        vec2(0.0, -2.0),
        Modifiers::NONE,
    );
    wheel(
        &mut h,
        ev.center(),
        MouseWheelUnit::Point,
        vec2(0.0, -30.0),
        Modifiers::NONE,
    );
    // Shift (finer for a focused slider) still scrolls.
    let ev2 = slider(&h, EV);
    wheel(
        &mut h,
        ev2.center(),
        MouseWheelUnit::Line,
        vec2(0.0, -1.0),
        Modifiers::SHIFT,
    );
    assert_eq!((f(&h, EV), view(&h)), before, "the wheel changed a value");
    assert!(
        slider(&h, EV).top() < ev.top() - 10.0,
        "the panel did not scroll: {ev:?} → {:?}",
        slider(&h, EV)
    );
    assert!(steps(&h).is_empty(), "{:?}", steps(&h));
}

#[test]
fn a_focused_slider_takes_the_wheel_by_default_and_the_panel_stays() {
    let mut h = harness();
    let ev = slider(&h, EV);
    // Focused by a click: the plain wheel steps it, the panel still.
    click(&mut h, ev.center());
    let before = f(&h, EV);
    wheel(
        &mut h,
        ev.center(),
        MouseWheelUnit::Line,
        vec2(0.0, 1.0),
        Modifiers::NONE,
    );
    assert!(f(&h, EV) > before, "{} after {before}", f(&h, EV));
    assert_eq!(slider(&h, EV), ev, "the panel scrolled");
}

#[test]
fn darktable_s_guarded_wheel_leaves_a_slider_to_alt_or_the_focus() {
    let mut h = harness();
    profile(&mut h, "darktable");
    setting(&mut h, "input.slider_wheel", "guarded");
    let ev = slider(&h, EV);
    // The plain wheel without the focus: the value stays.
    wheel(
        &mut h,
        ev.center(),
        MouseWheelUnit::Line,
        vec2(0.0, 1.0),
        Modifiers::NONE,
    );
    assert_eq!(f(&h, EV), 0.0, "the guarded wheel changed the value");
    let ev = slider(&h, EV);
    // Alt held over it, without the focus: a step up, the panel still.
    wheel(
        &mut h,
        ev.center(),
        MouseWheelUnit::Line,
        vec2(0.0, 1.0),
        Modifiers::ALT,
    );
    assert!(f(&h, EV) > 0.0, "{}", f(&h, EV));
    assert_eq!(slider(&h, EV), ev, "the panel scrolled");
    // Focused by a click: the plain wheel steps it back down.
    let up = f(&h, EV);
    click(&mut h, ev.center());
    wheel(
        &mut h,
        ev.center(),
        MouseWheelUnit::Line,
        vec2(0.0, -1.0),
        Modifiers::NONE,
    );
    assert!(f(&h, EV) < up, "{} after {up}", f(&h, EV));
    assert_eq!(slider(&h, EV), ev, "the panel scrolled");
}

#[test]
fn darktable_s_wheel_over_the_image_zooms_and_leaves_the_panels() {
    let mut h = harness();
    profile(&mut h, "darktable");
    let ev = slider(&h, EV);
    let at = image(&h) + vec2(-100.0, 0.0);
    let zoom = shown_zoom(&h);
    wheel(
        &mut h,
        at,
        MouseWheelUnit::Line,
        vec2(0.0, 1.0),
        Modifiers::NONE,
    );
    assert!(shown_zoom(&h) > zoom, "{} after {zoom}", shown_zoom(&h));
    assert_eq!(f(&h, FIT), 0.0);
    // Shift and the wheel pan, at the same zoom.
    let (zoom, cy) = (f(&h, ZOOM), f(&h, CENTER_Y));
    let cx = f(&h, CENTER_X);
    wheel(
        &mut h,
        at,
        MouseWheelUnit::Line,
        vec2(0.0, 1.0),
        Modifiers::SHIFT,
    );
    assert_eq!(f(&h, ZOOM), zoom);
    assert!((f(&h, CENTER_X), f(&h, CENTER_Y)) != (cx, cy), "no pan");
    // The panels did not move, no value changed, nothing to undo.
    assert_eq!(slider(&h, EV), ev);
    assert_eq!(f(&h, EV), 0.0);
    assert!(steps(&h).is_empty(), "{:?}", steps(&h));
}

// --- Drags across panels ---

#[test]
fn a_slider_drag_that_ends_over_the_image_is_the_slider_s_alone() {
    let mut h = harness();
    let ev = slider(&h, EV);
    let v0 = view(&h);
    let from = ev.center();
    let to = image(&h);
    h.hover_at(from);
    h.step();
    button(&mut h, PointerButton::Primary, from, true, Modifiers::NONE);
    h.step();
    for i in 1..=8 {
        h.hover_at(from + (to - from) * (i as f32 / 8.0));
        h.step();
    }
    button(&mut h, PointerButton::Primary, to, false, Modifiers::NONE);
    settle(&mut h);
    assert!(f(&h, EV) != 0.0, "the drag did nothing");
    assert_eq!(view(&h), v0, "the image moved");
    assert_eq!(steps(&h), ["drag exposure"]);
    assert_eq!(text(&h, SHAPE), "", "a click on the image selected");
}

#[test]
fn a_pan_that_ends_over_the_parameters_moves_only_the_view() {
    let mut h = harness();
    // Zoomed in, so a pan moves.
    h.state_mut()
        .run("viewport.zoom.100", json!(null), Origin::User)
        .unwrap();
    settle(&mut h);
    let ev = slider(&h, EV);
    let (cx, cy) = (f(&h, CENTER_X), f(&h, CENTER_Y));
    let from = image(&h);
    let to = ev.center();
    h.hover_at(from);
    h.step();
    // Photoshop's drag pans, as its Hand tool does.
    button(&mut h, PointerButton::Primary, from, true, Modifiers::NONE);
    h.step();
    for i in 1..=8 {
        h.hover_at(from + (to - from) * (i as f32 / 8.0));
        h.step();
    }
    // A wheel notch over the slider during the pan: the slider is left.
    h.input_mut().events.push(Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: vec2(0.0, 1.0),
        phase: TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    h.step();
    button(&mut h, PointerButton::Primary, to, false, Modifiers::NONE);
    settle(&mut h);
    assert!((f(&h, CENTER_X), f(&h, CENTER_Y)) != (cx, cy), "no pan");
    assert_eq!(f(&h, EV), 0.0, "the pan changed a parameter");
    assert!(steps(&h).is_empty(), "{:?}", steps(&h));
}

// --- Double clicks ---

#[test]
fn a_double_click_on_the_image_set_to_fit_fits_and_resets_nothing() {
    let mut h = harness();
    // darktable's profile, whose plain click on the image does nothing:
    // Lightroom Classic's toggles fit and 100 % on a click.
    profile(&mut h, "darktable");
    setting(&mut h, "input.image_double_click", "fit");
    let ev = slider(&h, EV);
    // A value changed, a shape selected, the view zoomed.
    two_clicks(&mut h, ev.center(), ev.center(), 0.1);
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": EV, "value": 1.5 }),
            Origin::User,
        )
        .unwrap();
    h.state_mut()
        .run("mask.add.circle", json!(null), Origin::User)
        .unwrap();
    h.state_mut()
        .run("viewport.zoom.100", json!(null), Origin::User)
        .unwrap();
    settle(&mut h);
    let radius = f(&h, "node/circle/radius");
    let steps0 = steps(&h);
    // Away from the circle's handles and edge.
    let at = rect(&h, "viewport.display").left_top() + vec2(30.0, 30.0);
    two_clicks(&mut h, at, at, 0.1);
    assert_eq!(f(&h, FIT), 1.0, "the double click did not fit");
    assert_eq!(f(&h, EV), 1.5);
    assert_eq!(f(&h, "node/circle/radius"), radius);
    assert_eq!(steps(&h), steps0);
}

#[test]
fn a_double_click_resets_a_slider_but_not_across_panels() {
    let mut h = harness();
    let ev = slider(&h, EV);
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": EV, "value": 1.5 }),
            Origin::User,
        )
        .unwrap();
    settle(&mut h);
    // A click on the image then one on the slider, quick: no reset.
    let centre = image(&h);
    two_clicks(&mut h, centre, ev.center(), 0.1);
    assert_eq!(f(&h, EV), 1.5);
    // Two on the slider: reset.
    two_clicks(&mut h, ev.center(), ev.center(), 0.1);
    assert_eq!(f(&h, EV), 0.0);
}

// --- Key repeats ---

#[test]
fn a_held_key_runs_its_command_once() {
    let mut h = harness();
    // The image focused, `C` held: one circle.
    let corner = rect(&h, "viewport.display").left_top() + vec2(30.0, 30.0);
    click(&mut h, corner);
    hold(&mut h, Key::C, 10);
    settle(&mut h);
    let store = h.state().values.lock().unwrap();
    let circles = store
        .list("node/")
        .into_iter()
        .filter(|(p, _)| p.ends_with("/radius"))
        .count();
    drop(store);
    assert_eq!(circles, 1, "a held key ran its command again");
    assert_eq!(text(&h, SHAPE), "circle");
}

#[test]
fn a_held_arrow_steps_a_slider_as_one_undo_step() {
    let mut h = harness();
    let ev = slider(&h, EV);
    click(&mut h, ev.center());
    let step = {
        hold(&mut h, Key::ArrowRight, 0);
        f(&h, EV)
    };
    assert!(step > 0.0);
    settle(&mut h);
    let before = steps(&h).len();
    hold(&mut h, Key::ArrowRight, 9);
    assert!((f(&h, EV) - 11.0 * step).abs() < 1e-9, "{}", f(&h, EV));
    settle(&mut h);
    assert_eq!(steps(&h).len(), before + 1, "{:?}", steps(&h));
    // One undo takes the whole run back.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    settle(&mut h);
    assert!((f(&h, EV) - step).abs() < 1e-9, "{}", f(&h, EV));
}

// --- Focus ---

#[test]
fn typing_in_a_field_fires_no_shortcut_and_an_agent_does_not_take_the_focus() {
    let mut h = harness();
    let ev = slider(&h, EV);
    click(&mut h, ev.center());
    h.key_press(Key::Enter);
    settle(&mut h);
    let focused = h.ctx.memory(egui::Memory::focused);
    assert!(focused.is_some());
    // `c` and `p` are typed, not run.
    for (key, t) in [(Key::Num1, "1"), (Key::C, "c"), (Key::P, "p")] {
        h.key_press(key);
        h.input_mut().events.push(Event::Text(t.into()));
        h.step();
    }
    assert_eq!(f(&h, EV), 0.0, "nothing committed by typing");
    // An agent writes another parameter meanwhile.
    let other = {
        let store = h.state().values.lock().unwrap();
        store
            .list("node/")
            .into_iter()
            .map(|(p, _)| p.to_string())
            .find(|p| p != EV && matches!(store.get(p).unwrap().value, Value::Float(_)))
            .unwrap()
    };
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": other, "value": 0.5 }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    settle(&mut h);
    assert_eq!(
        h.ctx.memory(egui::Memory::focused),
        focused,
        "the focus moved"
    );
    assert_eq!(text(&h, SHAPE), "", "a typed letter ran a command");
    assert_eq!(f(&h, EV), 0.0, "nothing committed while typing");
    // Escape lets go of the field; the value is untouched.
    h.key_press(Key::Escape);
    settle(&mut h);
    assert_eq!(f(&h, EV), 0.0);
    // Then the image's keys work again once it has the focus.
    let corner = rect(&h, "viewport.display").left_top() + vec2(30.0, 30.0);
    click(&mut h, corner);
    h.key_press(Key::C);
    settle(&mut h);
    assert_eq!(text(&h, SHAPE), "circle");
}

#[test]
fn the_palette_lets_go_with_escape_and_gives_the_focus_back() {
    let mut h = harness();
    let corner = rect(&h, "viewport.display").left_top() + vec2(30.0, 30.0);
    click(&mut h, corner);
    let panel = h.state().focused();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
    settle(&mut h);
    assert!(h.state().palette_open());
    h.input_mut().events.push(Event::Text("c".into()));
    settle(&mut h);
    assert_eq!(text(&h, SHAPE), "", "typing in the palette ran a key");
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(!h.state().palette_open());
    assert_eq!(h.state().focused(), panel);
    h.key_press(Key::C);
    settle(&mut h);
    assert_eq!(text(&h, SHAPE), "circle");
}

#[test]
fn a_click_elsewhere_ends_typing_and_takes_the_focus() {
    let mut h = harness();
    let ev = slider(&h, EV);
    click(&mut h, ev.center());
    h.key_press(Key::Enter);
    settle(&mut h);
    h.input_mut().events.push(Event::Text("2".into()));
    settle(&mut h);
    assert!(h.ctx.egui_wants_keyboard_input());
    // A click on the image: the field lets go, what was typed is kept
    // (`+0.00` then `2`).
    let corner = rect(&h, "viewport.display").left_top() + vec2(30.0, 30.0);
    click(&mut h, corner);
    settle(&mut h);
    assert!((f(&h, EV) - 0.002).abs() < 1e-9, "{}", f(&h, EV));
    h.key_press(Key::C);
    settle(&mut h);
    assert_eq!(
        text(&h, SHAPE),
        "circle",
        "the image's key went to the field"
    );
}

// --- The first run ---

const ASKED: &str = "Coming from darktable?";

/// A desktop whose settings are kept in `dir`, offering the first run's
/// choice as the desktop does.
fn first_run(dir: &std::path::Path) -> Harness<'static, Shell> {
    let mut shell = Shell::builder(helios_panels(), Some(dir.to_path_buf())).build();
    shell.offer_pointer_choice();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    // The labels below are English, whatever the system's language.
    setting(&mut h, "appearance.language", "en");
    h
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-input-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn profile_id(h: &Harness<'_, Shell>) -> String {
    egui_pop::api::pointer::profile_id(h.state().app().handle())
}

#[test]
fn the_first_run_offers_darktable_s_profile_once() {
    let dir = temp("first-run");
    let mut h = first_run(&dir);
    assert_eq!(profile_id(&h), helios_gui::pointer_choice::DEFAULT_PROFILE);
    assert!(h.query_by_label(ASKED).is_some(), "the first run asks");
    assert!(
        h.state()
            .target_rect(helios_gui::pointer_choice::RECT)
            .is_ok()
    );
    // It blocks nothing: the palette opens and closes meanwhile.
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
    settle(&mut h);
    assert!(h.state().palette_open(), "the notice held the keys");
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(
        h.query_by_label(ASKED).is_some(),
        "Escape closed the notice"
    );
    h.get_by_label("Use darktable's").click();
    settle(&mut h);
    assert_eq!(profile_id(&h), "darktable");
    assert!(h.query_by_label(ASKED).is_none(), "answered, it goes");
    h.state_mut().exit().unwrap();
    drop(h);
    // The next run keeps the profile and does not ask again.
    let h = first_run(&dir);
    assert_eq!(profile_id(&h), "darktable");
    assert!(h.query_by_label(ASKED).is_none(), "asked again");
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn keeping_lightroom_s_habits_or_closing_the_notice_is_remembered() {
    for (button, name) in [
        ("Keep Photoshop-style", "keep"),
        ("Close the pointer profile notice", "close"),
    ] {
        let dir = temp(name);
        let mut h = first_run(&dir);
        h.get_by_label(button).click();
        settle(&mut h);
        assert_eq!(profile_id(&h), helios_gui::pointer_choice::DEFAULT_PROFILE);
        assert!(h.query_by_label(ASKED).is_none(), "{button}: still shown");
        h.state_mut().exit().unwrap();
        drop(h);
        let h = first_run(&dir);
        assert!(h.query_by_label(ASKED).is_none(), "{button}: asked again");
        drop(h);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn the_first_run_notice_shows_only_when_offered_and_no_profile_is_saved() {
    // Tests and tools: never offered.
    let h = harness();
    assert!(h.query_by_label(ASKED).is_none());
    // A profile the user saved: not asked.
    let dir = temp("saved");
    let mut h = first_run(&dir);
    profile(&mut h, "document");
    assert!(
        h.query_by_label(ASKED).is_none(),
        "a saved profile was asked about"
    );
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
