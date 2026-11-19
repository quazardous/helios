//! An edited photo shows it: a parameter moved marks itself, its
//! module's header and the photo; a double click puts it back and clears
//! the three; a module's Reset and "Reset all" are one undo step each.

use egui::{Key, Modifiers, PointerButton, vec2};
use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

const EV: &str = "node/exposure/exposure";
const BLACK: &str = "node/exposure/black";

fn harness() -> egui_kittest::Harness<'static, Shell> {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(64, 48), 4, |_, _, _| 0.2),
    );
    let exp = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    let shell = Shell::new(helios_panels(), None);
    helios_values::bind_node(&mut shell.values().lock().unwrap(), &g, exp).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.state_mut()
        .run("helios.params.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
    h
}

fn value(h: &egui_kittest::Harness<'_, Shell>, path: &str) -> f64 {
    h.state()
        .values()
        .lock()
        .unwrap()
        .get(path)
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

fn steps(h: &egui_kittest::Harness<'_, Shell>) -> usize {
    h.state().values().lock().unwrap().undo_steps().len()
}

fn shown(h: &egui_kittest::Harness<'_, Shell>, target: &str) -> bool {
    h.state().target_rect(target).is_ok()
}

fn press(h: &egui_kittest::Harness<'_, Shell>, at: egui::Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

fn click(h: &mut egui_kittest::Harness<'_, Shell>, target: &str) {
    let r = h.state().target_rect(target).unwrap();
    h.hover_at(r.center());
    h.step();
    press(h, r.center(), true);
    press(h, r.center(), false);
    h.run();
}

#[test]
fn a_moved_slider_marks_itself_its_module_and_the_photo_and_a_double_click_clears_them() {
    let mut h = harness();
    let before = h.state().app().handle().last_event();
    assert!(!h.state().edit_marks().edited());
    assert!(!shown(&h, &format!("param:{EV}:changed")));
    assert!(!shown(&h, "module:exposure:changed"));
    assert!(!shown(&h, "module:exposure:reset"));
    // Moved by the keys, as the user.
    click(&mut h, &format!("param:{EV}"));
    h.key_press(Key::ArrowRight);
    h.run();
    assert!(value(&h, EV) > 0.0);
    assert!(shown(&h, &format!("param:{EV}:changed")));
    assert!(shown(&h, "module:exposure:changed"));
    assert!(shown(&h, "module:exposure:reset"));
    assert!(h.state().edit_marks().edited());
    // Said once on the journal, with the listener that ran.
    let said: Vec<_> = h
        .state()
        .app()
        .handle()
        .events_since(before)
        .into_iter()
        .filter(|e| e.name == "helios.edit.changed")
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert_eq!(said[0].data["edited"], true);
    assert_eq!(said[0].data["nodes"], json!(["exposure"]));
    assert_eq!(said[0].data["ran"], json!(["helios.edit.marks"]));
    // A double click resets it, and the marks go.
    let r = h.state().target_rect(&format!("param:{EV}")).unwrap();
    h.hover_at(r.center());
    h.step();
    let mut t = 100.0;
    for _ in 0..2 {
        for down in [true, false] {
            t += 0.05;
            h.input_mut().time = Some(t);
            press(&h, r.center(), down);
            h.step();
        }
    }
    h.run();
    assert_eq!(value(&h, EV), 0.0);
    assert!(!shown(&h, &format!("param:{EV}:changed")));
    assert!(!shown(&h, "module:exposure:changed"));
    assert!(!h.state().edit_marks().edited());
}

#[test]
fn a_module_reset_is_one_undo_step() {
    let mut h = harness();
    for (path, v) in [(EV, 1.0), (BLACK, 0.01)] {
        h.state_mut()
            .run(
                "param.set",
                json!({ "path": path, "value": v }),
                Origin::User,
            )
            .unwrap();
    }
    h.run();
    let before = steps(&h);
    click(&mut h, "module:exposure:reset");
    assert_eq!((value(&h, EV), value(&h, BLACK)), (0.0, 0.0));
    assert_eq!(steps(&h), before + 1);
    assert!(!shown(&h, "module:exposure:changed"));
    // One undo brings both back, and the marks with them.
    h.state_mut()
        .run("param.undo", json!(null), Origin::User)
        .unwrap();
    h.run();
    assert_eq!((value(&h, EV), value(&h, BLACK)), (1.0, 0.01));
    assert!(shown(&h, "module:exposure:changed"));
}

#[test]
fn reset_all_is_one_undo_step_and_brings_back_the_bypass() {
    let mut h = harness();
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": EV, "value": 1.0 }),
            Origin::User,
        )
        .unwrap();
    h.state_mut()
        .run(
            "node.bypass",
            json!({ "node": "exposure", "on": true }),
            Origin::User,
        )
        .unwrap();
    h.run();
    assert!(h.state().edit_marks().bypassed.contains("exposure"));
    let before = steps(&h);
    h.state_mut()
        .run("edit.resetAll", json!(null), Origin::User)
        .unwrap();
    h.run();
    assert_eq!(value(&h, EV), 0.0);
    assert_eq!(steps(&h), before + 1);
    assert!(!h.state().edit_marks().edited());
    let bypassed = h
        .state()
        .values()
        .lock()
        .unwrap()
        .get("bypass/exposure")
        .map(|e| e.value.clone());
    assert_eq!(bypassed, Some(Value::Bool(false)));
    // Nothing left to reset: no step.
    h.state_mut()
        .run("edit.resetAll", json!(null), Origin::User)
        .unwrap();
    h.run();
    assert_eq!(steps(&h), before + 1);
}
