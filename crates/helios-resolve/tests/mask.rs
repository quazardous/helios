//! An AI object mask member answered through resolvers, with no model and
//! no network: a scripted answer, the cache, refusals and failures, an
//! external program, and replay from the book.

use std::sync::Arc;
use std::time::Duration;

use helios_core::*;
use helios_decision::{Book, Mode, Model, Status};
use helios_module_masks::MaskObject;
use helios_module_masks::object::{self, encode_mask};
use helios_resolve::command::Command;
use helios_resolve::*;

const IMAGE: Region = Region {
    x: 0,
    y: 0,
    width: 64,
    height: 48,
};

/// A 16×12 mask: the left half found, the right half not.
fn half() -> Vec<u8> {
    (0..12)
        .flat_map(|_| (0..16).map(|x| if x < 8 { 255 } else { 0 }))
        .collect()
}

fn found(confidence: f64) -> Resolution {
    Resolution {
        options: vec![object::answer(16, 12, &half(), IMAGE, confidence).unwrap()],
        confidence,
        rationale: "the sky is on the left".into(),
        model: Model {
            name: "segmenter".into(),
            version: "1".into(),
        },
    }
}

struct Setup {
    engine: Engine,
    scheduler: Scheduler,
    graph: Graph,
    book: Book,
}

fn setup() -> Setup {
    let bus = Arc::new(Bus::new());
    let mut graph = Graph::new();
    let src = graph.add_source("src", Image::from_fn(IMAGE, 4, |x, _, _| x as f32 / 64.0));
    let sky = graph
        .add_module("sky", ModuleRef::new(MaskObject), &Params::new())
        .unwrap();
    graph
        .connect(src, graph::SOURCE_PORT, sky, "input")
        .unwrap();
    let mut book = Book::new();
    book.declare("sky", Mode::FullAi);
    Setup {
        engine: Engine::new(bus.clone()),
        scheduler: Scheduler::with_resources(bus, 2, &[(AI_CALLS, 1)]),
        graph,
        book,
    }
}

fn settings() -> Settings {
    Settings {
        region: IMAGE,
        scale: 0.25,
        brief: None,
        timeout: Duration::from_secs(10),
        now: 7,
    }
}

fn resolve(s: &mut Setup, resolver: Arc<dyn Resolver>) -> Report {
    resolve_waiting(
        &mut s.book,
        &mut s.graph,
        &s.engine,
        &s.scheduler,
        resolver,
        &settings(),
    )
    .unwrap()
}

fn mask(s: &Setup) -> Result<Image, Error> {
    let sky = s.graph.find("sky").unwrap();
    s.engine.render(&s.graph, sky, "mask", IMAGE)
}

fn ask(s: &mut Setup, kind: &str) {
    s.book
        .pose(&mut s.graph, "sky", "kind", Value::Choice(kind.into()))
        .unwrap();
}

#[test]
fn a_scripted_resolver_gives_the_mask() {
    let mut s = setup();
    assert!(matches!(mask(&s), Err(Error::Pending { .. })));
    let scripted = Arc::new(Scripted::new().answer("mask_object", found(0.9)));
    let report = resolve(&mut s, scripted.clone());
    assert_eq!(report.answered, ["sky"], "{report:?}");
    let m = mask(&s).unwrap();
    assert_eq!(m.pixel(2, 2)[0], 1.0);
    assert_eq!(m.pixel(60, 40)[0], 0.0);
    let answer = s.book.answer("sky").unwrap();
    assert_eq!(answer.values.float("confidence"), 0.9);
    assert_eq!(
        answer.values.get("kind"),
        Some(&Value::Choice("subject".into())),
        "recorded with its question"
    );
    assert_eq!(scripted.calls(), ["sky"]);
}

#[test]
fn the_cache_answers_the_same_question_without_asking_again() {
    let mut s = setup();
    let cached = Arc::new(Cached::new(
        Scripted::new().answer("mask_object", found(0.9)),
    ));
    resolve(&mut s, cached.clone());
    s.book.reask(&mut s.graph, "sky").unwrap();
    let report = resolve(&mut s, cached.clone());
    assert_eq!(report.answered, ["sky"]);
    assert_eq!(cached.inner().calls().len(), 1, "asked once");

    // Another question is asked; the first one again is not.
    ask(&mut s, "sky");
    assert_eq!(s.book.status("sky").unwrap(), Status::Pending);
    resolve(&mut s, cached.clone());
    assert_eq!(cached.inner().calls().len(), 2);
    ask(&mut s, "subject");
    resolve(&mut s, cached.clone());
    assert_eq!(cached.inner().calls().len(), 2);
    assert!(mask(&s).is_ok());
}

