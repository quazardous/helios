//! The events of the nodes the desktop is connected to, in one
//! listener table.
//!
//! Every node runs one bus for its whole process. The desktop follows
//! each node it is connected to through **one relay**: a remote node's
//! [`crate::relay::Relay`] (one `events.subscribe` on its inspect
//! endpoint, all kinds, resumed from the last sequence number). A remote
//! node's facts reach the listeners of [`NodeEvents`] by one of two
//! paths, never both:
//!
//! - **Relayable kinds** (`FactDecl::relayable`: the decisions, the
//!   library's imports, a settings layer changed, an export written),
//!   when the desktop's application bus has a relay: the relay link takes
//!   them in once, on the desktop's fact bus, with source
//!   `relayed:<name>` (`bus_contract::Relay::ingest`). [`NodeEvents`]
//!   hears them as one named listener (`helios.node-events`) of each
//!   relayable fact point ([`NodeEvents::attach`]), beside the points'
//!   other listeners; the event stream leaves them out.
//! - **Every other kind** (untyped, or a desktop without a relay): the
//!   event stream republishes the kinds listeners want on the shell bus,
//!   with source `node:<name>` and the node's own envelope in the payload
//!   (`{"node", "event"}`), as the shell's record, and hands them to the
//!   listeners.
//!
//! The desktop's own node runs in process on the desktop's fact bus,
//! which is the application bus's journal: its facts are the journaled
//! points' facts, heard by named listeners of the application bus like
//! any other ([`NodeEvents::listen_local`], or the volume points' own
//! listeners), with no second copy.
//!
//! Windows, panels and services never subscribe to a node themselves:
//! they ask [`NodeEvents::listen`], under an id of their own, for the
//! kinds they need by pattern (`helios.volume.*`, `helios.settings.layerChanged`,
//! `storage.*`, `*`), and read [`NodeEvent`]s from their [`Listener`].
//! [`NodeEvents::listeners`] is the one table of who listens to what
//! (agents, the inspector); a listener dropped stops being listed and
//! asked for. No listener holds a subscription of its own to a bus.
//!
//! - **Resume and resync.** A relay that loses its link resumes from the
//!   last event it relayed, so a listener sees no event twice and misses
//!   none. When it cannot (the node restarted, or its journal no longer
//!   holds what was missed), every listener gets one [`RESYNC`] event for
//!   that node: read the state again.
//! - **Round trip.** A window sends a command to the node; the facts come
//!   back as events. [`NodeEvent::event`] keeps the node's `cause` (the
//!   node's `seq` of the event that caused it) and the payload keeps the
//!   promise or job id where the node puts one, so a window tells the
//!   change it asked for from another client's.
//! - **What is not relayed.** A node's `helios.job.*` events go through the jobs
//!   relay only (ids `node:<name>/<id>`, the jobs centre).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use egui_pop::api::bus::bus_contract::{self, ListenOpts};
use helios_core::{Bus, Envelope, Fact, FactDecl};
use serde_json::{Value, json};

/// What a node's source starts with on the shell bus: `node:<name>`.
pub const SOURCE: &str = "node:";
/// Said to every listener when a node's events cannot be followed on
/// without a hole: read the state again (`reason` in the payload).
pub const RESYNC: &str = "helios.node.resync";
/// The kinds the jobs relay carries, never relayed here.
const JOBS: &str = "helios.job.";
/// The id of [`NodeEvents`]' listener on each relayable fact point.
pub const LISTENER: &str = "helios.node-events";

/// A kind as the bus names it (`&'static`): each distinct kind is kept
/// once for the process.
pub fn intern(kind: &str) -> &'static str {
    static KINDS: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut kinds = KINDS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(k) = kinds.get(kind) {
        return k;
    }
    let k: &'static str = Box::leak(kind.to_owned().into_boxed_str());
    kinds.insert(k);
    k
}

/// What wakes the window when a listener has something to read.
type Wake = Arc<dyn Fn() + Send + Sync>;

/// The nodes' events: where relays hand them and listeners listen.
/// Cloned, it is the same.
#[derive(Clone)]
pub struct NodeEvents {
    /// The shell's bus: the record of what the event stream handed on.
    bus: Arc<Bus>,
    /// Who listens to what, by a number of the listener's own.
    listening: Arc<Mutex<Listening>>,
    /// The kinds the application bus's relay takes in
    /// ([`NodeEvents::attach`]): the event stream leaves them out.
    relayed: Arc<Mutex<BTreeSet<&'static str>>>,
    /// Wakes the window once a listener has something to read.
    wake: Arc<Mutex<Option<Wake>>>,
    /// The fact bus's number of the desktop's own node's last fact
    /// recorded on the shell bus ([`NodeEvents::record_local`]).
    recorded: Arc<Mutex<u64>>,
}

