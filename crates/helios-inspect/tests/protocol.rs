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
