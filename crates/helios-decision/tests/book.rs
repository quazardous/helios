//! The decision book with recorded answers only: no model is called. Each
//! mode resolves through its own path, answers are versioned and replayed,
//! and nothing a refused answer carries is recorded.

use std::sync::Arc;

use helios_config::{Config, Layer, Schema};
use helios_core::*;
use helios_decision::*;

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

const REGION: Region = Region {
    x: 0,
    y: 0,
    width: 4,
    height: 1,
};

fn network(pixels: [f32; 4]) -> Graph {
    let mut g = Graph::new();
    let src = g.add_source(
        "src",
        Image::from_fn(REGION, 1, |x, _, _| pixels[x as usize]),
    );
    let sky = g
        .add_module("sky", ModuleRef::new(HowBright), &Params::new())
        .unwrap();
    g.connect(src, graph::SOURCE_PORT, sky, "input").unwrap();
    g
}

fn gain(v: f64) -> Params {
    Params::new().with("gain", Value::Float(v))
}

fn model() -> Model {
    Model {
        name: "judge".into(),
        version: "2026-10".into(),
    }
}

fn render(e: &Engine, g: &Graph) -> Result<Vec<f32>, Error> {
    let sky = g.find("sky").unwrap();
    e.render(g, sky, "output", REGION)
        .map(|i| i.data().to_vec())
}

fn kinds(bus: &Bus) -> Vec<&'static str> {
    bus.since(0).iter().map(|e| e.kind).collect()
}

#[test]
fn full_ai_answers_and_the_render_goes_on() {
    let bus = Arc::new(Bus::new());
    let e = Engine::new(bus.clone());
    let mut g = network([1.0, 2.0, 3.0, 4.0]);
    let mut book = Book::new().with_bus(bus.clone());
    book.declare("sky", Mode::FullAi);
    assert_eq!(book.waiting(), [("sky", Status::Pending)]);
    assert!(matches!(render(&e, &g), Err(Error::Pending { .. })));

    let inputs = input_keys(&e, &g, "sky", REGION).unwrap();
    let status = book
        .model_answer(&mut g, "sky", gain(2.0), model(), inputs.clone(), 100)
        .unwrap();
    assert_eq!(status, Status::Answered);
    assert_eq!(render(&e, &g).unwrap(), [2.0, 4.0, 6.0, 8.0]);
    let answer = book.answer("sky").unwrap();
    assert_eq!(
        (answer.version, &answer.decider, &answer.inputs, answer.at),
        (1, &Decider::Model { model: model() }, &inputs, 100)
    );
    assert!(book.waiting().is_empty());
    assert_eq!(
        kinds(&bus)
            .into_iter()
            .filter(|k| k.starts_with("decision."))
            .collect::<Vec<_>>(),
        ["decision.pending", "decision.answered"]
    );

    // The human reviews and overrides: a new version, by a human.
    let v = book
        .decide(&mut g, "sky", "ana", gain(1.5), inputs, 200)
        .unwrap();
    assert_eq!(v, 2);
    assert_eq!(
        book.answer("sky").unwrap().decider,
        Decider::Human { who: "ana".into() }
    );
    assert_eq!(render(&e, &g).unwrap(), [1.5, 3.0, 4.5, 6.0]);
}

#[test]
fn ai_assisted_proposes_and_a_human_accepts_or_corrects() {
    let e = Engine::new(Arc::new(Bus::new()));
    let mut g = network([1.0, 2.0, 3.0, 4.0]);
    let mut book = Book::new();
    book.declare("sky", Mode::AiAssisted);

    // A model answer is only a proposal here.
    let status = book
        .model_answer(&mut g, "sky", gain(3.0), model(), vec![], 1)
        .unwrap();
    assert_eq!(status, Status::Proposed);
    assert!(
        matches!(render(&e, &g), Err(Error::Pending { .. })),
        "nothing renders before the human"
    );
    assert!(
        book.propose(&g, "sky", vec![gain(1.0), gain(9.0)], model(), vec![], 2)
            .unwrap_err()
            .contains("outside"),
        "options are checked when proposed"
    );
    assert_eq!(
        book.proposal("sky").unwrap().options,
        [gain(3.0)],
        "a refused proposal changes nothing"
    );

    book.propose(&g, "sky", vec![gain(1.0), gain(2.0)], model(), vec![], 3)
        .unwrap();
    assert!(
        book.accept(&mut g, "sky", "ana", 5, 4)
            .unwrap_err()
            .contains("no option 5")
    );
    book.accept(&mut g, "sky", "ana", 1, 4).unwrap();
    assert_eq!(
        book.answer("sky").unwrap().decider,
        Decider::Accepted {
            who: "ana".into(),
            model: model()
        }
    );
    assert_eq!(render(&e, &g).unwrap(), [2.0, 4.0, 6.0, 8.0]);
    assert!(book.proposal("sky").is_none());

    // Asked again: the model proposes, the human corrects.
    book.reask(&mut g, "sky").unwrap();
    assert!(matches!(render(&e, &g), Err(Error::Pending { .. })));
    book.propose(&g, "sky", vec![gain(1.0)], model(), vec![], 5)
        .unwrap();
    let v = book
        .decide(&mut g, "sky", "ana", gain(0.5), vec![], 6)
        .unwrap();
    assert_eq!(v, 2);
    assert_eq!(
        book.answer("sky").unwrap().decider,
        Decider::Corrected {
            who: "ana".into(),
            model: model()
        }
    );
    assert_eq!(
        book.history("sky").len(),
        2,
        "asking again rewrites nothing"
    );
}

