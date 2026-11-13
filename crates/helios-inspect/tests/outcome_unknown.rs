//! A request sent whose answer never came is told apart from one that
//! never left: its outcome is unknown, and the client says so rather than
//! "cannot reach the node", so an agent reads the state before resending.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use helios_inspect::rpc::code;
use helios_inspect::{Address, request_within, unanswered};
use serde_json::json;

const DEADLINE: Duration = Duration::from_secs(10);

fn socket(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-unknown-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("node.sock")
}

/// A node that reads one request whole, then closes without answering:
/// it may have run it.
fn reads_then_drops(path: &PathBuf) -> thread::JoinHandle<String> {
    let listener = UnixListener::bind(path).unwrap();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(DEADLINE)).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        line
    })
}

#[test]
fn a_request_sent_and_never_answered_has_an_unknown_outcome() {
    let path = socket("request");
    let node = reads_then_drops(&path);
    let request = json!({ "cmd": "set_param", "node": "exposure", "param": "ev", "value": 1.0 });
    let error = request_within(&Address::Unix(path.clone()), &request, DEADLINE).unwrap_err();
    assert!(
        node.join().unwrap().contains("set_param"),
        "the node read it"
    );
    assert!(unanswered(&error), "sent, then dropped: {error}");
    assert!(
        error.to_string().contains("never resend it blindly"),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_request_that_never_left_is_not_unknown() {
    let path = socket("absent");
    let error = request_within(&Address::Unix(path.clone()), &json!({}), DEADLINE).unwrap_err();
    assert!(!unanswered(&error), "no node there: {error}");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_call_whose_connection_closes_before_its_answer_is_outcome_unknown() {
    let path = socket("call");
    let node = reads_then_drops(&path);
    let client = helios_inspect::client::Client::connect(&Address::Unix(path.clone())).unwrap();
    let pending = client.call("command.run", json!({ "id": "edit.undo" }));
    let error = pending
        .wait_timeout(DEADLINE)
        .expect("answered before the deadline")
        .unwrap_err();
    assert!(
        node.join().unwrap().contains("command.run"),
        "the node read it"
    );
    assert_eq!(error.code, code::OUTCOME_UNKNOWN, "{}", error.message);
    assert!(
        error.message.contains("read the state"),
        "{}",
        error.message
    );
    // The connection is gone now: a new call is not sent, and says so.
    let after = client
        .call("command.run", json!({ "id": "edit.undo" }))
        .wait_timeout(DEADLINE)
        .expect("answered before the deadline")
        .unwrap_err();
    assert_eq!(after.code, code::INTERNAL_ERROR, "{}", after.message);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
