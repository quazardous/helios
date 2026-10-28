//! The value store: ranges, steps, gestures, guards, undo, events, the
//! graph it mirrors and the commands over it.

use std::sync::{Arc, Mutex};

use helios_core::{Bus, Engine, Graph, Image, ModuleRef, Params, Region, Value};
use helios_shell::commands::{Origin, Registry};
use helios_shell::when::Context;
use helios_values::*;
use serde_json::json;

fn agent() -> Origin {
    Origin::Agent("a1".into())
}

/// An exposure node over a grey source, its parameters in a store.
fn exposure() -> (Graph, helios_core::NodeId, Store, Arc<Bus>) {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(
            Region::sized(4, 4),
            4,
            |_, _, c| if c == 3 { 1.0 } else { 0.25 },
        ),
    );
    let exp = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    let bus = Arc::new(Bus::new());
    let mut store = Store::new().with_bus(bus.clone());
    assert_eq!(bind_node(&mut store, &g, exp).unwrap(), 3);
    (g, exp, store, bus)
}

fn ev(store: &Store) -> f64 {
    store
        .get("node/exposure/exposure")
        .unwrap()
        .value
        .as_f64()
        .unwrap()
}

#[test]
fn descriptors_come_from_the_manifest_and_its_hints() {
    let (_, _, store, _) = exposure();
    let d = &store.get("node/exposure/exposure").unwrap().descriptor;
    assert_eq!(d.ty, Type::Float);
    assert_eq!(d.hard, Some((-18.0, 18.0)));
    assert_eq!(d.soft, Some((-3.0, 4.0)));
    assert_eq!((d.step, d.coarse), (0.02, 0.25));
    assert_eq!(d.unit.as_deref(), Some("EV"));
    assert_eq!(d.default, Value::Float(0.0));
    let paths: Vec<&str> = store
        .list("node/exposure/")
        .iter()
        .map(|(p, _)| *p)
        .collect();
    assert_eq!(
        paths,
        [
            "node/exposure/black",
            "node/exposure/compensate_bias",
            "node/exposure/exposure"
        ]
    );
}

/// A module's hints say when a slider is logarithmic and when it offers
/// a picker; neither is serialized when off.
#[test]
fn hints_carry_the_logarithmic_bar_and_the_picker() {
    use helios_core::manifest::{ParamHint, ParamSpec};

    let spec = ParamSpec::float("radius", "The radius", 0.0, 100.0, 4.0);
    let hint = ParamHint::new("radius")
        .soft(0.5, 64.0)
        .logarithmic()
        .picker();
    let d = Descriptor::from_spec(&spec, Some(&hint));
    assert!(d.logarithmic && d.picker);
    let plain = Descriptor::from_spec(&spec, None);
    assert!(!plain.logarithmic && !plain.picker);
    let json = serde_json::to_value(&plain).unwrap();
    assert!(json.get("logarithmic").is_none() && json.get("picker").is_none());
}

#[test]
fn writes_are_held_to_the_soft_range_unless_forced_and_say_so() {
    let (_, _, mut store, _) = exposure();
    let path = "node/exposure/exposure";
    let a = store
        .set(Set::new(path, Change::To(Value::Float(6.0)), Origin::User))
        .unwrap();
    assert_eq!(
        (a.value.clone(), a.clamped),
        (Value::Float(4.0), Clamped::Soft)
    );
    let a = store
        .set(Set::new(path, Change::To(Value::Float(6.0)), Origin::User).force())
        .unwrap();
    assert_eq!((a.value, a.clamped), (Value::Float(6.0), Clamped::None));
    let a = store
        .set(Set::new(path, Change::To(Value::Float(40.0)), Origin::User).force())
        .unwrap();
    assert_eq!((a.value, a.clamped), (Value::Float(18.0), Clamped::Hard));
    // Top and bottom go to the soft ends, or the hard ones when forced.
    store
        .set(Set::new(path, Change::Bottom, Origin::User))
        .unwrap();
    assert_eq!(ev(&store), -3.0);
    store
        .set(Set::new(path, Change::Top, Origin::User).force())
        .unwrap();
    assert_eq!(ev(&store), 18.0);
    // A wrong type is refused, with the value it stays at.
    let r = store
        .set(Set::new(path, Change::To(Value::Bool(true)), Origin::User))
        .unwrap_err();
    assert!(
        matches!(r, Refused::Invalid { current: Value::Float(v), .. } if v == 18.0),
        "{r:?}"
    );
}

