//! The controls layer without a window: the registry and its description,
//! the value flow on the store, the exposure control's ergonomics, the
//! commands.

use std::sync::{Arc, Mutex};

use helios_controls::catalogue::{self, EXPOSURE};
use helios_controls::flow::{self, Cause, Outcome, Steps, Verdict};
use helios_controls::model::{Control, Representation, Surface};
use helios_controls::slot::{Grow, SizePolicy, Slot};
use helios_controls::{HeliosRegistry, Patch, Registry, implicit};
use helios_core::{ParamHint, ParamSpec, Value};
use helios_shell::commands::{Origin, Registry as Commands};
use helios_values::{Descriptor, Store};
use serde_json::json;

const PATH: &str = "node/exp/exposure";
const BLACK: &str = "node/exp/black";

/// A store with an exposure node's two numbers, as the graph declares
/// them (the exposure module's specs and hints).
fn store() -> Store {
    let mut s = Store::new();
    let mut d = Descriptor::from_spec(
        &ParamSpec::float("exposure", "exposure adjustment, in EV", -18.0, 18.0, 0.0),
        Some(
            &ParamHint::new("exposure")
                .soft(-3.0, 4.0)
                .steps(0.02, 0.25)
                .unit("EV")
                .display(1.0, 2),
        ),
    );
    d.module = Some("exposure".into());
    s.declare(PATH, d);
    let mut b = Descriptor::from_spec(
        &ParamSpec::float("black", "black level correction", -1.0, 1.0, 0.0),
        Some(
            &ParamHint::new("black")
                .soft(-0.1, 0.1)
                .steps(0.0005, 0.005)
                .unit("%")
                .display(100.0, 2),
        ),
    );
    b.module = Some("exposure".into());
    s.declare(BLACK, b);
    s
}

fn registry() -> Registry {
    let mut r = Registry::standard();
    let errors = r.patch([Patch::new("exposure", "exposure", EXPOSURE, "value")]);
    assert!(errors.is_empty(), "{errors:?}");
    r
}

fn value(s: &Store, path: &str) -> f64 {
    s.get(path).and_then(|e| e.value.as_f64()).unwrap()
}

#[test]
fn the_catalogue_lists_exposure_and_black_level_and_describes_them() {
    let r = registry();
    let cat = r.catalogue();
    let ids: Vec<&str> = cat
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["black-level", "exposure"]);
    let b = r.describe(catalogue::BLACK_LEVEL).unwrap();
    assert_eq!(b["connector"], json!(["number<relative>"]));
    assert_eq!(b["variants"], json!(["slider", "knob"]));
    let black = catalogue::black_level();
    assert_eq!(black.value.number().unwrap().format(0.0012), "0.12 %");
    assert_eq!(black.detented(0.001, false), 0.0, "held at 0");
    assert_eq!(black.detented(0.001, true), 0.001, "Alt lets it go");
    let d = r.describe(EXPOSURE).unwrap();
    assert_eq!(d["connector"], json!(["number<EV>"]));
    assert_eq!(d["variants"], json!(["slider", "knob", "scrub"]));
    assert_eq!(
        d["surfaces"]["slider"],
        json!(["panel", "dock", "toolbar", "float", "hud"])
    );
    let slider = &d["layout"]["variants"]["slider"];
    assert_eq!(
        slider["slots"]["full"]["summary"],
        "1x1 (from 3/4x1 to 4x1, grows across)"
    );
    assert_eq!(slider["slots"]["full"]["policy"]["max"], "4x1");
    assert_eq!(slider["slots"]["compact"]["summary"], "fixed 1/2x1");
    assert_eq!(slider["slots"]["simplified"]["summary"], "fixed 1/3x1");
    assert_eq!(slider["orientations"], json!(["horizontal", "vertical"]));
    assert_eq!(slider["preferred_orientation"], "horizontal");
    assert_eq!(slider["integration"]["panel"]["level"], "managed");
    assert_eq!(slider["integration"]["hud"]["level"], "managed");
    let scrub = &d["layout"]["variants"]["scrub"];
    assert_eq!(scrub["slots"]["full"]["summary"], "fixed 1/2x1");
    assert!(scrub["slots"].get("compact").is_none());
    assert_eq!(scrub["orientations"], json!(["horizontal"]));
    let knob = &d["layout"]["variants"]["knob"];
    assert_eq!(knob["slots"]["full"]["summary"], "fixed 1/2x1");
    assert_eq!(knob["orientations"], json!(["horizontal"]));
    assert_eq!(d["layout"]["warnings"], json!([]));
    assert_eq!(d["detents"][0]["value"], json!(0.0));
    assert!(
        d["gestures"]
            .as_array()
            .unwrap()
            .contains(&json!("double-click-reset"))
    );
    assert_eq!(
        d["config_schema"]["properties"]["variant"]["enum"],
        json!(["slider", "knob", "scrub"])
    );
    assert_eq!(d["patches"][0]["param"], json!("exposure"));
    assert!(r.describe("nope").is_none());
}

