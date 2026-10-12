//! Parameter widgets: found by path, a drag is one gesture that keeps
//! agents off, a change by an agent shows with who made it, typed values
//! with units, reset and keys.

use egui::{Key, Modifiers, PointerButton, vec2};
use egui_kittest::kittest::Queryable;
use helios_core::{Graph, Image, ModuleRef, Params, Region, Value};
use helios_gui::params::parse_typed;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

const EV: &str = "node/exposure/exposure";

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
    // The node's values only: no viewport rendering in the background.
    helios_values::bind_node(&mut shell.values.lock().unwrap(), &g, exp).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.state_mut()
        .run("helios.params.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
    h
}

fn ev(h: &egui_kittest::Harness<'_, Shell>) -> f64 {
    h.state()
        .values
        .lock()
        .unwrap()
        .get(EV)
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

fn press(h: &egui_kittest::Harness<'_, Shell>, at: egui::Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

#[test]
fn widgets_come_from_descriptors_and_are_found_by_path() {
    let h = harness();
    for p in [EV, "node/exposure/black", "node/exposure/compensate_bias"] {
        assert!(h.state().target_rect(&format!("param:{p}")).is_ok(), "{p}");
    }
    // The slider is a slider to assistive technology, labelled.
    h.get_by_role_and_label(egui::accesskit::Role::Slider, "exposure");
}

#[test]
fn a_drag_is_one_gesture_that_keeps_agents_off() {
    let mut h = harness();
    let r = h.state().target_rect(&format!("param:{EV}")).unwrap();
    let from = r.center();
    h.hover_at(from);
    h.run();
    press(&h, from, true);
    h.run();
    for k in 1..=5 {
        h.hover_at(from + vec2(10.0 * k as f32, 0.0));
        h.run();
    }
    // An agent during the drag: refused, the value stays.
    let mid = ev(&h);
    assert!(mid > 0.0, "the drag moved it: {mid}");
    let refused = h.state_mut().run(
        "param.set",
        json!({ "path": EV, "value": -2.0 }),
        Origin::Agent("a1".into()),
    );
    assert!(refused.unwrap_err().contains("held"));
    press(&h, from + vec2(50.0, 0.0), false);
    h.run();
    let store = h.state().values.lock().unwrap();
    let steps: Vec<&str> = store
        .undo_steps()
        .iter()
        .map(|s| s.label.as_str())
        .collect();
    assert_eq!(steps, ["drag exposure"], "one step for the whole drag");
}

#[test]
fn a_change_by_an_agent_shows_and_says_who() {
    let mut h = harness();
    assert!(h.state().target_rect(&format!("param:{EV}:moved")).is_err());
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": EV, "value": 1.5 }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    h.step();
    h.step();
    assert!(h.state().target_rect(&format!("param:{EV}:moved")).is_ok());
    assert!(
        h.state()
            .target_rect(&format!("param:{EV}:changed"))
            .is_ok()
    );
    // The value shown follows.
    h.get_by_role_and_label(egui::accesskit::Role::Slider, "exposure");
}

#[test]
fn keys_step_enter_types_and_double_click_resets() {
    let mut h = harness();
    let r = h.state().target_rect(&format!("param:{EV}")).unwrap();
    h.hover_at(r.center());
    press(&h, r.center(), true);
    press(&h, r.center(), false);
    h.run();
    h.key_press(Key::ArrowRight);
    h.run();
    assert!((ev(&h) - 0.02).abs() < 1e-9, "{}", ev(&h));
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
    h.run();
    assert!((ev(&h) - 0.27).abs() < 1e-9, "{}", ev(&h));
    // Enter types; past the soft range (-3..4 EV) is forced, up to the hard one.
    h.key_press(Key::Enter);
    h.run();
    h.event(egui::Event::Text("6,5 EV".into()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert!((ev(&h) - 6.5).abs() < 1e-9, "{}", ev(&h));
    // Double click resets.
    let r = h.state().target_rect(&format!("param:{EV}")).unwrap();
    h.hover_at(r.center());
    h.step();
    // Two clicks 50 ms apart (the harness's clock only moves when told).
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
    assert_eq!(ev(&h), 0.0);
}

#[test]
fn typed_numbers_take_units_and_commas() {
    assert_eq!(parse_typed("1.5 EV"), Some(1.5));
    assert_eq!(parse_typed(" -0,25%"), Some(-0.25));
    assert_eq!(parse_typed("+3"), Some(3.0));
    assert_eq!(parse_typed("EV"), None);
    let _ = Value::Float(0.0);
}

/// Pictures of the Parameters panel in each theme, for `docs/gui.md`;
/// written to `docs/gui/` when `HELIOS_DOC_PICTURES` is set.
#[test]
fn pictures_of_the_parameters_panel() {
    let out = std::env::var_os("HELIOS_DOC_PICTURES").map(std::path::PathBuf::from);
    for theme in ["helios-dark", "helios-light"] {
        let mut h = harness();
        h.state_mut()
            .set_theme(theme, std::collections::BTreeMap::default());
        h.state_mut()
            .run(
                "param.set",
                json!({ "path": EV, "value": 1.25 }),
                Origin::Agent("a1".into()),
            )
            .unwrap();
        h.run();
        let picture = headless::snapshot(&mut h, "helios.params").unwrap();
        assert!(picture.width() > 100);
        if let Some(dir) = &out {
            let path = dir.join(format!("params-{theme}.png"));
            helios_gui::snapshot::save(&picture, &path).unwrap();
        }
    }
}

#[test]
fn a_parameter_s_context_menu_runs_its_commands_on_its_path() {
    let mut h = harness();
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": EV, "value": 1.5 }),
            Origin::User,
        )
        .unwrap();
    h.run();
    assert!((ev(&h) - 1.5).abs() < 1e-9);
    h.get_by_role_and_label(egui::accesskit::Role::Slider, "exposure")
        .click_secondary();
    h.run();
    // egui-pop's declared menu, its commands given the widget's path.
    assert!(h.query_by_label("Copy a value's path").is_some());
    h.get_by_label("Reset a value to its default").click();
    h.run();
    h.run();
    let default = h
        .state()
        .values
        .lock()
        .unwrap()
        .get(EV)
        .unwrap()
        .descriptor
        .default
        .as_f64()
        .unwrap();
    assert!((ev(&h) - default).abs() < 1e-9, "reset to {default}");
    let run = h.state().history().pop().unwrap();
    assert_eq!(run.id, "param.reset");
    assert_eq!(run.args, json!({ "path": EV }));
}
