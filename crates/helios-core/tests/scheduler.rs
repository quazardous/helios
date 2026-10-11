//! The scheduler: named jobs, cancellation, idle waits and events, observed
//! without sleeping (jobs are held by channels the test controls).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use helios_core::*;

const LONG: Duration = Duration::from_secs(10);

/// A job that says when it starts and waits to be released.
fn gated() -> (
    Receiver<()>,
    Sender<()>,
    impl FnOnce(&CancelToken) -> Result<(), String> + Send + 'static,
) {
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel::<()>();
    let job = move |_: &CancelToken| {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        Ok(())
    };
    (started_rx, release_tx, job)
}

fn kinds(bus: &Bus) -> Vec<&'static str> {
    bus.since(0).iter().map(|e| e.kind).collect()
}

#[test]
fn jobs_run_and_the_scheduler_becomes_idle() {
    let bus = Arc::new(Bus::new());
    let s = Scheduler::new(bus.clone(), 2);
    assert!(s.is_idle());
    let a = s.submit("a", |_| Ok(()));
    let b = s.submit("b", |_| Err("disk full".into()));
    assert!(s.wait_until_idle(LONG));
    assert_eq!(
        (s.job(a), s.job(b)),
        (Some(JobState::Done), Some(JobState::Failed))
    );
    let d = s.describe();
    assert_eq!(d["totals"]["done"], 1);
    assert_eq!(d["totals"]["failed"], 1);
    assert_eq!(kinds(&bus).last(), Some(&"scheduler.idle"));
    let failed = bus
        .since(0)
        .into_iter()
        .find(|e| e.kind == "scheduler.job_ended" && e.payload["state"] == "failed")
        .unwrap();
    assert_eq!(failed.payload["error"], "disk full");
}

#[test]
fn waiting_for_idle_times_out_while_a_job_runs() {
    let s = Scheduler::new(Arc::new(Bus::new()), 1);
    let (started, release, job) = gated();
    let id = s.submit("held", job);
    started.recv().unwrap();
    assert_eq!(s.job(id), Some(JobState::Running));
    assert!(!s.is_idle());
    assert!(!s.wait_until_idle(Duration::from_millis(20)));
    release.send(()).unwrap();
    assert!(s.wait_until_idle(LONG));
}

#[test]
fn a_queued_job_cancelled_never_starts() {
    let bus = Arc::new(Bus::new());
    let s = Scheduler::new(bus.clone(), 1);
    let (started, release, job) = gated();
    s.submit("held", job);
    started.recv().unwrap();
    let queued = s.submit("never", |_| panic!("a cancelled job must not run"));
    assert_eq!(s.job(queued), Some(JobState::Queued));
    assert!(s.cancel(queued));
    assert_eq!(s.job(queued), Some(JobState::Cancelled));
    release.send(()).unwrap();
    assert!(s.wait_until_idle(LONG));
    assert!(
        !bus.since(0)
            .iter()
            .any(|e| e.kind == "scheduler.job_started" && e.payload["name"] == "never")
    );
}

#[test]
fn a_running_job_sees_its_cancellation() {
    let s = Scheduler::new(Arc::new(Bus::new()), 1);
    let (started_tx, started_rx) = channel();
    let (go_tx, go_rx) = channel::<()>();
    let id = s.submit("cooperative", move |token: &CancelToken| {
        started_tx.send(()).unwrap();
        go_rx.recv().unwrap();
        if token.is_cancelled() {
            Err("stopped".into())
        } else {
            Ok(())
        }
    });
    started_rx.recv().unwrap();
    assert!(s.cancel(id));
    go_tx.send(()).unwrap();
    assert!(s.wait_until_idle(LONG));
    assert_eq!(s.job(id), Some(JobState::Cancelled));
    assert!(!s.cancel(id), "an ended job cannot be cancelled");
}

#[test]
fn events_of_a_job_follow_its_queuing() {
    let bus = Arc::new(Bus::new());
    let s = Scheduler::new(bus.clone(), 1);
    s.submit("one", |_| Ok(()));
    assert!(s.wait_until_idle(LONG));
    let events = bus.since(0);
    assert_eq!(
        kinds(&bus),
        [
            "scheduler.job_queued",
            "scheduler.job_started",
            "scheduler.job_ended",
            "scheduler.idle"
        ]
    );
    assert_eq!(events[1].cause, Some(events[0].seq));
    assert_eq!(events[2].cause, Some(events[0].seq));
}