#[test]
fn a_description_round_trips_through_json() {
    let c = catalogue::exposure();
    let text = serde_json::to_string(&c).unwrap();
    let back: Control = serde_json::from_str(&text).unwrap();
    assert_eq!(back, c);
    // And the description carries the same declaration.
    let d = registry().describe(EXPOSURE).unwrap();
    let from_describe: Control = serde_json::from_value(d["control"].clone()).unwrap();
    assert_eq!(from_describe, c);
}

#[test]
fn bad_declarations_and_patches_are_refused() {
    let mut r = registry();
    assert!(
        r.register(catalogue::exposure()).is_err(),
        "a second exposure"
    );
    let mut bad = catalogue::exposure();
    bad.id = "bad".into();
    bad.variants[0].slots[0].policy.min = Slot::wide(2);
    assert!(
        r.register(bad).is_err(),
        "a minimum above the preferred slot"
    );
    let mut bad = catalogue::exposure();
    bad.id = "bad".into();
    bad.variants[0].slots.swap(0, 1);
    assert!(r.register(bad).is_err(), "compact before full");
    let errors = r.patch([
        Patch::new("exposure", "exposure", EXPOSURE, "value"),
        Patch::new("blur", "radius", "radius", "value"),
        Patch::new("exposure", "black", EXPOSURE, "nope"),
    ]);
    assert_eq!(errors.len(), 3, "{errors:?}");
}

#[test]
fn implicit_controls_follow_the_descriptor() {
    let s = store();
    let r = registry();
    let d = &s.get(BLACK).unwrap().descriptor;
    assert!(r.control_for(BLACK, d).is_none(), "black has no patch");
    let c = implicit(BLACK, d);
    assert!(c.implicit);
    let full = c.variants[0].form(Representation::Full).unwrap();
    assert_eq!(full.policy.size, Slot::ROW);
    assert_eq!(full.policy.grow, Grow::Width);
    assert!(c.variants[0].shows_on(Surface::Panel));
    assert!(!c.variants[0].shows_on(Surface::Hud));
    let n = c.value.number().unwrap();
    assert_eq!((n.soft, n.step, n.precision), ((-0.1, 0.1), 0.0005, 2));
    assert_eq!(n.format(0.0012), "0.12 %");
    let e = &s.get(PATH).unwrap().descriptor;
    assert_eq!(r.control_for(PATH, e).unwrap().id, EXPOSURE);
}

#[test]
fn preview_then_commit_is_one_undo_step() {
    let mut s = store();
    let c = catalogue::exposure();
    let steps = Steps::new();
    let g = flow::begin(&mut s, &c, PATH, Origin::User).unwrap();
    for v in [0.1, 0.4, 0.67] {
        g.preview(&mut s, v, false).unwrap();
        assert_eq!(value(&s, PATH), v, "the preview is in the store");
    }
    // Another origin is kept off meanwhile.
    let agent = s.set(helios_values::Set::new(
        PATH,
        helios_values::Change::To(Value::Float(2.0)),
        Origin::Agent("a".into()),
    ));
    assert!(agent.is_err());
    let out = g.commit(&mut s, &steps, Cause::Drag, "pointer");
    assert_eq!(
        out,
        Outcome::Committed {
            from: 0.0,
            to: 0.67
        }
    );
    assert_eq!(s.undo_steps().len(), 1);
    assert_eq!(s.undo_steps()[0].label, "Exposure");
    s.undo(&Origin::User).unwrap();
    assert_eq!(value(&s, PATH), 0.0);
}

#[test]
fn cancel_leaves_no_trace() {
    let mut s = store();
    let c = catalogue::exposure();
    let g = flow::begin(&mut s, &c, PATH, Origin::User).unwrap();
    g.preview(&mut s, 1.5, false).unwrap();
    g.cancel(&mut s);
    assert_eq!(value(&s, PATH), 0.0);
    assert!(s.undo_steps().is_empty());
    assert!(s.open_gesture().is_none());
}

