//! Faults injected on purpose: they fail once, are not cached, and reach
//! whatever depends on them; and a paused scheduler starts nothing.

use std::sync::Arc;
use std::time::Duration;

use helios_core::*;

struct Double;

impl Module for Double {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "double".into(),
            version: 1,
            description: String::new(),
            params: vec![],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Tone,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        Ok(vec![Image::new(
            output,
            1,
            inputs[0].data().iter().map(|v| v * 2.0).collect(),
        )])
    }
}

const R: Region = Region::sized(4, 2);

fn network() -> (Graph, NodeId, NodeId) {
    let mut g = Graph::new();
    let src = g.add_source("src", Image::from_fn(R, 1, |x, _, _| x as f32));
    let double = g
        .add_module("double", ModuleRef::new(Double), &Params::new())
        .unwrap();
    g.connect(src, "image", double, "input").unwrap();
    (g, src, double)
}

#[test]
fn a_compute_fault_fails_once_and_is_not_cached() {
    let (g, _, double) = network();
    let e = Engine::new(Arc::new(Bus::new()));
    e.inject_fault(&g, double, FaultKind::Compute);
    let err = e.render(&g, double, "output", R).unwrap_err();
    assert_eq!(err.to_string(), "double: injected fault (computation)");
    assert!(err.is_transient());
    let image = e.render(&g, double, "output", R).unwrap();
    assert_eq!(image.data(), &[0.0, 2.0, 4.0, 6.0, 0.0, 2.0, 4.0, 6.0]);
}

#[test]
fn a_source_fault_reaches_what_depends_on_it() {
    let (g, src, double) = network();
    let e = Engine::new(Arc::new(Bus::new()));
    e.inject_fault(&g, src, FaultKind::SourceRead);
    assert!(matches!(
        e.render(&g, double, "output", R),
        Err(Error::Fault { node, .. }) if node == "src"
    ));
    assert!(
        e.render(&g, double, "output", R).is_ok(),
        "nothing poisoned the cache"
    );
    let failed: Vec<_> = e
        .bus()
        .since(0)
        .into_iter()
        .filter(|ev| ev.kind == "engine.step_computed" && ev.payload["ok"] == false)
        .map(|ev| ev.payload["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(failed, ["src", "double"], "every failed step is an event");
}

#[test]
fn a_paused_scheduler_starts_nothing_until_resumed() {
    let bus = Arc::new(Bus::new());
    let s = Scheduler::new(bus.clone(), 2);
    s.pause();
    let job = s.submit("waits", |_| Ok(()));
    assert!(!s.wait_until_idle(Duration::from_millis(20)));
    assert_eq!(s.job(job), Some(JobState::Queued));
    assert_eq!(s.describe()["paused"], true);
    s.resume();
    assert!(s.wait_until_idle(Duration::from_secs(10)));
    let paused: Vec<_> = bus
        .since(0)
        .into_iter()
        .filter(|e| e.kind == "scheduler.paused")
        .map(|e| e.payload["paused"].as_bool().unwrap())
        .collect();
    assert_eq!(paused, [true, false]);
}
