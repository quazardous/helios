//! The inspection endpoint in JSON-RPC 2.0, over a real socket: calls,
//! coded errors, several calls and event notifications on one connection,
//! cancellation, the older form still answered, and `node.hello`.

#![cfg(unix)]

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use helios_core::{Bus, Describe, Event};
use helios_inspect::rpc::{self, Call, Service, code};
use helios_inspect::{Address, Client, Inspector, Listen, serve, serve_rpc_at};
use serde::Serialize;
use serde_json::{Value, json};

struct Counter;

impl Describe for Counter {
    fn describe(&self) -> Value {
        json!({ "count": 7 })
    }
}

#[derive(Serialize)]
struct Tick {
    n: u32,
}

impl Event for Tick {
    const KIND: &'static str = "test.tick";
}

fn socket(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("helios-jsonrpc-{name}-{}.sock", std::process::id()))
}

fn start(name: &str) -> (Arc<Bus>, Inspector, Client) {
    let bus = Arc::new(Bus::new());
    let inspector = Inspector::new(bus.clone()).with("counter", Arc::new(Counter));
    let path = serve(inspector.clone(), &socket(name)).unwrap();
    (bus, inspector, connect(path))
}

fn connect(path: PathBuf) -> Client {
    let client = Client::connect(&Address::Unix(path)).unwrap();
    // A test never hangs on a missing line.
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    client
}

fn call(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// Reads lines until the answer to `id`, keeping the others in `seen`.
fn answer(client: &mut Client, id: i64, seen: &mut Vec<Value>) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let line = client.read().unwrap();
        if line["id"] == id {
            return line;
        }
        seen.push(line);
    }
    panic!("no answer to {id}; saw {seen:?}");
}

#[test]
fn a_call_answers_its_result() {
    let (_bus, _inspector, mut client) = start("call");
    let line = client.request(&call(1, "state", json!({}))).unwrap();
    assert_eq!(line["jsonrpc"], "2.0");
    assert_eq!(line["id"], 1);
    assert_eq!(line["result"]["state"]["counter"]["count"], 7);
    assert!(line["result"].get("ok").is_none(), "{line}");
    let named = client
        .request(&json!({ "jsonrpc": "2.0", "id": "two", "method": "describe", "params": { "entity": "counter" } }))
        .unwrap();
    assert_eq!(named["id"], "two");
    assert_eq!(named["result"]["describe"]["count"], 7);
}

#[test]
fn errors_carry_a_code() {
    let (_bus, _inspector, mut client) = start("errors");
    let unknown = client.request(&call(1, "nope", json!({}))).unwrap();
    assert_eq!(unknown["error"]["code"], code::METHOD_NOT_FOUND);

    let params = client
        .request(&call(2, "describe", json!({ "entity": 3 })))
        .unwrap();
    assert_eq!(params["error"]["code"], code::INVALID_PARAMS, "{params}");

    let refused = client
        .request(&call(3, "describe", json!({ "entity": "nope" })))
        .unwrap();
    assert_eq!(refused["error"]["code"], code::REFUSED);
    assert_eq!(refused["error"]["message"], "no entity `nope`");
    assert_eq!(
        refused["error"]["data"]["entities"],
        json!(["bus", "counter"])
    );

    let timed = client
        .request(&call(
            4,
            "wait_event",
            json!({ "kind": "never", "since": 0, "timeout_ms": 50 }),
        ))
        .unwrap();
    assert_eq!(timed["error"]["code"], code::TIMED_OUT, "{timed}");

    client.send(&json!("not a request")).unwrap();
    // A string is not JSON-RPC: the older form answers it.
    assert_eq!(client.read().unwrap()["ok"], false);
    let batch = client
        .request(&json!([call(5, "state", json!({}))]))
        .unwrap();
    assert_eq!(batch["error"]["code"], code::INVALID_REQUEST);
    let version = client
        .request(&json!({ "jsonrpc": "1.0", "id": 6, "method": "state" }))
        .unwrap();
    assert_eq!(
        (version["id"].clone(), version["error"]["code"].clone()),
        (json!(6), json!(code::INVALID_REQUEST))
    );
}

