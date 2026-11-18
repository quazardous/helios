//! The event bus: orchestration, never data.
//!
//! Events are small facts in the past tense, published by the owner of what
//! changed. Each layer defines its own event types; the bus wraps them in an
//! [`Envelope`] (sequence number, time, source, cause) and keeps a journal of the
//! most recent ones, so a test or an agent can read what happened since a
//! given point instead of sleeping.
//!
//! The journal is a ring: a burst of frequent events pushes the oldest out,
//! so a reader that must not miss a rare event does not poll it. It
//! subscribes to the kinds it needs ([`Bus::subscribe_kinds`]): each one is
//! kept in its channel until read, whatever the journal overran, and a
//! [`Wake`] tells it at once. A journal reader can still tell what it
//! missed ([`Bus::missed`]).
//!
//! Every kind is declared once, in a contract table ([`FactDecl`]) next to
//! its type: the kind, its payload type and what it says. A process
//! [`Bus::declare`]s the tables of the crates it links; from then on a
//! kind published that no table holds is a bug: a debug build stops on it,
//! a release build counts it in [`Describe`].

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::describe::Describe;

/// A typed event. `KIND` names it on the bus (`layer.event`).
pub trait Event: Serialize {
    const KIND: &'static str;
}

/// The payload a fact kind carries.
#[derive(Clone, Copy)]
pub enum Payload {
    /// A typed [`Event`]: its Rust type, by name.
    Type(fn() -> &'static str),
    /// JSON built in place, the shape of its fields written out
    /// (`{ count }`), for a kind with no type yet.
    Json(&'static str),
}

/// One fact kind of the bus, as a contract table declares it.
#[derive(Clone, Copy)]
pub struct FactDecl {
    /// Its name on the bus (`layer.event`).
    pub kind: &'static str,
    pub payload: Payload,
    /// What it says, in one line.
    pub about: &'static str,
}

impl FactDecl {
    /// A typed event: its kind and payload type come from `E`.
    pub const fn of<E: Event>(about: &'static str) -> Self {
        Self {
            kind: E::KIND,
            payload: Payload::Type(std::any::type_name::<E>),
            about,
        }
    }

    /// A kind published as JSON (`publish_value`) made from a value of
    /// type `T`, for a kind several kinds share the payload of (the states
    /// of a promise).
    pub const fn typed<T: Serialize>(kind: &'static str, about: &'static str) -> Self {
        Self {
            kind,
            payload: Payload::Type(std::any::type_name::<T>),
            about,
        }
    }

    /// A kind published as JSON (`publish_value`), `shape` its fields.
    pub const fn json(kind: &'static str, shape: &'static str, about: &'static str) -> Self {
        Self {
            kind,
            payload: Payload::Json(shape),
            about,
        }
    }

    /// Its payload: the type name, or the shape of its JSON.
    pub fn payload_name(&self) -> &'static str {
        match self.payload {
            Payload::Type(name) => name(),
            Payload::Json(shape) => shape,
        }
    }
}

impl std::fmt::Debug for FactDecl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FactDecl")
            .field("kind", &self.kind)
            .field("payload", &self.payload_name())
            .field("about", &self.about)
            .finish()
    }
}

/// The fact kinds this crate publishes: the engine's, the scheduler's and
/// the jobs'.
pub const FACTS: &[&[FactDecl]] = &[
    crate::engine::FACTS,
    crate::scheduler::FACTS,
    crate::jobs::FACTS,
];

/// An event as the bus carries it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Envelope {
    /// Strictly increasing, from 1.
    pub seq: u64,
    /// When it was published, in microseconds since the bus was created,
    /// from a monotonic clock: never goes back, follows `seq`, and measures
    /// latency. Only comparable between events of the same bus.
    pub mono_us: u64,
    /// When it was published, in milliseconds since the Unix epoch, from
    /// the wall clock: for logs and for relays between processes. It can
    /// jump (the clock is set), so it orders nothing.
    pub wall_ms: u64,
    /// Who published it (a layer, a node...).
    pub source: String,
    /// The sequence number of the event that caused it, if any.
    pub cause: Option<u64>,
    pub kind: &'static str,
    pub payload: serde_json::Value,
}

/// How many events the journal keeps by default.
pub const JOURNAL: usize = 1024;

/// Called after an event a subscriber asked for is published (outside
/// the bus's lock): a window asks to be drawn again, a thread is unparked.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct Bus {
    inner: Mutex<Inner>,
}