#[test]
fn steps_move_by_fine_or_coarse_amounts() {
    let (_, _, mut store, _) = exposure();
    let path = "node/exposure/exposure";
    store
        .set(Set::new(path, Change::Step(5), Origin::Midi))
        .unwrap();
    assert!((ev(&store) - 0.1).abs() < 1e-12, "{}", ev(&store));
    store
        .set(Set::new(path, Change::Coarse(-2), Origin::Midi))
        .unwrap();
    assert!((ev(&store) + 0.4).abs() < 1e-12, "{}", ev(&store));
    store
        .set(Set::new(path, Change::Reset, Origin::Midi))
        .unwrap();
    assert_eq!(ev(&store), 0.0);
    // A bool steps by toggling.
    let flag = "node/exposure/compensate_bias";
    store
        .set(Set::new(flag, Change::Step(1), Origin::User))
        .unwrap();
    assert_eq!(store.get(flag).unwrap().value, Value::Bool(true));
}

#[test]
fn an_agent_cannot_move_what_a_hand_is_dragging() {
    let (_, _, mut store, _) = exposure();
    let path = "node/exposure/exposure";
    let drag = store.begin_gesture(Origin::User, "drag exposure");
    for v in [0.5, 0.8, 1.0] {
        store
            .set(Set::new(path, Change::To(Value::Float(v)), Origin::User).gesture(drag))
            .unwrap();
    }
    let r = store
        .set(Set::new(path, Change::To(Value::Float(-1.0)), agent()))
        .unwrap_err();
    assert_eq!(
        r,
        Refused::Held {
            current: Value::Float(1.0),
            by: Origin::User
        }
    );
    // Other values stay free.
    store
        .set(Set::new(
            "node/exposure/black",
            Change::To(Value::Float(0.01)),
            agent(),
        ))
        .unwrap();
    store.end(drag);
    store
        .set(Set::new(path, Change::To(Value::Float(-1.0)), agent()))
        .unwrap();
    assert_eq!(ev(&store), -1.0);
}

#[test]
fn a_writer_can_ask_that_nothing_changed_since_it_read() {
    let (_, _, mut store, _) = exposure();
    let path = "node/exposure/exposure";
    let read = store.get(path).unwrap().seq;
    store
        .set(Set::new(path, Change::To(Value::Float(1.0)), Origin::User))
        .unwrap();
    let r = store
        .set(Set::new(path, Change::To(Value::Float(2.0)), agent()).if_seq(read))
        .unwrap_err();
    assert!(
        matches!(r, Refused::Stale { current: Value::Float(v), .. } if v == 1.0),
        "{r:?}"
    );
    let now = store.get(path).unwrap().seq;
    let a = store
        .set(Set::new(path, Change::To(Value::Float(2.0)), agent()).if_seq(now))
        .unwrap();
    assert!(a.changed && a.seq > now);
}

#[test]
fn a_gesture_or_a_group_is_one_undo_step_and_a_write_another() {
    let (_, _, mut store, _) = exposure();
    let path = "node/exposure/exposure";
    let drag = store.begin_gesture(Origin::User, "drag exposure");
    for v in [0.5, 0.8, 1.0] {
        store
            .set(Set::new(path, Change::To(Value::Float(v)), Origin::User).gesture(drag))
            .unwrap();
    }
    store.end(drag);
    let group = store.begin_group(agent(), "brighten and lift");
    store
        .set(Set::new(path, Change::Step(10), agent()).gesture(group))
        .unwrap();
    store
        .set(
            Set::new(
                "node/exposure/black",
                Change::To(Value::Float(-0.02)),
                agent(),
            )
            .gesture(group),
        )
        .unwrap();
    store.end(group);
    store
        .set(Set::new(path, Change::To(Value::Float(3.0)), Origin::User))
        .unwrap();
    let labels: Vec<&str> = store
        .undo_steps()
        .iter()
        .map(|s| s.label.as_str())
        .collect();
    assert_eq!(
        labels,
        [
            "drag exposure",
            "brighten and lift",
            "set node/exposure/exposure"
        ]
    );
    store.undo(&Origin::User).unwrap();
    store.undo(&Origin::User).unwrap();
    assert_eq!(ev(&store), 1.0);
    assert_eq!(
        store.get("node/exposure/black").unwrap().value,
        Value::Float(0.0)
    );
    // Undo is a write: it carries who asked for it.
    assert_eq!(store.get(path).unwrap().origin, Some(Origin::User));
    assert_eq!(
        store.undo(&Origin::User).unwrap().as_deref(),
        Some("drag exposure")
    );
    assert_eq!(ev(&store), 0.0);
    store.redo(&Origin::User).unwrap();
    assert_eq!(ev(&store), 1.0);
}