impl NodeEvents {
    /// Recording on `bus` (the shell's).
    pub fn new(bus: Arc<Bus>) -> Self {
        Self {
            bus,
            listening: Arc::default(),
            relayed: Arc::default(),
            wake: Arc::default(),
            recorded: Arc::default(),
        }
    }

    /// Hears the relayable kinds of `tables` taken in by `app`'s relay
    /// (the desktop's application bus): one listener ([`LISTENER`]) on
    /// each relayable fact point, which hands a fact relayed from a node
    /// (source `relayed:<name>`) to the listeners that want it. From
    /// then on the event stream leaves those kinds out, so each fact
    /// comes once. Nothing on a bus without a relay. What the bus
    /// refused, as text.
    pub fn attach<'a>(
        &self,
        app: &dyn bus_contract::Bus,
        tables: impl IntoIterator<Item = &'a [FactDecl]>,
    ) -> Vec<String> {
        if app.relay().is_none() {
            return Vec::new();
        }
        let mut refused = Vec::new();
        for point in tables
            .into_iter()
            .flatten()
            .filter_map(|d| d.point)
            .filter(|p| p.relay.is_some())
        {
            let me = self.clone();
            let run: bus_contract::AnyListener = Arc::new(move |input, _| {
                if let Some(fact) = input.downcast_ref::<Fact>() {
                    me.heard(fact);
                }
                Ok(())
            });
            let opts = ListenOpts::new().owner("helios.node-events");
            match app.listen_any(point.key(), LISTENER.to_string(), opts, run) {
                Ok(()) => {
                    self.relayed
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(point.id);
                }
                Err(e) => refused.push(format!("{}: {e}", point.id)),
            }
        }
        refused
    }

    /// Whether facts of `kind` reach the listeners through the
    /// application bus's relay ([`NodeEvents::attach`]), not the event
    /// stream.
    pub fn taken_in(&self, kind: &str) -> bool {
        self.relayed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(kind)
    }

    /// Wakes the window with `wake` once a listener has something to
    /// read (the shell's first frame gives it).
    pub fn set_wake(&self, wake: Wake) {
        *self.wake.lock().unwrap_or_else(PoisonError::into_inner) = Some(wake);
    }

    /// Whether the window is woken already ([`NodeEvents::set_wake`]).
    pub fn wakes(&self) -> bool {
        self.wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// The events of every node whose kind matches one of `patterns`
    /// ([`matches()`]), from now on, and every [`RESYNC`]; `id` names the
    /// listener ([`NodeEvents::listeners`]).
    pub fn listen(&self, id: &str, patterns: &[&str]) -> Listener {
        let patterns: Vec<String> = patterns.iter().map(|p| (*p).to_string()).collect();
        let (tx, rx) = channel();
        let mut listening = self
            .listening
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        listening.next += 1;
        let number = listening.next;
        listening.by_number.insert(
            number,
            Entry {
                id: id.to_string(),
                patterns,
                tx,
            },
        );
        Listener {
            rx,
            number,
            listening: self.listening.clone(),
        }
    }

    /// Who listens, and to what: each listener's id and patterns, in the
    /// order they started.
    pub fn listeners(&self) -> Vec<(String, Vec<String>)> {
        self.listening
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .by_number
            .values()
            .map(|e| (e.id.clone(), e.patterns.clone()))
            .collect()
    }

    /// Whether some listener wants events of `kind`.
    pub fn wants(&self, kind: &str) -> bool {
        !kind.starts_with(JOBS)
            && self
                .listening
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .by_number
                .values()
                .any(|e| e.wants(kind))
    }

    /// Hands `event` (a node's envelope, as its bus or its inspect
    /// stream gives it) of node `node` to the listeners that want it,
    /// recorded on the shell bus; its number there.
    pub fn publish(&self, node: &str, event: &Value) -> Option<u64> {
        let kind = event["kind"].as_str()?;
        if !self.wants(kind) {
            return None;
        }
        let kind = intern(kind);
        let seq = self.bus.republish_value(
            &format!("{SOURCE}{node}"),
            None,
            kind,
            json!({ "node": node, "event": event }),
        );
        self.deliver(&NodeEvent::of_value(node, kind, event));
        Some(seq)
    }

    /// Tells every listener that node `node`'s events have a hole.
    pub fn resync(&self, node: &str, reason: &str) -> u64 {
        let event = json!({
            "seq": 0,
            "source": "relay",
            "kind": RESYNC,
            "payload": { "node": node, "reason": reason },
        });
        let seq = self.bus.publish_value(
            &format!("{SOURCE}{node}"),
            None,
            RESYNC,
            json!({ "node": node, "event": event }),
        );
        self.deliver(&NodeEvent::of_value(node, RESYNC, &event));
        seq
    }

    /// A fact of a relayable point, as its listener hears it: handed on
    /// when a node's relay took it in.
    fn heard(&self, fact: &Fact) {
        let Some(node) = fact.source.strip_prefix(crate::relay::RELAYED) else {
            return;
        };
        self.deliver(&NodeEvent::of_fact(node, fact));
    }

    /// Hands `event` to each listener that wants it (a [`RESYNC`] to
    /// all), then wakes the window.
    fn deliver(&self, event: &NodeEvent) {
        let resync = event.is_resync();
        let mut any = false;
        {
            let listening = self
                .listening
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for e in listening.by_number.values() {
                if resync || e.wants(event.event.kind) {
                    any |= e.tx.send(event.clone()).is_ok();
                }
            }
        }
        if any {
            let wake = self
                .wake
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(wake) = wake {
                wake();
            }
        }
    }

    /// Wakes the window, as a listener with something to read does.
    pub fn wake_window(&self) {
        let wake = self
            .wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    /// Records a fact of the desktop's own node on the shell bus, once
    /// however many listeners hear it, as a remote node's event stream
    /// records its: agents `wait` on the shell bus's journal, and see the
    /// node's facts there until it is the application bus's journal.
    fn record_local(&self, fact: &Fact) {
        {
            let mut last = self.recorded.lock().unwrap_or_else(PoisonError::into_inner);
            if fact.seq <= *last {
                return;
            }
            *last = fact.seq;
        }
        let event = NodeEvent::of_fact(LOCAL, fact);
        let Ok(envelope) = serde_json::to_value(&event.event) else {
            return;
        };
        self.bus.republish_value(
            &format!("{SOURCE}{LOCAL}"),
            None,
            event.event.kind,
            json!({ "node": LOCAL, "event": envelope }),
        );
    }

    /// Hears the facts the desktop's own node publishes in process, as
    /// listener `id` (owned by `owner`) of `app`'s journaled fact points
    /// (those of `tables`) whose kind matches one of `patterns`: each is
    /// handed to `heard` as node [`LOCAL`]'s event, on the thread that
    /// runs the point's pass, then the window is woken; it is recorded
    /// on the shell bus once ([`NodeEvents::record_local`]). A fact a node's
    /// relay took in (source `relayed:<name>`) is left to
    /// [`NodeEvents::attach`]'s listener, so each fact comes once. What
    /// the bus refused, as text.
    pub fn listen_local<'a>(
        &self,
        app: &dyn bus_contract::Bus,
        id: &str,
        owner: &str,
        patterns: &[&str],
        tables: impl IntoIterator<Item = &'a [FactDecl]>,
        heard: impl Fn(NodeEvent) + Send + Sync + 'static,
    ) -> Vec<String> {
        let heard = Arc::new(heard);
        let mut refused = Vec::new();
        for point in tables
            .into_iter()
            .flatten()
            .filter_map(|d| d.point)
            .filter(|p| patterns.iter().any(|pattern| matches(pattern, p.id)))
        {
            let (me, heard) = (self.clone(), heard.clone());
            let run: bus_contract::AnyListener = Arc::new(move |input, _| {
                if let Some(fact) = input.downcast_ref::<Fact>()
                    && !fact.source.starts_with(crate::relay::RELAYED)
                {
                    me.record_local(fact);
                    heard(NodeEvent::of_fact(LOCAL, fact));
                    me.wake_window();
                }
                Ok(())
            });
            app.unlisten(point.id, id);
            let opts = ListenOpts::new().owner(owner);
            if let Err(e) = app.listen_any(point.key(), id.to_string(), opts, run) {
                refused.push(format!("{}: {e}", point.id));
            }
        }
        refused
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Whether `kind` matches `pattern`: `*` every kind, `storage.*` (or
/// `storage.`) every kind starting with `storage.`, else that kind alone.
pub fn matches(pattern: &str, kind: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => kind.starts_with(prefix),
        None if pattern.ends_with('.') => kind.starts_with(pattern),
        None => kind == pattern,
    }
}

/// One listener of the table: its id, its patterns and where its events
/// go.
struct Entry {
    id: String,
    patterns: Vec<String>,
    tx: Sender<NodeEvent>,
}

impl Entry {
    fn wants(&self, kind: &str) -> bool {
        self.patterns.iter().any(|p| matches(p, kind))
    }
}

/// The listeners, by a number of their own.
#[derive(Default)]
struct Listening {
    next: u64,
    by_number: BTreeMap<u64, Entry>,
}

/// The desktop's own node's name on the shell bus.
pub const LOCAL: &str = "local";

/// A node's event, heard by a [`Listener`].
#[derive(Clone, Debug, PartialEq)]
pub struct NodeEvent {
    /// The node's name on the desktop.
    pub node: String,
    /// The node's envelope: `seq` and `cause` in the node's numbering
    /// (`mono_us` means nothing outside the node). A fact taken in by the
    /// relay keeps the node's `cause`; its `seq` is the desktop fact
    /// bus's, and its source `relayed:<name>`.
    pub event: Envelope,
}

impl NodeEvent {
    /// Node `node`'s fact, as a point's listener hears it.
    pub fn of_fact(node: &str, fact: &Fact) -> NodeEvent {
        NodeEvent {
            node: node.to_string(),
            event: Envelope {
                seq: fact.seq,
                mono_us: 0,
                wall_ms: now_ms(),
                source: fact.source.clone(),
                cause: fact.cause,
                kind: intern(&fact.kind),
                payload: fact.payload.clone(),
            },
        }
    }

    /// Node `node`'s event of kind `kind`, from its envelope as JSON.
    fn of_value(node: &str, kind: &'static str, e: &Value) -> NodeEvent {
        NodeEvent {
            node: node.to_string(),
            event: Envelope {
                seq: e["seq"].as_u64().unwrap_or(0),
                mono_us: e["mono_us"].as_u64().unwrap_or(0),
                wall_ms: e["wall_ms"].as_u64().unwrap_or_else(now_ms),
                source: e["source"].as_str().unwrap_or_default().to_string(),
                cause: e["cause"].as_u64(),
                kind,
                payload: e["payload"].clone(),
            },
        }
    }

    /// Whether it says the node's events have a hole ([`RESYNC`]).
    pub fn is_resync(&self) -> bool {
        self.event.kind == RESYNC
    }
}

/// The nodes' events of the kinds asked for ([`NodeEvents::listen`]).
/// Dropped, it stops listening.
pub struct Listener {
    rx: Receiver<NodeEvent>,
    number: u64,
    listening: Arc<Mutex<Listening>>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.listening
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .by_number
            .remove(&self.number);
    }
}

impl Listener {
    /// Hands an event to this listener, as the table does: what a
    /// listener of the application bus's points ([`NodeEvents::listen_local`])
    /// gives it. Nothing once the listener is dropped.
    pub fn hand(&self) -> impl Fn(NodeEvent) + Send + Sync + 'static {
        let tx = self
            .listening
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .by_number
            .get(&self.number)
            .map(|e| e.tx.clone());
        move |event| {
            if let Some(tx) = &tx {
                let _ = tx.send(event);
            }
        }
    }

    /// The events heard so far, without waiting.
    pub fn try_iter(&self) -> impl Iterator<Item = NodeEvent> + '_ {
        self.rx.try_iter()
    }

    /// The next event, waiting for it; `None` once nothing can come.
    pub fn recv(&self) -> Option<NodeEvent> {
        self.rx.recv().ok()
    }

    /// The next event, waiting at most `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<NodeEvent, RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_kept_once() {
        let a = intern("storage.changed");
        let b = intern(&String::from("storage.changed"));
        assert!(std::ptr::eq(a, b));
    }

    #[test]
    fn listeners_hear_their_prefixes_and_resyncs() {
        let shell = Arc::new(Bus::new());
        let events = NodeEvents::new(shell.clone());
        let storage = events.listen("storage-page", &["storage.*"]);
        let event = json!({ "seq": 7, "cause": 5, "source": "vault", "kind": "storage.changed",
            "payload": { "promise": "p1" } });
        assert!(events.publish("box", &event).is_some());
        // Nobody asked for it: not republished.
        assert!(
            events
                .publish("box", &json!({ "seq": 8, "kind": "other.thing" }))
                .is_none()
        );
        // The jobs relay's.
        assert!(
            events
                .publish("box", &json!({ "seq": 9, "kind": "helios.job.started" }))
                .is_none()
        );
        events.resync("box", "restarted");
        let heard: Vec<NodeEvent> = storage.try_iter().collect();
        assert_eq!(heard.len(), 2);
        assert_eq!(heard[0].node, "box");
        assert_eq!(heard[0].event.seq, 7);
        assert_eq!(heard[0].event.cause, Some(5));
        assert_eq!(heard[0].event.payload["promise"], "p1");
        assert!(heard[1].is_resync());
        assert_eq!(
            events.listeners(),
            vec![("storage-page".to_string(), vec!["storage.*".to_string()])]
        );
        drop(storage);
        assert!(events.listeners().is_empty());
        assert!(!events.wants("storage.changed"));
    }

    #[test]
    fn patterns_match_kinds() {
        assert!(matches("*", "helios.volume.mounted"));
        assert!(matches("helios.volume.*", "helios.volume.mounted"));
        assert!(matches("helios.volume.", "helios.volume.mounted"));
        assert!(!matches("helios.volume.*", "helios.volumes.x"));
        assert!(matches(
            "helios.settings.layerChanged",
            "helios.settings.layerChanged"
        ));
        assert!(!matches(
            "helios.settings.layer",
            "helios.settings.layerChanged"
        ));
    }
}
