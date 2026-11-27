//! The read-only inspection endpoint of a Helios process.
//!
//! A process registers the entities it wants to show (anything that
//! implements [`Describe`]) and its [`Bus`]; [`serve`] then answers requests
//! on a local socket, one JSON object per line in each direction. Nothing in
//! this protocol changes the process: it is safe to ship in every build.
//! See `docs/testing-strategy.md`, "The control surface".
//!
//! Requests (`cmd` field):
//! - `state`: every entity's `describe()` snapshot, by name;
//! - `describe` (`entity`): one snapshot;
//! - `events` (`since`): the journaled bus events after a sequence number;
//! - `wait_event` (`kind`, `since`, `timeout_ms`): the first event of that
//!   kind after `since`, waiting for it if needed; on timeout the error
//!   carries the last events seen, so the caller learns what happened instead;
//! - `wait_idle` (`timeout_ms`): returns once the scheduler has nothing queued
//!   or running; on timeout the error carries what is still queued or running;
//! - `describe jobs`, when the process shows them ([`Inspector::with_jobs`]):
//!   its running jobs and the last ones ended, to catch up from;
//! - `subscribe` (`prefix`, `since`): turns the connection into a stream of
//!   the bus events whose kind starts with `prefix`, pushed as they are
//!   published (see [`Inspector::stream`]); with `relay` (a cursor
//!   `{run, seq}`), a stream of the relayable facts of the process's
//!   application bus after that cursor, for another process to take in
//!   (see [`Inspector::stream_relay`]). Without `prefixed`, as a client
//!   of 0.92 or before asks, both name each fact kind as that release did
//!   ([`helios_core::legacy_kinds`]).
//!
//! Responses: `{"ok": true, ...}` or `{"ok": false, "error": "...", ...}`.
//!
//! The same requests are served as JSON-RPC 2.0 ([`rpc`]): the method is
//! the `cmd`, the params its other fields; `node.hello`, `commands.list`,
//! `events.subscribe` (notifications on the same connection, with other
//! calls) and `events.unsubscribe` are added, and `$/cancel` stops a
//! `wait_event` or `wait_idle` still waiting. See `docs/protocol.md`.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bus_contract_crossing::{Cursor, Lost, Relay};
use helios_core::{Bus, Describe, Envelope, Scheduler};

pub mod batch;
pub mod cache;
pub mod client;
pub mod discovery;
pub mod jobs;
pub mod request_ids;
pub mod rpc;
pub mod transport;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub use transport::{
    Activity, Address, Bound, Client, Closer, Connection, Listen, MAX_CONNECTIONS, OUTCOME_UNKNOWN,
    Unanswered, activity, new_token, request_at, request_within, serve_at, serve_connections_at,
    serve_rpc_at, unanswered, write_token,
};

/// A request, as sent on the wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    State,
    Describe {
        entity: String,
    },
    Events {
        since: u64,
    },
    WaitEvent {
        kind: String,
        since: u64,
        timeout_ms: u64,
    },
    WaitIdle {
        timeout_ms: u64,
    },
    /// The events whose kind starts with `prefix`, after `since` (from
    /// now when not given), pushed for as long as the connection stays.
    Subscribe {
        #[serde(default)]
        prefix: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<u64>,
        /// Instead of the events, the relayable facts after this cursor
        /// ([`Inspector::stream_relay`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        relay: Option<RelayCursor>,
        /// The client reads the fact kinds under their `helios.` prefix.
        /// A desktop of Helios 0.92 or before does not say it: the
        /// stream names each kind as that release did, and `prefix` is
        /// read in those names ([`helios_core::legacy_kinds`]).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        prefixed: bool,
    },
}

/// How a stream names the fact kinds to its client: as they are now, or
/// as a client of an older release knows them.
#[derive(Clone, Copy)]
struct Names {
    legacy: bool,
}

impl Names {
    /// The prefix asked for, in the names the bus uses.
    fn prefix(self, asked: &str) -> String {
        if !self.legacy || asked.is_empty() {
            return asked.to_string();
        }
        let renamed = helios_core::legacy_kinds::ALIASES
            .iter()
            .any(|(old, _)| old.starts_with(asked));
        if renamed {
            format!("{}{asked}", helios_core::legacy_kinds::PREFIX)
        } else {
            asked.to_string()
        }
    }

    /// A line of the stream, its kind named for the client.
    fn line(self, line: &Value) -> std::borrow::Cow<'_, Value> {
        if !self.legacy {
            return std::borrow::Cow::Borrowed(line);
        }
        let mut line = line.clone();
        for at in ["event", "relayed"] {
            let old = line
                .get(at)
                .and_then(|e| e.get("kind"))
                .and_then(Value::as_str)
                .map(|k| helios_core::legacy_kinds::legacy(k).to_string());
            if let Some(old) = old {
                line[at]["kind"] = Value::String(old);
            }
        }
        std::borrow::Cow::Owned(line)
    }
}

/// Where a reader of a relay stands ([`bus_contract_crossing::Cursor`]), on the
/// wire: the journal's run (a process restarted has another) and the last
/// number taken in it. `{run: 0, seq: 0}` has read nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayCursor {
    pub run: u64,
    pub seq: u64,
}

impl From<RelayCursor> for Cursor {
    fn from(c: RelayCursor) -> Cursor {
        Cursor {
            run: c.run,
            seq: c.seq,
        }
    }
}

impl From<Cursor> for RelayCursor {
    fn from(c: Cursor) -> RelayCursor {
        RelayCursor {
            run: c.run,
            seq: c.seq,
        }
    }
}

/// How many relayed facts a stream reads at once.
const RELAY_BATCH: usize = 256;

