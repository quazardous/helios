//! The controls layer without a window: the registry and its description,
//! the value flow on the store, the exposure control's ergonomics, the
//! commands.

use std::sync::{Arc, Mutex};

use helios_controls::catalogue::{self, EXPOSURE};
use helios_controls::flow::{self, Cause, Outcome, Steps, Verdict};
use helios_controls::model::{Control, Footprint, Surface};
use helios_controls::{Patch, Registry, implicit};
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
fn the_catalogue_lists_exposure_and_describes_it() {
    let r = registry();
    let cat = r.catalogue();
    let ids: Vec<&str> = cat
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["exposure"]);
    let d = r.describe(EXPOSURE).unwrap();
    assert_eq!(d["connector"], json!(["number<EV>"]));
    assert_eq!(d["variants"], json!(["slider", "scrub"]));
    assert_eq!(d["footprints"]["slider"], json!(["row", "chip"]));
    assert_eq!(d["footprints"]["scrub"], json!(["half", "chip"]));
    assert_eq!(d["detents"][0]["value"], json!(0.0));
    assert!(
        d["gestures"]
            .as_array()
            .unwrap()
            .contains(&json!("double-click-reset"))
    );
    assert_eq!(
        d["config_schema"]["properties"]["variant"]["enum"],
        json!(["slider", "scrub"])
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
    bad.variants[0].footprints = vec![Footprint::Block(9)];
    assert!(r.register(bad).is_err(), "a block of nine rows");
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
    assert_eq!(
        c.variants[0].footprint(Surface::Panel),
        Some(Footprint::Row)
    );
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
    assert_eq!(cat[0]["id"], "exposure");
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
