//! The events of the nodes the desktop is connected to, on the shell bus.
//!
//! Every node runs one bus for its whole process. The desktop follows
//! each node it is connected to through **one relay**: a remote node's
//! [`crate::relay::Relay`] (one `events.subscribe` on its inspect
//! endpoint, all kinds, resumed from the last sequence number), the
//! desktop's own in-process node through a [`LocalRelay`]. A relay
//! republishes the events listeners want on the shell bus, with source
//! `node:<name>` and the node's own envelope in the payload
//! (`{"node", "event"}`): its `seq`, `cause`, `source`, `kind` and
//! `payload`, in the node's numbering.
//!
//! Windows, panels and services never subscribe to a node themselves:
//! they ask [`NodeEvents::listen`], under an id of their own, for the
//! kinds they need by pattern (`volume.*`, `settings.layerChanged`,
//! `storage.*`, `*`), and read [`NodeEvent`]s from their [`Listener`].
//! [`NodeEvents::listeners`] says who listens to what (agents, the
//! inspector); a listener dropped stops being listed and asked for.
//!
//! - **Resume and resync.** A relay that loses its link resumes from the
//!   last event it relayed, so a listener sees no event twice and misses
//!   none. When it cannot (the node restarted, or its journal no longer
//!   holds what was missed), every listener gets a [`RESYNC`] event for
//!   that node: read the state again.
//! - **Round trip.** A window sends a command to the node; the facts come
//!   back as events. [`NodeEvent::event`] keeps the node's `cause` (the
//!   node's `seq` of the event that caused it) and the payload keeps the
//!   promise or job id where the node puts one, so a window tells the
//!   change it asked for from another client's.
//! - **What is not relayed.** A node's `job.*` events go through the jobs
//!   relay only (ids `node:<name>/<id>`, the jobs centre). The desktop's
//!   own vault and viewport events stay on their buses (process-local,
//!   high rate): a [`LocalRelay`] never forwards [`INTERNAL`] kinds.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use helios_core::{Bus, Envelope};
use serde_json::{Value, json};

/// What a node's source starts with on the shell bus: `node:<name>`.
pub const SOURCE: &str = "node:";
/// Said to every listener when a node's events cannot be followed on
/// without a hole: read the state again (`reason` in the payload).
pub const RESYNC: &str = "node.resync";
/// The kinds an in-process node keeps to its own bus: its vault's
/// (read by `Persist`) and its engine's.
pub const INTERNAL: [&str; 3] = ["vault.", "edit.", "engine."];
/// The kinds the jobs relay carries, never relayed here.
const JOBS: &str = "job.";

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

/// The nodes' events on the shell bus: where relays publish and
/// listeners listen. Cloned, it is the same.
#[derive(Clone)]
pub struct NodeEvents {
    bus: Arc<Bus>,
    /// Who listens to what, by a number of the listener's own.
    listening: Arc<Mutex<Listening>>,
    /// The desktop's own node, followed in process (one relay per bus).
    local: Arc<Mutex<Vec<LocalRelay>>>,
}

impl NodeEvents {
    /// Republishing on `bus` (the shell's).
    pub fn new(bus: Arc<Bus>) -> Self {
        Self {
            bus,
            listening: Arc::default(),
            local: Arc::default(),
        }
    }

