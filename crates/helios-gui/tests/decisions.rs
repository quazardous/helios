//! Decision prompts on the desktop: a decision waiting for an answer
//! shows as a card in the Decisions panel and stops the render; Accept,
//! Correct and "accept all above" answer it through the edit's book, and
//! the render goes on; the status bar's badge counts what waits; an
//! agent's answer removes the card.

use std::time::Duration;

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::core::indicator::{self, Tone};
use helios_core::*;
use helios_decision::{Book, Decider, Mode, Model};
use helios_gui::decisions::{MARK, PANEL};
use helios_gui::engine_source::Stats;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

/// A judgment: how much to brighten. The answer is its `gain`.
struct HowBright;

impl Module for HowBright {
    type Plan = f32;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "how_bright".into(),
            version: 1,
            description: "how much to brighten".into(),
            params: vec![ParamSpec::float("gain", "decided factor", 0.0, 4.0, 1.0)],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
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
        let input = inputs[0].sample(output);
        Ok(vec![Image::new(
            output,
            4,
            input.data().iter().map(|v| v * gain).collect(),
        )])
    }
}

fn gain(g: f64) -> Params {
    Params::new().with("gain", Value::Float(g))
}

fn model() -> Model {
    Model {
        name: "judge".into(),
        version: "1".into(),
    }
}

/// A source through decisions named `names`, one after the other; the
/// last one is shown.
fn network(names: &[&str]) -> (Graph, NodeId) {
    let mut g = Graph::new();
    let mut at = g.add_source(
        "source",
        Image::from_fn(Region::sized(64, 48), 4, |x, _, c| match c {
            3 => 1.0,
            _ => 0.1 + x as f32 / 200.0,
        }),
    );
    let mut port = graph::SOURCE_PORT.to_string();
    for name in names {
        let d = g
            .add_module(name, ModuleRef::new(HowBright), &Params::new())
            .unwrap();
        g.connect(at, &port, d, "input").unwrap();
        at = d;
        port = "output".into();
    }
    (g, at)
}

/// A shell showing `names` behind one another, each AI-assisted with a
/// proposal the model is `confidence` sure of, or in human mode for none.
fn shell(decisions: &[(&str, Option<f64>)]) -> Shell {
    let names: Vec<&str> = decisions.iter().map(|(n, _)| *n).collect();
    let (g, shown) = network(&names);
    let mut book = Book::new();
    for (name, confidence) in decisions {
        match confidence {
            Some(c) => {
                book.declare(name, Mode::AiAssisted);
                book.propose(&g, name, vec![gain(2.0), gain(1.5)], model(), vec![], 1)
                    .unwrap();
                book.explain(name, *c, "an overcast sky").unwrap();
            }
            None => book.declare(name, Mode::Human),
        }
    }
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(g, shown, "output").unwrap();
    shell.set_decisions(book).unwrap();
    shell
}

fn harness<'a>(shell: Shell) -> Harness<'a, Shell> {
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h.state_mut()
        .run(&format!("{PANEL}.focus"), json!(null), Origin::User)
        .unwrap();
    h.run();
    h
}

/// Steps frames, waiting after each for the render threads, until `done`
/// holds.
fn until(h: &mut Harness<'_, Shell>, done: impl Fn(&Shell) -> bool) {
    for _ in 0..40 {
        h.step();
        let stats = h.state().viewport_stats();
        assert!(
            stats.wait_idle(Duration::from_mins(1)),
            "the render thread never answered: {stats:?}"
        );
        h.step();
        if done(h.state()) {
            return;
        }
    }
    panic!("never got there: {:?}", h.state().viewport_stats());
}

fn tiles(s: &Shell) -> u64 {
    Stats::get(&s.viewport_stats().tiles)
}

fn failed(s: &Shell) -> u64 {
    Stats::get(&s.viewport_stats().failed)
}

fn badge(h: &Harness<'_, Shell>) -> Option<indicator::Record> {
    indicator::find(&h.ctx, MARK).filter(|r| r.shown)
}

fn decider(s: &Shell, node: &str) -> Decider {
    s.decisions().book().answer(node).unwrap().decider.clone()
}

#[test]
fn a_pending_decision_shows_a_card_and_accept_lets_the_render_through() {
    let mut h = harness(shell(&[("sky", Some(0.9))]));
    // Nothing renders while the decision waits.
    until(&mut h, |s| failed(s) > 0);
    assert_eq!(tiles(h.state()), 0);

    // Its card: the question, the proposal, how sure and why.
    {
        let s = h.state();
        let cards = s.decisions().cards().to_vec();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].node, "sky");
        assert_eq!(cards[0].description, "how much to brighten");
        let proposal = cards[0].proposal.as_ref().unwrap();
        assert_eq!(proposal.confidence, Some(0.9));
        assert_eq!(proposal.rationale, "an overcast sky");
        assert!(s.target_rect("helios.decisions.sky").is_ok());
        assert!(s.target_rect("helios.decisions.sky.accept").is_ok());
    }
    assert!(h.query_by_label_contains("Why: an overcast sky").is_some());

    // Accepted: recorded as the user's, the card goes, the render goes on.
    h.state_mut().decisions_mut().accept("sky", 0);
    h.run();
    assert_eq!(
        decider(h.state(), "sky"),
        Decider::Accepted {
            who: "user".into(),
            model: model()
        }
    );
    assert_eq!(
        h.state().decisions().book().answer("sky").unwrap().values,
        gain(2.0)
    );
    assert!(h.state().decisions().cards().is_empty());
    until(&mut h, |s| tiles(s) > 0);
    assert!(h.query_by_label_contains("No decision waiting").is_some());
}