/// The name of a loss on the wire (`gap`, `restarted`).
fn lost_name(lost: Option<Lost>) -> Option<&'static str> {
    lost.map(|l| match l {
        Lost::Gap => "gap",
        _ => "restarted",
    })
}

/// How often a quiet stream says it is still there.
pub const HEARTBEAT: Duration = Duration::from_secs(1);

/// How many recent events a failed wait reports.
const RECENT: usize = 10;

/// What a process shows: named entities and its bus.
#[derive(Clone)]
pub struct Inspector {
    bus: Arc<Bus>,
    entities: BTreeMap<String, Arc<dyn Describe + Send + Sync>>,
    scheduler: Option<Arc<Scheduler>>,
    /// Names this process among others served at the same address: a
    /// node restarted has another, and its sequence numbers start over.
    instance: String,
    /// The JSON-RPC subscriptions streaming, each with its stop flag.
    subscriptions: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    /// The application bus's relay, whose relayable facts a `subscribe`
    /// with `relay` streams ([`Inspector::with_relay`]).
    relay: Option<Arc<dyn Relay>>,
}

/// How often a wait looks whether it was cancelled.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// A subscription opened: the bus listened to before the journal was
/// read, so an event published in between is seen once.
struct Opened {
    rx: Receiver<Envelope>,
    journal: Vec<Envelope>,
    since: u64,
    last_seq: u64,
    gap: bool,
}

/// What a stream sends: an event, or a heartbeat with the last sequence.
enum Beat<'a> {
    Event(&'a Envelope),
    Heartbeat(u64),
}

/// The inspect endpoint's methods, as `commands.list` lists them there.
const METHODS: &[(&str, &str)] = &[
    ("node.hello", "What this endpoint and node are"),
    ("commands.list", "The methods of this endpoint"),
    ("state", "Every entity's snapshot"),
    ("describe", "One entity's snapshot"),
    ("events", "The journaled events after a sequence number"),
    (
        "wait_event",
        "The first event of a kind after a sequence number",
    ),
    ("wait_idle", "Waits until the scheduler has nothing to do"),
    ("events.subscribe", "Streams events as notifications"),
    ("events.unsubscribe", "Stops a subscription"),
];

impl Inspector {
    pub fn new(bus: Arc<Bus>) -> Self {
        Self {
            bus,
            entities: BTreeMap::new(),
            scheduler: None,
            instance: new_token()[..16].to_string(),
            subscriptions: Arc::default(),
            relay: None,
        }
    }

    /// Serves the relayable facts of the application's bus `relay` (its
    /// journal is this inspector's bus): a `subscribe` with `relay`
    /// streams them from a cursor ([`Inspector::stream_relay`]).
    pub fn with_relay(mut self, relay: Arc<dyn Relay>) -> Self {
        self.relay = Some(relay);
        self
    }

    /// Shows the jobs running now and the last ones ended, folded from
    /// the bus, as `jobs` ([`jobs::JobsView`]): what a client that lost
    /// the event stream reads to catch up.
    pub fn with_jobs(self) -> Self {
        let view = jobs::JobsView::follow(&self.bus, &self.instance);
        self.with("jobs", Arc::new(view))
    }

    /// What names this process among others at the same address.
    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// The scheduler `wait_idle` waits on.
    pub fn with_scheduler(mut self, scheduler: Arc<Scheduler>) -> Self {
        self.scheduler = Some(scheduler);
        self
    }

    /// Shows an entity under a name. The bus is always shown as `bus`.
    pub fn with(mut self, name: &str, entity: Arc<dyn Describe + Send + Sync>) -> Self {
        self.entities.insert(name.to_string(), entity);
        self
    }

    /// Answers one request.
    pub fn answer(&self, request: &Request) -> Value {
        match request {
            Request::State => {
                let mut state = serde_json::Map::new();
                state.insert("bus".into(), self.bus.describe());
                for (name, entity) in &self.entities {
                    state.insert(name.clone(), entity.describe());
                }
                json!({ "ok": true, "state": state })
            }
            Request::Describe { entity } => match (entity.as_str(), self.entities.get(entity)) {
                ("bus", _) => {
                    json!({ "ok": true, "entity": entity, "describe": self.bus.describe() })
                }
                (_, Some(e)) => json!({ "ok": true, "entity": entity, "describe": e.describe() }),
                (_, None) => {
                    let mut known: Vec<&str> = self.entities.keys().map(String::as_str).collect();
                    known.push("bus");
                    known.sort_unstable();
                    json!({ "ok": false, "error": format!("no entity `{entity}`"), "entities": known })
                }
            },
            Request::Events { since } => json!({ "ok": true, "events": self.bus.since(*since) }),
            Request::WaitEvent {
                kind,
                since,
                timeout_ms,
            } => self
                .wait_event(kind, *since, Duration::from_millis(*timeout_ms), &never)
                .unwrap_or_default(),
            Request::WaitIdle { timeout_ms } => self
                .wait_idle(Duration::from_millis(*timeout_ms), &never)
                .unwrap_or_default(),
            Request::Subscribe { .. } => json!({
                "ok": false,
                "error": "subscribe needs a connection of its own: send it on the socket",
            }),
        }
    }

    /// Answers one request on a connection: `subscribe` streams there
    /// ([`Inspector::stream`]), the others are answered by
    /// [`Inspector::answer`].
    pub fn serve_request(
        &self,
        request: &Request,
        connection: &mut Connection<'_>,
    ) -> Option<Value> {
        match request {
            Request::Subscribe {
                relay: Some(cursor),
                prefixed,
                ..
            } => {
                let names = Names { legacy: !prefixed };
                self.stream_relay(*cursor, HEARTBEAT, &never, &mut |line: &Value| {
                    connection.send(&names.line(line))
                });
                None
            }
            Request::Subscribe {
                prefix,
                since,
                prefixed,
                ..
            } => {
                let names = Names { legacy: !prefixed };
                self.stream(
                    &names.prefix(prefix),
                    *since,
                    HEARTBEAT,
                    &mut |line: &Value| connection.send(&names.line(line)),
                );
                None
            }
            other => Some(self.answer(other)),
        }
    }