#[test]
fn a_hook_vetoes_or_redirects_the_change() {
    let mut s = store();
    let c = catalogue::exposure();
    let mut steps = Steps::new();
    steps.before("no brightening past 1 EV", |e| {
        assert_eq!(e.control, "exposure");
        assert_eq!(e.node.as_deref(), Some("exp"));
        if e.to > 2.0 {
            Verdict::Veto
        } else if e.to > 1.0 {
            Verdict::Redirect(1.0)
        } else {
            Verdict::Go
        }
    });
    let out = flow::apply(
        &mut s,
        &steps,
        &c,
        PATH,
        flow::OneShot {
            to: 3.0,
            force: false,
            origin: Origin::User,
            cause: Cause::Set,
            device: "test",
        },
    )
    .unwrap();
    assert_eq!(out, Outcome::Vetoed);
    assert_eq!(value(&s, PATH), 0.0);
    assert!(s.undo_steps().is_empty(), "a veto leaves no step");
    let out = flow::apply(
        &mut s,
        &steps,
        &c,
        PATH,
        flow::OneShot {
            to: 1.5,
            force: false,
            origin: Origin::User,
            cause: Cause::Set,
            device: "test",
        },
    )
    .unwrap();
    assert_eq!(out, Outcome::Committed { from: 0.0, to: 1.0 });
    assert_eq!(s.undo_steps().len(), 1);
}

#[test]
fn exposure_holds_on_its_zero_detent_and_crosses_it() {
    let c = catalogue::exposure();
    // 1 EV per 200 points: from -0.5, 92 points is -0.04 and 108 points
    // +0.04, both held on 0.
    assert_eq!(c.scrubbed(-0.5, 92.0, false, false), 0.0);
    assert_eq!(c.scrubbed(-0.5, 108.0, false, false), 0.0);
    // Moving on crosses it.
    assert!((c.scrubbed(-0.5, 120.0, false, false) - 0.1).abs() < 1e-9);
    // Alt turns the detent off.
    assert!((c.scrubbed(-0.5, 90.0, false, true) + 0.05).abs() < 1e-9);
    // The slider's hook holds the same way.
    assert_eq!(c.detented(0.03, false), 0.0);
    assert_eq!(c.detented(0.06, false), 0.06);
}

#[test]
fn exposure_fine_mode_is_ten_times_slower() {
    let c = catalogue::exposure();
    let coarse = c.scrubbed(1.0, 40.0, false, false);
    let fine = c.scrubbed(1.0, 40.0, true, false);
    assert!((coarse - 1.2).abs() < 1e-9);
    assert!((fine - 1.02).abs() < 1e-9);
    // Steps: 0.01, fine 0.001, coarse a third of a stop; back on 0 exactly.
    assert!((c.nudged(0.0, 3, false, false) - 0.03).abs() < 1e-9);
    assert!((c.nudged(0.0, 3, true, false) - 0.003).abs() < 1e-9);
    assert!((c.nudged(0.0, 1, false, true) - 1.0 / 3.0).abs() < 1e-9);
    assert_eq!(c.nudged(0.03, -3, false, false), 0.0);
    // Held to the soft range.
    assert_eq!(c.nudged(3.9, 10, false, true), 4.0);
}

#[test]
fn the_readout_reads_signed_stops() {
    let n = catalogue::exposure();
    let n = n.value.number().unwrap();
    assert_eq!(n.format(0.67), "+0.67 EV");
    assert_eq!(n.format(-1.0), "-1.00 EV");
    assert_eq!(n.format(-0.0001), "0.00 EV");
}

#[test]
fn commands_set_nudge_and_reset_exposure() {
    let store = Arc::new(Mutex::new(store()));
    let mut cmds = Commands::new();
    helios_controls::commands::register(
        &mut cmds,
        store.clone(),
        Arc::new(registry()),
        Arc::new(Mutex::new(Steps::new())),
    )
    .unwrap();
    let agent = Origin::Agent("t".into());
    let cx = helios_shell::when::Context::new();
    let mut run = |id: &str, args: serde_json::Value| cmds.run(id, args, agent.clone(), &cx);
    let cat = run("controls.catalogue", json!({})).unwrap();
    assert_eq!(cat[1]["id"], "exposure");
    let d = run("controls.describe", json!({"id": "exposure"})).unwrap();
    assert_eq!(d["id"], "exposure");
    let out = run(
        "controls.set",
        json!({"node": "exp", "control": "exposure", "value": 0.7}),
    )
    .unwrap();
    assert_eq!(out["outcome"], "committed");
    assert_eq!(out["shown"], "+0.70 EV");
    let out = run(
        "controls.nudge",
        json!({"node": "exp", "control": "exposure", "steps": -2}),
    )
    .unwrap();
    assert!((out["value"].as_f64().unwrap() - 0.68).abs() < 1e-9);
    let out = run(
        "controls.reset",
        json!({"node": "exp", "control": "exposure"}),
    )
    .unwrap();
    assert_eq!(out["value"], 0.0);
    assert_eq!(store.lock().unwrap().undo_steps().len(), 3, "one step each");
    assert!(
        run(
            "controls.set",
            json!({"node": "exp", "control": "feather", "value": 1})
        )
        .is_err()
    );
    let all = helios_controls::commands::instances(&store.lock().unwrap(), &registry());
    let rows = all.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let black = rows.iter().find(|r| r["path"] == BLACK).unwrap();
    assert_eq!(black["implicit"], true);
    let exp = rows.iter().find(|r| r["path"] == PATH).unwrap();
    assert_eq!(exp["control"], "exposure");
}