#[test]
fn correct_records_the_corrected_answer() {
    let mut h = harness(shell(&[("sky", Some(0.6))]));
    // The form opens on the model's best option.
    h.state_mut().decisions_mut().correct("sky").unwrap();
    h.run();
    assert_eq!(h.state().decisions().form("sky"), Some(&gain(2.0)));
    assert!(h.state().target_rect("helios.decisions.sky.gain").is_ok());
    assert!(h.state().target_rect("helios.decisions.sky.record").is_ok());
    assert!(
        h.state_mut()
            .decisions_mut()
            .set_field("sky", "strength", Value::Float(1.0))
            .is_err(),
        "only the answer's fields"
    );
    h.state_mut()
        .decisions_mut()
        .set_field("sky", "gain", Value::Float(3.0))
        .unwrap();
    h.state_mut().decisions_mut().submit("sky").unwrap();
    h.run();
    let answer = h.state().decisions().book().answer("sky").unwrap().clone();
    assert_eq!(answer.values, gain(3.0));
    assert_eq!(
        answer.decider,
        Decider::Corrected {
            who: "user".into(),
            model: model()
        }
    );
    assert!(h.state().decisions().cards().is_empty());
    until(&mut h, |s| tiles(s) > 0);
}

#[test]
fn the_badge_counts_and_accept_all_above_takes_the_sure_ones() {
    let mut h = harness(shell(&[
        ("sure", Some(0.9)),
        ("unsure", Some(0.5)),
        ("by_hand", None),
    ]));
    let b = badge(&h).expect("the badge shows while decisions wait");
    assert_eq!(b.value, Some(3.0));
    assert_eq!(b.tone, Tone::Warning, "they wait for a person");
    assert!(
        h.state()
            .target_rect("helios.decisions.accept_above")
            .is_ok()
    );
    assert_eq!(h.state().decisions().above(0.8), ["sure"]);

    // Above 80 %: only the sure one.
    let t = h.state().decisions().threshold();
    h.state_mut().decisions_mut().accept_above(t);
    h.run();
    h.run();
    assert!(h.state().decisions().book().answer("sure").is_some());
    assert!(h.state().decisions().book().answer("unsure").is_none());
    assert_eq!(badge(&h).unwrap().value, Some(2.0));

    // The same through the command, lower.
    h.state_mut()
        .run(
            "decision.acceptAbove",
            json!({ "confidence": 0.4 }),
            Origin::User,
        )
        .unwrap();
    h.run();
    h.run();
    assert!(h.state().decisions().book().answer("unsure").is_some());
    assert_eq!(badge(&h).unwrap().value, Some(1.0));

    // The one in human mode has no proposal: answered through its form.
    h.state_mut().decisions_mut().correct("by_hand").unwrap();
    assert_eq!(h.state().decisions().form("by_hand"), Some(&gain(1.0)));
    h.state_mut().decisions_mut().submit("by_hand").unwrap();
    h.run();
    h.run();
    assert_eq!(
        decider(h.state(), "by_hand"),
        Decider::Human { who: "user".into() }
    );
    assert_eq!(h.state().decisions().pending(), 0);
    assert!(badge(&h).is_none(), "nothing waits: no badge");
    until(&mut h, |s| tiles(s) > 0);
}

#[test]
fn an_agents_answer_removes_the_card() {
    let mut h = harness(shell(&[("sky", Some(0.7)), ("ground", Some(0.7))]));
    assert_eq!(h.state().decisions().cards().len(), 2);
    h.state_mut()
        .run(
            "decision.decide",
            json!({ "node": "sky", "values": { "gain": 1.25 } }),
            Origin::Agent("helper".into()),
        )
        .unwrap();
    h.state_mut()
        .run(
            "decision.accept",
            json!({ "node": "ground", "option": 1 }),
            Origin::Agent("helper".into()),
        )
        .unwrap();
    h.run();
    h.run();
    assert!(h.state().decisions().cards().is_empty());
    assert_eq!(
        decider(h.state(), "sky"),
        Decider::Corrected {
            who: "helper".into(),
            model: model()
        }
    );
    assert_eq!(
        h.state().decisions().book().answer("sky").unwrap().values,
        gain(1.25)
    );
    assert_eq!(
        h.state()
            .decisions()
            .book()
            .answer("ground")
            .unwrap()
            .values,
        gain(1.5),
        "the second option"
    );
    // A value the answer cannot take is refused, nothing recorded.
    let refused = h.state_mut().answer_decision(
        helios_gui::decisions::Request::Accept {
            node: "sky".into(),
            option: 0,
        },
        &Origin::Agent("helper".into()),
    );
    assert!(refused.is_err(), "nothing left to accept");
    until(&mut h, |s| tiles(s) > 0);
}