#[test]
fn a_step_keeps_the_place_it_was_made_in() {
    let (_, _, mut store, _) = exposure();
    let path = "node/exposure/exposure";
    let place = |frame: &str| Place {
        frame: frame.into(),
        payload: json!(null),
    };
    let s1 = vec![place("helios.library"), place("helios.darkroom")];
    store
        .set(Set::new(path, Change::To(Value::Float(0.5)), Origin::User))
        .unwrap();
    store.set_place(s1.clone());
    // A drag begun in the darkroom is made there, wherever it ends.
    let drag = store.begin_gesture(Origin::User, "drag exposure");
    store
        .set(Set::new(path, Change::To(Value::Float(1.0)), Origin::User).gesture(drag))
        .unwrap();
    store.set_place(Vec::new());
    store.end(drag);
    store
        .set(Set::new(path, Change::To(Value::Float(2.0)), Origin::User))
        .unwrap();
    let places: Vec<&[Place]> = store
        .undo_steps()
        .iter()
        .map(|s| s.place.as_slice())
        .collect();
    assert_eq!(places, [&[][..], &s1[..], &[][..]]);
    // An undo and a redo move the step with its place.
    store.undo(&Origin::User).unwrap();
    store.undo(&Origin::User).unwrap();
    assert_eq!(store.redo_steps().last().unwrap().place, s1);
    store.redo(&Origin::User).unwrap();
    assert_eq!(store.undo_steps().last().unwrap().place, s1);
}

#[test]
fn an_agent_s_request_is_one_attributed_step_and_a_named_one_spans_calls() {
    let (_, _, mut store, _) = exposure();
    let path = "node/exposure/exposure";
    let black = "node/exposure/black";
    let to = |v: f64| Change::To(Value::Float(v));
    // One request: a write and a group of the agent are one step, under
    // the first one's label.
    let g = store.begin_request(agent(), None);
    store.set(Set::new(path, to(0.5), agent())).unwrap();
    let inner = store.begin_group(agent(), "lift");
    store
        .set(Set::new(black, to(-0.01), agent()).gesture(inner))
        .unwrap();
    store.end(inner);
    assert_eq!(store.end_request(), Some(g));
    let last = store.undo_steps().last().unwrap();
    assert_eq!(last.label, "set node/exposure/exposure");
    assert_eq!(last.group, Some(g));
    assert_eq!(last.origin, agent());
    assert_eq!(store.undo_steps().len(), 1);
    assert_eq!(store.group_size(g), 2);
    // A request that changes nothing has no group to name.
    store.begin_request(agent(), None);
    assert_eq!(store.end_request(), None);
    // Two calls under one name, nothing between: one step, labelled with
    // the name.
    let named = store.begin_request(agent(), Some("Retouch skin"));
    store.set(Set::new(path, to(0.7), agent())).unwrap();
    store.end_request();
    assert_eq!(store.begin_request(agent(), Some("Retouch skin")), named);
    store.set(Set::new(black, to(-0.02), agent())).unwrap();
    store.end_request();
    assert_eq!(store.undo_steps().len(), 2);
    assert_eq!(store.undo_steps()[1].changes.len(), 2);
    assert_eq!(store.undo_steps()[1].label, "Retouch skin");
    // Another origin's write during a request is not the agent's.
    store.begin_request(agent(), None);
    store.set(Set::new(path, to(0.9), Origin::User)).unwrap();
    store.end_request();
    let last = store.undo_steps().last().unwrap();
    assert_eq!((last.origin.clone(), last.group), (Origin::User, None));
    // One undo takes the named request back whole.
    store.undo(&Origin::User).unwrap();
    assert_eq!(
        store.undo(&Origin::User).unwrap().as_deref(),
        Some("Retouch skin")
    );
    assert_eq!(ev(&store), 0.5);
    // An agent's write outside any request is a group of its own.
    store.set(Set::new(path, to(1.0), agent())).unwrap();
    assert!(store.undo_steps().last().unwrap().group.is_some());
}

