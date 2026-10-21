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

/// Values are written in the interface's language (egui-pop's
/// `Strings::number`): French puts a comma; an exposure is signed.
#[test]
fn values_are_written_in_the_interface_language() {
    use egui::accesskit::Role;

    let mut h = harness();
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": EV, "value": 0.7 }),
            Origin::User,
        )
        .unwrap();
    h.run();
    let read = |h: &egui_kittest::Harness<'_, Shell>, label: &str| {
        h.get_by_role_and_label(Role::Slider, label).value()
    };
    assert_eq!(read(&h, "exposure").as_deref(), Some("+0.70 EV"));
    assert_eq!(read(&h, "black").as_deref(), Some("0.00 %"));
    let app = h.state().app().handle().clone();
    egui_pop::i18n::plugin::set_language(&app, "fr").unwrap();
    h.run();
    h.run();
    assert_eq!(read(&h, "exposition").as_deref(), Some("+0,70\u{202f}EV"));
    assert_eq!(read(&h, "noir").as_deref(), Some("0,00\u{202f}%"));
    // A negative exposure: its minus sign, no plus, the comma.
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": EV, "value": -1.25 }),
            Origin::User,
        )
        .unwrap();
    h.run();
    assert_eq!(read(&h, "exposition").as_deref(), Some("-1,25\u{202f}EV"));
}

/// The labels are the modules' translations (`param-<module>-<name>`),
/// back to English when the language changes again.
#[test]
fn labels_are_translated() {
    use egui::accesskit::Role;

    let mut h = harness();
    let app = h.state().app().handle().clone();
    egui_pop::i18n::plugin::set_language(&app, "fr").unwrap();
    h.run();
    h.run();
    for label in ["exposition", "noir"] {
        h.get_by_role_and_label(Role::Slider, label);
    }
    h.get_by_label("compenser la correction");
    assert!(h.query_by_label("compensate_bias").is_none());
    egui_pop::i18n::plugin::set_language(&app, "en").unwrap();
    h.run();
    h.run();
    h.get_by_role_and_label(Role::Slider, "exposure");
    h.get_by_label("compensate_bias");
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
    {
        let store = h.state().values.lock().unwrap();
        let steps: Vec<&str> = store
            .undo_steps()
            .iter()
            .map(|s| s.label.as_str())
            .collect();
        assert_eq!(steps, ["drag exposure"], "one step for the whole drag");
    }
    // The release ran egui-pop's `slider.change` step once, from the
    // value before the drag.
    let changes: Vec<serde_json::Value> = h
        .state()
        .app()
        .handle()
        .events_since(0)
        .into_iter()
        .filter(|e| e.name == "slider.change")
        .map(|e| e.data)
        .collect();
    assert_eq!(changes.len(), 1, "{changes:?}");
    // One undo puts the whole drag back.
    h.state()
        .values
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    assert_eq!(ev(&h), 0.0);
}

/// Escape during a drag puts the value back and leaves no undo step.
#[test]
fn escape_gives_a_drag_up_without_a_trace() {
    let mut h = harness();
    let r = h.state().target_rect(&format!("param:{EV}")).unwrap();
    let from = r.center();
    h.hover_at(from);
    h.run();
    press(&h, from, true);
    h.run();
    for k in 1..=3 {
        h.hover_at(from + vec2(10.0 * k as f32, 0.0));
        h.run();
    }
    assert!(ev(&h) > 0.0);
    h.key_press(Key::Escape);
    h.run();
    press(&h, from + vec2(30.0, 0.0), false);
    h.run();
    assert_eq!(ev(&h), 0.0);
    assert!(h.state().values.lock().unwrap().undo_steps().is_empty());
}

/// With the wheel over sliders guarded (the setting; darktable's
/// profile, Helios's default, steps the slider under the pointer), the
/// wheel scrolls past a slider: it steps one only once focused (or with
/// Alt held over it).
#[test]
fn the_wheel_guarded_steps_a_slider_only_once_focused() {
    let mut h = harness();
    {
        use egui_pop::model::values::{Change, Set, Value as PopValue};
        let store = h.state().app().handle().store();
        store
            .lock()
            .unwrap()
            .set(
                Set::new(
                    "settings/input.slider_wheel",
                    Change::To(PopValue::Choice("guarded".into())),
                    egui_pop::model::commands::Origin::User,
                )
                .untracked(),
            )
            .unwrap();
    }
    h.run();
    // Steps, not `run`: a tooltip fading in keeps asking for frames.
    let wheel = |h: &mut egui_kittest::Harness<'_, Shell>, modifiers: Modifiers| {
        let at = h
            .state()
            .target_rect(&format!("param:{EV}"))
            .unwrap()
            .center();
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::ModifiersChanged(modifiers));
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            phase: egui::TouchPhase::Move,
            delta: vec2(0.0, 1.0),
            modifiers,
        });
        for _ in 0..4 {
            h.step();
        }
        h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
        for _ in 0..4 {
            h.step();
        }
    };
    wheel(&mut h, Modifiers::NONE);
    assert_eq!(ev(&h), 0.0, "not focused: the wheel scrolls the panel");
    wheel(&mut h, Modifiers::ALT);
    assert!((ev(&h) - 0.02).abs() < 1e-9, "Alt held: {}", ev(&h));
    // Focused by a click: the wheel steps it.
    let at = h
        .state()
        .target_rect(&format!("param:{EV}"))
        .unwrap()
        .center();
    press(&h, at, true);
    press(&h, at, false);
    for _ in 0..4 {
        h.step();
    }
    wheel(&mut h, Modifiers::NONE);
    assert!((ev(&h) - 0.04).abs() < 1e-9, "focused: {}", ev(&h));
}

