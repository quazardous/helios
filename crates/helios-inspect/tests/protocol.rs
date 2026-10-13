//! The inspection protocol, over a real socket.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use helios_core::{Bus, Describe, Event};
use helios_inspect::{Inspector, Request, request, serve};
use serde::Serialize;
use serde_json::{Value, json};

struct Counter(u32);

impl Describe for Counter {
    fn describe(&self) -> Value {
        json!({ "count": self.0 })
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
    std::env::temp_dir().join(format!("helios-inspect-{name}-{}.sock", std::process::id()))
}

fn start(name: &str) -> (Arc<Bus>, PathBuf) {
    let bus = Arc::new(Bus::new());
    let inspector = Inspector::new(bus.clone()).with("counter", Arc::new(Counter(7)));
    let path = serve(inspector, &socket(name)).unwrap();
    (bus, path)
}

#[test]
fn state_and_describe() {
    let (_bus, path) = start("state");
    let state = request(&path, &Request::State).unwrap();
    assert_eq!(state["ok"], true);
    assert_eq!(state["state"]["counter"]["count"], 7);
    assert_eq!(state["state"]["bus"]["last_seq"], 0);

    let one = request(
        &path,
        &Request::Describe {
            entity: "counter".into(),
        },
    )
    .unwrap();
    assert_eq!(one["describe"]["count"], 7);

    let unknown = request(
        &path,
        &Request::Describe {
            entity: "nope".into(),
        },
    )
    .unwrap();
    assert_eq!(unknown["ok"], false);
    assert_eq!(
        unknown["entities"],
        json!(["bus", "counter"]),
        "a miss lists what exists"
    );
}

#[test]
fn events_since() {
    let (bus, path) = start("events");
    bus.publish("test", None, &Tick { n: 1 });
    let second = bus.publish("test", None, &Tick { n: 2 });
    let events = request(&path, &Request::Events { since: 1 }).unwrap();
    let events = events["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["seq"], second);
    assert_eq!(events[0]["payload"]["n"], 2);
}

#[test]
fn wait_event_finds_past_and_future_events_and_explains_timeouts() {
    let (bus, path) = start("wait");
    bus.publish("test", None, &Tick { n: 1 });
    let past = request(
        &path,
        &Request::WaitEvent {
            kind: "test.tick".into(),
            since: 0,
            timeout_ms: 1000,
        },
    )
    .unwrap();
    assert_eq!(past["event"]["payload"]["n"], 1);

    // A waiter for an event published later, from another thread.
    let waiter = {
        let path = path.clone();
        thread::spawn(move || {
            request(
                &path,
                &Request::WaitEvent {
                    kind: "test.tick".into(),
                    since: 1,
                    timeout_ms: 5000,
                },
            )
            .unwrap()
        })
    };
    bus.publish("test", None, &Tick { n: 2 });
    assert_eq!(waiter.join().unwrap()["event"]["payload"]["n"], 2);

    let missed = request(
        &path,
        &Request::WaitEvent {
            kind: "test.never".into(),
            since: 0,
            timeout_ms: 50,
        },
    )
    .unwrap();
    assert_eq!(missed["ok"], false);
    assert_eq!(
        missed["recent"].as_array().unwrap().len(),
        2,
        "a timeout reports what happened instead"
    );
}

#[test]
fn bad_requests_are_answered_not_dropped() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let (_bus, path) = start("bad");
    let mut stream = UnixStream::connect(&path).unwrap();
    writeln!(stream, "{{\"cmd\":\"reboot\"}}").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    let answer: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(answer["ok"], false);
    assert!(answer["error"].as_str().unwrap().starts_with("bad request"));
}

#[derive(Serialize)]
struct Started {
    id: &'static str,
}

impl Event for Started {
    const KIND: &'static str = "job.started";
}

#[derive(Serialize)]
struct Finished {
    id: &'static str,
}

impl Event for Finished {
    const KIND: &'static str = "job.finished";
}

/// The next event of a subscription, heartbeats skipped.
fn next_event(client: &mut helios_inspect::Client) -> Value {
    loop {
        let line = client.read().unwrap();
        if line.get("heartbeat").is_none() {
            return line["event"].clone();
        }
    }
}

#[test]
fn a_subscriber_gets_the_events_in_order_and_resumes_after_a_reconnect() {
    let (bus, path) = start("subscribe");
    let address = helios_inspect::Address::Unix(path);
    bus.publish("jobs", None, &Started { id: "a" });
    bus.publish("test", None, &Tick { n: 1 });
    let (ack, mut client) = helios_inspect::subscribe(&address, "job.", Some(0)).unwrap();
    assert_eq!(ack["ok"], true, "{ack}");
    assert_eq!(ack["gap"], false, "{ack}");
    let instance = ack["instance"].as_str().unwrap().to_string();
    // Published once the stream runs: pushed, never asked for.
    bus.publish("jobs", None, &Started { id: "b" });
    bus.publish("test", None, &Tick { n: 2 });
    bus.publish("jobs", None, &Finished { id: "a" });
    let first = next_event(&mut client);
    assert_eq!(
        (first["kind"].as_str(), first["payload"]["id"].as_str()),
        (Some("job.started"), Some("a"))
    );
    let second = next_event(&mut client);
    assert_eq!(second["payload"]["id"], "b");
    let resume = second["seq"].as_u64().unwrap();
    drop(client);

    // Events published while away come first after a reconnect, the
    // other kinds left out.
    bus.publish("jobs", None, &Finished { id: "b" });
    let (ack, mut client) = helios_inspect::subscribe(&address, "job.", Some(resume)).unwrap();
    assert_eq!(ack["instance"], instance.as_str(), "the same process");
    assert_eq!(ack["gap"], false);
    let kinds: Vec<(String, String)> = (0..2)
        .map(|_| {
            let e = next_event(&mut client);
            (
                e["kind"].as_str().unwrap().to_string(),
                e["payload"]["id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("job.finished".to_string(), "a".to_string()),
            ("job.finished".to_string(), "b".to_string())
        ]
    );
}

#[test]
fn a_subscriber_too_far_behind_is_told_of_the_gap() {
    let bus = Arc::new(Bus::with_journal(2));
    let path = serve(Inspector::new(bus.clone()), &socket("gap")).unwrap();
    for n in 0..5 {
        bus.publish("test", None, &Tick { n });
    }
    let address = helios_inspect::Address::Unix(path);
    let (ack, _client) = helios_inspect::subscribe(&address, "", Some(1)).unwrap();
    assert_eq!(ack["gap"], true, "{ack}");
    let (ack, _client) = helios_inspect::subscribe(&address, "", Some(3)).unwrap();
    assert_eq!(ack["gap"], false, "{ack}");
    let (ack, _client) = helios_inspect::subscribe(&address, "", None).unwrap();
    assert_eq!(ack["gap"], false, "from now: nothing missed");
    assert_eq!(ack["last_seq"], 5);
}

#[test]
fn a_server_hears_its_client_go() {
    use std::sync::mpsc::channel;

    let (tx, closed) = channel();
    let tx = std::sync::Mutex::new(tx);
    let path = socket("closed");
    helios_inspect::serve_connections_at(
        &helios_inspect::Listen::Unix(path.clone()),
        None,
        move |request: Value, connection| {
            let tx = tx.lock().unwrap().clone();
            let name = request["hold"].as_str().unwrap_or_default().to_string();
            connection.on_close(move || {
                let _ = tx.send(name);
            });
            Some(json!({ "ok": true }))
        },
    )
    .unwrap();
    let mut client = helios_inspect::Client::connect(&helios_inspect::Address::Unix(path)).unwrap();
    assert_eq!(
        client.request(&json!({ "hold": "batch-1" })).unwrap()["ok"],
        true
    );
    assert!(
        closed.try_recv().is_err(),
        "nothing runs while it is connected"
    );
    drop(client);
    assert_eq!(
        closed
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        "batch-1"
    );
}
