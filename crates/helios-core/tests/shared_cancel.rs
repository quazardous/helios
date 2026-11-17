//! A computation shared by several askers (the viewport's proxy, the
//! preview, a warm-up) must not be lost to the one that cancels: it runs
//! until every asker cancelled, and a cancelled computation is never
//! cached as an answer.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use helios_core::*;

/// Stands for the demosaic: long, checks the current cancel token between
/// bands, gives up with [`cancel::CANCELLED`]. It runs until `go` is set
/// (or it is cancelled), so a test can join it while it runs.
struct Slow {
    calls: Arc<AtomicUsize>,
    started: Arc<Barrier>,
    go: Arc<AtomicBool>,
}

impl Module for Slow {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "slow".into(),
            version: 1,
            description: "a cancellable demosaic stand-in".into(),
            params: vec![],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Detail,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        if first {
            self.started.wait();
        }
        let token = cancel::current();
        let deadline = Instant::now() + Duration::from_secs(20);
        while first && !self.go.load(Ordering::SeqCst) {
            if token.is_cancelled() {
                return Err(cancel::CANCELLED.to_string());
            }
            assert!(Instant::now() < deadline, "never released");
            thread::sleep(Duration::from_millis(1));
        }
        Ok(vec![inputs[0].sample(output)])
    }
}

struct Setup {
    engine: Arc<Engine>,
    graph: Arc<Graph>,
    node: NodeId,
    calls: Arc<AtomicUsize>,
    started: Arc<Barrier>,
    go: Arc<AtomicBool>,
}

fn setup() -> Setup {
    let calls = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Barrier::new(2));
    let go = Arc::new(AtomicBool::new(false));
    let mut g = Graph::new();
    let src = g.add_source(
        "src",
        Image::from_fn(Region::sized(64, 48), 1, |x, y, _| (x + y) as f32),
    );
    let node = g
        .add_module(
            "slow",
            ModuleRef::new(Slow {
                calls: calls.clone(),
                started: started.clone(),
                go: go.clone(),
            }),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", node, "input").unwrap();
    Setup {
        engine: Arc::new(Engine::new(Arc::new(Bus::new()))),
        graph: Arc::new(g),
        node,
        calls,
        started,
        go,
    }
}

const REGION: Region = Region::sized(64, 48);

/// Waits until the engine counted `n` step requests.
fn requests_reach(engine: &Engine, n: u64) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while engine.counters().requests < n {
        assert!(Instant::now() < deadline, "requests never reached {n}");
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_cancelled_asker_does_not_cancel_the_one_sharing_its_computation() {
    let s = setup();
    let first = cancel::Token::new();
    let (e, g, node, token) = (s.engine.clone(), s.graph.clone(), s.node, first.clone());
    let a = thread::spawn(move || cancel::scope(&token, || e.render(&g, node, "output", REGION)));
    s.started.wait();
    // The first asker's computation runs (its step and its source asked).
    // A second asker, with no token, joins it.
    let before = s.engine.counters().requests;
    let (e, g, node) = (s.engine.clone(), s.graph.clone(), s.node);
    let b = thread::spawn(move || e.render(&g, node, "output", REGION));
    requests_reach(&s.engine, before + 1);
    // Let the join land inside the in-flight cell, then cancel the first.
    thread::sleep(Duration::from_millis(50));
    first.cancel();
    thread::sleep(Duration::from_millis(50));
    s.go.store(true, Ordering::SeqCst);
    let second = b.join().unwrap();
    let _ = a.join().unwrap();
    let image = second.expect("the second asker gets the result");
    assert_eq!(image.region(), REGION);
    assert_eq!(
        s.calls.load(Ordering::SeqCst),
        1,
        "one computation, kept alive by the asker that did not cancel"
    );
}

#[test]
fn a_cancelled_computation_is_computed_again_on_the_next_ask() {
    let s = setup();
    let token = cancel::Token::new();
    let (e, g, node, t) = (s.engine.clone(), s.graph.clone(), s.node, token.clone());
    let a = thread::spawn(move || cancel::scope(&t, || e.render(&g, node, "output", REGION)));
    s.started.wait();
    token.cancel();
    let err = a.join().unwrap().expect_err("the only asker cancelled");
    assert!(err.is_cancelled(), "{err:?}");
    assert!(err.is_transient(), "a cancel is not an answer: {err:?}");
    s.go.store(true, Ordering::SeqCst);
    let image = s
        .engine
        .render(&s.graph, s.node, "output", REGION)
        .expect("asked again, it is computed again");
    assert_eq!(image.region(), REGION);
    assert_eq!(s.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn every_asker_cancelled_stops_the_computation() {
    let s = setup();
    let (one, two) = (cancel::Token::new(), cancel::Token::new());
    let (e, g, node, t) = (s.engine.clone(), s.graph.clone(), s.node, one.clone());
    let a = thread::spawn(move || cancel::scope(&t, || e.render(&g, node, "output", REGION)));
    s.started.wait();
    let before = s.engine.counters().requests;
    let (e, g, node, t) = (s.engine.clone(), s.graph.clone(), s.node, two.clone());
    let b = thread::spawn(move || cancel::scope(&t, || e.render(&g, node, "output", REGION)));
    requests_reach(&s.engine, before + 1);
    thread::sleep(Duration::from_millis(50));
    one.cancel();
    thread::sleep(Duration::from_millis(50));
    two.cancel();
    // `go` is never set: only the cancels can end it.
    for asker in [a, b] {
        let err = asker.join().unwrap().expect_err("all askers cancelled");
        assert!(err.is_cancelled(), "{err:?}");
    }
    assert_eq!(s.calls.load(Ordering::SeqCst), 1);
}
