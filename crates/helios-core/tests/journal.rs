//! Helios's fact bus as the journal and relay of the application's bus
//! (`helios_core::journal`), on egui-pop's hook kernel.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bus_contract::{FailureKind, ListenOpts, PointDecl, PointKind};
use bus_contract_crossing::{
    BusCrossing, Cursor, IngestError, Journal, Lost, Origin, RelayedEntry, codec,
};
use bus_contract_journal::is_journaled;
use bus_kernel::RelayHost;
use egui_pop_hooks::Kernel;
use helios_core::{Bus, BusJournal, Event, Fact, FactDecl, FactPoint};
use serde::{Deserialize, Serialize};

/// A fact every node version publishes.
#[derive(Serialize)]
struct Moved {
    photo: u32,
    to: String,
}

impl Event for Moved {
    const KIND: &'static str = "test.moved";
}

/// A fact crossing to another process.
#[derive(Serialize)]
struct Saved {
    photo: u32,
}

impl Event for Saved {
    const KIND: &'static str = "test.saved";
}

/// A fact that stays in its process.
#[derive(Serialize)]
struct Local {
    n: u32,
}

impl Event for Local {
    const KIND: &'static str = "test.local";
}

const FACTS: &[FactDecl] = &[
    FactDecl::of::<Moved>("A photo moved"),
    FactDecl::of::<Saved>("A photo was saved").relayable(),
    FactDecl::of::<Local>("A local fact").fact_only(),
    FactDecl::json("test.untyped", "{ n }", "An untyped fact"),
];

/// How long a test waits for what must come at once.
const SOON: Duration = Duration::from_secs(5);

/// A kernel on a fresh fact bus, with its journal and the table declared.
fn kernel() -> (Arc<Kernel>, Arc<BusJournal>) {
    let bus = Arc::new(Bus::new());
    bus.declare([FACTS]);
    let (kernel, journal) = BusJournal::attach(
        bus,
        |j| Kernel::new().with_journal(j.clone()).with_relay(j),
        Kernel::declaration,
    );
    let refused = journal.declare(&*kernel, [FACTS]);
    assert!(refused.is_empty(), "{refused:?}");
    (kernel, journal)
}

/// Each `(seq, listener)` heard, in order.
type Heard = Arc<Mutex<Vec<(u64, String)>>>;