#[test]
fn an_integrated_variant_without_fallback_is_flagged_not_refused() {
    use helios_controls::Variant;
    use helios_controls::slot::{Fallback, Integration};
    let base = catalogue::exposure();
    // A drawn variant with the slider as its managed fallback: fine.
    let mut c = base.clone();
    c.id = "drawn".into();
    c.variants.push(
        Variant::new("dial", "Dial")
            .slot(Representation::Full, SizePolicy::fixed(Slot::HALF))
            .integrated(
                Surface::Panel,
                Integration::integrated(Fallback::Variant("slider".into())),
            ),
    );
    assert!(c.check().is_ok());
    assert!(c.warnings().is_empty());
    // No fallback: registered, but counted.
    let mut lone = c.clone();
    lone.id = "lone".into();
    lone.variants[3]
        .integration
        .insert(Surface::Panel, Integration::Integrated { fallback: None });
    assert!(lone.check().is_ok());
    assert_eq!(
        lone.warnings(),
        ["control `lone`, variant `dial`: integrated on panel with no managed fallback"]
    );
    let mut r = Registry::standard();
    r.register(lone).unwrap();
    assert_eq!(r.warnings().len(), 1);
    let d = r.describe("lone").unwrap();
    assert_eq!(
        d["layout"]["variants"]["dial"]["integration"]["panel"]["level"],
        "integrated"
    );
    assert_eq!(
        d["layout"]["variants"]["dial"]["integration"]["panel"]["fallback"],
        json!(null)
    );
    assert_eq!(d["layout"]["warnings"].as_array().unwrap().len(), 1);
    // Sorted by id: black-level, exposure, lone.
    assert_eq!(r.catalogue()[2]["warnings"], 1);
    // A fallback that is not a managed variant is refused.
    let mut bad = c.clone();
    bad.variants[3].integration.insert(
        Surface::Panel,
        Integration::integrated(Fallback::Variant("nope".into())),
    );
    assert!(bad.check().is_err());
    let mut bad = c;
    bad.variants[3]
        .integration
        .insert(Surface::Panel, Integration::integrated(Fallback::Parts));
    assert!(bad.check().is_err(), "parts, but not a case");
}

#[test]
fn an_indicator_has_read_only_jacks() {
    use helios_controls::Variant;
    use helios_controls::model::{Jack, Kind, ValueType};
    let meter = |read_only: bool| {
        let jack = Jack::new("level", ValueType::Number);
        Control::new(
            "meter",
            "Meter",
            helios_controls::model::ValueSpec::Other {
                stored: ValueType::Number,
            },
        )
        .jack(if read_only { jack.read_only() } else { jack })
        .variant(
            Variant::new("bar", "Bar").slot(Representation::Full, SizePolicy::fixed(Slot::ROW)),
        )
    };
    assert!(meter(true).indicator().check().is_ok());
    assert!(meter(false).indicator().check().is_err(), "it writes");
    assert!(
        meter(true).check().is_err(),
        "a control that writes nothing"
    );
    let c = meter(true).indicator();
    assert_eq!(c.kind, Kind::Indicator);
    let text = serde_json::to_string(&c).unwrap();
    assert!(text.contains(r#""kind":"indicator""#), "{text}");
    assert!(text.contains(r#""read_only":true"#), "{text}");
    let back: Control = serde_json::from_str(&text).unwrap();
    assert_eq!(back, c);
    // The value flow refuses to write it.
    let mut s = store();
    assert!(flow::begin(&mut s, &c, PATH, Origin::User).is_err());
}
