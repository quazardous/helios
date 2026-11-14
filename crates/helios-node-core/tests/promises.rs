//! Promises through the node: started, progressing, settled, cancelled,
//! retried, waited on, and taken up after a stop.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helios_core::Bus;
use helios_core::jobs::{Jobs, Start};
use helios_inspect::client::Client;
use helios_node_core::endpoint::VaultService;
use helios_node_core::promises::Promises;
use helios_vault::promises::{DONE, INTERRUPTED, PROGRESS, State};
use helios_vault::{MemoryStore, Vault};
use helios_vault_sqlite::SqliteStore;
use serde_json::{Value, json};

/// How long a test waits for anything at most.
const DEADLINE: Duration = Duration::from_secs(10);

/// A gate the test opens to let a runner end.
#[derive(Clone, Default)]
struct Gate(Arc<Mutex<bool>>);

impl Gate {
    fn open(&self) {
        *self.0.lock().unwrap() = true;
    }

    fn is_open(&self) -> bool {
        *self.0.lock().unwrap()
    }
}

/// Polls `f` until it holds, at most [`DEADLINE`].
fn until(what: &str, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < DEADLINE, "never: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The node's promises with two kinds: `count` counts to its request's
/// `n` once the gate opens, as a job that can be cancelled; `flaky` fails,
/// retryable, on its first attempt and ends on its second.
fn promises(vault: &Arc<Vault<MemoryStore>>, jobs: &Jobs, gate: &Gate) -> Promises<MemoryStore> {
    let (gate, jobs_run) = (gate.clone(), jobs.clone());
    Promises::new(vault.clone(), jobs.clone())
        .kind("count", false, move |p| {
            let job = jobs_run.start(Start::new(p.owner(), "Count", "test"));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = stop.clone();
            job.on_cancel(move || flag.store(true, Ordering::SeqCst));
            let n = p.request()["n"].as_u64().unwrap_or(1);
            p.progress(0, Some(n), Some("waiting"));
            while !gate.is_open() {
                if stop.load(Ordering::SeqCst) || p.cancel_asked() {
                    job.cancelled();
                    p.cancelled();
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            p.progress(n, Some(n), None);
            job.finished();
            p.done(json!({ "counted": n }));
        })
        .kind("flaky", false, |p| {
            if p.promise().attempts == 1 {
                p.failed("the source went away", true);
            } else {
                p.done(json!("ok"));
            }
        })
}

#[test]
fn a_promise_answers_at_once_then_settles_with_its_result_heard_on_the_bus() {
    let bus = Arc::new(Bus::new());
    let heard = bus.subscribe();
    let vault = Arc::new(Vault::new(MemoryStore::default()).with_bus(bus.clone()));
    let jobs = Jobs::new(bus.clone());
    let gate = Gate::default();
    let promises = Arc::new(promises(&vault, &jobs, &gate));
    let service =
        VaultService::new(vault.clone(), bus.clone(), jobs, "test").with_promises(promises.clone());
    let client = Client::in_process(Arc::new(service)).with_origin("agent:test");

    let p = promises
        .start("count", "Count to 3", json!({ "n": 3 }))
        .unwrap();
    assert_eq!(p.owner, format!("count-{}", p.id));
    assert!(
        promises.start("nothing", "x", json!(null)).is_err(),
        "an unknown kind"
    );
    // A wait shorter than the work answers at its timeout, unsettled.
    let waited = client
        .request("promises.wait", json!({ "id": p.id, "timeout_ms": 50 }))
        .unwrap();
    assert_eq!(waited["settled"], false);
    gate.open();
    let waited = client
        .request(
            "promises.wait",
            json!({ "id": p.id, "timeout_ms": DEADLINE.as_millis() as u64 }),
        )
        .unwrap();
    assert_eq!(waited["settled"], true, "{waited}");
    assert_eq!(waited["promise"]["state"], "done");
    assert_eq!(waited["promise"]["result"], json!({ "counted": 3 }));
    let got = client
        .request("promises.get", json!({ "id": p.id }))
        .unwrap();
    assert_eq!(got["promise"], waited["promise"]);
    let listed = client
        .request("promises.list", json!({ "states": ["done"] }))
        .unwrap();
    assert_eq!(listed["total"], 1);
    assert!(
        client
            .request(
                "promises.wait",
                json!({ "id": p.id, "timeout_ms": 700_000 })
            )
            .is_err(),
        "over the wait cap"
    );
    // Its life on the bus: progress while unsettled, done once.
    let events: Vec<_> = std::iter::from_fn(|| heard.try_recv().ok())
        .filter(|e| e.payload["id"] == p.id.as_str())
        .collect();
    let states: Vec<(&str, &str)> = events
        .iter()
        .map(|e| (e.kind, e.payload["state"].as_str().unwrap_or("")))
        .collect();
    assert_eq!(states.first(), Some(&(PROGRESS, "pending")), "{states:?}");
    assert!(states.contains(&(PROGRESS, "running")), "{states:?}");
    assert_eq!(states.last(), Some(&(DONE, "done")), "{states:?}");
    assert_eq!(states.iter().filter(|(k, _)| *k == DONE).count(), 1);
}

#[test]
fn a_cancel_stops_its_job_and_the_runner_settles_it_cancelled() {
    let bus = Arc::new(Bus::new());
    let vault = Arc::new(Vault::new(MemoryStore::default()).with_bus(bus.clone()));
    let jobs = Jobs::new(bus.clone());
    let promises = Arc::new(promises(&vault, &jobs, &Gate::default()));
    let service = VaultService::new(vault.clone(), bus.clone(), jobs.clone(), "test")
        .with_promises(promises.clone());
    let client = Client::in_process(Arc::new(service)).with_origin("cli");
    let p = promises.start("count", "Count", json!({ "n": 2 })).unwrap();
    until("its job runs", || jobs.cancellable().contains(&p.owner));
    let asked = client
        .request("promises.cancel", json!({ "id": p.id }))
        .unwrap();
    assert_eq!(asked["promise"]["cancel_asked"], true);
    let waited = client
        .request(
            "promises.wait",
            json!({ "id": p.id, "timeout_ms": DEADLINE.as_millis() as u64 }),
        )
        .unwrap();
    assert_eq!(waited["promise"]["state"], "cancelled", "{waited}");
    until("its job ended", || !jobs.cancellable().contains(&p.owner));
    assert!(
        client
            .request("promises.cancel", json!({ "id": p.id }))
            .is_err(),
        "a settled promise"
    );
    // One with no runner here is settled at once.
    let orphan = vault
        .create_promise("count", "count-x", "Orphan", Value::Null, 1)
        .unwrap();
    let cancelled = client
        .request("promises.cancel", json!({ "id": orphan.id }))
        .unwrap();
    assert_eq!(cancelled["promise"]["state"], "cancelled");
}

#[test]
fn a_retryable_failure_is_retried_and_one_for_good_is_refused() {
    let bus = Arc::new(Bus::new());
    let vault = Arc::new(Vault::new(MemoryStore::default()).with_bus(bus.clone()));
    let jobs = Jobs::new(bus.clone());
    let promises = Arc::new(promises(&vault, &jobs, &Gate::default()));
    let service =
        VaultService::new(vault.clone(), bus.clone(), jobs, "test").with_promises(promises.clone());
    let client = Client::in_process(Arc::new(service)).with_origin("cli");
    let wait = |id: &str| {
        client
            .request(
                "promises.wait",
                json!({ "id": id, "timeout_ms": DEADLINE.as_millis() as u64 }),
            )
            .unwrap()["promise"]
            .clone()
    };
    let p = promises.start("flaky", "Flaky", Value::Null).unwrap();
    let failed = wait(&p.id);
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["retryable"], true);
    let again = client
        .request("promises.retry", json!({ "id": p.id }))
        .unwrap();
    assert_eq!(again["promise"]["attempts"], 2);
    let done = wait(&p.id);
    assert_eq!(done["state"], "done", "{done}");
    assert!(
        client
            .request("promises.retry", json!({ "id": p.id }))
            .is_err()
    );
    let lost = vault
        .create_promise("flaky", "f", "Lost", Value::Null, 1)
        .unwrap();
    vault
        .settle_promise(
            &lost.id,
            State::Failed {
                reason: "gone".into(),
                retryable: false,
            },
            2,
        )
        .unwrap();
    assert!(
        client
            .request("promises.retry", json!({ "id": lost.id }))
            .is_err()
    );
}

#[test]
fn after_a_stop_resumable_promises_run_again_and_the_others_fail_interrupted() {
    let file = std::env::temp_dir().join(format!(
        "helios-promises-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&file);
    // A node stops with three promises unsettled.
    let (resumable, other, unknown) = {
        let vault = Vault::new(SqliteStore::open(&file).unwrap());
        let a = vault
            .create_promise("resume", "r", "Resumable", json!({ "n": 1 }), 1)
            .unwrap();
        vault.progress_promise(&a.id, 1, Some(4), None, 2).unwrap();
        let b = vault
            .create_promise("again", "g", "Not resumable", Value::Null, 1)
            .unwrap();
        let c = vault
            .create_promise("gone", "x", "Unknown kind", Value::Null, 1)
            .unwrap();
        (a.id, b.id, c.id)
    };
    // The next start takes them up.
    let vault = Arc::new(Vault::new(SqliteStore::open(&file).unwrap()));
    let bus = Arc::new(Bus::new());
    let resumed_runs = Arc::new(AtomicU32::new(0));
    let runs = resumed_runs.clone();
    let promises = Promises::new(vault.clone(), Jobs::new(bus))
        .kind("resume", true, move |p| {
            assert!(p.resumed());
            runs.fetch_add(1, Ordering::SeqCst);
            p.done(json!({ "from": 1 }));
        })
        .kind("again", false, |p| p.done(Value::Null));
    assert_eq!(promises.recover().unwrap(), [resumable.clone()]);
    let p = vault.wait_promise(&resumable, DEADLINE, &|| false).unwrap();
    assert_eq!(p.state.name(), "done");
    assert_eq!(p.attempts, 2);
    assert_eq!(resumed_runs.load(Ordering::SeqCst), 1);
    let other = vault.promise(&other).unwrap().unwrap();
    assert_eq!(
        other.state,
        State::Failed {
            reason: INTERRUPTED.into(),
            retryable: true
        }
    );
    let unknown = vault.promise(&unknown).unwrap().unwrap();
    assert_eq!(
        unknown.state,
        State::Failed {
            reason: INTERRUPTED.into(),
            retryable: false
        }
    );
    // A second recovery finds nothing left to take up.
    assert!(promises.recover().unwrap().is_empty());
    drop(promises);
    drop(vault);
    let _ = std::fs::remove_file(&file);
}