fn listen<E: Event + 'static>(k: &Kernel, id: &str, opts: ListenOpts, heard: &Heard) {
    let heard = heard.clone();
    let name = id.to_string();
    k.listen::<FactPoint<E>>(id, opts, move |fact, ()| {
        heard.lock().unwrap().push((fact.seq, name.clone()));
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_typed_kind_is_a_journaled_event_point_and_the_others_are_facts_only() {
    let (k, _) = kernel();
    let points: Vec<PointDecl> = helios_core::journal::points([FACTS]);
    assert_eq!(
        points.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["test.moved", "test.saved"]
    );
    let (_, moved) = k.declaration(Moved::KIND).unwrap();
    assert!(is_journaled(&moved.facets) && codec(&moved.facets).is_none());
    let (key, saved) = k.declaration(Saved::KIND).unwrap();
    assert!(is_journaled(&saved.facets) && codec(&saved.facets).is_some());
    assert_eq!(key.kind, PointKind::Event);
    assert!(k.declaration(Local::KIND).is_none());
}

#[test]
fn facts_published_on_the_bus_reach_their_named_listeners_in_order() {
    let (k, j) = kernel();
    let heard: Heard = Arc::default();
    listen::<Moved>(&k, "test.first", ListenOpts::new(), &heard);
    let payloads = Arc::new(Mutex::new(Vec::new()));
    let p = payloads.clone();
    k.listen::<FactPoint<Moved>>(
        "test.second",
        ListenOpts::new().after("test.first"),
        move |f, ()| {
            p.lock()
                .unwrap()
                .push((f.source.clone(), f.payload["to"].clone()));
            Ok(())
        },
    )
    .unwrap();
    let bus = j.bus();
    let a = bus.publish(
        "library",
        None,
        &Moved {
            photo: 1,
            to: "a".into(),
        },
    );
    bus.publish("library", None, &Local { n: 1 });
    let b = bus.publish(
        "library",
        Some(a),
        &Moved {
            photo: 2,
            to: "b".into(),
        },
    );
    assert!(Journal::flush(&*j, SOON), "the pump delivered every fact");
    assert_eq!(
        *heard.lock().unwrap(),
        [(a, "test.first".to_string()), (b, "test.first".to_string())]
    );
    assert_eq!(
        *payloads.lock().unwrap(),
        [
            ("library".to_string(), "a".into()),
            ("library".to_string(), "b".into())
        ]
    );
}

#[test]
fn a_slow_listener_does_not_stall_the_pump_past_its_deadline() {
    let deadline = Duration::from_millis(100);
    let (k, j) = kernel();
    let failures = Arc::new(Mutex::new(Vec::new()));
    let f = failures.clone();
    bus_contract::Bus::add_failure_sink(
        &*k,
        Arc::new(move |failure| {
            f.lock()
                .unwrap()
                .push((failure.listener.clone(), failure.kind.clone()));
        }),
    );
    k.listen::<FactPoint<Moved>>(
        "test.stuck",
        ListenOpts::new().threaded().deadline(deadline),
        |_, ()| {
            thread::sleep(Duration::from_secs(30));
            Ok(())
        },
    )
    .unwrap();
    let heard: Heard = Arc::default();
    listen::<Moved>(&k, "test.quick", ListenOpts::new(), &heard);
    let start = Instant::now();
    for photo in 0..3 {
        j.bus().publish(
            "library",
            None,
            &Moved {
                photo,
                to: "x".into(),
            },
        );
    }
    // Each fact waits for the stuck listener's deadline, never longer.
    let bound = deadline * 3 + Duration::from_secs(3);
    assert!(
        Journal::flush(&*j, bound),
        "the pump stalled past three deadlines ({:?}) behind `stuck`",
        start.elapsed()
    );
    assert_eq!(heard.lock().unwrap().len(), 3, "`quick` heard every fact");
    let failures = failures.lock().unwrap();
    assert_eq!(failures.len(), 3, "{failures:?}");
    assert!(
        failures
            .iter()
            .all(|(listener, kind)| listener == "test.stuck"
                && matches!(kind, FailureKind::Late { .. })),
        "each failure names the stuck listener: {failures:?}"
    );
}

#[test]
fn publishing_through_the_journal_lands_on_the_fact_bus() {
    let (k, j) = kernel();
    let heard: Heard = Arc::default();
    listen::<Moved>(&k, "test.first", ListenOpts::new(), &heard);
    let journal =
        bus_contract_journal::BusJournal::journal(&*k).expect("the kernel offers a journal");
    let fact = Fact {
        payload: serde_json::json!({ "photo": 7, "to": "z" }),
        ..Fact::default()
    };
    let seq = journal.publish::<FactPoint<Moved>>(fact).unwrap();
    assert!(journal.flush(SOON));
    assert_eq!(*heard.lock().unwrap(), [(seq, "test.first".to_string())]);
    let on_bus = j.bus().since(seq - 1);
    assert_eq!(on_bus[0].kind, Moved::KIND);
    assert_eq!(on_bus[0].payload["to"], "z");
    // Read again, typed, from the journal.
    let (n, read) = journal.wait_for::<FactPoint<Moved>>(0, SOON).unwrap();
    assert_eq!((n, read.payload["photo"].as_u64()), (seq, Some(7)));
    let since = journal.since(0, 10);
    assert_eq!(since.last_seq, seq);
    assert!(!since.gap);
}

#[test]
fn only_relayable_kinds_are_served_from_a_cursor_of_this_run() {
    let (_kernel, node) = kernel();
    let bus = node.bus();
    bus.publish(
        "library",
        None,
        &Moved {
            photo: 1,
            to: "a".into(),
        },
    );
    let saved = bus.publish("library", None, &Saved { photo: 1 });
    let read = node.resume(Cursor::START, 10);
    assert_eq!(read.lost, None);
    assert_eq!(
        read.entries
            .iter()
            .map(|e| (e.seq, e.kind.as_str(), e.text.as_str()))
            .collect::<Vec<_>>(),
        [(saved, "test.saved", r#"{"photo":1}"#)]
    );
    assert_eq!(read.next, node.cursor());
    let again = node.resume(read.next, 10);
    assert!(again.entries.is_empty() && again.lost.is_none());
}

#[test]
fn a_node_restarted_answers_an_old_cursor_with_restarted() {
    let (_before, before) = kernel();
    before.bus().publish("library", None, &Saved { photo: 1 });
    let cursor = before.resume(Cursor::START, 10).next;
    // The node restarts: a new process, its numbers from 1 again.
    let (_after, after) = kernel();
    let first = after.bus().publish("library", None, &Saved { photo: 2 });
    let read = after.resume(cursor, 10);
    assert_eq!(read.lost, Some(Lost::Restarted), "the reader resyncs");
    assert_eq!(
        read.entries[0].seq, first,
        "then reads the run from its start"
    );
    assert_ne!(read.next.run, cursor.run);
}

#[test]
fn a_reader_behind_the_window_is_told_of_the_gap() {
    let bus = Arc::new(Bus::with_journal(2));
    bus.declare([FACTS]);
    let (k, node) = BusJournal::attach(
        bus,
        |j| Kernel::new().with_journal(j.clone()).with_relay(j),
        Kernel::declaration,
    );
    assert!(node.declare(&*k, [FACTS]).is_empty());
    node.bus().publish("library", None, &Saved { photo: 1 });
    let cursor = node.resume(Cursor::START, 10).next;
    for photo in 2..6 {
        node.bus().publish("library", None, &Saved { photo });
    }
    let read = node.resume(cursor, 10);
    assert_eq!(read.lost, Some(Lost::Gap));
    assert_eq!(read.entries.len(), 2, "what the window still holds");
}

/// The desktop's view of a saved photo: what it needs, whatever node
/// version sent it.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct SavedView {
    photo: u32,
    /// Added by a newer node; an older one does not send it.
    version: Option<u32>,
}

#[test]
fn the_codec_reads_a_payload_of_an_older_or_newer_node() {
    let (desktop, j) = kernel();
    let views = Arc::new(Mutex::new(Vec::new()));
    let v = views.clone();
    desktop
        .listen::<FactPoint<Saved>>("test.view", ListenOpts::new(), move |f, ()| {
            v.lock()
                .unwrap()
                .push((f.source.clone(), f.cause, f.read::<SavedView>()));
            Ok(())
        })
        .unwrap();
    let relay = BusCrossing::relay(&*desktop).expect("the kernel offers a relay");
    // An older node: a field missing. A newer one: a field unknown here.
    let mut old = RelayedEntry::new(4, Saved::KIND, r#"{"photo":3}"#);
    old.cause = Some(2);
    relay.ingest(&Origin::process("node:old"), old).unwrap();
    relay
        .ingest(
            &Origin::process("node:new"),
            RelayedEntry::new(9, Saved::KIND, r#"{"photo":5,"version":2,"colour":"red"}"#),
        )
        .unwrap();
    // A text that is not a payload at all is refused, nothing recorded.
    let last = j.bus().last_seq();
    let broken = relay.ingest(
        &Origin::process("node:new"),
        RelayedEntry::new(10, Saved::KIND, "{photo"),
    );
    assert!(
        matches!(broken, Err(IngestError::Decode { .. })),
        "{broken:?}"
    );
    assert_eq!(j.bus().last_seq(), last);
    // A kind declared here but not relayable may not be published from afar.
    let refused = relay.ingest(
        &Origin::process("node:new"),
        RelayedEntry::new(11, Moved::KIND, "{}"),
    );
    assert!(
        matches!(refused, Err(IngestError::NotRelayable { .. })),
        "{refused:?}"
    );
    // A kind this desktop does not know is kept as text, heard by no one.
    let unknown = relay
        .ingest(
            &Origin::process("node:new"),
            RelayedEntry::new(12, "test.future", r#"{"x":1}"#),
        )
        .unwrap();
    assert!(Journal::flush(&*j, SOON));
    assert_eq!(
        *views.lock().unwrap(),
        [
            (
                "node:old".to_string(),
                Some(2),
                Ok(SavedView {
                    photo: 3,
                    version: None
                })
            ),
            (
                "node:new".to_string(),
                None,
                Ok(SavedView {
                    photo: 5,
                    version: Some(2)
                })
            ),
        ]
    );
    let kept = j.bus().since(unknown - 1);
    assert_eq!(kept[0].kind, "test.future");
    assert_eq!(kept[0].source, "node:new");
    // Facts taken in are not served again: two processes relaying to
    // each other do not echo.
    assert!(j.resume(Cursor::START, 10).entries.is_empty());
}

/// Another process-local bus (the viewport engine's), its journaled kinds
/// relayed to the application's bus: each reaches its listeners once,
/// whatever burst of its other kinds came between, and those stay on it.
#[test]
fn a_relayed_bus_hands_its_point_kinds_on_and_keeps_the_others() {
    let (k, j) = kernel();
    let heard: Heard = Arc::default();
    listen::<Moved>(&k, "test.notices", ListenOpts::new(), &heard);
    let engine = Bus::with_journal(16);
    engine.declare([FACTS]);
    let into: std::sync::Weak<dyn bus_contract::Bus> = Arc::downgrade(&k) as _;
    let kinds = helios_core::journal::relay_points(&engine, [FACTS], into);
    assert_eq!(kinds, [Moved::KIND, Saved::KIND]);
    let before = j.bus().last_seq();
    engine.publish(
        "engine",
        None,
        &Moved {
            photo: 1,
            to: "gpu".into(),
        },
    );
    for n in 0..100 {
        engine.publish("engine", None, &Local { n });
    }
    assert!(Journal::flush(&*j, SOON));
    assert_eq!(heard.lock().unwrap().len(), 1, "heard once");
    let on_bus = j.bus().since(before);
    assert_eq!(
        on_bus
            .iter()
            .map(|e| (e.kind, e.source.as_str()))
            .collect::<Vec<_>>(),
        [(Moved::KIND, "engine")],
        "the other kinds stay on the engine's bus"
    );
    // The application's bus gone, the relay ends with it.
    drop(k);
    engine.publish(
        "engine",
        None,
        &Moved {
            photo: 2,
            to: "gone".into(),
        },
    );
}

/// An application bus without a journal (a test's, a tool's): the
/// relayed kinds are emitted to their listeners at once.
#[test]
fn without_a_journal_a_relayed_kind_is_emitted_at_once() {
    let k = Arc::new(Kernel::new());
    let heard: Heard = Arc::default();
    listen::<Moved>(&k, "test.notices", ListenOpts::new(), &heard);
    let engine = Bus::new();
    let into: std::sync::Weak<dyn bus_contract::Bus> = Arc::downgrade(&k) as _;
    helios_core::journal::relay_points(&engine, [FACTS], into);
    let seq = engine.publish(
        "engine",
        None,
        &Moved {
            photo: 1,
            to: "gpu".into(),
        },
    );
    assert_eq!(*heard.lock().unwrap(), [(seq, "test.notices".to_string())]);
}