/// Enter types an expression of the current value, `x`.
#[test]
fn a_typed_expression_sets_the_value() {
    let mut h = harness();
    h.state_mut()
        .run(
            "param.set",
            json!({ "path": EV, "value": 1.5 }),
            Origin::User,
        )
        .unwrap();
    h.run();
    let at = h
        .state()
        .target_rect(&format!("param:{EV}"))
        .unwrap()
        .center();
    h.hover_at(at);
    press(&h, at, true);
    press(&h, at, false);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    clear_typed(&mut h);
    h.event(egui::Event::Text("x * 2 - 0,5".into()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert!((ev(&h) - 2.5).abs() < 1e-9, "{}", ev(&h));
    // The typed value is an undo step of its own.
    h.state()
        .values
        .lock()
        .unwrap()
        .undo(&Origin::User)
        .unwrap();
    assert!((ev(&h) - 1.5).abs() < 1e-9, "{}", ev(&h));
}

/// Empties the typed entry (it opens on the value shown).
fn clear_typed(h: &mut egui_kittest::Harness<'_, Shell>) {
    for _ in 0..12 {
        h.key_press(Key::Backspace);
    }
    h.run();
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
    // Shift: ten steps at once (up to the coarse step).
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
    h.run();
    assert!((ev(&h) - 0.22).abs() < 1e-9, "{}", ev(&h));
    // Enter types; past the soft range (-3..4 EV) is forced, up to the hard one.
    h.key_press(Key::Enter);
    h.run();
    clear_typed(&mut h);
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

#[test]
fn a_path_s_points_fold_into_a_summary_that_opens() {
    let mut g = Graph::new();
    let path = g
        .add_module(
            "path",
            ModuleRef::new(helios_module_masks::MaskPath),
            &Params::new(),
        )
        .unwrap();
    let shell = Shell::new(helios_panels(), None);
    helios_values::bind_node(&mut shell.values.lock().unwrap(), &g, path).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.state_mut()
        .run("helios.params.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
    // Folded, the four points of the default square are a summary, and
    // the rows stay within the panel.
    let panel = h.state().target_rect("helios.params").unwrap();
    for p in ["node/path/points", "node/path/feather"] {
        let row = h.state().target_rect(&format!("param:{p}")).unwrap();
        assert!(
            row.right() <= panel.right() + 1.0,
            "{p}: {row:?} in {panel:?}"
        );
    }
    // A click opens it: a point a line, numbered.
    h.get_by_label("4 points").click();
    h.run();
    h.get_by_label_contains("4: ");
    // And another closes it.
    h.get_by_label("4 points").click();
    h.run();
    assert!(h.query_by_label_contains("4: ").is_none());
}

/// A choice's list opens on a click and stays open to be picked from:
/// the parameter's context menu must not close it.
#[test]
fn a_choice_s_list_opens_and_picks() {
    use egui::accesskit::Role;
    use egui_kittest::kittest::by;

    let mut g = Graph::new();
    let ramp = g
        .add_module(
            "ramp",
            ModuleRef::new(helios_module_masks::MaskGradient),
            &Params::new(),
        )
        .unwrap();
    let shell = Shell::new(helios_panels(), None);
    helios_values::bind_node(&mut shell.values.lock().unwrap(), &g, ramp).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.state_mut()
        .run("helios.params.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
    let at = h.get(by().role(Role::ComboBox)).rect().center();
    h.hover_at(at);
    h.run();
    press(&h, at, true);
    press(&h, at, false);
    h.run();
    assert!(egui::Popup::is_any_open(&h.ctx), "the list is open");
    h.get_by_label("sigmoid").click();
    h.run();
    let profile = h
        .state()
        .values
        .lock()
        .unwrap()
        .get("node/ramp/profile")
        .unwrap()
        .value
        .clone();
    assert_eq!(profile, Value::Choice("sigmoid".into()));
}
