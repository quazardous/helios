//! The same protocol over loopback TCP: a session token is required, only
//! loopback is served, and requests behave as over a Unix socket.

use std::sync::Arc;

use helios_core::{Bus, Event};
use helios_inspect::{Address, Inspector, Listen, Request, new_token, request_at, serve_inspector};
use serde::Serialize;

#[derive(Serialize)]
struct Tick;

impl Event for Tick {
    const KIND: &'static str = "test.tick";
}

fn start(token: &str) -> (Arc<Bus>, String) {
    let bus = Arc::new(Bus::new());
    let bound = serve_inspector(
        Inspector::new(bus.clone()),
        &Listen::parse("tcp://127.0.0.1:0").unwrap(),
        Some(token),
    )
    .unwrap();
    (bus, bound.address)
}

#[test]
fn requests_work_over_tcp_with_the_token() {
    let token = new_token();
    assert_eq!(token.len(), 64);
    assert_ne!(token, new_token(), "tokens differ");
    let (bus, address) = start(&token);
    assert!(address.starts_with("tcp://127.0.0.1:"), "{address}");
    bus.publish("test", None, &Tick);
    let address = Address::parse(&address, Some(token)).unwrap();
    let events = request_at(&address, &Request::Events { since: 0 }).unwrap();
    assert_eq!(events["events"][0]["kind"], "test.tick");
    let waited = request_at(
        &address,
        &Request::WaitEvent {
            kind: "test.tick".into(),
            since: 0,
            timeout_ms: 1000,
        },
    )
    .unwrap();
    assert_eq!(waited["ok"], true);
}

#[test]
fn a_wrong_or_missing_token_is_refused() {
    let (_bus, address) = start(&new_token());
    let wrong = Address::parse(&address, Some("nope".into())).unwrap();
    let answer = request_at(&wrong, &Request::State).unwrap();
    assert_eq!(answer["ok"], false);
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .contains("authentication failed")
    );
    assert!(
        Address::parse(&address, None)
            .unwrap_err()
            .contains("needs the session token")
    );
}

#[test]
fn only_loopback_is_served_and_tcp_needs_a_token() {
    assert!(
        Listen::parse("tcp://0.0.0.0:7000")
            .unwrap_err()
            .contains("only loopback")
    );
    assert!(Listen::parse("tcp://[::1]:0").is_ok());
    let no_token = serve_inspector(
        Inspector::new(Arc::new(Bus::new())),
        &Listen::parse("tcp://127.0.0.1:0").unwrap(),
        None,
    );
    assert!(no_token.is_err());
}