    /// Streams the events whose kind starts with `prefix` to `send`, until
    /// it fails (the client has gone).
    ///
    /// First a line saying what the stream is: `{"ok": true, "subscribed":
    /// prefix, "instance", "last_seq", "gap"}`, `gap` true when the journal
    /// no longer holds every event after `since` (the subscriber missed
    /// some, and should read the state again). Then, in order, each event
    /// after `since` (`{"event": ...}`), the journaled ones first, then the
    /// new ones as they are published; and `{"heartbeat": last_seq}` after
    /// `heartbeat` without one, so a client sees a dead link and the
    /// server a gone client.
    pub fn stream(
        &self,
        prefix: &str,
        since: Option<u64>,
        heartbeat: Duration,
        send: &mut dyn FnMut(&Value) -> io::Result<()>,
    ) {
        let opened = self.open(since);
        let ack = json!({
            "ok": true,
            "subscribed": prefix,
            "instance": self.instance,
            "last_seq": opened.last_seq,
            "gap": opened.gap,
        });
        if send(&ack).is_err() {
            return;
        }
        self.pump(opened, prefix, heartbeat, &never, &mut |beat| match beat {
            Beat::Event(e) => send(&json!({ "event": e })),
            Beat::Heartbeat(seq) => send(&json!({ "heartbeat": seq })),
        });
    }

