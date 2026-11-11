//! The framework on a store in memory: the value flow, implicit controls
//! from descriptors, the commands, a size policy as data.

use std::sync::{Arc, Mutex};

use egui_hud::commands::{self, COMMANDS, Shared};
use egui_hud::flow::{self, Cause, Outcome, Steps, Verdict};
use egui_hud::model::{
    Control, Gesture, Intent, Jack, Kind, NumberSpec, Representation, ValueSpec, ValueType,
};
use egui_hud::slot::{Grow, Size, SizePolicy, Slot};
use egui_hud::{
    MemoryStore, ParamDescriptor, ParamType, Patch, Registry, Surface, Value, ValueSource, Variant,
    Widget, implicit,
};
use serde_json::json;

const GAIN: &str = "node/amp/gain";
const LEVEL: &str = "node/amp/level";
const STEPS: &str = "node/amp/steps";

/// A gain control: -10 to +10 dB, the bar spanning -6 to +6.
fn gain() -> Control {
    Control::new(
        "gain",
        "Gain",
        ValueSpec::Number(NumberSpec {
            unit: Some("dB".into()),
            hard: (-10.0, 10.0),
            soft: (-6.0, 6.0),
            step: 0.5,
            coarse: 2.0,
            default: 0.0,
            precision: 1,
            display_scale: 1.0,
            signed: true,
            integer: false,
            wraps: false,
            open: false,
        }),
    )
    .jack(Jack::new("value", ValueType::Number).unit("dB"))
    .gestures(&[Gesture::Drag, Gesture::Keys])
    .intents(&[Intent::Nudge, Intent::Set, Intent::Reset])
    .variant(
        Variant::new("slider", "Slider")
            .slot(Representation::Full, SizePolicy::fixed(Slot::ROW))
            .chip(),
    )
}

/// An amp node: its gain (patched), a level and a whole number (implicit).
fn store() -> MemoryStore {
    let mut s = MemoryStore::new();
    let mut g = ParamDescriptor::float("gain", -10.0, 10.0, 0.0);
    g.soft = Some((-6.0, 6.0));
    g.module = Some("amp".into());
    s.declare(GAIN, g);
    let mut l = ParamDescriptor::float("level", 0.0, 1.0, 0.5);
    l.unit = Some("%".into());
    l.display_scale = 100.0;
    l.module = Some("amp".into());
    s.declare(LEVEL, l);
    let mut n = ParamDescriptor::new(ParamType::Int, "steps", Value::Int(2));
    n.hard = Some((0.0, 8.0));
    n.module = Some("amp".into());
    s.declare(STEPS, n);
    s
}

fn registry() -> Registry {
    let mut r = Registry::with([gain()]).unwrap();
    let errors = r.patch([Patch::new("amp", "gain", "gain", "value")]);
    assert!(errors.is_empty(), "{errors:?}");
    r
}

fn value(s: &MemoryStore, path: &str) -> f64 {
    s.get(path).and_then(|v| v.as_f64()).unwrap()
}

#[test]
fn preview_then_commit_is_one_undo_step() {
    let mut s = store();
    let steps = Steps::new();
    let g = flow::begin(&mut s, &gain(), GAIN, "pointer").unwrap();
    for v in [1.0, 2.5, 4.0] {
        g.preview(&mut s, v, false).unwrap();
        assert_eq!(value(&s, GAIN), v, "the preview is in the store");
    }
    assert_eq!(s.open_gesture(), Some("Gain"));
    // Held to the soft range unless forced.
    assert_eq!(g.preview(&mut s, 8.0, false).unwrap(), 6.0);
    assert_eq!(g.preview(&mut s, 8.0, true).unwrap(), 8.0);
    let out = g.commit(&mut s, &steps, Cause::Drag, "pointer");
    assert_eq!(out, Outcome::Committed { from: 0.0, to: 8.0 });
    assert_eq!(s.undo_steps(), ["Gain"]);
}

