//! Request ids over a real socket: a change sent again with the same key
//! (`_meta.request_id`), on a new connection as after an unknown outcome,
//! runs once, and the second answer says it was already applied. The
//! store's bound and expiry are the unit tests of `request_ids`.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use helios_inspect::client::Client;
use helios_inspect::rpc::{self, Call, Service, code};
use helios_inspect::{Address, Connection, Listen, request_within, serve_rpc_at};
use serde_json::{Value, json};

const DEADLINE: Duration = Duration::from_secs(10);

/// Counts the changes it applies, in both forms; `refuse` is refused.
#[derive(Default)]
struct Counter {
    runs: AtomicU32,
}

impl Counter {
    fn apply(&self, slow: bool) -> u32 {
        if slow {
            std::thread::sleep(Duration::from_millis(300));
        }
        self.runs.fetch_add(1, Ordering::SeqCst) + 1
    }
}

impl Service for Counter {
    fn call(&self, method: &str, params: Value, _: &Call) -> Result<Value, rpc::Error> {
        match method {
            "add" | "command.run" => Ok(json!({ "runs": self.apply(params["slow"] == true) })),
            "refuse" => {
                self.apply(false);
                Err(rpc::Error::new(code::REFUSED, "refused"))
            }
            other => Err(rpc::Error::method_not_found(other)),
        }
    }
}

fn serve(name: &str) -> (Arc<Counter>, Address) {
    let path: PathBuf =
        std::env::temp_dir().join(format!("helios-keys-{name}-{}.sock", std::process::id()));
    let counter = Arc::new(Counter::default());
    let older = counter.clone();
    serve_rpc_at(
        &Listen::Unix(path.clone()),
        None,
        move |request: Value, _: &mut Connection<'_>| {
            Some(match request["cmd"].as_str() {
                Some("add") => json!({ "ok": true, "runs": older.apply(false) }),
                _ => json!({ "ok": false, "error": "refused", "runs": older.apply(false) }),
            })
        },
        counter.clone(),
    )
    .unwrap();
    (counter, Address::Unix(path))
}

/// `method` keyed `key`, on a connection of its own.
fn keyed(address: &Address, method: &str, params: Value, key: &str) -> Result<Value, rpc::Error> {
    Client::connect(address)
        .unwrap()
        .call_keyed(method, params, key)
        .wait_timeout(DEADLINE)
        .expect("answered before the deadline")
}

#[test]
fn a_call_sent_again_with_its_key_runs_once_and_says_already_applied() {
    let (counter, address) = serve("rpc");
    let first = keyed(&address, "add", json!({ "n": 1 }), "k1").unwrap();
    assert_eq!(
        first,
        json!({ "runs": 1 }),
        "the first answer is the call's own"
    );
    let again = keyed(&address, "add", json!({ "n": 1 }), "k1").unwrap();
    assert_eq!(again, json!({ "runs": 1, "already_applied": true }));
    assert_eq!(counter.runs.load(Ordering::SeqCst), 1, "it ran once");

    // Another key is another call; a key given to another call is refused.
    assert_eq!(
        keyed(&address, "add", json!({ "n": 1 }), "k2").unwrap()["runs"],
        2
    );
    let reused = keyed(&address, "add", json!({ "n": 2 }), "k1").unwrap_err();
    assert_eq!(reused.code, code::INVALID_REQUEST, "{}", reused.message);
    assert_eq!(counter.runs.load(Ordering::SeqCst), 2);
}

#[test]
fn a_refused_call_is_not_remembered() {
    let (counter, address) = serve("refused");
    for _ in 0..2 {
        let error = keyed(&address, "refuse", json!({}), "r").unwrap_err();
        assert_eq!(error.code, code::REFUSED);
    }
    assert_eq!(counter.runs.load(Ordering::SeqCst), 2, "it changed nothing");
}

#[test]
fn a_key_sent_again_while_its_call_runs_waits_for_it() {
    let (counter, address) = serve("running");
    let params = json!({ "slow": true });
    let (a, b) = (address.clone(), params.clone());
    let first = std::thread::spawn(move || keyed(&a, "add", b, "slow").unwrap());
    std::thread::sleep(Duration::from_millis(100));
    let again = keyed(&address, "add", params, "slow").unwrap();
    assert_eq!(first.join().unwrap(), json!({ "runs": 1 }));
    assert_eq!(again, json!({ "runs": 1, "already_applied": true }));
    assert_eq!(counter.runs.load(Ordering::SeqCst), 1);
}

#[test]
fn the_older_form_takes_a_key_too() {
    let (counter, address) = serve("older");
    let request = json!({ "cmd": "add", "_meta": { "request_id": "o1" } });
    let first = request_within(&address, &request, DEADLINE).unwrap();
    assert_eq!(first, json!({ "ok": true, "runs": 1 }));
    let again = request_within(&address, &request, DEADLINE).unwrap();
    assert_eq!(
        again,
        json!({ "ok": true, "runs": 1, "already_applied": true })
    );
    // An answer that is not `ok` is not remembered.
    let refused = json!({ "cmd": "nope", "_meta": { "request_id": "o2" } });
    for _ in 0..2 {
        assert_eq!(
            request_within(&address, &refused, DEADLINE).unwrap()["ok"],
            false
        );
    }
    assert_eq!(counter.runs.load(Ordering::SeqCst), 3);
    let bad = json!({ "cmd": "add", "_meta": { "request_id": 7 } });
    let error = request_within(&address, &bad, DEADLINE).unwrap();
    assert_eq!(error["ok"], false, "{error}");
    assert_eq!(counter.runs.load(Ordering::SeqCst), 3);
}

#[test]
fn a_batch_step_sent_again_with_its_key_runs_once() {
    let (counter, address) = serve("batch");
    let batch = |steps: Value| {
        Client::connect(&address)
            .unwrap()
            .call("command.batch", json!({ "steps": steps }))
            .wait_timeout(DEADLINE)
            .expect("answered before the deadline")
            .unwrap()
    };
    let first = batch(json!([{ "id": "a", "request_id": "s1" }, { "id": "b" }]));
    assert_eq!(first["ran"], 2, "{first}");
    assert_eq!(first["steps"][0]["request_id"], "s1");
    // A later batch, as after an outcome unknown: the keyed step is
    // applied once, the unkeyed one runs again.
    let again = batch(json!([{ "id": "a", "request_id": "s1" }, { "id": "b" }]));
    assert_eq!(again["steps"][0]["already_applied"], true, "{again}");
    assert_eq!(again["steps"][0]["answer"]["runs"], 1);
    assert_eq!(again["steps"][1]["answer"]["runs"], 3);
    assert_eq!(counter.runs.load(Ordering::SeqCst), 3);
}