    /// Streams the relayable facts of the application's bus after
    /// `cursor` ([`Relay::resume`]) to `send`, until it fails (the client
    /// has gone) or `stop` says so.
    ///
    /// First a line saying what the stream is: `{"ok": true, "relay":
    /// true, "present", "next": {run, seq}, "lost"}` (`present`: the
    /// journal's cursor when the stream opened, for a reader that starts
    /// from now), `lost` `"restarted"` when the
    /// cursor is of another run (this process restarted: the reader reads
    /// its state again, then the facts from the run's start) or `"gap"`
    /// when the facts after it left the journal, else `null`. Then each
    /// fact, in order, `{"relayed": {seq, kind, cause, text}}`, as they
    /// are published; a later `{"lost": "gap", "next"}` if the reader fell
    /// behind the journal; and `{"heartbeat": seq}` after `heartbeat`
    /// without a line. A process with no relay answers `{"ok": false}`.
    pub fn stream_relay(
        &self,
        cursor: RelayCursor,
        heartbeat: Duration,
        stop: &dyn Fn() -> bool,
        send: &mut dyn FnMut(&Value) -> io::Result<()>,
    ) {
        let Some(relay) = &self.relay else {
            let _ = send(&json!({ "ok": false, "error": "this process relays nothing" }));
            return;
        };
        // Listened to before the first read, so a fact published in
        // between wakes the next one.
        let rx = self.bus.subscribe();
        let present = relay.cursor();
        let mut read = relay.resume(cursor.into(), RELAY_BATCH);
        let ack = json!({
            "ok": true,
            "relay": true,
            "present": RelayCursor::from(present),
            "next": RelayCursor::from(read.next),
            "lost": lost_name(read.lost),
        });
        if send(&ack).is_err() {
            return;
        }
        let mut quiet = Instant::now();
        loop {
            for e in &read.entries {
                let line = json!({ "relayed": {
                    "seq": e.seq, "kind": e.kind, "cause": e.cause, "text": e.text,
                } });
                if send(&line).is_err() {
                    return;
                }
                quiet = Instant::now();
            }
            let cursor = read.next;
            if read.entries.len() < RELAY_BATCH {
                // Caught up: wait for the bus to move.
                loop {
                    if stop() {
                        return;
                    }
                    match rx.recv_timeout(CANCEL_POLL.min(heartbeat)) {
                        Ok(_) => {
                            while rx.try_recv().is_ok() {}
                            break;
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    if quiet.elapsed() >= heartbeat {
                        quiet = Instant::now();
                        if send(&json!({ "heartbeat": cursor.seq })).is_err() {
                            return;
                        }
                    }
                }
            }
            read = relay.resume(cursor, RELAY_BATCH);
            if read.lost.is_some() {
                let line =
                    json!({ "lost": lost_name(read.lost), "next": RelayCursor::from(read.next) });
                if send(&line).is_err() {
                    return;
                }
            }
        }
    }

    /// Listens to the bus, then reads the journal after `since` (from now
    /// when not given).
    fn open(&self, since: Option<u64>) -> Opened {
        // Subscribe before reading the journal, so an event published in
        // between is seen by one or the other, and sent once.
        let rx = self.bus.subscribe();
        let last_seq = self.bus.last_seq();
        let since = since.unwrap_or(last_seq).min(last_seq);
        let journal = self.bus.since(since);
        let gap = since < last_seq && journal.first().is_none_or(|e| e.seq > since + 1);
        Opened {
            rx,
            journal,
            since,
            last_seq,
            gap,
        }
    }

    /// Sends the events of `opened` whose kind starts with `prefix`, the
    /// journaled ones first, and a heartbeat after `heartbeat` without one,
    /// until `send` fails or `stop` says so.
    fn pump(
        &self,
        opened: Opened,
        prefix: &str,
        heartbeat: Duration,
        stop: &dyn Fn() -> bool,
        send: &mut dyn FnMut(Beat<'_>) -> io::Result<()>,
    ) {
        let mut sent = opened.since;
        for e in &opened.journal {
            sent = e.seq;
            if e.kind.starts_with(prefix) && send(Beat::Event(e)).is_err() {
                return;
            }
        }
        let mut quiet = Instant::now();
        loop {
            if stop() {
                return;
            }
            match opened.rx.recv_timeout(CANCEL_POLL.min(heartbeat)) {
                Ok(e) if e.seq <= sent => {}
                Ok(e) => {
                    sent = e.seq;
                    if e.kind.starts_with(prefix) {
                        quiet = Instant::now();
                        if send(Beat::Event(&e)).is_err() {
                            return;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if quiet.elapsed() >= heartbeat {
                quiet = Instant::now();
                if send(Beat::Heartbeat(self.bus.last_seq())).is_err() {
                    return;
                }
            }
        }
    }

    /// The first `kind` event after `since`, or the refusal on timeout;
    /// `None` once `cancelled` says so.
    fn wait_event(
        &self,
        kind: &str,
        since: u64,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Option<Value> {
        // Subscribe before reading the journal, so an event published in
        // between is seen by one or the other.
        let rx = self.bus.subscribe();
        // A client of an older release names the kind without its prefix.
        let kind = helios_core::legacy_kinds::current(kind);
        let found = |e: &Envelope| e.seq > since && e.kind == kind;
        if let Some(e) = self.bus.since(since).into_iter().find(found) {
            return Some(json!({ "ok": true, "event": e }));
        }
        let deadline = Instant::now() + timeout;
        loop {
            if cancelled() {
                return None;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match rx.recv_timeout(left.min(CANCEL_POLL)) {
                Ok(e) if found(&e) => return Some(json!({ "ok": true, "event": e })),
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let journal = self.bus.since(since);
        let recent: Vec<_> = journal.iter().rev().take(RECENT).rev().collect();
        Some(json!({
            "ok": false,
            "error": format!("no `{kind}` event after seq {since} within {} ms", timeout.as_millis()),
            "last_seq": self.bus.last_seq(),
            "recent": recent,
        }))
    }

    /// Waits until the scheduler is idle, or the refusal on timeout;
    /// `None` once `cancelled` says so.
    fn wait_idle(&self, timeout: Duration, cancelled: &dyn Fn() -> bool) -> Option<Value> {
        let Some(scheduler) = &self.scheduler else {
            return Some(json!({ "ok": false, "error": "this process has no scheduler" }));
        };
        let deadline = Instant::now() + timeout;
        loop {
            if cancelled() {
                return None;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if scheduler.wait_until_idle(left.min(CANCEL_POLL)) {
                return Some(json!({ "ok": true, "idle": true }));
            }
            if left <= CANCEL_POLL {
                return Some(json!({
                    "ok": false,
                    "error": format!("still busy after {} ms", timeout.as_millis()),
                    "scheduler": scheduler.describe(),
                }));
            }
        }
    }

    /// `events.subscribe`: answers the subscription, then streams its
    /// events as `events.event` notifications and heartbeats as
    /// `events.heartbeat`, until `events.unsubscribe` or the client goes.
    fn subscribe_rpc(&self, params: Value, call: &rpc::Call) -> Result<Value, rpc::Error> {
        #[derive(Deserialize)]
        struct Params {
            #[serde(default)]
            prefix: String,
            #[serde(default)]
            since: Option<u64>,
        }
        let Params { prefix, since } =
            serde_json::from_value(params).map_err(rpc::Error::invalid_params)?;
        let id = new_token()[..16].to_string();
        let stop = Arc::new(AtomicBool::new(false));
        self.subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), stop.clone());
        let opened = self.open(since);
        let ack = json!({
            "subscription": id,
            "prefix": prefix,
            "instance": self.instance,
            "last_seq": opened.last_seq,
            "gap": opened.gap,
        });
        let (this, sink) = (self.clone(), call.sink());
        call.after_reply(move || {
            this.pump(
                opened,
                &prefix,
                HEARTBEAT,
                &|| stop.load(Ordering::SeqCst),
                &mut |beat| match beat {
                    Beat::Event(e) => {
                        sink.notify("events.event", json!({ "subscription": id, "event": e }))
                    }
                    Beat::Heartbeat(seq) => sink.notify(
                        "events.heartbeat",
                        json!({ "subscription": id, "last_seq": seq }),
                    ),
                },
            );
            this.subscriptions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
        });
        Ok(ack)
    }
}

/// Never cancelled: the older form has no `$/cancel`.
fn never() -> bool {
    false
}

impl rpc::Service for Inspector {
    fn call(&self, method: &str, params: Value, call: &rpc::Call) -> Result<Value, rpc::Error> {
        let cancelled = || call.is_cancelled();
        match method {
            "node.hello" => Ok(rpc::hello(&self.instance, "inspect", &["inspect"])),
            "commands.list" => Ok(json!({
                "commands": METHODS
                    .iter()
                    .map(|(id, label)| json!({ "id": id, "label": label, "enabled": true }))
                    .collect::<Vec<_>>(),
            })),
            "events.subscribe" | "subscribe" => self.subscribe_rpc(params, call),
            "events.unsubscribe" => {
                let id = params["subscription"].as_str().ok_or_else(|| {
                    rpc::Error::invalid_params("`events.unsubscribe` takes a `subscription`")
                })?;
                let stop = self
                    .subscriptions
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(id);
                if let Some(stop) = &stop {
                    stop.store(true, Ordering::SeqCst);
                }
                Ok(json!({ "unsubscribed": stop.is_some() }))
            }
            "state" | "describe" | "events" | "wait_event" | "wait_idle" => {
                let mut fields = params;
                if let Some(map) = fields.as_object_mut() {
                    map.insert("cmd".into(), json!(method));
                }
                let request: Request =
                    serde_json::from_value(fields).map_err(rpc::Error::invalid_params)?;
                let answer = match &request {
                    Request::WaitEvent {
                        kind,
                        since,
                        timeout_ms,
                    } => self.wait_event(
                        kind,
                        *since,
                        Duration::from_millis(*timeout_ms),
                        &cancelled,
                    ),
                    Request::WaitIdle { timeout_ms } => {
                        self.wait_idle(Duration::from_millis(*timeout_ms), &cancelled)
                    }
                    other => Some(self.answer(other)),
                };
                let timed = matches!(
                    request,
                    Request::WaitEvent { .. } | Request::WaitIdle { .. }
                );
                match answer {
                    None => Err(rpc::Error::cancelled()),
                    Some(answer) => rpc::from_answer(answer).map_err(|mut e| {
                        if timed && e.message.contains(" ms") {
                            e.code = rpc::code::TIMED_OUT;
                        }
                        e
                    }),
                }
            }
            other => Err(rpc::Error::method_not_found(other)),
        }
    }

    fn cancellable(&self, method: &str) -> bool {
        matches!(method, "wait_event" | "wait_idle")
    }
}

/// The socket where a node whose inspect socket is `inspect` serves its
/// relayable facts over bus-bridge (Unix only): `<inspect>.bridge`, next
/// to it. The desktop finds it from the inspect address it was given.
pub fn bridge_socket(inspect: &Path) -> PathBuf {
    let mut name = inspect.as_os_str().to_owned();
    name.push(".bridge");
    PathBuf::from(name)
}

/// Serves the inspector on a Unix socket at `path`, one thread per
/// connection. Returns once the socket is bound; serving goes on in the
/// background for the life of the process.
#[cfg(unix)]
pub fn serve(inspector: Inspector, path: &Path) -> io::Result<PathBuf> {
    serve_inspector(inspector, &Listen::Unix(path.to_path_buf()), None).map(|_| path.to_path_buf())
}

/// Serves the inspector on any transport (see [`transport`]).
pub fn serve_inspector(
    inspector: Inspector,
    listen: &Listen,
    token: Option<&str>,
) -> io::Result<Bound> {
    let service: Arc<dyn rpc::Service> = Arc::new(inspector.clone());
    serve_rpc_at(
        listen,
        token,
        move |request: Request, connection: &mut Connection<'_>| {
            inspector.serve_request(&request, connection)
        },
        service,
    )
}

/// Subscribes to the events of the endpoint at `address` whose kind
/// starts with `prefix`, after `since` (from now when not given): the
/// stream's first line (see [`Inspector::stream`]), and the connection to
/// read the events from, one [`Client::read`] each.
pub fn subscribe(
    address: &Address,
    prefix: &str,
    since: Option<u64>,
) -> io::Result<(Value, Client)> {
    let mut client = Client::connect(address)?;
    let ack = client.request(&Request::Subscribe {
        prefix: prefix.to_string(),
        since,
        relay: None,
        prefixed: true,
    })?;
    if ack["ok"] != true {
        return Err(io::Error::other(
            ack["error"]
                .as_str()
                .unwrap_or("subscribe refused")
                .to_string(),
        ));
    }
    Ok((ack, client))
}

/// Subscribes to the relayable facts of the endpoint at `address` after
/// `cursor`: the stream's first line (see [`Inspector::stream_relay`]),
/// and the connection to read the facts from, one [`Client::read`] each.
pub fn subscribe_relay(address: &Address, cursor: RelayCursor) -> io::Result<(Value, Client)> {
    let mut client = Client::connect(address)?;
    let ack = client.request(&Request::Subscribe {
        prefix: String::new(),
        since: None,
        relay: Some(cursor),
        prefixed: true,
    })?;
    if ack["ok"] != true {
        return Err(io::Error::other(
            ack["error"].as_str().unwrap_or("relay refused").to_string(),
        ));
    }
    Ok((ack, client))
}

/// Serves JSON lines on a Unix socket at `path`: each line is parsed as an
/// `R` and answered by `handler`; a line that does not parse is answered
/// with the reason.
#[cfg(unix)]
pub fn serve_lines<R, H>(path: &Path, handler: H) -> io::Result<PathBuf>
where
    R: DeserializeOwned,
    H: Fn(R) -> Value + Send + Sync + 'static,
{
    serve_at(&Listen::Unix(path.to_path_buf()), None, handler).map(|_| path.to_path_buf())
}

/// Sends one request to the Unix socket at `path` and reads the response.
#[cfg(unix)]
pub fn request<R: Serialize>(path: &Path, request: &R) -> io::Result<Value> {
    request_at(&Address::Unix(path.to_path_buf()), request)
}

/// The debug control endpoint's requests: actions on a running node. A node
/// serves them only in debug builds and when asked to (see `helios-node`);
/// the inspect endpoint stays read-only.
pub mod control {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use helios_core::{FaultKind, Params, Region, Value};
    use serde::{Deserialize, Serialize};

    /// The format of a file an export writes.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum FileFormat {
        /// 8-bit JPEG, at `quality` (92 when not given).
        Jpeg,
        /// 8-bit PNG.
        Png,
        /// 16-bit TIFF.
        Tiff,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "cmd", rename_all = "snake_case")]
    pub enum ControlRequest {
        /// Renders a node's output over a region, as a scheduler job.
        Render {
            node: String,
            port: String,
            region: Region,
        },
        /// Changes one parameter of a module node.
        SetParam {
            node: String,
            param: String,
            value: Value,
        },
        /// Bypasses a module node (`on`), or brings it back: bypassed, it
        /// keeps its parameters and renders as if absent, its output the
        /// input it passes through.
        #[serde(rename = "node.bypass")]
        NodeBypass {
            node: String,
            on: bool,
        },
        /// Gives a source node the image of a file (a raw or a picture),
        /// read by the node: the network stays, its results follow the new
        /// image. `path` is absolute.
        LoadSource {
            node: String,
            path: PathBuf,
        },
        /// Exports a node's output port to a file, as a scheduler job
        /// holding the destination: the file appears whole or not at all.
        /// The network is taken as it is when asked, so a later change
        /// (another source, another parameter) does not reach this export;
        /// `params` changes it for this export only, by node name. The
        /// region is the node's whole extent unless given. `path` is
        /// absolute.
        Export {
            node: String,
            #[serde(default = "output")]
            port: String,
            #[serde(default)]
            region: Option<Region>,
            path: PathBuf,
            format: FileFormat,
            #[serde(default)]
            quality: Option<u8>,
            #[serde(default)]
            params: BTreeMap<String, Params>,
            /// The batch job it is part of ([`ControlRequest::BatchStart`]),
            /// which the export's job is a child of.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            batch: Option<String>,
            /// A one-channel port (a mask) written as its values, 0 black
            /// and 1 white, rather than as light.
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            as_mask: bool,
        },
        /// Cancels a job.
        Cancel {
            job: u64,
        },
        /// Cancels a job people follow by its id (`export-3`, `batch-1`),
        /// as egui-pop's jobs centre asks: the job stops the way it gave
        /// when it started and says `helios.job.cancelled`.
        #[serde(rename = "job.cancel")]
        JobCancel {
            id: String,
        },
        /// Starts a batch job, the parent of the exports asked for with its
        /// id as `batch`: answers its id (`batch-1`). Cancelling it
        /// cancels its exports and refuses new ones.
        ///
        /// A batch whose client goes without ending it does not stay
        /// running: it fails once no export was asked for it, and none of
        /// its exports ran, for `idle_timeout_s` seconds (600 when not
        /// given); with `hold`, also as soon as the connection that asked
        /// for it closes (the client keeps it open while the batch runs).
        BatchStart {
            title: String,
            total: u64,
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            hold: bool,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            idle_timeout_s: Option<u64>,
        },
        /// Ends a batch job: cancelled when it was, or said so here;
        /// failed when some of its images failed; else finished.
        BatchEnd {
            id: String,
            #[serde(default)]
            failed: u64,
            #[serde(default)]
            cancelled: bool,
        },
        /// Stops starting jobs; running ones finish.
        Pause,
        Resume,
        /// Makes the next computation or source read of a node fail once.
        Fault {
            node: String,
            kind: FaultKind,
        },
        /// The decisions of the network: mode, status, answer, proposal.
        Decisions,
        /// Asks the network's resolver about every decision waiting on a
        /// model, and records what comes back.
        DecisionResolve,
        /// A human takes one of a proposal's options (0 is the best).
        DecisionAccept {
            node: String,
            #[serde(default)]
            option: usize,
            #[serde(default = "someone")]
            who: String,
        },
        /// A human gives the answer.
        DecisionDecide {
            node: String,
            values: Params,
            #[serde(default = "someone")]
            who: String,
        },
        /// Withdraws a decision's answer: it waits again.
        DecisionReask {
            node: String,
        },
        /// Submits a batch as a **run** of the workflow level: one journal
        /// row per item and queueing node (the export, and the decisions
        /// it depends on), worked by the node with leases and retries.
        /// The run's id follows from the network and the export settings:
        /// submitting them again extends the same run, leaving the items
        /// already in it as they are, unless `again` asks for a new run.
        /// Each item's output path is decided here, once. Answers the run's
        /// id, its derived workflow, and how many items were new, already
        /// there, or the same file twice (merged).
        #[serde(rename = "workflow.submit")]
        WorkflowSubmit {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            title: Option<String>,
            items: Vec<RunItem>,
            /// The folder the files are written to, absolute.
            out: PathBuf,
            format: FileFormat,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            quality: Option<u8>,
            /// The output file name: `{stem}` and `{ext}` are replaced.
            #[serde(default = "name_pattern")]
            name: String,
            /// The node exported: the network's first export, else its
            /// only end, unless given.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            node: Option<String>,
            /// The source given each item: the network's only source
            /// unless given.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            source: Option<String>,
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            again: bool,
            /// Exports of this run out at once, counted for this run
            /// alone (`helios batch --jobs`); the export node's
            /// concurrency, shared by its runs, when not given. Not part
            /// of the run's id: the last submission sets it.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            concurrency: Option<usize>,
        },
        /// Submits library photos to extractor nodes as an **extract
        /// run**, worked by the node on a thread of its own as a job of
        /// kind `extract`: one row per (photo, node), each concluded once.
        /// The run's id follows from its nodes and photos: submitting them
        /// again while it is at work gives back that run. Answers
        /// `{run, job, total, already}`.
        #[serde(rename = "workflow.extract")]
        WorkflowExtract {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            title: Option<String>,
            nodes: Vec<ExtractUse>,
            /// Library photos, `image:` and their content hash.
            items: Vec<String>,
        },
        /// The runs of the node: id, title, state, counts, durable.
        #[serde(rename = "workflow.runs")]
        WorkflowRuns,
        /// One page of a run's items at a queueing node (the export when
        /// not given), in the order they are worked, with their rows;
        /// `state` keeps only the items in that state: `waiting` (can be
        /// claimed, or a retry due), `upstream`, `held` (behind a failure),
        /// `asking` (waiting for a person), or another row status.
        #[serde(rename = "workflow.items")]
        WorkflowItems {
            run: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            node: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            state: Option<String>,
            #[serde(default)]
            offset: usize,
            #[serde(default = "page")]
            limit: usize,
        },
        /// Removes a run that is not at work (finished, or paused with
        /// nothing running): its run, items and rows, in the vault too.
        /// Nothing is kept of it. Refused for a run another live node
        /// works.
        #[serde(rename = "workflow.forget")]
        WorkflowForget {
            run: String,
        },
        /// Cancels a run's `items` (every item when not given) at `node`
        /// (every queueing node when not given): their rows not concluded
        /// become `cancelled`, their running exports stopped first (a file
        /// is written whole or not at all); an item not yet at a node gets
        /// a cancelled row there, so it is never claimed. Done rows stay
        /// done. An item is named by its id (`path:…`) or its absolute
        /// path. `job.cancel` of a run's job is this, for the whole run.
        #[serde(rename = "workflow.cancel")]
        WorkflowCancel {
            run: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            items: Option<Vec<String>>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            node: Option<String>,
        },
        /// Retries a run's failed and cancelled `items` (every item when
        /// not given) at `node` (every queueing node when not given):
        /// their rows are scheduled again, due now, with the full allowance
        /// of attempts. A finished run is worked again; a paused one stays
        /// paused.
        #[serde(rename = "workflow.retry")]
        WorkflowRetry {
            run: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            items: Option<Vec<String>>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            node: Option<String>,
        },
        /// Moves a run's `items` before (`top`) or after (`bottom`) every
        /// other item of the run, in the order they are claimed; items
        /// running or concluded at the export are left where they are.
        #[serde(rename = "workflow.prioritize")]
        WorkflowPrioritize {
            run: String,
            items: Vec<String>,
            to: MoveTo,
        },
        /// Stops claiming a run's items; those running finish. Kept in the
        /// run document: a paused run stays paused after a restart.
        #[serde(rename = "workflow.pause")]
        WorkflowPause {
            run: String,
        },
        /// Claims a paused run's items again.
        #[serde(rename = "workflow.resume")]
        WorkflowResume {
            run: String,
        },
        /// Imports folders, cards or files into the node's vault (a node
        /// started with `--vault`), as a job; answers once it ends, with
        /// its report and one line per file (`files`). Referenced where
        /// they are, or copied (`copy`, into `into`) and renamed by a
        /// pattern (`rename`), filed by `layout` (`day`, `month`, `year`,
        /// `flat`); exact duplicates `skip`ped, kept or listed to `ask`
        /// (`duplicates`); near-duplicate signals measured unless
        /// `signals` is false. The import is announced on the bus
        /// (`helios.library.imported`), which starts the backup strategy's
        /// after-import run; `$/cancel` stops it before its next file.
        /// `paths` are absolute. An import by `source` and `rule` is
        /// answered by the node with a promise before it becomes this
        /// request; with neither, `paths` is empty and refused.
        #[serde(rename = "library.import")]
        LibraryImport {
            #[serde(default)]
            paths: Vec<PathBuf>,
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            copy: bool,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            into: Option<PathBuf>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            rename: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            layout: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            duplicates: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            signals: Option<bool>,
        },
        /// A settings layer the node keeps in its vault (a node started
        /// with `--vault`): the person's (`user`) or a library's
        /// (`project/<library folder>`); any other name is refused.
        /// Answers its TOML `text`, `null` when the node keeps none.
        #[serde(rename = "settings.layer.get")]
        SettingsLayerGet {
            layer: String,
        },
        /// Keeps `text` (TOML) as a settings layer the node keeps (as
        /// [`ControlRequest::SettingsLayerGet`] names them). A change is
        /// announced on the bus as `helios.settings.layerChanged` (`{layer}`),
        /// for the node's other clients to read it again; the same text
        /// again announces nothing.
        #[serde(rename = "settings.layer.set")]
        SettingsLayerSet {
            layer: String,
            text: String,
        },
        /// What the node's caches hold: the cache service's tiers (budget,
        /// use) and, per scope, kind and tier, entries, bytes and
        /// statistics.
        #[serde(rename = "cache.stats")]
        CacheStats,
        /// Clears the node's caches, `kind` alone when given (never what a
        /// render running pins): answers the bytes freed.
        #[serde(rename = "cache.clear")]
        CacheClear {
            #[serde(default)]
            kind: Option<String>,
        },
        /// Measures a colour chart shot and solves its correction
        /// (`docs/design/calibration.md`): `path` an absolute picture
        /// file or a raw (measured on its sensor data), or `image` an
        /// image of the node's vault (its id, or a path the vault knows: a
        /// raw then measured with its edit's raw-front parameters);
        /// `corners` the centres of the four corner patches, `[x, y]` in
        /// its pixels as stored, in any order. Answers the report; with
        /// `save` (a node with a vault), the result is kept as a
        /// calibration profile, named `name`, and its id answered.
        #[serde(rename = "calibrate.chart")]
        CalibrateChart {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            path: Option<PathBuf>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            image: Option<String>,
            corners: [[f64; 2]; 4],
            /// The chart's id (`xrite.cc24.2014` when not given).
            #[serde(default, skip_serializing_if = "Option::is_none")]
            chart: Option<String>,
            /// Patch names left out of the solve (`B3`).
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            exclude: Vec<String>,
            #[serde(default = "yes")]
            dry_run: bool,
            /// Keep the result as a profile in the vault.
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            save: bool,
            /// The saved profile's name (the camera and the light when
            /// not given).
            #[serde(default, skip_serializing_if = "Option::is_none")]
            name: Option<String>,
        },
        /// The node's commands: every verb above as a command, with its
        /// label, the JSON schema of its params, what it needs (a vault,
        /// a window) and whether it can run here, with the reason when it
        /// cannot. `filter` keeps the commands whose id or label holds
        /// every one of its words; `enabled_only` those that can run.
        #[serde(rename = "commands.list")]
        CommandsList {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            filter: Option<String>,
            #[serde(default, skip_serializing_if = "std::ops::Not::not")]
            enabled_only: bool,
        },
        /// Runs a command by its id: its `params` are checked against its
        /// schema, then it runs as the request it names would, its change
        /// to the edit saved under `origin` (`user`, `cli`, `control`,
        /// `agent:NAME`, `script:NAME`; `control` when not given).
        #[serde(rename = "command.run")]
        CommandRun {
            id: String,
            #[serde(default = "no_params")]
            params: serde_json::Value,
            #[serde(default = "control_origin")]
            origin: String,
        },
    }

    impl ControlRequest {
        /// The verb that names it on the wire (its `cmd`), which is also
        /// its command id.
        pub fn verb(&self) -> &'static str {
            match self {
                Self::Render { .. } => "render",
                Self::SetParam { .. } => "set_param",
                Self::NodeBypass { .. } => "node.bypass",
                Self::LoadSource { .. } => "load_source",
                Self::Export { .. } => "export",
                Self::Cancel { .. } => "cancel",
                Self::JobCancel { .. } => "job.cancel",
                Self::BatchStart { .. } => "batch_start",
                Self::BatchEnd { .. } => "batch_end",
                Self::Pause => "pause",
                Self::Resume => "resume",
                Self::Fault { .. } => "fault",
                Self::Decisions => "decisions",
                Self::DecisionResolve => "decision_resolve",
                Self::DecisionAccept { .. } => "decision_accept",
                Self::DecisionDecide { .. } => "decision_decide",
                Self::DecisionReask { .. } => "decision_reask",
                Self::WorkflowSubmit { .. } => "workflow.submit",
                Self::WorkflowExtract { .. } => "workflow.extract",
                Self::WorkflowRuns => "workflow.runs",
                Self::WorkflowItems { .. } => "workflow.items",
                Self::WorkflowForget { .. } => "workflow.forget",
                Self::WorkflowCancel { .. } => "workflow.cancel",
                Self::WorkflowRetry { .. } => "workflow.retry",
                Self::WorkflowPrioritize { .. } => "workflow.prioritize",
                Self::WorkflowPause { .. } => "workflow.pause",
                Self::WorkflowResume { .. } => "workflow.resume",
                Self::LibraryImport { .. } => "library.import",
                Self::SettingsLayerGet { .. } => "settings.layer.get",
                Self::SettingsLayerSet { .. } => "settings.layer.set",
                Self::CacheStats => "cache.stats",
                Self::CacheClear { .. } => "cache.clear",
                Self::CalibrateChart { .. } => "calibrate.chart",
                Self::CommandsList { .. } => "commands.list",
                Self::CommandRun { .. } => "command.run",
            }
        }
    }

    /// Where `workflow.prioritize` moves items within their run.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum MoveTo {
        /// Before every other item: claimed next.
        Top,
        /// After every other item: claimed last.
        Bottom,
    }

    /// One extractor node of an extract run (`workflow.extract`).
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct ExtractUse {
        /// Its name in the run, unique.
        pub name: String,
        /// The extractor and its version, `id@version`.
        pub extractor: String,
        /// The nodes it waits for; none: it follows the run's root.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub after: Vec<String>,
        /// The prompt's input variables, the same for every photo.
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        pub input: serde_json::Value,
        /// Route overrides (`helios_ai::routing::Overrides`).
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        pub overrides: serde_json::Value,
    }

    /// One item of a run: an image file (absolute), with parameters
    /// changed for it only, by node name (a sidecar's edit).
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    #[serde(untagged)]
    pub enum RunItem {
        Path(PathBuf),
        With {
            path: PathBuf,
            #[serde(default)]
            params: BTreeMap<String, Params>,
        },
    }

    impl RunItem {
        pub fn path(&self) -> &std::path::Path {
            match self {
                RunItem::Path(path) | RunItem::With { path, .. } => path,
            }
        }

        pub fn params(&self) -> BTreeMap<String, Params> {
            match self {
                RunItem::Path(_) => BTreeMap::new(),
                RunItem::With { params, .. } => params.clone(),
            }
        }
    }

    fn name_pattern() -> String {
        "{stem}.{ext}".into()
    }

    fn page() -> usize {
        100
    }

    fn yes() -> bool {
        true
    }

    fn someone() -> String {
        "user".into()
    }

    fn output() -> String {
        "output".into()
    }

    fn no_params() -> serde_json::Value {
        serde_json::Value::Object(serde_json::Map::new())
    }

    fn control_origin() -> String {
        "control".into()
    }

    #[cfg(test)]
    mod tests {
        use super::ControlRequest;

        /// `verb` is the `cmd` the request is written with.
        #[test]
        fn the_verb_is_the_wire_name() {
            for request in [
                ControlRequest::Pause,
                ControlRequest::WorkflowRuns,
                ControlRequest::CacheStats,
                ControlRequest::JobCancel {
                    id: "export-1".into(),
                },
                ControlRequest::SettingsLayerGet {
                    layer: "user".into(),
                },
                ControlRequest::CommandsList {
                    filter: None,
                    enabled_only: false,
                },
                ControlRequest::CommandRun {
                    id: "pause".into(),
                    params: serde_json::json!({}),
                    origin: "cli".into(),
                },
            ] {
                let wire = serde_json::to_value(&request).unwrap();
                assert_eq!(wire["cmd"], request.verb(), "{wire}");
            }
        }

        #[test]
        fn a_command_run_defaults_its_params_and_origin() {
            let request: ControlRequest =
                serde_json::from_str(r#"{"cmd":"command.run","id":"pause"}"#).unwrap();
            assert_eq!(
                request,
                ControlRequest::CommandRun {
                    id: "pause".into(),
                    params: serde_json::json!({}),
                    origin: "control".into(),
                }
            );
        }
    }
}
