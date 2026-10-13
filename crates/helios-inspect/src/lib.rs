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
//!   published (see [`Inspector::stream`]).
//!
//! Responses: `{"ok": true, ...}` or `{"ok": false, "error": "...", ...}`.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use transport::Connection;

use helios_core::{Bus, Describe, Envelope, Scheduler};

pub mod jobs;
pub mod transport;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub use transport::{
    Address, Bound, Client, Closer, Listen, new_token, request_at, serve_at, serve_connections_at,
    write_token,
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
    },
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
}

impl Inspector {
    pub fn new(bus: Arc<Bus>) -> Self {
        Self {
            bus,
            entities: BTreeMap::new(),
            scheduler: None,
            instance: new_token()[..16].to_string(),
        }
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
            } => self.wait_event(kind, *since, Duration::from_millis(*timeout_ms)),
            Request::WaitIdle { timeout_ms } => match &self.scheduler {
                None => json!({ "ok": false, "error": "this process has no scheduler" }),
                Some(s) if s.wait_until_idle(Duration::from_millis(*timeout_ms)) => {
                    json!({ "ok": true, "idle": true })
                }
                Some(s) => json!({
                    "ok": false,
                    "error": format!("still busy after {timeout_ms} ms"),
                    "scheduler": s.describe(),
                }),
            },
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
            Request::Subscribe { prefix, since } => {
                self.stream(prefix, *since, HEARTBEAT, &mut |line: &Value| {
                    connection.send(line)
                });
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
        // Subscribe before reading the journal, so an event published in
        // between is seen by one or the other, and sent once.
        let rx = self.bus.subscribe();
        let last_seq = self.bus.last_seq();
        let since = since.unwrap_or(last_seq).min(last_seq);
        let journal = self.bus.since(since);
        let gap = since < last_seq && journal.first().is_none_or(|e| e.seq > since + 1);
        let ack = json!({
            "ok": true,
            "subscribed": prefix,
            "instance": self.instance,
            "last_seq": last_seq,
            "gap": gap,
        });
        if send(&ack).is_err() {
            return;
        }
        let mut sent = since;
        for e in journal {
            sent = e.seq;
            if e.kind.starts_with(prefix) && send(&json!({ "event": e })).is_err() {
                return;
            }
        }
        loop {
            match rx.recv_timeout(heartbeat) {
                Ok(e) if e.seq <= sent => {}
                Ok(e) => {
                    sent = e.seq;
                    if e.kind.starts_with(prefix) && send(&json!({ "event": e })).is_err() {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if send(&json!({ "heartbeat": self.bus.last_seq() })).is_err() {
                        return;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn wait_event(&self, kind: &str, since: u64, timeout: Duration) -> Value {
        // Subscribe before reading the journal, so an event published in
        // between is seen by one or the other.
        let rx = self.bus.subscribe();
        let found = |e: &Envelope| e.seq > since && e.kind == kind;
        if let Some(e) = self.bus.since(since).into_iter().find(found) {
            return json!({ "ok": true, "event": e });
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(e) if found(&e) => return json!({ "ok": true, "event": e }),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
        let journal = self.bus.since(since);
        let recent: Vec<_> = journal.iter().rev().take(RECENT).rev().collect();
        json!({
            "ok": false,
            "error": format!("no `{kind}` event after seq {since} within {} ms", timeout.as_millis()),
            "last_seq": self.bus.last_seq(),
            "recent": recent,
        })
    }
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
    serve_connections_at(listen, token, move |request: Request, connection| {
        inspector.serve_request(&request, connection)
    })
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
        },
        /// Cancels a job.
        Cancel {
            job: u64,
        },
        /// Cancels a job people follow by its id (`export-3`, `batch-1`),
        /// as egui-pop's jobs centre asks: the job stops the way it gave
        /// when it started and says `job.cancelled`.
        #[serde(rename = "job.cancel")]
        JobCancel {
            id: String,
        },
        /// Starts a batch job, the parent of the exports asked for with its
        /// id as `batch`: answers its id (`batch-1`). Cancelling it
        /// cancels its exports and refuses new ones.
        BatchStart {
            title: String,
            total: u64,
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
    }

    fn someone() -> String {
        "user".into()
    }

    fn output() -> String {
        "output".into()
    }
}