#[test]
fn an_agent_s_group_is_undone_alone_unless_a_later_step_touched_it() {
    let (_, _, mut store, _) = exposure();
    let path = "node/exposure/exposure";
    let black = "node/exposure/black";
    let to = |v: f64| Change::To(Value::Float(v));
    let g = store.begin_request(agent(), Some("brighten"));
    store.set(Set::new(path, to(1.0), agent())).unwrap();
    store.end_request();
    // The user changes something else: the agent's batch is undone
    // alone, the user's change stays, and a redo makes it again.
    store.set(Set::new(black, to(-0.02), Origin::User)).unwrap();
    assert_eq!(store.last_agent_group(None), Some(g));
    assert_eq!(store.last_agent_group(Some("other")), None);
    assert_eq!(
        store.undo_group(g, &Origin::User).unwrap().as_deref(),
        Some("brighten")
    );
    assert_eq!(ev(&store), 0.0);
    assert_eq!(
        store.get(black).unwrap().value,
        Value::Float(-0.02),
        "the user's step stays"
    );
    assert_eq!(store.undo_steps().len(), 1);
    assert_eq!(store.redo_steps().last().unwrap().group, Some(g));
    assert_eq!(store.undo_group(g, &Origin::User).unwrap(), None);
    store.redo(&Origin::User).unwrap();
    assert_eq!(ev(&store), 1.0);
    // The user then moves the agent's value: undoing the batch alone is
    // refused, with what touched it, and nothing changes.
    store.set(Set::new(path, to(2.0), Origin::User)).unwrap();
    let rev = store.history_rev();
    let Err(Refused::Touched { since }) = store.undo_group(g, &Origin::User) else {
        panic!("refused");
    };
    assert_eq!(since, ["set node/exposure/exposure"]);
    assert_eq!(ev(&store), 2.0);
    assert_eq!(store.history_rev(), rev);
}

#[test]
fn a_cancelled_gesture_puts_its_values_back_and_is_no_undo_step() {
    let (_, _, mut store, bus) = exposure();
    let path = "node/exposure/exposure";
    store
        .set(Set::new(path, Change::To(Value::Float(0.5)), Origin::User))
        .unwrap();
    let committed = store.committed();
    let rx = bus.subscribe();
    let drag = store.begin_gesture(Origin::User, "drag exposure");
    for v in [0.8, 1.2] {
        store
            .set(Set::new(path, Change::To(Value::Float(v)), Origin::User).gesture(drag))
            .unwrap();
    }
    assert_eq!(store.cancel(drag), 1);
    assert_eq!(ev(&store), 0.5);
    assert_eq!(store.committed(), committed);
    assert_eq!(store.undo_steps().len(), 1);
    assert!(!store.uncommitted());
    // The value put back is announced, so the graph follows.
    let news: Vec<_> = rx
        .try_iter()
        .filter(|e| e.kind == "param.changed")
        .collect();
    assert_eq!(news.len(), 3, "{news:?}");
    assert!(store.take_dirty().contains(&path.to_string()));
    // The gesture is gone: an agent writes again.
    store
        .set(Set::new(path, Change::To(Value::Float(2.0)), agent()))
        .unwrap();
    assert_eq!(store.cancel(drag), 0);
}