#[test]
fn cancel_leaves_no_trace() {
    let mut s = store();
    let g = flow::begin(&mut s, &gain(), GAIN, "pointer").unwrap();
    g.preview(&mut s, 3.0, false).unwrap();
    assert_eq!(g.cancel(&mut s), 1);
    assert_eq!(value(&s, GAIN), 0.0);
    assert!(s.undo_steps().is_empty());
    assert!(s.open_gesture().is_none());
}

#[test]
fn a_hook_vetoes_or_redirects_the_change() {
    let mut s = store();
    let mut steps = Steps::new();
    steps.before("no more than 2 dB", |e| {
        assert_eq!(e.control, "gain");
        assert_eq!(e.node.as_deref(), Some("amp"));
        if e.to > 4.0 {
            Verdict::Veto
        } else if e.to > 2.0 {
            Verdict::Redirect(2.0)
        } else {
            Verdict::Go
        }
    });
    let ask = |to| flow::OneShot {
        to,
        force: false,
        origin: "agent",
        cause: Cause::Set,
        device: "test",
    };
    let out = flow::apply(&mut s, &steps, &gain(), GAIN, ask(5.0)).unwrap();
    assert_eq!(out, Outcome::Vetoed);
    assert_eq!(value(&s, GAIN), 0.0);
    assert!(s.undo_steps().is_empty(), "a veto leaves no step");
    let out = flow::apply(&mut s, &steps, &gain(), GAIN, ask(3.0)).unwrap();
    assert_eq!(out, Outcome::Committed { from: 0.0, to: 2.0 });
    assert_eq!(s.undo_steps().len(), 1);
    // Nothing changed: no step.
    let out = flow::apply(&mut s, &steps, &gain(), GAIN, ask(2.0)).unwrap();
    assert_eq!(out, Outcome::Unchanged);
    assert_eq!(s.undo_steps().len(), 1);
}

#[test]
fn whole_numbers_stay_whole_and_indicators_are_read_only() {
    let mut s = store();
    let r = registry();
    let d = &s.params(STEPS)[0].descriptor;
    let c = implicit(STEPS, d);
    let out = flow::apply(
        &mut s,
        &Steps::new(),
        &c,
        STEPS,
        flow::OneShot {
            to: 3.6,
            force: false,
            origin: "pointer",
            cause: Cause::Typed,
            device: "pointer",
        },
    )
    .unwrap();
    assert_eq!(out, Outcome::Committed { from: 2.0, to: 4.0 });
    assert_eq!(s.get(STEPS), Some(Value::Int(4)));
    let meter = Control::new(
        "meter",
        "Meter",
        ValueSpec::Other {
            stored: ValueType::Number,
        },
    )
    .jack(Jack::new("level", ValueType::Number).read_only())
    .variant(Variant::new("bar", "Bar").slot(Representation::Full, SizePolicy::fixed(Slot::ROW)))
    .indicator();
    assert_eq!(meter.kind, Kind::Indicator);
    assert!(flow::begin(&mut s, &meter, LEVEL, "pointer").is_err());
    assert!(r.get("gain").is_some());
}

#[test]
fn implicit_controls_follow_the_descriptor() {
    let s = store();
    let r = registry();
    let level = &s.params(LEVEL)[0].descriptor;
    assert!(r.control_at(LEVEL, level.module.as_deref()).is_none());
    let c = implicit(LEVEL, level);
    assert!(c.implicit);
    let full = c.faces[0].form(Representation::Full).unwrap();
    assert_eq!(full.policy.size, Slot::ROW);
    assert_eq!(full.policy.grow, Grow::Width);
    assert!(c.faces[0].shows_on(Surface::Panel));
    assert!(!c.faces[0].shows_on(Surface::Hud));
    assert_eq!(c.value.number().unwrap().format(0.25), "25.00 %");
    let g = &s.params(GAIN)[0].descriptor;
    assert_eq!(r.control_at(GAIN, g.module.as_deref()).unwrap().id, "gain");
    // A curve draws itself in a block six rows high.
    let mut curve = ParamDescriptor::new(ParamType::Floats, "curve", Value::Floats(vec![]));
    curve.widget = Some(Widget::Curve);
    let c = implicit("node/amp/curve", &curve);
    let full = c.faces[0].form(Representation::Full).unwrap();
    assert_eq!(full.policy.size, Slot::block(6));
}