struct Subscriber {
    to: Sender<Envelope>,
    /// The kinds it gets; every kind when `None`.
    kinds: Option<Vec<&'static str>>,
    wake: Option<Wake>,
}

impl Subscriber {
    fn wants(&self, kind: &str) -> bool {
        self.kinds
            .as_ref()
            .is_none_or(|kinds| kinds.contains(&kind))
    }
}

struct Inner {
    next: u64,
    /// The zero of every envelope's `mono_us`.
    start: Instant,
    capacity: usize,
    journal: VecDeque<Envelope>,
    subscribers: Vec<Subscriber>,
    /// The kinds declared ([`Bus::declare`]); `None` checks nothing.
    declared: Option<std::collections::BTreeMap<&'static str, FactDecl>>,
    /// Kinds published that no declared table holds (a release build).
    undeclared: std::collections::BTreeSet<&'static str>,
}

impl Default for Bus {
    fn default() -> Self {
        Self::with_journal(JOURNAL)
    }
}

impl Bus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_journal(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                next: 1,
                start: Instant::now(),
                capacity,
                journal: VecDeque::new(),
                subscribers: Vec::new(),
                declared: None,
                undeclared: std::collections::BTreeSet::new(),
            }),
        }
    }

    /// Declares the fact kinds of `tables` (each crate's `FACTS`): from
    /// now on a kind published that none of the declared tables holds is
    /// a bug. A debug build panics at the publish; a release build
    /// publishes it and lists it under `undeclared` in [`Describe`].
    /// Called again, it adds to what was declared.
    pub fn declare<'a>(&self, tables: impl IntoIterator<Item = &'a [FactDecl]>) {
        let mut inner = self.inner.lock().expect("bus lock");
        let declared = inner.declared.get_or_insert_with(Default::default);
        for decl in tables.into_iter().flatten() {
            declared.insert(decl.kind, *decl);
        }
    }

    /// The declared kind `kind`, if the bus declared it.
    pub fn declared(&self, kind: &str) -> Option<FactDecl> {
        let inner = self.inner.lock().expect("bus lock");
        inner.declared.as_ref()?.get(kind).copied()
    }

    /// Publishes an event; returns its sequence number.
    pub fn publish<E: Event>(&self, source: &str, cause: Option<u64>, event: &E) -> u64 {
        let payload = serde_json::to_value(event).expect("events always serialize");
        self.publish_value(source, cause, E::KIND, payload)
    }

    /// Publishes an event already in its JSON form, for a layer that
    /// relays events it does not type (the GUI relays its toolkit's).
    pub fn publish_value(
        &self,
        source: &str,
        cause: Option<u64>,
        kind: &'static str,
        payload: serde_json::Value,
    ) -> u64 {
        self.publish_inner(source, cause, kind, payload, true)
    }

    /// Publishes again an event another bus published (a node's, relayed
    /// to the desktop): its kind was checked against the tables of the
    /// process that published it, which this one may not link.
    pub fn republish_value(
        &self,
        source: &str,
        cause: Option<u64>,
        kind: &'static str,
        payload: serde_json::Value,
    ) -> u64 {
        self.publish_inner(source, cause, kind, payload, false)
    }

    fn publish_inner(
        &self,
        source: &str,
        cause: Option<u64>,
        kind: &'static str,
        payload: serde_json::Value,
        check: bool,
    ) -> u64 {
        let mut wakes = Vec::new();
        let mut inner = self.inner.lock().expect("bus lock");
        if check
            && inner
                .declared
                .as_ref()
                .is_some_and(|declared| !declared.contains_key(kind))
        {
            inner.undeclared.insert(kind);
            if cfg!(debug_assertions) {
                drop(inner);
                panic!("bus: kind `{kind}` published by `{source}` is in no declared FACTS table");
            }
        }
        let seq = inner.next;
        inner.next += 1;
        // Stamped under the lock, so `mono_us` follows `seq`.
        let envelope = Envelope {
            seq,
            mono_us: micros(inner.start.elapsed()),
            wall_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            source: source.to_string(),
            cause,
            kind,
            payload,
        };
        inner.subscribers.retain(|s| {
            if !s.wants(kind) {
                return true;
            }
            let sent = s.to.send(envelope.clone()).is_ok();
            if sent && let Some(wake) = &s.wake {
                wakes.push(wake.clone());
            }
            sent
        });
        if inner.journal.len() == inner.capacity {
            inner.journal.pop_front();
        }
        inner.journal.push_back(envelope);
        drop(inner);
        for wake in wakes {
            wake();
        }
        seq
    }

    /// Every event published from now on.
    pub fn subscribe(&self) -> Receiver<Envelope> {
        let (to, from) = channel();
        self.inner
            .lock()
            .expect("bus lock")
            .subscribers
            .push(Subscriber {
                to,
                kinds: None,
                wake: None,
            });
        from
    }

    /// The events of `kinds` published from now on, each kept in the
    /// channel until read: what the journal overran in between does not
    /// matter. `wake`, if any, is called after each one is published. For
    /// rare events a reader must not miss (a GPU lost), among frequent ones
    /// it does not read; the subscription ends when the receiver is dropped.
    pub fn subscribe_kinds(
        &self,
        kinds: &[&'static str],
        wake: Option<Wake>,
    ) -> Receiver<Envelope> {
        let (to, from) = channel();
        self.inner
            .lock()
            .expect("bus lock")
            .subscribers
            .push(Subscriber {
                to,
                kinds: Some(kinds.to_vec()),
                wake,
            });
        from
    }

    /// The journaled events with a sequence number above `seq`.
    pub fn since(&self, seq: u64) -> Vec<Envelope> {
        self.inner
            .lock()
            .expect("bus lock")
            .journal
            .iter()
            .filter(|e| e.seq > seq)
            .cloned()
            .collect()
    }

    /// How many events published after `seq` the journal no longer holds:
    /// a reader whose cursor is `seq` fell that far behind and missed them.
    pub fn missed(&self, seq: u64) -> u64 {
        let inner = self.inner.lock().expect("bus lock");
        let oldest = inner.journal.front().map_or(inner.next, |e| e.seq);
        oldest.saturating_sub(seq.saturating_add(1))
    }

    /// How long ago `envelope`, published on this bus, was published.
    pub fn age(&self, envelope: &Envelope) -> Duration {
        let now = micros(self.inner.lock().expect("bus lock").start.elapsed());
        Duration::from_micros(now.saturating_sub(envelope.mono_us))
    }

    /// The sequence number of the last event published (0 if none).
    pub fn last_seq(&self) -> u64 {
        self.inner.lock().expect("bus lock").next - 1
    }
}

fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

impl Describe for Bus {
    fn describe(&self) -> serde_json::Value {
        let inner = self.inner.lock().expect("bus lock");
        serde_json::json!({
            "last_seq": inner.next - 1,
            "journal": { "len": inner.journal.len(), "capacity": inner.capacity,
                         "oldest": inner.journal.front().map(|e| e.seq) },
            "subscribers": inner.subscribers.len(),
            "declared": inner.declared.as_ref().map(std::collections::BTreeMap::len),
            "undeclared": inner.undeclared,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Ping {
        n: u32,
    }

    impl Event for Ping {
        const KIND: &'static str = "test.ping";
    }

    #[test]
    fn events_are_numbered_journaled_and_delivered() {
        let bus = Bus::with_journal(2);
        let rx = bus.subscribe();
        let first = bus.publish("a", None, &Ping { n: 1 });
        bus.publish("b", Some(first), &Ping { n: 2 });
        bus.publish("c", None, &Ping { n: 3 });
        assert_eq!(bus.last_seq(), 3);
        let journal = bus.since(0);
        assert_eq!(
            journal.iter().map(|e| e.seq).collect::<Vec<_>>(),
            [2, 3],
            "journal keeps the last two"
        );
        assert_eq!(journal[0].cause, Some(1));
        assert_eq!(journal[0].payload["n"], 2);
        assert_eq!(rx.try_iter().count(), 3, "a subscriber gets every event");
    }

    #[derive(Serialize)]
    struct Rare;

    impl Event for Rare {
        const KIND: &'static str = "test.rare";
    }

    #[test]
    fn a_kind_subscriber_keeps_its_events_through_any_burst_and_is_woken() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let bus = Bus::with_journal(4);
        let woken = Arc::new(AtomicUsize::new(0));
        let w = woken.clone();
        let rare = bus.subscribe_kinds(
            &[Rare::KIND],
            Some(Arc::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            })),
        );
        let first = bus.publish("a", None, &Rare);
        for n in 0..100 {
            bus.publish("a", None, &Ping { n });
        }
        assert!(
            bus.since(0).iter().all(|e| e.kind != Rare::KIND),
            "the journal overran it"
        );
        let got: Vec<u64> = rare.try_iter().map(|e| e.seq).collect();
        assert_eq!(got, [first], "kept until read, the pings left out");
        assert_eq!(woken.load(Ordering::SeqCst), 1, "woken once, for it alone");
        drop(rare);
        bus.publish("a", None, &Rare);
        assert_eq!(bus.describe()["subscribers"], 0, "gone with its receiver");
    }