/// What a host that saves the edit watches: a committed step is a write
/// outside a gesture, a gesture's end, an undo or a redo; the writes of a
/// gesture going on are not committed yet.
#[test]
fn committed_steps_are_counted_and_a_gesture_in_progress_is_not_one() {
    let (_, _, mut store, _) = exposure();
    let path = "node/exposure/exposure";
    assert_eq!(store.committed(), 0);
    store
        .set(Set::new(path, Change::To(Value::Float(0.5)), Origin::User))
        .unwrap();
    assert_eq!(
        (store.committed(), store.last_committed()),
        (1, "set node/exposure/exposure")
    );
    let drag = store.begin_gesture(Origin::User, "drag exposure");
    assert!(!store.uncommitted(), "an empty gesture changed nothing");
    store
        .set(Set::new(path, Change::To(Value::Float(0.9)), Origin::User).gesture(drag))
        .unwrap();
    assert_eq!(store.committed(), 1);
    assert!(store.uncommitted());
    assert_eq!(store.open_gesture(), Some("drag exposure"));
    store.end(drag);
    assert!(!store.uncommitted());
    assert_eq!(
        (store.committed(), store.last_committed()),
        (2, "drag exposure")
    );
    store.undo(&Origin::User).unwrap();
    assert_eq!(
        (store.committed(), store.last_committed()),
        (3, "undo drag exposure")
    );
    store.redo(&Origin::User).unwrap();
    assert_eq!(
        (store.committed(), store.last_committed()),
        (4, "redo drag exposure")
    );
    // A write to the same value is no step.
    store
        .set(Set::new(path, Change::To(Value::Float(0.9)), Origin::User))
        .unwrap();
    assert_eq!(store.committed(), 4);
}

#[test]
fn a_transient_value_is_announced_but_never_undone_nor_committed() {
    let (_, _, mut store, bus) = exposure();
    let events = bus.subscribe();
    let zoom = "view/zoom";
    store.declare(zoom, Descriptor::float("zoom", 0.01, 64.0, 1.0).transient());
    store
        .set(Set::new(zoom, Change::To(Value::Float(2.0)), Origin::User))
        .unwrap();
    let pan = store.begin_gesture(Origin::User, "pan");
    store
        .set(Set::new(zoom, Change::To(Value::Float(3.0)), Origin::User).gesture(pan))
        .unwrap();
    // Held by the gesture like any value, though it records nothing.
    assert!(matches!(
        store.set(Set::new(zoom, Change::To(Value::Float(4.0)), agent())),
        Err(Refused::Held { .. })
    ));
    assert!(!store.uncommitted());
    store.end(pan);
    assert_eq!(store.committed(), 0);
    assert!(store.undo_steps().is_empty());
    assert_eq!(store.undo(&Origin::User).unwrap(), None);
    assert_eq!(store.get(zoom).unwrap().value, Value::Float(3.0));
    // Announced all the same, so whoever follows the view sees it.
    let kinds: Vec<_> = events.try_iter().map(|e| e.kind).collect();
    assert_eq!(kinds, ["param.changed", "param.changed"]);
}

#[test]
fn every_change_is_announced_once_and_no_change_is_not() {
    let (_, _, mut store, bus) = exposure();
    let path = "node/exposure/exposure";
    let start = bus.last_seq();
    store
        .set(Set::new(path, Change::To(Value::Float(1.0)), agent()))
        .unwrap();
    let a = store
        .set(Set::new(path, Change::To(Value::Float(1.0)), Origin::User))
        .unwrap();
    assert!(!a.changed);
    let events = bus.since(start);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].kind, "param.changed");
    assert_eq!(
        events[0].payload,
        json!({
            "path": path,
            "old": { "float": 0.0 },
            "new": { "float": 1.0 },
            "origin": { "kind": "agent", "id": "a1" },
            "seq": a.seq,
        })
    );
}

#[test]
fn the_graph_renders_what_the_store_holds() {
    let (mut g, exp, mut store, _) = exposure();
    store
        .set(Set::new(
            "node/exposure/exposure",
            Change::To(Value::Float(1.0)),
            agent(),
        ))
        .unwrap();
    assert_eq!(sync(&mut store, &mut g).unwrap(), 1);
    assert_eq!(sync(&mut store, &mut g).unwrap(), 0, "nothing more to send");
    let engine = Engine::new(Arc::new(Bus::new()));
    let out = engine
        .render(&g, exp, "output", Region::sized(4, 4))
        .unwrap();
    assert!(
        (out.pixel(1, 1)[0] - 0.5).abs() < 1e-6,
        "{:?}",
        out.pixel(1, 1)
    );
}