#[test]
fn a_line_that_is_not_json_is_a_parse_error() {
    let path = socket("parse");
    let bus = Arc::new(Bus::new());
    serve(Inspector::new(bus), &path).unwrap();
    let mut stream = std::os::unix::net::UnixStream::connect(&path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    writeln!(stream, r#"{{"jsonrpc": "2.0", "id": 1, "method""#).unwrap();
    let mut line = String::new();
    std::io::BufReader::new(stream)
        .read_line(&mut line)
        .unwrap();
    let line: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(line["error"]["code"], code::PARSE_ERROR, "{line}");
    assert_eq!(line["id"], Value::Null);
}

#[test]
fn calls_and_events_share_one_connection() {
    let (bus, _inspector, mut client) = start("shared");
    let mut seen = Vec::new();
    client
        .send(&call(1, "events.subscribe", json!({ "prefix": "test." })))
        .unwrap();
    let subscribed = answer(&mut client, 1, &mut seen);
    let subscription = subscribed["result"]["subscription"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(subscribed["result"]["last_seq"], 0);

    // A wait still running does not hold the next call back.
    client
        .send(&call(
            2,
            "wait_event",
            json!({ "kind": "test.tick", "since": 0, "timeout_ms": 10_000 }),
        ))
        .unwrap();
    client.send(&call(3, "state", json!({}))).unwrap();
    let state = answer(&mut client, 3, &mut seen);
    assert_eq!(state["result"]["state"]["counter"]["count"], 7);

    bus.publish("test", None, &Tick { n: 1 });
    let waited = answer(&mut client, 2, &mut seen);
    assert_eq!(waited["result"]["event"]["kind"], "test.tick");

    // The event arrives as a notification, before or after the answer.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !seen.iter().any(|l| l["method"] == "events.event") && Instant::now() < deadline {
        seen.push(client.read().unwrap());
    }
    let event = seen
        .iter()
        .find(|l| l["method"] == "events.event")
        .expect("an event notification");
    assert!(event.get("id").is_none(), "{event}");
    assert_eq!(event["params"]["subscription"], subscription.as_str());
    assert_eq!(event["params"]["event"]["payload"]["n"], 1);

    let gone = client
        .request(&call(
            4,
            "events.unsubscribe",
            json!({ "subscription": subscription }),
        ))
        .unwrap();
    let gone = if gone["id"] == 4 {
        gone
    } else {
        answer(&mut client, 4, &mut seen)
    };
    assert_eq!(gone["result"]["unsubscribed"], true);
}

#[test]
fn a_wait_is_cancelled() {
    let (_bus, _inspector, mut client) = start("cancel");
    let mut seen = Vec::new();
    client
        .send(&call(
            1,
            "wait_event",
            json!({ "kind": "never", "since": 0, "timeout_ms": 60_000 }),
        ))
        .unwrap();
    // Cancel until the wait is seen running: the call starts on its own thread.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut id = 100;
    loop {
        id += 1;
        client
            .send(&call(id, "$/cancel", json!({ "id": 1 })))
            .unwrap();
        let answered = answer(&mut client, id, &mut seen);
        if answered["result"]["cancelled"] == true {
            break;
        }
        assert!(Instant::now() < deadline, "the wait was never running");
    }
    let started = Instant::now();
    let cancelled = answer(&mut client, 1, &mut seen);
    assert_eq!(cancelled["error"]["code"], code::CANCELLED, "{cancelled}");
    assert!(started.elapsed() < Duration::from_secs(5));

    let none = client
        .request(&call(200, "$/cancel", json!({ "id": 1 })))
        .unwrap();
    assert_eq!(none["result"]["cancelled"], false);
}

/// A service whose `slow` call runs to its end.
struct Slow;

impl Service for Slow {
    fn call(&self, method: &str, _: Value, _: &Call) -> Result<Value, rpc::Error> {
        match method {
            "slow" => {
                std::thread::sleep(Duration::from_millis(500));
                Ok(json!("done"))
            }
            other => Err(rpc::Error::method_not_found(other)),
        }
    }
}

#[test]
fn a_call_that_is_not_cancellable_says_so() {
    let path = socket("slow");
    serve_rpc_at(
        &Listen::Unix(path.clone()),
        None,
        |_: Value, _: &mut helios_inspect::Connection<'_>| Some(json!({ "ok": false })),
        Arc::new(Slow),
    )
    .unwrap();
    let mut client = connect(path);
    let mut seen = Vec::new();
    client.send(&call(1, "slow", json!({}))).unwrap();
    client
        .send(&call(2, "$/cancel", json!({ "id": 1 })))
        .unwrap();
    let refused = answer(&mut client, 2, &mut seen);
    // The cancel may come before the call is registered: then nothing runs yet.
    if refused.get("error").is_some() {
        assert_eq!(refused["error"]["code"], code::NOT_CANCELLABLE);
    }
    assert_eq!(answer(&mut client, 1, &mut seen)["result"], "done");
}

#[test]
fn the_older_form_is_still_answered_on_the_same_connection() {
    let (_bus, _inspector, mut client) = start("older");
    let old = client.request(&json!({ "cmd": "state" })).unwrap();
    assert_eq!(old["ok"], true);
    assert_eq!(old["state"]["counter"]["count"], 7);
    let new = client.request(&call(1, "state", json!({}))).unwrap();
    assert_eq!(new["result"]["state"]["counter"]["count"], 7);
    let again = client
        .request(&json!({ "cmd": "describe", "entity": "counter" }))
        .unwrap();
    assert_eq!(again["describe"]["count"], 7);
}

#[test]
fn hello_says_what_the_endpoint_is() {
    let (_bus, inspector, mut client) = start("hello");
    let hello = client.request(&call(1, "node.hello", json!({}))).unwrap();
    let hello = &hello["result"];
    assert_eq!(hello["protocol"], rpc::PROTOCOL);
    assert_eq!(hello["oldest"], rpc::OLDEST);
    assert_eq!(hello["instance"], inspector.instance());
    assert_eq!(hello["endpoint"], "inspect");
    assert_eq!(hello["permissions"], json!(["inspect"]));
    assert!(hello["server"]["version"].is_string());

    let list = client
        .request(&call(2, "commands.list", json!({})))
        .unwrap();
    let ids: Vec<&str> = list["result"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["id"].as_str())
        .collect();
    for method in ["node.hello", "state", "wait_event", "events.subscribe"] {
        assert!(ids.contains(&method), "{method} in {ids:?}");
    }
}