#[test]
fn a_refused_answer_leaves_the_mask_as_it_was_and_says_why_by_key() {
    let mut s = setup();
    let refusal = Resolution {
        options: vec![],
        rationale: "no sky in this picture".into(),
        ..found(0.0)
    };
    let report = resolve(
        &mut s,
        Arc::new(Scripted::new().answer("mask_object", refusal)),
    );
    let (node, message) = &report.failed[0];
    assert_eq!(node, "sky");
    assert_eq!(message.key, "resolve-refused");
    assert!(
        message.english.contains("no sky in this picture"),
        "{message}"
    );
    assert_eq!(s.book.status("sky").unwrap(), Status::Pending);
    assert!(s.book.history("sky").is_empty());
}

#[test]
fn a_failed_answer_leaves_the_mask_as_it_was_and_says_why_by_key() {
    let mut s = setup();
    resolve(
        &mut s,
        Arc::new(Scripted::new().answer("mask_object", found(0.9))),
    );
    let sky = s.graph.find("sky").unwrap();
    let before = s.graph.params(sky).unwrap().unwrap();

    // Asked again about the sky; the program answers with no PNG.
    ask(&mut s, "sky");
    let script = r#"cat > /dev/null; echo '{"options":[{"mask":"aGVsbG8="}],"confidence":0.5,"model":{"name":"m","version":"1"}}'"#;
    let report = resolve(&mut s, Arc::new(Command::new("sh").arg("-c").arg(script)));
    let (node, message) = &report.failed[0];
    assert_eq!(node, "sky");
    assert_eq!(message.key, "resolve-failed");
    assert!(message.english.contains("not a PNG"), "{message}");
    assert_eq!(s.book.history("sky").len(), 1, "nothing recorded");
    assert_eq!(s.book.status("sky").unwrap(), Status::Pending);
    let after = s.graph.params(sky).unwrap().unwrap();
    assert_eq!(
        (after.get("mask"), after.get("area")),
        (before.get("mask"), before.get("area")),
        "the mask is unchanged"
    );
}

#[test]
fn an_external_program_answers_a_mask_as_a_png() {
    let mut s = setup();
    ask(&mut s, "prompt");
    s.book
        .pose(
            &mut s.graph,
            "sky",
            "prompt",
            Value::Text("the red car".into()),
        )
        .unwrap();
    let png = encode_mask(16, 12, &half()).unwrap();
    // The program checks the question and the region, then answers.
    let script = format!(
        r#"
in=$(cat)
case "$in" in *'"prompt":"the red car"'*) ;; *) echo "no prompt" >&2; exit 4;; esac
case "$in" in *'"width":64'*) ;; *) echo "no region" >&2; exit 5;; esac
echo '{{"options":[{{"mask":{{"png_base64":"{png}"}}}}],"confidence":0.7,"rationale":"found","model":{{"name":"script","version":"1"}}}}'
"#
    );
    let report = resolve(&mut s, Arc::new(Command::new("sh").arg("-c").arg(&script)));
    assert_eq!(report.answered, ["sky"], "{report:?}");
    let m = mask(&s).unwrap();
    assert_eq!((m.pixel(2, 2)[0], m.pixel(60, 40)[0]), (1.0, 0.0));
    assert_eq!(
        s.book.answer("sky").unwrap().values.float("confidence"),
        0.7
    );
}

#[test]
fn a_named_resolver_answers_the_member_that_names_it() {
    let mut s = setup();
    s.book
        .pose(&mut s.graph, "sky", "resolver", Value::Text("local".into()))
        .unwrap();
    let local = Arc::new(Scripted::new().answer("mask_object", found(0.6)));
    let routed = Routed::new()
        .default_to(Arc::new(Scripted::new()))
        .named("local", local.clone());
    let report = resolve(&mut s, Arc::new(routed));
    assert_eq!(report.answered, ["sky"], "{report:?}");
    assert_eq!(local.calls(), ["sky"]);

    s.book
        .pose(
            &mut s.graph,
            "sky",
            "resolver",
            Value::Text("remote".into()),
        )
        .unwrap();
    let report = resolve(&mut s, Arc::new(Routed::new()));
    assert!(
        report.failed[0]
            .1
            .english
            .contains("no resolver named `remote`"),
        "{report:?}"
    );
}

#[test]
fn a_stored_book_replays_the_mask_offline() {
    let mut s = setup();
    ask(&mut s, "sky");
    resolve(
        &mut s,
        Arc::new(Scripted::new().answer("mask_object", found(0.9))),
    );
    let before = mask(&s).unwrap();
    let stored = serde_json::to_string(&s.book.to_json()).unwrap();

    // Another process: a fresh network, the stored book, no resolver.
    let mut fresh = setup();
    let loaded = Book::from_json(serde_json::from_str(&stored).unwrap()).unwrap();
    assert_eq!(loaded.replay(&mut fresh.graph), Ok(1));
    fresh.book = loaded;
    assert_eq!(mask(&fresh).unwrap().data(), before.data());
    let sky = fresh.graph.find("sky").unwrap();
    assert_eq!(
        fresh.graph.question(sky).unwrap().get("kind"),
        Some(&Value::Choice("sky".into())),
        "the question is replayed with its answer"
    );
}