#[test]
fn a_proposal_keeps_how_sure_the_model_is_and_why() {
    let mut g = network([1.0; 4]);
    let mut book = Book::new();
    book.declare("sky", Mode::AiAssisted);
    assert!(
        book.explain("sky", 0.5, "nothing proposed")
            .unwrap_err()
            .contains("no proposal")
    );
    book.model_answer(&mut g, "sky", gain(2.0), model(), vec![], 1)
        .unwrap();
    assert_eq!(book.proposal("sky").unwrap().confidence, None);
    book.explain("sky", 1.5, "an overcast sky").unwrap();
    let proposal = book.proposal("sky").unwrap();
    assert_eq!(proposal.confidence, Some(1.0), "clamped to 0..1");
    assert_eq!(proposal.rationale, "an overcast sky");
    // Kept with the edit.
    let back = Book::from_json(book.to_json()).unwrap();
    assert_eq!(back.proposal("sky"), book.proposal("sky"));
}

#[test]
fn human_mode_takes_no_model_answer() {
    let mut g = network([1.0; 4]);
    let mut book = Book::new();
    book.declare("sky", Mode::Human);
    let err = book
        .model_answer(&mut g, "sky", gain(2.0), model(), vec![], 1)
        .unwrap_err();
    assert!(err.contains("decided by a human"), "{err}");
    assert!(
        book.propose(&g, "sky", vec![gain(2.0)], model(), vec![], 1)
            .is_err()
    );
    let err = book
        .decide(&mut g, "sky", "ana", gain(99.0), vec![], 1)
        .unwrap_err();
    assert!(err.contains("outside"), "{err}");
    assert!(
        book.history("sky").is_empty(),
        "a refused answer is not recorded"
    );
    assert_eq!(
        book.decide(&mut g, "sky", "ana", gain(2.0), vec![], 2),
        Ok(1)
    );
    assert!(
        book.decide(&mut g, "nope", "ana", gain(2.0), vec![], 2)
            .is_err()
    );
}

#[test]
fn a_stored_book_replays_offline_into_a_fresh_network() {
    let e = Engine::new(Arc::new(Bus::new()));
    let mut g = network([1.0, 2.0, 3.0, 4.0]);
    let mut book = Book::new();
    book.declare("sky", Mode::FullAi);
    book.model_answer(&mut g, "sky", gain(2.0), model(), vec![], 1)
        .unwrap();
    let before = render(&e, &g).unwrap();
    let stored = serde_json::to_string(&book.to_json()).unwrap();

    // Another process: a fresh graph, the stored book, no model.
    let mut fresh = network([1.0, 2.0, 3.0, 4.0]);
    let loaded = Book::from_json(serde_json::from_str(&stored).unwrap()).unwrap();
    assert_eq!(loaded.replay(&mut fresh), Ok(1));
    assert_eq!(render(&e, &fresh).unwrap(), before);
    assert_eq!(loaded.describe()["nodes"]["sky"]["status"], "answered");
    assert_eq!(loaded.describe()["nodes"]["sky"]["answer"]["by"], "model");

    // A book whose answer is withdrawn replays as pending.
    let mut withdrawn = loaded.clone();
    withdrawn.reask(&mut fresh, "sky").unwrap();
    let mut third = network([1.0, 2.0, 3.0, 4.0]);
    third
        .answer(third.find("sky").unwrap(), &gain(3.0))
        .unwrap();
    assert_eq!(withdrawn.replay(&mut third), Ok(0));
    assert!(matches!(render(&e, &third), Err(Error::Pending { .. })));
}

#[test]
fn an_answer_given_on_other_inputs_is_flagged_not_redone() {
    let e = Engine::new(Arc::new(Bus::new()));
    let mut g = network([1.0, 2.0, 3.0, 4.0]);
    let mut book = Book::new();
    book.declare("sky", Mode::FullAi);
    let seen = input_keys(&e, &g, "sky", REGION).unwrap();
    book.model_answer(&mut g, "sky", gain(2.0), model(), seen.clone(), 1)
        .unwrap();
    assert!(!book.is_stale("sky", &seen));

    // Same edit on a darker image: the answer still renders, flagged for review.
    let mut other = network([0.1, 0.2, 0.3, 0.4]);
    book.replay(&mut other).unwrap();
    let now = input_keys(&e, &other, "sky", REGION).unwrap();
    assert!(book.is_stale("sky", &now));
    assert!(render(&e, &other).is_ok());
}