#[test]
fn a_bypass_is_a_value_the_graph_follows_and_an_undo_step() {
    let (mut g, exp, mut store, _) = exposure();
    let path = bypass_path("exposure");
    assert_eq!(path, "bypass/exposure");
    assert_eq!(store.get(&path).unwrap().value, Value::Bool(false));
    assert!(
        store.get(&bypass_path("source")).is_none(),
        "a source has none"
    );
    store
        .set(Set::new(&path, Change::To(Value::Bool(true)), agent()))
        .unwrap();
    assert_eq!(sync(&mut store, &mut g).unwrap(), 1);
    assert!(g.is_bypassed(exp).unwrap());
    // Bypassed, the node shows its input: the grey source.
    let engine = Engine::new(Arc::new(Bus::new()));
    let r = Region::sized(4, 4);
    let out = engine.render(&g, exp, "output", r).unwrap();
    assert!(
        (out.pixel(1, 1)[0] - 0.25).abs() < 1e-6,
        "{:?}",
        out.pixel(1, 1)
    );
    // One undo step: undone, the graph follows.
    assert_eq!(
        store.undo(&agent()).unwrap().as_deref(),
        Some("set bypass/exposure")
    );
    assert_eq!(sync(&mut store, &mut g).unwrap(), 1);
    assert!(!g.is_bypassed(exp).unwrap());
    // Bound again, the store takes the graph's state.
    g.set_bypass(exp, true).unwrap();
    bind_node(&mut store, &g, exp).unwrap();
    assert_eq!(store.get(&path).unwrap().value, Value::Bool(true));
}

#[test]
fn the_commands_drive_the_store_with_their_origin() {
    let (_, _, store, _) = exposure();
    let store = Arc::new(Mutex::new(store));
    let mut r = Registry::new();
    register(&mut r, store.clone()).unwrap();
    let cx = Context::new();
    let path = "node/exposure/exposure";
    let a = r
        .run(
            "param.set",
            json!({ "path": path, "value": 9 }),
            agent(),
            &cx,
        )
        .unwrap();
    assert_eq!(a["value"], json!(4.0));
    assert_eq!(a["clamped"], json!("soft"));
    let got = r
        .run("param.get", json!({ "path": path }), agent(), &cx)
        .unwrap();
    assert_eq!(got["origin"], json!({ "kind": "agent", "id": "a1" }));
    assert_eq!(got["changed"], json!(true));
    assert_eq!(got["descriptor"]["unit"], json!("EV"));
    let stale = r
        .run(
            "param.set",
            json!({ "path": path, "step": 1, "if_seq": 0 }),
            agent(),
            &cx,
        )
        .unwrap_err();
    assert!(stale.contains("\"refused\":\"stale\""), "{stale}");
    r.run("param.reset", json!({ "path": path }), Origin::User, &cx)
        .unwrap();
    assert_eq!(ev(&store.lock().unwrap()), 0.0);
    let list = r
        .run(
            "param.list",
            json!({ "prefix": "node/exposure/" }),
            agent(),
            &cx,
        )
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 3);
    let undone = r.run("param.undo", json!(null), Origin::User, &cx).unwrap();
    assert_eq!(undone["undone"], json!("set node/exposure/exposure"));
    assert_eq!(ev(&store.lock().unwrap()), 4.0);
}

#[test]
fn a_path_is_a_text_that_says_what_it_names() {
    let d = Descriptor::path("sidecar", PathKind::File, &["XMP"]);
    assert_eq!(d.ty, Type::Text);
    assert_eq!(d.default, Value::Text(String::new()));
    let hint = d.path.as_ref().unwrap();
    assert!(hint.accepts(std::path::Path::new("a/b.xmp")));
    assert!(hint.accepts(std::path::Path::new("b.XMP")));
    assert!(!hint.accepts(std::path::Path::new("b.jpg")));
    let j = serde_json::to_value(&d).unwrap();
    assert_eq!(j["path"], json!({ "kind": "file", "extensions": ["xmp"] }));
    let dir = Descriptor::path("workspace", PathKind::Dir, &[]);
    assert_eq!(
        serde_json::to_value(&dir).unwrap()["path"],
        json!({ "kind": "dir" })
    );
    // Other values say nothing of paths.
    let text = serde_json::to_value(Descriptor::text("name", "")).unwrap();
    assert!(text.get("path").is_none());
}
