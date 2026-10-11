//! Decision nodes in the core: pending until answered, answered through
//! their own door, rendered from the recorded answer like any parameters.

use std::sync::Arc;

use helios_core::*;

/// A judgment: how much to brighten. The answer is its `gain` parameter.
struct HowBright;

impl Module for HowBright {
    type Plan = f32;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "how_bright".into(),
            version: 1,
            description: "a decided gain".into(),
            params: vec![ParamSpec::float("gain", "decided factor", 0.0, 4.0, 1.0)],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Decision,
            family: Family::Ai,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<f32, String> {
        Ok(params.float("gain") as f32)
    }

    fn process(&self, gain: &f32, inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        Ok(vec![Image::new(
            output,
            1,
            inputs[0].data().iter().map(|v| v * gain).collect(),
        )])
    }
}

/// Source → decision → a second decision reading the first.
fn network() -> (Graph, NodeId, NodeId) {
    let mut g = Graph::new();
    let src = g.add_source(
        "src",
        Image::from_fn(Region::sized(4, 2), 1, |x, _, _| x as f32),
    );
    let first = g
        .add_module("first", ModuleRef::new(HowBright), &Params::new())
        .unwrap();
    let second = g
        .add_module("second", ModuleRef::new(HowBright), &Params::new())
        .unwrap();
    g.connect(src, graph::SOURCE_PORT, first, "input").unwrap();
    g.connect(first, "output", second, "input").unwrap();
    (g, first, second)
}

fn gain(v: f64) -> Params {
    Params::new().with("gain", Value::Float(v))
}

#[test]
fn an_unanswered_decision_is_pending_and_so_is_what_reads_it() {
    let (mut g, first, second) = network();
    g.answer(second, &gain(1.0)).unwrap();
    let e = Engine::new(Arc::new(Bus::new()));
    let r = Region::sized(4, 2);
    assert_eq!(
        e.render(&g, second, "output", r).unwrap_err(),
        Error::Pending {
            node: "first".into()
        }
    );
    assert!(Error::Pending { node: "x".into() }.is_transient());
    let status = e.status(&g, second, r).unwrap();
    let states: Vec<_> = status.iter().map(|s| (s.name.as_str(), s.state)).collect();
    assert_eq!(
        states,
        [
            ("src", NodeState::Idle),
            ("first", NodeState::Pending),
            ("second", NodeState::Idle)
        ],
        "a pending decision reads nothing, computes nothing and keeps nothing downstream"
    );
    assert_eq!(g.describe()["nodes"][1]["answered"], false);

    // Answered: the render goes through, from the recorded answer.
    g.answer(first, &gain(2.0)).unwrap();
    let out = e.render(&g, second, "output", r).unwrap();
    assert_eq!(out.data(), &[0.0, 2.0, 4.0, 6.0, 0.0, 2.0, 4.0, 6.0]);
    assert_eq!(g.describe()["nodes"][1]["answered"], true);

    // Withdrawn: pending again, and the earlier content is not served.
    g.withdraw(first).unwrap();
    assert!(matches!(
        e.render(&g, second, "output", r),
        Err(Error::Pending { .. })
    ));
}

#[test]
fn a_decision_is_answered_not_edited_and_answers_are_checked() {
    let (mut g, first, _) = network();
    let err = g.set_param(first, "gain", Value::Float(2.0)).unwrap_err();
    assert!(err.to_string().contains("answered, not edited"), "{err}");
    let err = g.answer(first, &gain(9.0)).unwrap_err();
    assert!(matches!(err, Error::Param { .. }), "{err}");
    assert_eq!(
        g.describe()["nodes"][1]["answered"],
        false,
        "a refused answer settles nothing"
    );
    let src = g.find("src").unwrap();
    assert!(
        g.answer(src, &gain(1.0)).is_err(),
        "only decisions are answered"
    );
    assert_eq!(g.input_nodes(first).unwrap(), [Some(src)]);
}

#[test]
fn the_same_answer_renders_the_same_content_once() {
    let (mut g, first, second) = network();
    let e = Engine::new(Arc::new(Bus::new()));
    let r = Region::sized(4, 2);
    g.answer(first, &gain(2.0)).unwrap();
    g.answer(second, &gain(0.5)).unwrap();
    let key = e.key(&g, second, r).unwrap();
    e.render(&g, second, "output", r).unwrap();
    let computed = e.counters().computed;
    // Answering again with the same values changes nothing.
    g.answer(first, &gain(2.0)).unwrap();
    assert_eq!(e.key(&g, second, r).unwrap(), key);
    e.render(&g, second, "output", r).unwrap();
    assert_eq!(e.counters().computed, computed);
}