/// A judgment with a question: what to judge, set like a parameter.
struct GainOf;

impl Module for GainOf {
    type Plan = f32;

    fn manifest(&self) -> Manifest {
        let mut m = HowBright.manifest();
        m.name = "gain_of".into();
        m.params.push(ParamSpec {
            name: "subject".into(),
            description: "what to judge".into(),
            kind: ParamKind::Text {
                capacity: 32,
                default: "sky".into(),
            },
        });
        m
    }

    fn question(&self) -> Vec<String> {
        vec!["subject".into()]
    }

    fn prepare(&self, params: &Params, c: &Context) -> Result<f32, String> {
        HowBright.prepare(params, c)
    }

    fn process(&self, gain: &f32, inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        HowBright.process(gain, inputs, output)
    }
}

fn questioned() -> Graph {
    let mut g = Graph::new();
    let src = g.add_source("src", Image::from_fn(REGION, 1, |x, _, _| x as f32));
    let sky = g
        .add_module("sky", ModuleRef::new(GainOf), &Params::new())
        .unwrap();
    g.connect(src, graph::SOURCE_PORT, sky, "input").unwrap();
    g
}

#[test]
fn an_answer_holds_for_its_question_and_replays_with_it() {
    let e = Engine::new(Arc::new(Bus::new()));
    let text = |t: &str| Value::Text(t.into());
    let mut g = questioned();
    let mut book = Book::new();
    book.declare("sky", Mode::FullAi);
    book.pose(&mut g, "sky", "subject", text("face")).unwrap();
    book.model_answer(&mut g, "sky", gain(2.0), model(), vec![], 1)
        .unwrap();
    assert_eq!(
        book.answer("sky").unwrap().values.get("subject"),
        Some(&text("face")),
        "the answer is recorded with its question"
    );

    // Replayed into a fresh graph: the question first, then the answer.
    let stored = book.to_json();
    let mut fresh = questioned();
    let loaded = Book::from_json(stored).unwrap();
    assert_eq!(loaded.replay(&mut fresh), Ok(1));
    let sky = fresh.find("sky").unwrap();
    assert_eq!(
        fresh.question(sky).unwrap().get("subject"),
        Some(&text("face"))
    );
    assert_eq!(render(&e, &fresh).unwrap(), [0.0, 2.0, 4.0, 6.0]);

    // The question changed on the graph (a surface): pending again.
    let id = g.find("sky").unwrap();
    g.set_param(id, "subject", text("skin")).unwrap();
    assert_eq!(book.follow(&g), ["sky"]);
    assert_eq!(book.status("sky").unwrap(), Status::Pending);
    assert_eq!(
        book.question("sky").unwrap().get("subject"),
        Some(&text("skin"))
    );
    assert_eq!(
        book.history("sky").len(),
        1,
        "the answer stays in the history"
    );
    assert!(book.follow(&g).is_empty());
    // The same question through the book changes nothing.
    book.model_answer(&mut g, "sky", gain(1.0), model(), vec![], 2)
        .unwrap();
    book.pose(&mut g, "sky", "subject", text("skin")).unwrap();
    assert_eq!(book.status("sky").unwrap(), Status::Answered);
}

#[test]
fn a_project_or_a_run_overrides_the_node_mode() {
    let mut config = Config::new(Schema::core());
    // Nothing set: the node's own mode, else the default (AI-assisted).
    assert_eq!(effective_mode(Some(Mode::Human), &config), Ok(Mode::Human));
    assert_eq!(effective_mode(None, &config), Ok(Mode::AiAssisted));
    // A global preference does not override a node that says otherwise.
    config
        .set_str(Layer::Global, MODE_SETTING, "full_ai")
        .unwrap();
    assert_eq!(effective_mode(Some(Mode::Human), &config), Ok(Mode::Human));
    assert_eq!(effective_mode(None, &config), Ok(Mode::FullAi));
    // A project does, and a run over it.
    config
        .set_str(Layer::Project, MODE_SETTING, "ai_assisted")
        .unwrap();
    assert_eq!(
        effective_mode(Some(Mode::Human), &config),
        Ok(Mode::AiAssisted)
    );
    config.set_str(Layer::Run, MODE_SETTING, "full_ai").unwrap();
    assert_eq!(effective_mode(Some(Mode::Human), &config), Ok(Mode::FullAi));
    assert!(
        Mode::parse("robot")
            .unwrap_err()
            .contains("full_ai, ai_assisted, human")
    );
}