#[test]
fn commands_set_nudge_reset_and_list() {
    let mut s = store();
    let shared = Shared {
        registry: Arc::new(registry()),
        steps: Arc::new(Mutex::new(Steps::new())),
        ..Shared::default()
    };
    let mut run = |id: &str, args| commands::run(id, &args, &"agent", &mut s, &shared);
    assert_eq!(
        run("controls.catalogue", json!({})).unwrap()[0]["id"],
        "gain"
    );
    let out = run(
        "controls.set",
        json!({"node": "amp", "control": "gain", "value": 1.5}),
    )
    .unwrap();
    assert_eq!(out["outcome"], "committed");
    assert_eq!(out["shown"], "+1.5 dB");
    let out = run(
        "controls.nudge",
        json!({"node": "amp", "control": "gain", "steps": -1}),
    )
    .unwrap();
    assert_eq!(out["value"], 1.0);
    let out = run("controls.reset", json!({"node": "amp", "control": "gain"})).unwrap();
    assert_eq!(out["value"], 0.0);
    let found = run("controls.find", json!({"node": "amp", "query": "lev"})).unwrap();
    assert!(found.to_string().contains("level"), "{found}");
    assert!(run("controls.nope", json!({})).is_err());
    assert!(COMMANDS.iter().any(|c| c.name == "controls.reveal_all"));
    assert_eq!(s.undo_steps().len(), 3, "one step each");
    let all = commands::instances(&s, &registry());
    let rows = all.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    let level = rows.iter().find(|r| r["path"] == LEVEL).unwrap();
    assert_eq!(level["implicit"], true);
    assert_eq!(level["value"], 0.5);
}

#[test]
fn a_size_policy_reads_as_slots_in_json() {
    let p = SizePolicy::growable(Slot::HALF, Slot::ROW, Slot::wide(3), Grow::Width);
    let j = serde_json::to_value(p).unwrap();
    assert_eq!(
        j,
        json!({"min": "1/2x1", "size": "1x1", "max": "3x1", "grow": "width"})
    );
    let back: SizePolicy = serde_json::from_value(j).unwrap();
    assert_eq!(back, p);
    assert!(serde_json::from_value::<Slot>(json!("5/7x1")).is_err());
    assert_eq!(Slot::new(Size::TWO_THIRDS, Size::ONE).to_string(), "2/3x1");
}

#[test]
fn a_committed_step_undoes_and_redoes() {
    let mut s = store();
    let steps = Steps::new();
    for to in [2.0, 3.0] {
        let g = flow::begin(&mut s, &gain(), GAIN, "pointer").unwrap();
        g.preview(&mut s, to, false).unwrap();
        g.commit(&mut s, &steps, Cause::Drag, "pointer");
    }
    assert_eq!(s.undo().as_deref(), Some("Gain"));
    assert_eq!(value(&s, GAIN), 2.0);
    assert_eq!(s.undo_steps(), ["Gain"]);
    assert_eq!(s.redo_steps(), ["Gain"]);
    assert_eq!(s.redo().as_deref(), Some("Gain"));
    assert_eq!(value(&s, GAIN), 3.0);
    s.undo();
    s.undo();
    assert_eq!(value(&s, GAIN), 0.0);
    assert!(s.undo().is_none(), "nothing left");
    // A new step forgets what was undone.
    let g = flow::begin(&mut s, &gain(), GAIN, "pointer").unwrap();
    g.preview(&mut s, 1.0, false).unwrap();
    // No undo while a gesture is open.
    assert!(s.undo().is_none());
    g.commit(&mut s, &steps, Cause::Drag, "pointer");
    assert!(s.redo_steps().is_empty());
}
