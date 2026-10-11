//! Resolvers behind decision nodes, with no network: scripted answers, the
//! ai_calls limit, the cache, an external program.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helios_core::*;
use helios_decision::{Book, Mode, Model, Status};
use helios_resolve::command::Command;
use helios_resolve::*;

/// A judgment: how much to brighten. The answer is its `gain` parameter.
struct HowBright;

impl Module for HowBright {
    type Plan = f32;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "how_bright".into(),
            version: 1,
            description: "how much to brighten".into(),
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
    width: 8,
    height: 4,
};

/// A source feeding `n` decision nodes named d0, d1…
fn network(n: usize) -> Graph {
    let mut g = Graph::new();
    let src = g.add_source(
        "src",
        Image::from_fn(REGION, 1, |x, y, _| (x + y) as f32 / 10.0),
    );
    for i in 0..n {
        let d = g
            .add_module(&format!("d{i}"), ModuleRef::new(HowBright), &Params::new())
            .unwrap();
        g.connect(src, graph::SOURCE_PORT, d, "input").unwrap();
    }
    g
}

fn resolution(gain: f64) -> Resolution {
    Resolution {
        options: vec![
            Params::new().with("gain", Value::Float(gain)),
            Params::new().with("gain", Value::Float(gain / 2.0)),
        ],
        confidence: 0.8,
        rationale: "dark scene".into(),
        model: Model {
            name: "judge".into(),
            version: "1".into(),
        },
    }
}

fn settings() -> Settings {
    Settings {
        region: REGION,
        scale: 0.5,
        brief: Some("bright and airy".into()),
        timeout: Duration::from_secs(10),
        now: 42,
    }
}

struct Setup {
    engine: Engine,
    scheduler: Scheduler,
    graph: Graph,
    book: Book,
}

fn setup(n: usize, mode: Mode, ai_calls: u64) -> Setup {
    let bus = Arc::new(Bus::new());
    let mut book = Book::new();
    for i in 0..n {
        book.declare(&format!("d{i}"), mode);
    }
    Setup {
        engine: Engine::new(bus.clone()),
        scheduler: Scheduler::with_resources(bus, 4, &[(AI_CALLS, ai_calls)]),
        graph: network(n),
        book,
    }
}

fn render(s: &Setup, node: &str) -> Result<Image, Error> {
    let id = s.graph.find(node).unwrap();
    s.engine.render(&s.graph, id, "output", REGION)
}

#[test]
fn full_ai_answers_every_waiting_decision_and_the_render_goes_on() {
    let mut s = setup(2, Mode::FullAi, 1);
    assert!(matches!(render(&s, "d0"), Err(Error::Pending { .. })));
    let scripted = Arc::new(Scripted::new().answer("how_bright", resolution(2.0)));
    let report = resolve_waiting(
        &mut s.book,
        &mut s.graph,
        &s.engine,
        &s.scheduler,
        scripted.clone(),
        &settings(),
    )
    .unwrap();
    let mut answered = report.answered.clone();
    answered.sort();
    assert_eq!(answered, ["d0", "d1"]);
    assert!(report.failed.is_empty() && report.proposed.is_empty());
    let out = render(&s, "d1").unwrap();
    assert_eq!(out.pixel(3, 2)[0], 1.0);
    assert_eq!(s.book.answer("d0").unwrap().at, 42);
    assert_eq!(scripted.calls().len(), 2);
}

#[test]
fn ai_assisted_proposes_and_waits_for_a_human() {
    let mut s = setup(1, Mode::AiAssisted, 1);
    let report = resolve_waiting(
        &mut s.book,
        &mut s.graph,
        &s.engine,
        &s.scheduler,
        Arc::new(Scripted::new().answer("how_bright", resolution(2.0))),
        &settings(),
    )
    .unwrap();
    assert_eq!(report.proposed, ["d0"]);
    assert_eq!(s.book.status("d0").unwrap(), Status::Proposed);
    assert!(matches!(render(&s, "d0"), Err(Error::Pending { .. })));
    // The human takes the second option.
    s.book.accept(&mut s.graph, "d0", "ana", 1, 50).unwrap();
    assert_eq!(render(&s, "d0").unwrap().pixel(3, 2)[0], 0.5);
}

