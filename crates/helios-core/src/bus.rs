//! The event bus: orchestration, never data.
//!
//! Events are small facts in the past tense, published by the owner of what
//! changed. Each layer defines its own event types; the bus wraps them in an
//! [`Envelope`] (sequence number, time, source, cause) and keeps a journal of the
//! most recent ones, so a test or an agent can read what happened since a
//! given point instead of sleeping.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::describe::Describe;

/// A typed event. `KIND` names it on the bus (`layer.event`).
pub trait Event: Serialize {
    const KIND: &'static str;
}

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

pub struct Bus {
    inner: Mutex<Inner>,
}

struct Inner {
    next: u64,
    /// The zero of every envelope's `mono_us`.
    start: Instant,
    capacity: usize,
    journal: VecDeque<Envelope>,
    subscribers: Vec<Sender<Envelope>>,
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
            }),
        }
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
        let mut inner = self.inner.lock().expect("bus lock");
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
        inner
            .subscribers
            .retain(|s| s.send(envelope.clone()).is_ok());
        if inner.journal.len() == inner.capacity {
            inner.journal.pop_front();
        }
        inner.journal.push_back(envelope);
        seq
    }

    /// Every event published from now on.
    pub fn subscribe(&self) -> Receiver<Envelope> {
        let (tx, rx) = channel();
        self.inner.lock().expect("bus lock").subscribers.push(tx);
        rx
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
        assert!(age < Duration::from_secs(60), "{age:?}");
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