    const PING_FACTS: &[FactDecl] = &[FactDecl::of::<Ping>("A test ping")];

    #[test]
    fn a_declared_bus_holds_its_kinds_with_their_payload() {
        let bus = Bus::new();
        bus.declare([PING_FACTS]);
        let ping = bus.declared(Ping::KIND).expect("declared");
        assert!(ping.payload_name().ends_with("::Ping"), "{ping:?}");
        assert!(bus.declared(Rare::KIND).is_none());
        bus.publish("a", None, &Ping { n: 1 });
        assert_eq!(bus.describe()["declared"], 1);
        assert_eq!(bus.describe()["undeclared"], serde_json::json!([]));
    }

    #[test]
    #[cfg_attr(debug_assertions, should_panic(expected = "test.rare"))]
    fn a_kind_no_declared_table_holds_fails_its_publish() {
        let bus = Bus::new();
        bus.declare([PING_FACTS]);
        bus.publish("a", None, &Rare);
        // A release build publishes it and says so.
        assert_eq!(
            bus.describe()["undeclared"],
            serde_json::json!(["test.rare"])
        );
    }

    #[test]
    fn a_relayed_kind_was_checked_where_it_was_published() {
        let bus = Bus::new();
        bus.declare([PING_FACTS]);
        bus.republish_value("node:a", None, Rare::KIND, serde_json::json!(null));
        assert_eq!(bus.describe()["undeclared"], serde_json::json!([]));
    }

    #[test]
    fn an_undeclared_bus_checks_nothing() {
        let bus = Bus::new();
        bus.publish("a", None, &Rare);
        assert_eq!(bus.describe()["declared"], serde_json::Value::Null);
    }

    #[test]
    fn the_core_tables_name_each_kind_once() {
        let mut kinds: Vec<_> = FACTS
            .iter()
            .flat_map(|t| t.iter().map(|d| d.kind))
            .collect();
        let all = kinds.len();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), all, "a kind declared twice");
        assert_eq!(all, 20);
    }

    #[test]
    fn a_reader_tells_what_the_journal_overran() {
        let bus = Bus::with_journal(2);
        assert_eq!(bus.missed(0), 0, "nothing published");
        bus.publish("a", None, &Ping { n: 1 });
        bus.publish("a", None, &Ping { n: 2 });
        assert_eq!(bus.missed(0), 0, "all still journaled");
        bus.publish("a", None, &Ping { n: 3 });
        bus.publish("a", None, &Ping { n: 4 });
        assert_eq!(bus.missed(0), 2, "1 and 2 are gone");
        assert_eq!(bus.missed(1), 1);
        assert_eq!(bus.missed(2), 0);
        assert_eq!(bus.missed(4), 0);
    }

    #[test]
    fn events_are_stamped_when_published() {
        let bus = Bus::new();
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        for n in 0..50 {
            bus.publish("a", None, &Ping { n });
        }
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let journal = bus.since(0);
        assert!(
            journal.windows(2).all(|w| w[0].mono_us <= w[1].mono_us),
            "the monotonic time follows the sequence"
        );
        for e in &journal {
            let wall = u128::from(e.wall_ms);
            assert!(
                (before..=after).contains(&wall),
                "{wall} outside {before}..={after}"
            );
        }
        let last = journal.last().unwrap();
        let age = bus.age(last);
        assert!(age < Duration::from_mins(1), "{age:?}");
        assert!(bus.age(&journal[0]) >= age, "older events are older");
    }

    #[test]
    fn the_time_is_on_the_wire() {
        let bus = Bus::new();
        bus.publish("a", Some(7), &Ping { n: 1 });
        let wire = serde_json::to_value(&bus.since(0)[0]).unwrap();
        // The fields readers already use keep their names and types.
        assert_eq!(wire["seq"], 1);
        assert_eq!(wire["source"], "a");
        assert_eq!(wire["cause"], 7);
        assert_eq!(wire["kind"], "test.ping");
        assert_eq!(wire["payload"]["n"], 1);
        // The time is two plain integers, readable in any language.
        assert!(wire["mono_us"].is_u64(), "{wire}");
        assert!(
            wire["wall_ms"].as_u64().unwrap() > 1_600_000_000_000,
            "{wire}"
        );
    }
}