#[test]
fn human_decisions_are_not_asked() {
    let mut s = setup(1, Mode::Human, 1);
    let scripted = Arc::new(Scripted::new().answer("how_bright", resolution(2.0)));
    let report = resolve_waiting(
        &mut s.book,
        &mut s.graph,
        &s.engine,
        &s.scheduler,
        scripted.clone(),
        &settings(),
    )
    .unwrap();
    assert_eq!(report, Report::default());
    assert!(scripted.calls().is_empty());
}

#[test]
fn a_failed_call_leaves_the_decision_waiting_and_says_why() {
    let mut s = setup(1, Mode::FullAi, 1);
    let report = resolve_waiting(
        &mut s.book,
        &mut s.graph,
        &s.engine,
        &s.scheduler,
        Arc::new(Scripted::new()),
        &settings(),
    )
    .unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(
        report.failed[0].1.contains("no scripted answer"),
        "{report:?}"
    );
    assert_eq!(s.book.status("d0").unwrap(), Status::Pending);
}

#[test]
fn the_same_pixels_are_not_asked_about_twice() {
    let mut s = setup(1, Mode::FullAi, 1);
    let cached = Arc::new(Cached::new(
        Scripted::new().answer("how_bright", resolution(2.0)),
    ));
    for _ in 0..2 {
        resolve_waiting(
            &mut s.book,
            &mut s.graph,
            &s.engine,
            &s.scheduler,
            cached.clone(),
            &settings(),
        )
        .unwrap();
        s.book.reask(&mut s.graph, "d0").unwrap();
    }
    assert_eq!(cached.inner().calls().len(), 1);
}

/// Counts how many calls run at the same time.
struct Slow {
    running: AtomicUsize,
    most: Mutex<usize>,
}

impl Resolver for Slow {
    fn resolve(&self, _: &Request) -> Result<Resolution, String> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut most = self.most.lock().unwrap();
            *most = (*most).max(now);
        }
        std::thread::sleep(Duration::from_millis(60));
        self.running.fetch_sub(1, Ordering::SeqCst);
        Ok(resolution(1.0))
    }
}

#[test]
fn the_ai_calls_limit_decides_how_many_run_at_once() {
    for (limit, expected) in [(1, 1), (3, 3)] {
        let mut s = setup(4, Mode::FullAi, limit);
        let slow = Arc::new(Slow {
            running: AtomicUsize::new(0),
            most: Mutex::new(0),
        });
        let report = resolve_waiting(
            &mut s.book,
            &mut s.graph,
            &s.engine,
            &s.scheduler,
            slow.clone(),
            &settings(),
        )
        .unwrap();
        assert_eq!(report.answered.len(), 4);
        assert_eq!(*slow.most.lock().unwrap(), expected, "limit {limit}");
    }
}

#[test]
fn an_external_program_answers_in_json() {
    // The program checks it got a PNG it can read, then answers.
    let script = r#"
in=$(cat)
png=$(printf '%s' "$in" | sed -n 's/.*"png":"\([^"]*\)".*/\1/p')
[ -f "$png" ] || { echo "no image at '$png'" >&2; exit 3; }
case "$in" in *'"brief":"bright and airy"'*) ;; *) echo "no brief" >&2; exit 4;; esac
echo '{"options":[{"gain":3}],"confidence":0.7,"rationale":"low key","model":{"name":"script","version":"1"}}'
"#;
    let mut s = setup(1, Mode::FullAi, 1);
    let report = resolve_waiting(
        &mut s.book,
        &mut s.graph,
        &s.engine,
        &s.scheduler,
        Arc::new(Command::new("sh").arg("-c").arg(script)),
        &settings(),
    )
    .unwrap();
    assert_eq!(report.answered, ["d0"], "{report:?}");
    assert_eq!(s.book.answer("d0").unwrap().values.float("gain"), 3.0);
    assert_eq!(render(&s, "d0").unwrap().pixel(3, 2)[0], 1.5);
}

#[test]
fn an_answer_outside_its_declarations_is_refused() {
    let script = r#"cat > /dev/null; echo '{"options":[{"brightness":3}],"model":{"name":"m","version":"1"}}'"#;
    let mut s = setup(1, Mode::FullAi, 1);
    let report = resolve_waiting(
        &mut s.book,
        &mut s.graph,
        &s.engine,
        &s.scheduler,
        Arc::new(Command::new("sh").arg("-c").arg(script)),
        &settings(),
    )
    .unwrap();
    assert!(
        report.failed[0].1.contains("no parameter `brightness`"),
        "{report:?}"
    );
}