    /// The events of every node whose kind matches one of `patterns`
    /// ([`matches()`]), from now on, and every [`RESYNC`]; `id` names the
    /// listener ([`NodeEvents::listeners`]).
    pub fn listen(&self, id: &str, patterns: &[&str]) -> Listener {
        let patterns: Vec<String> = patterns.iter().map(|p| (*p).to_string()).collect();
        let mut listening = self
            .listening
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        listening.next += 1;
        let number = listening.next;
        listening
            .by_number
            .insert(number, (id.to_string(), patterns.clone()));
        Listener {
            rx: self.bus.subscribe(),
            patterns,
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
            .cloned()
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
                .any(|(_, patterns)| patterns.iter().any(|p| matches(p, kind)))
    }

    /// Republishes `event` (a node's envelope, as its bus or its inspect
    /// stream gives it) of node `node`, when a listener wants it.
    pub fn publish(&self, node: &str, event: &Value) -> Option<u64> {
        let kind = event["kind"].as_str()?;
        if !self.wants(kind) {
            return None;
        }
        Some(self.bus.publish_value(
            &format!("{SOURCE}{node}"),
            None,
            intern(kind),
            json!({ "node": node, "event": event }),
        ))
    }

    /// Tells every listener that node `node`'s events have a hole.
    pub fn resync(&self, node: &str, reason: &str) -> u64 {
        let event = json!({
            "seq": 0,
            "source": "relay",
            "kind": RESYNC,
            "payload": { "node": node, "reason": reason },
        });
        self.bus.publish_value(
            &format!("{SOURCE}{node}"),
            None,
            RESYNC,
            json!({ "node": node, "event": event }),
        )
    }

    /// Follows the desktop's own node, in process, on its bus `bus`
    /// (`local`). A bus followed already is followed once.
    pub fn follow_local(&self, bus: &Arc<Bus>) {
        let mut local = self.local.lock().unwrap_or_else(PoisonError::into_inner);
        if local.iter().any(|l| Arc::ptr_eq(&l.bus, bus)) {
            return;
        }
        local.extend(LocalRelay::start(LOCAL, bus.clone(), self.clone()));
    }
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

/// The listeners, by a number of their own.
#[derive(Default)]
struct Listening {
    next: u64,
    by_number: std::collections::BTreeMap<u64, (String, Vec<String>)>,
}

/// The desktop's own node's name on the shell bus.
pub const LOCAL: &str = "local";

/// A node's event, heard by a [`Listener`].
#[derive(Clone, Debug, PartialEq)]
pub struct NodeEvent {
    /// The node's name on the desktop.
    pub node: String,
    /// The node's envelope: `seq` and `cause` in the node's numbering
    /// (`mono_us` means nothing outside the node).
    pub event: Envelope,
}

impl NodeEvent {
    /// The node's event of a shell bus envelope a relay published.
    pub fn of(envelope: &Envelope) -> Option<NodeEvent> {
        envelope.source.strip_prefix(SOURCE)?;
        let node = envelope.payload["node"].as_str()?.to_string();
        let e = &envelope.payload["event"];
        Some(NodeEvent {
            node,
            event: Envelope {
                seq: e["seq"].as_u64().unwrap_or(0),
                mono_us: e["mono_us"].as_u64().unwrap_or(0),
                wall_ms: e["wall_ms"].as_u64().unwrap_or(envelope.wall_ms),
                source: e["source"].as_str().unwrap_or_default().to_string(),
                cause: e["cause"].as_u64(),
                kind: envelope.kind,
                payload: e["payload"].clone(),
            },
        })
    }

    /// Whether it says the node's events have a hole ([`RESYNC`]).
    pub fn is_resync(&self) -> bool {
        self.event.kind == RESYNC
    }
}

/// The nodes' events of the kinds asked for ([`NodeEvents::listen`]).
/// Dropped, it stops listening.
pub struct Listener {
    rx: Receiver<Envelope>,
    patterns: Vec<String>,
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
    fn keep(&self, envelope: &Envelope) -> Option<NodeEvent> {
        let wanted =
            envelope.kind == RESYNC || self.patterns.iter().any(|p| matches(p, envelope.kind));
        if wanted {
            NodeEvent::of(envelope)
        } else {
            None
        }
    }

    /// The events heard so far, without waiting.
    pub fn try_iter(&self) -> impl Iterator<Item = NodeEvent> + '_ {
        self.rx.try_iter().filter_map(|e| self.keep(&e))
    }

    /// The next event, waiting for it; `None` once the shell is gone.
    pub fn recv(&self) -> Option<NodeEvent> {
        loop {
            let e = self.rx.recv().ok()?;
            if let Some(n) = self.keep(&e) {
                return Some(n);
            }
        }
    }

    /// The next event, waiting at most `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<NodeEvent, RecvTimeoutError> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let e = self.rx.recv_timeout(left)?;
            if let Some(n) = self.keep(&e) {
                return Ok(n);
            }
        }
    }
}

/// How often a [`LocalRelay`]'s thread looks whether it should stop.
const STOP_POLL: Duration = Duration::from_millis(200);

/// Follows a node running in the desktop's process, on its bus: the
/// events listeners want, [`INTERNAL`] ones left out. In process nothing
/// is missed (the bus's channel keeps every event), so it never resyncs.
/// Dropped, it stops.
pub struct LocalRelay {
    bus: Arc<Bus>,
    stop: Arc<AtomicBool>,
}

impl LocalRelay {
    fn start(name: &str, bus: Arc<Bus>, events: NodeEvents) -> Option<LocalRelay> {
        // Subscribed here, before it returns: nothing published after is
        // missed.
        let rx = bus.subscribe();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let node = name.to_string();
        let spawned = std::thread::Builder::new()
            .name(format!("helios-node-events-{name}"))
            .spawn(move || {
                while !stopped.load(Ordering::Relaxed) {
                    match rx.recv_timeout(STOP_POLL) {
                        Ok(e) if INTERNAL.iter().any(|p| e.kind.starts_with(p)) => {}
                        Ok(e) if events.wants(e.kind) => {
                            if let Ok(event) = serde_json::to_value(&e) {
                                events.publish(&node, &event);
                            }
                        }
                        Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            });
        match spawned {
            Ok(_) => Some(LocalRelay { bus, stop }),
            Err(e) => {
                log::warn!(target: "helios", "node `{name}`: its events are not followed: {e}");
                None
            }
        }
    }
}

impl Drop for LocalRelay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
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
                .publish("box", &json!({ "seq": 9, "kind": "job.started" }))
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
        assert!(matches("*", "volume.mounted"));
        assert!(matches("volume.*", "volume.mounted"));
        assert!(matches("volume.", "volume.mounted"));
        assert!(!matches("volume.*", "volumes.x"));
        assert!(matches("settings.layerChanged", "settings.layerChanged"));
        assert!(!matches("settings.layer", "settings.layerChanged"));
    }

    #[test]
    fn a_local_node_s_internal_events_stay_home() {
        let shell = Arc::new(Bus::new());
        let node = Arc::new(Bus::new());
        let events = NodeEvents::new(shell);
        let heard = events.listen("test", &["vault.*", "volume.*"]);
        events.follow_local(&node);
        node.publish_value("vault", None, "vault.field_changed", json!({}));
        let seq = node.publish_value("watch", None, "volume.mounted", json!({ "id": "a" }));
        let e = heard.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(e.node, LOCAL);
        assert_eq!(e.event.kind, "volume.mounted");
        assert_eq!(e.event.seq, seq);
    }
}
