//! Amend and seal on the image's line (#5258 R7): successive changes to
//! the same module amend the open step until something seals it
//! (darktable's history model).

use crate::*;

const EV: &str = "node/exposure/exposure";
const BLACK: &str = "node/exposure/black";
const SAT: &str = "node/colorbalance/saturation";
const STATE: &str = "workspace/state";

fn store() -> Store {
    let mut s = Store::new();
    s.declare(EV, Descriptor::float("Exposure", -3.0, 3.0, 0.0));
    s.declare(BLACK, Descriptor::float("Black", -1.0, 1.0, 0.0));
    s.declare(SAT, Descriptor::float("Saturation", -1.0, 1.0, 0.0));
    s.declare(STATE, Descriptor::text("workspace", "a"));
    s.set_amend(true);
    s
}

fn set(s: &mut Store, path: &str, v: f64) {
    s.set(Set::new(path, Change::To(Value::Float(v)), Origin::User))
        .unwrap();
}

fn value(s: &Store, path: &str) -> f64 {
    s.get(path).unwrap().value.as_f64().unwrap()
}

#[test]
fn the_same_module_amends_the_open_step() {
    let mut s = store();
    set(&mut s, EV, 1.0);
    assert!(!s.last_amended());
    let committed = s.committed();
    set(&mut s, EV, 2.0);
    set(&mut s, BLACK, 0.5);
    assert_eq!(s.undo_steps().len(), 1, "one step for the module");
    assert!(s.last_amended());
    assert_eq!(s.committed(), committed + 2, "each amend is committed");
    assert_eq!(s.amended_since(), Some(committed), "made at the first");
    s.undo(&Origin::User).unwrap();
    assert_eq!((value(&s, EV), value(&s, BLACK)), (0.0, 0.0));
    s.redo(&Origin::User).unwrap();
    assert_eq!((value(&s, EV), value(&s, BLACK)), (2.0, 0.5));
}

#[test]
fn another_module_seals() {
    let mut s = store();
    set(&mut s, EV, 1.0);
    set(&mut s, SAT, 0.2);
    set(&mut s, EV, 2.0);
    assert_eq!(s.undo_steps().len(), 3);
    s.undo(&Origin::User).unwrap();
    assert_eq!(value(&s, EV), 1.0);
}

#[test]
fn commit_undo_and_other_lines_seal() {
    let mut s = store();
    set(&mut s, EV, 1.0);
    assert!(s.seal(), "a step was open");
    assert!(!s.seal(), "sealed already");
    set(&mut s, EV, 2.0);
    assert_eq!(s.undo_steps().len(), 2, "the commit sealed");
    // An undo seals: the redone step is not open again.
    s.undo(&Origin::User).unwrap();
    s.redo(&Origin::User).unwrap();
    set(&mut s, EV, 3.0);
    assert_eq!(s.undo_steps().len(), 3, "the undo sealed");
    // A step on the workspace's line seals the image's.
    s.set(Set::new(
        STATE,
        Change::To(Value::Text("b".into())),
        Origin::User,
    ))
    .unwrap();
    set(&mut s, EV, -1.0);
    assert_eq!(s.undo_steps().len(), 4, "the workspace's step sealed");
    // The image leaving seals.
    s.park_image("a");
    assert_eq!(s.open_step(), None);
}

#[test]
fn several_modules_or_another_origin_are_sealed_at_once() {
    let mut s = store();
    let g = s.begin_group(Origin::User, "preset");
    s.set(Set::new(EV, Change::To(Value::Float(1.0)), Origin::User).gesture(g))
        .unwrap();
    s.set(Set::new(SAT, Change::To(Value::Float(0.3)), Origin::User).gesture(g))
        .unwrap();
    s.end(g);
    assert_eq!(s.open_step(), None, "a step of two modules is sealed");
    set(&mut s, EV, 2.0);
    s.set(Set::new(
        EV,
        Change::To(Value::Float(2.5)),
        Origin::Agent("a".into()),
    ))
    .unwrap();
    assert_eq!(s.undo_steps().len(), 3, "an agent's change is its own");
}

#[test]
fn off_by_default() {
    let mut s = store();
    s.set_amend(false);
    set(&mut s, EV, 1.0);
    set(&mut s, EV, 2.0);
    assert_eq!(s.undo_steps().len(), 2);
}
