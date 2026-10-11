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
//!   or running; on timeout the error carries what is still queued or running.
//!
//! Responses: `{"ok": true, ...}` or `{"ok": false, "error": "...", ...}`.

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::{Duration, Instant};

use helios_core::{Bus, Describe, Envelope, Scheduler};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
}

/// How many recent events a failed wait reports.
const RECENT: usize = 10;

/// What a process shows: named entities and its bus.
#[derive(Clone)]
pub struct Inspector {
    bus: Arc<Bus>,
    entities: BTreeMap<String, Arc<dyn Describe + Send + Sync>>,
    scheduler: Option<Arc<Scheduler>>,
}

impl Inspector {
    pub fn new(bus: Arc<Bus>) -> Self {
        Self {
            bus,
            entities: BTreeMap::new(),
            scheduler: None,
        }
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
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => break,
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
    use std::os::unix::net::UnixListener;

    if path.exists() {
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let inspector = inspector.clone();
            thread::spawn(move || {
                let reader = BufReader::new(match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => return,
                });
                let mut writer = stream;
                for line in reader.lines() {
                    let Ok(line) = line else { return };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let response = match serde_json::from_str::<Request>(&line) {
                        Ok(request) => inspector.answer(&request),
                        Err(e) => json!({ "ok": false, "error": format!("bad request: {e}") }),
                    };
                    if writeln!(writer, "{response}").is_err() {
                        return;
                    }
                }
            });
        }
    });
    Ok(path.to_path_buf())
}

/// Sends one request to the endpoint at `path` and reads the response.
#[cfg(unix)]
pub fn request(path: &Path, request: &Request) -> io::Result<Value> {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(path)?;
    writeln!(
        stream,
        "{}",
        serde_json::to_string(request).expect("requests always serialize")
    )?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
