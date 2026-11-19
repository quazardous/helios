//! A connected node's events, followed on the desktop: its jobs, and
//! every other event a listener asks for.
//!
//! A [`Relay`] follows one node (a `helios-node` the desktop is told
//! about, [`NodeLink`]) on a thread of its own, off the UI thread, with
//! one subscription to all its events:
//!
//! - every event that is not a job's, nor of a kind the second link
//!   below takes in, goes to [`crate::node_events::NodeEvents`], which
//!   hands the kinds listeners want to them, with source `node:<name>`.
//!   Once the link is back, the relay resumes after the last event it
//!   read, so none is relayed twice; when the node restarted or its
//!   journal no longer holds what was missed, it starts over from the
//!   node's present and says `helios.node.resync` to the listeners (the second
//!   link says it instead, when there is one: one resync per hole);
//! - it reads the node's jobs whole (`describe jobs`), then follows its
//!   `helios.job.*` events after that point (`subscribe`, pushed by the node,
//!   see `helios_inspect`), and republishes each on the shell's bus with
//!   the job's id (and its parent's) as `node:<name>/<id>`: the jobs
//!   centre shows a node's batch as a parent with its exports as children,
//!   and the bus's waker asks for a frame;
//! - Cancel on such a job goes to the node's control endpoint as
//!   `job.cancel` ([`Relay::cancel`], from the jobs plugin);
//! - when the link drops (no line, not even the node's heartbeat, for
//!   [`Timing::silence`]), the node's running jobs are marked lost: paused,
//!   with a line in their log. It connects again every [`Timing::retry`];
//!   once back, it catches up from the node's jobs: the ones still running
//!   resume where they are, the ones that ended meanwhile end as they did
//!   (or as failed, when the node no longer remembers how), the new ones
//!   start. Still lost after [`Timing::forget_after`], they fail and are
//!   forgotten (`helios.node.jobs_forgotten`, which the shell reads to take them
//!   out of the jobs centre). A node that restarted (another instance)
//!   leaves its old jobs failed.
//!
//! Each change of the link is said on the bus too: `helios.node.link` with the
//! node's name, `up`, and why it dropped.
//!
//! When the desktop's application bus has a relay (`bus-contract`'s
//! `Relay`, Helios's fact bus behind it), a second link per node takes in
//! the node's relayable facts (`subscribe` with `relay`, see
//! `helios_inspect::Inspector::stream_relay`): each one `ingest`ed with
//! source `relayed:<name>`, decoded by the codec the desktop declares for
//! its point, and heard by that point's listeners as a fact published
//! here (`NodeEvents` among them, for its own listeners: those kinds
//! come by this link only). It resumes from its cursor; a node restarted
//! (`restarted`) or a cursor the node's journal no longer holds (`gap`)
//! says `helios.node.resync` to the listeners, then goes on from the node's
//! facts.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use egui_pop::api::bus::bus_contract;
use egui_pop::api::bus::bus_contract_crossing::{BusCrossing as _, Origin, RelayedEntry};
use helios_core::jobs::{
    CANCELLED, CANCELLING, FAILED, FINISHED, ITEM_FAILED, LOG, PAUSED, PROGRESS, RESUMED, STARTED,
};
use helios_core::{Bus, legacy_kinds};
use helios_inspect::control::ControlRequest;
use helios_inspect::{Address, Client, Closer, Request};

use crate::node_events::NodeEvents;
use serde_json::{Value, json};

/// What a node's job id starts with on the desktop.
pub const PREFIX: &str = "node:";
/// The bus event listing a lost node's jobs to forget (`node`, `ids`).
pub const FORGOTTEN: &str = "helios.node.jobs_forgotten";
/// The bus event saying a link went up or down (`node`, `up`, `error`).
pub const LINK: &str = "helios.node.link";
/// The source of the events a relay publishes.
const SOURCE: &str = "relay";

/// The fact kinds the relays publish (a node's own are republished).
pub const FACTS: &[helios_core::FactDecl] = &[
    helios_core::FactDecl::json(FORGOTTEN, "{ node, ids }", "A lost node's jobs to forget"),
    helios_core::FactDecl::json(
        LINK,
        "{ node, up, error? }",
        "A node's link went up or down",
    ),
    helios_core::FactDecl::json(
        crate::node_events::RESYNC,
        "{ node, event: { seq, source, kind, payload: { node, reason } } }",
        "A node's events have a hole: read its state again",
    ),
];
/// How long a cancel waits for the node's answer.
const CANCEL_TIMEOUT: Duration = Duration::from_secs(3);

/// A node's job id on the desktop: `node:<node>/<id>`.
pub fn node_job(node: &str, id: &str) -> String {
    format!("{PREFIX}{node}/{id}")
}

/// The node and its own id of a desktop job id, when it is a node's.
pub fn split_node_job(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(PREFIX)?.split_once('/')
}

/// The kind of a job event, as the bus names it (`&'static`); `None`
/// for an event a relay does not carry. A node of an older release says
/// it without its prefix (`job.started`).
fn job_kind(kind: &str) -> Option<&'static str> {
    let kind = legacy_kinds::current(kind);
    [
        STARTED,
        PROGRESS,
        ITEM_FAILED,
        LOG,
        CANCELLING,
        FINISHED,
        FAILED,
        CANCELLED,
        PAUSED,
        RESUMED,
    ]
    .into_iter()
    .find(|k| *k == kind)
}

/// A line read from a node, the kind it carries (an event's, a relayed
/// fact's) under its name now: a node of an older release says it
/// without its prefix (`vault.field_changed`), and the desktop takes it
/// in under its new one, once (`helios_core::legacy_kinds`).
fn current_kinds(mut line: Value) -> Value {
    for at in ["event", "relayed"] {
        let renamed = line
            .get(at)
            .and_then(|e| e.get("kind"))
            .and_then(Value::as_str)
            .filter(|k| legacy_kinds::current(k) != *k)
            .map(|k| legacy_kinds::current(k).to_string());
        if let Some(kind) = renamed {
            line[at]["kind"] = Value::String(kind);
        }
    }
    line
}

/// A node the desktop follows: its name on the desktop, its inspect
/// endpoint, and its control endpoint (debug or batch) for Cancel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeLink {
    pub name: String,
    pub inspect: Address,
    pub control: Option<Address>,
}

impl NodeLink {
    /// `NAME=INSPECT[,CONTROL]`, each address a socket path or
    /// `tcp://127.0.0.1:PORT` (which needs the node's session token).
    pub fn parse(text: &str, token: Option<&str>) -> Result<NodeLink, String> {
        let (name, rest) = text
            .split_once('=')
            .ok_or_else(|| format!("`{text}` is not NAME=INSPECT[,CONTROL]"))?;
        let (inspect, control) = match rest.split_once(',') {
            Some((i, c)) => (i, Some(c)),
            None => (rest, None),
        };
        let address = |a: &str| Address::parse(a, token.map(str::to_string));
        let link = NodeLink {
            name: name.to_string(),
            inspect: address(inspect)?,
            control: control.map(address).transpose()?,
        };
        link.check()?;
        Ok(link)
    }

    fn check(&self) -> Result<(), String> {
        if self.name.is_empty() || self.name.contains('/') {
            return Err(format!(
                "`{}`: a node's name is not empty and has no `/`",
                self.name
            ));
        }
        Ok(())
    }
}

/// How a relay waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// Between two attempts to connect.
    pub retry: Duration,
    /// How long a lost node's jobs stay, marked lost, before they are
    /// forgotten.
    pub forget_after: Duration,
    /// How long without a line from the node (it sends a heartbeat every
    /// second) before the link is taken as dropped.
    pub silence: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            retry: Duration::from_secs(1),
            forget_after: Duration::from_mins(1),
            silence: Duration::from_secs(5),
        }
    }
}

/// Follows a node's jobs while it lives; dropped, it stops.
pub struct Relay {
    link: NodeLink,
    /// Dropped with the relay: its threads stop.
    _stop: Sender<()>,
    _stop_in: Option<Sender<()>>,
    closer: Arc<Mutex<Option<Closer>>>,
    closer_in: Arc<Mutex<Option<Closer>>>,
}

impl Relay {
    /// Starts following `link`: its jobs republished on `bus`, its other
    /// events handed to `events`, its relayable facts taken in by the
    /// relay of `hooks` (the application's bus), when it has one.
    pub fn start(
        link: NodeLink,
        bus: Arc<Bus>,
        events: NodeEvents,
        hooks: Option<Arc<dyn bus_contract::Bus>>,
        timing: Timing,
    ) -> Result<Relay, String> {
        link.check()?;
        let closer_in: Arc<Mutex<Option<Closer>>> = Arc::default();
        let taken_after = Arc::new(AtomicU64::new(NOT_TAKEN));
        let stop_in = match hooks.filter(|h| h.relay().is_some()) {
            None => None,
            Some(hooks) => {
                let (stop_tx, stop) = channel();
                let mut take = TakeIn {
                    link: link.clone(),
                    hooks,
                    events: events.clone(),
                    timing,
                    stop,
                    closer: closer_in.clone(),
                    cursor: None,
                    taken_after: taken_after.clone(),
                };
                std::thread::Builder::new()
                    .name(format!("helios-relay-in-{}", link.name))
                    .spawn(move || take.run())
                    .map_err(|e| format!("cannot follow node `{}`: {e}", link.name))?;
                Some(stop_tx)
            }
        };
        let (stop_tx, stop) = channel();
        let closer: Arc<Mutex<Option<Closer>>> = Arc::default();
        let mut follow = Follow {
            link: link.clone(),
            bus,
            events,
            resume: None,
            timing,
            stop,
            closer: closer.clone(),
            instance: None,
            jobs: Vec::new(),
            lost_at: None,
            up: false,
            takes_in: stop_in.as_ref().map(|_| taken_after),
        };
        std::thread::Builder::new()
            .name(format!("helios-relay-{}", link.name))
            .spawn(move || follow.run())
            .map_err(|e| format!("cannot follow node `{}`: {e}", link.name))?;
        Ok(Relay {
            link,
            _stop: stop_tx,
            _stop_in: stop_in,
            closer,
            closer_in,
        })
    }

    pub fn link(&self) -> &NodeLink {
        &self.link
    }

    /// Asks the node to cancel its job `id` (its own id, without the
    /// `node:` prefix): `job.cancel` on its control endpoint.
    pub fn cancel(&self, id: &str) -> Result<Value, String> {
        let name = &self.link.name;
        let control = self.link.control.as_ref().ok_or_else(|| {
            format!("node `{name}` was given no control endpoint: its jobs are cancelled there")
        })?;
        let unreachable = |e: std::io::Error| format!("cannot reach node `{name}`: {e}");
        let mut client = Client::connect(control).map_err(unreachable)?;
        client
            .set_read_timeout(Some(CANCEL_TIMEOUT))
            .map_err(unreachable)?;
        let answer = client
            .request(&ControlRequest::JobCancel { id: id.into() })
            .map_err(unreachable)?;
        if answer["ok"] == true {
            Ok(json!({ "cancelling": node_job(name, id) }))
        } else {
            Err(answer["error"]
                .as_str()
                .unwrap_or("the node refused")
                .to_string())
        }
    }

    /// Drops the current link, as a network failure would: the relay
    /// connects again (and catches up) at its next attempt.
    pub fn drop_link(&self) {
        for closer in [&self.closer, &self.closer_in] {
            if let Some(c) = closer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
            {
                c.close();
            }
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.drop_link();
    }
}

/// A running job of the node, as the desktop was told.
struct Known {
    /// The node's own id.
    id: String,
    /// Marked lost: the link dropped while it ran.
    lost: bool,
}

/// The relay's thread.
struct Follow {
    link: NodeLink,
    bus: Arc<Bus>,
    events: NodeEvents,
    /// The node's instance and the `seq` of the last event read from it:
    /// where the next link resumes.
    resume: Option<(String, u64)>,
    timing: Timing,
    stop: Receiver<()>,
    closer: Arc<Mutex<Option<Closer>>>,
    /// The node's instance the jobs below belong to.
    instance: Option<String>,
    /// The node's running jobs, in the order they started.
    jobs: Vec<Known>,
    /// When the link dropped with jobs running.
    lost_at: Option<Instant>,
    up: bool,
    /// When a second link takes in the node's relayable facts
    /// ([`TakeIn`]): the node's `seq` after which their kinds are its
    /// own ([`NOT_TAKEN`] until it is linked: an older node serves no
    /// relay). Once linked, the resyncs are its own.
    takes_in: Option<Arc<AtomicU64>>,
}

impl Follow {
    fn stopped(&self) -> bool {
        !matches!(self.stop.try_recv(), Err(TryRecvError::Empty))
    }

    fn run(&mut self) {
        loop {
            let error = match self.session() {
                Ok(()) => return,
                Err(e) => e,
            };
            if self.stopped() {
                return;
            }
            *self.closer.lock().unwrap_or_else(PoisonError::into_inner) = None;
            if self.up {
                self.up = false;
                self.bus.publish_value(
                    SOURCE,
                    None,
                    LINK,
                    json!({ "node": self.link.name, "up": false, "error": error }),
                );
            }
            self.mark_lost();
            if self
                .lost_at
                .is_some_and(|at| at.elapsed() >= self.timing.forget_after)
            {
                self.forget();
            }
            match self.stop.recv_timeout(self.timing.retry) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// One link: catch up, then follow until it drops (`Err`) or the
    /// relay stops (`Ok`).
    fn session(&mut self) -> Result<(), String> {
        let unreachable = |e: std::io::Error| e.to_string();
        let jobs = helios_inspect::request_at(
            &self.link.inspect,
            &Request::Describe {
                entity: "jobs".into(),
            },
        )
        .map_err(unreachable)?;
        if jobs["ok"] != true {
            return Err(format!("the node shows no jobs: {}", jobs["error"]));
        }
        let jobs = &jobs["describe"];
        let seq = jobs["seq"].as_u64().unwrap_or(0);
        let instance = jobs["instance"].as_str().unwrap_or_default().to_string();
        // Where the last link left off, when the same node still holds it.
        let resume = self
            .resume
            .as_ref()
            .filter(|(i, last)| *i == instance && *last <= seq)
            .map(|(_, last)| *last);
        let mut resync = self.resume.is_some() && resume.is_none();
        if resync && let Some(after) = &self.takes_in {
            // Another run: the second link takes in its facts from its
            // first.
            let _ = after.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |a| {
                (a != NOT_TAKEN).then_some(0)
            });
        }
        let (mut ack, mut client) = self.subscribe(resume.unwrap_or(seq))?;
        if resume.is_some() && ack["gap"] == true && ack["instance"] == jobs["instance"] {
            // What was missed is gone: start over from the jobs' point.
            drop(client);
            resync = true;
            (ack, client) = self.subscribe(seq)?;
        }
        if ack["instance"] != jobs["instance"] || ack["gap"] == true {
            return Err("the node moved on while the desktop caught up".into());
        }
        self.resume = Some((instance, resume.filter(|_| !resync).unwrap_or(seq)));
        client
            .set_read_timeout(Some(self.timing.silence))
            .map_err(unreachable)?;
        *self.closer.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(client.closer().map_err(unreachable)?);
        self.catch_up(jobs);
        if !self.up {
            self.up = true;
            self.bus.publish_value(
                SOURCE,
                None,
                LINK,
                json!({ "node": self.link.name, "up": true }),
            );
        }
        // With a second link up, a hole shows there too: it says the one
        // resync.
        let linked = self
            .takes_in
            .as_ref()
            .is_some_and(|a| a.load(Ordering::SeqCst) != NOT_TAKEN);
        if resync && !linked {
            self.events
                .resync(&self.link.name, "the node's events were missed");
        }
        loop {
            if self.stopped() {
                return Ok(());
            }
            let line = current_kinds(client.read().map_err(unreachable)?);
            if let Some(event) = line.get("event") {
                if let (Some(n), Some((_, last))) = (event["seq"].as_u64(), self.resume.as_mut()) {
                    *last = n;
                }
                // The jobs as `describe jobs` gave them hold the job
                // events up to `seq`.
                if event["kind"]
                    .as_str()
                    .is_some_and(|k| k.starts_with("helios.job."))
                {
                    if event["seq"].as_u64().is_none_or(|n| n > seq) {
                        self.relay(event);
                    }
                } else if !self.taken_in(event) {
                    self.events.publish(&self.link.name, event);
                }
            }
        }
    }

    /// Whether `event`'s kind comes by the second link ([`TakeIn`]),
    /// so the listeners hear it once.
    fn taken_in(&self, event: &Value) -> bool {
        let Some(after) = &self.takes_in else {
            return false;
        };
        let after = after.load(Ordering::SeqCst);
        after != NOT_TAKEN
            && event["seq"].as_u64().is_some_and(|n| n > after)
            && event["kind"]
                .as_str()
                .is_some_and(|k| self.events.taken_in(k))
    }

    /// Every event of the node after `since`, on a connection of its own.
    fn subscribe(&self, since: u64) -> Result<(Value, Client), String> {
        helios_inspect::subscribe(&self.link.inspect, "", Some(since)).map_err(|e| e.to_string())
    }

    /// Publishes a job event of the node on the shell's bus, its ids
    /// made the desktop's.
    fn publish(&self, kind: &'static str, mut data: Value) {
        let name = &self.link.name;
        for field in ["id", "parent"] {
            if let Some(id) = data[field].as_str() {
                data[field] = json!(node_job(name, id));
            }
        }
        self.bus.publish_value(SOURCE, None, kind, data);
    }

    /// One event of the stream.
    fn relay(&mut self, event: &Value) {
        let Some(kind) = event["kind"].as_str().and_then(job_kind) else {
            return;
        };
        let payload = &event["payload"];
        let Some(id) = payload["id"].as_str() else {
            return;
        };
        match kind {
            STARTED if !self.jobs.iter().any(|k| k.id == id) => {
                self.jobs.push(Known {
                    id: id.to_string(),
                    lost: false,
                });
            }
            FINISHED | FAILED | CANCELLED => self.jobs.retain(|k| k.id != id),
            _ => {}
        }
        self.publish(kind, payload.clone());
    }

    /// Brings the desktop's view of the node's jobs to what `jobs` (the
    /// node's `describe jobs`) says.
    fn catch_up(&mut self, jobs: &Value) {
        let name = self.link.name.clone();
        let instance = jobs["instance"].as_str().map(str::to_string);
        if self.instance.is_some() && self.instance != instance {
            for k in std::mem::take(&mut self.jobs) {
                self.publish(
                    FAILED,
                    json!({ "id": k.id, "error": format!("lost: node {name} restarted") }),
                );
            }
        }
        self.instance = instance;
        let empty = Vec::new();
        let running = jobs["running"].as_array().unwrap_or(&empty);
        let ended = jobs["ended"].as_array().unwrap_or(&empty);
        let id_of = |r: &Value| r["started"]["id"].as_str().map(str::to_string);
        // Ended while the desktop did not follow: as they did, else failed.
        for k in std::mem::take(&mut self.jobs) {
            if running
                .iter()
                .any(|r| id_of(r).as_deref() == Some(k.id.as_str()))
            {
                self.jobs.push(k);
                continue;
            }
            let end = ended
                .iter()
                .rev()
                .find(|e| e["data"]["id"].as_str() == Some(k.id.as_str()))
                .and_then(|e| Some((job_kind(e["kind"].as_str()?)?, e["data"].clone())));
            match end {
                Some((kind, data)) => self.publish(kind, data),
                None => self.publish(
                    FAILED,
                    json!({
                        "id": k.id,
                        "error": format!("ended while node {name} was out of reach"),
                    }),
                ),
            }
        }
        for r in running {
            let Some(id) = id_of(r) else { continue };
            let mut progress = r["progress"].as_object().cloned().unwrap_or_default();
            match self.jobs.iter().position(|k| k.id == id) {
                Some(i) if self.jobs[i].lost => {
                    self.jobs[i].lost = false;
                    self.publish(RESUMED, json!({ "id": id }));
                    self.publish(
                        LOG,
                        json!({ "id": id, "text": format!("node {name} is back") }),
                    );
                }
                Some(_) => {}
                None => {
                    self.jobs.push(Known {
                        id: id.clone(),
                        lost: false,
                    });
                    self.publish(STARTED, r["started"].clone());
                }
            }
            if !progress.is_empty() {
                progress.insert("id".into(), json!(id));
                self.publish(PROGRESS, Value::Object(progress));
            }
            if r["cancelling"] == true {
                self.publish(CANCELLING, json!({ "id": id }));
            }
        }
        self.lost_at = None;
    }

    /// The link dropped: the jobs running are marked lost, once.
    fn mark_lost(&mut self) {
        let name = self.link.name.clone();
        let mut marked = Vec::new();
        for k in self.jobs.iter_mut().filter(|k| !k.lost) {
            k.lost = true;
            marked.push(k.id.clone());
        }
        for id in marked {
            self.publish(
                LOG,
                json!({ "id": id, "text": format!("lost: node {name} is out of reach") }),
            );
            self.publish(PAUSED, json!({ "id": id }));
        }
        if self.lost_at.is_none() && !self.jobs.is_empty() {
            self.lost_at = Some(Instant::now());
        }
    }

    /// Still lost after the delay: the jobs fail, and the shell forgets
    /// them.
    fn forget(&mut self) {
        let name = self.link.name.clone();
        let lost = std::mem::take(&mut self.jobs);
        let mut ids = Vec::new();
        for k in &lost {
            self.publish(
                FAILED,
                json!({ "id": k.id, "error": format!("lost: node {name} is out of reach") }),
            );
            ids.push(node_job(&name, &k.id));
        }
        self.lost_at = None;
        if !ids.is_empty() {
            self.bus
                .publish_value(SOURCE, None, FORGOTTEN, json!({ "node": name, "ids": ids }));
        }
    }
}

/// The source of a node's facts taken in: `relayed:<name>`.
pub const RELAYED: &str = "relayed:";

/// [`TakeIn`] not linked yet: the event stream hands on every kind.
const NOT_TAKEN: u64 = u64::MAX;

/// The thread taking in a node's relayable facts.
struct TakeIn {
    link: NodeLink,
    /// The desktop's application bus: its relay takes them in.
    hooks: Arc<dyn bus_contract::Bus>,
    events: NodeEvents,
    timing: Timing,
    stop: Receiver<()>,
    closer: Arc<Mutex<Option<Closer>>>,
    /// Where the next link resumes: the node's run and the last fact
    /// taken in. `None` before the first link, which starts from the
    /// node's present.
    cursor: Option<helios_inspect::RelayCursor>,
    /// The node's `seq` after which the relayable kinds come by this
    /// link, shared with the event stream ([`Follow::taken_in`]).
    taken_after: Arc<AtomicU64>,
}

impl TakeIn {
    fn stopped(&self) -> bool {
        !matches!(self.stop.try_recv(), Err(TryRecvError::Empty))
    }

    fn run(&mut self) {
        loop {
            if self.session().is_ok() || self.stopped() {
                return;
            }
            *self.closer.lock().unwrap_or_else(PoisonError::into_inner) = None;
            match self.stop.recv_timeout(self.timing.retry) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// One link: take in the node's facts until it drops (`Err`) or the
    /// relay stops (`Ok`).
    fn session(&mut self) -> Result<(), String> {
        let unreachable = |e: std::io::Error| e.to_string();
        let from = self.cursor.unwrap_or_default();
        let (ack, mut client) =
            helios_inspect::subscribe_relay(&self.link.inspect, from).map_err(unreachable)?;
        let cursor_of = |v: &Value| helios_inspect::RelayCursor {
            run: v["run"].as_u64().unwrap_or(0),
            seq: v["seq"].as_u64().unwrap_or(0),
        };
        // The first link starts from the node's present: what it
        // published before the desktop followed it is its state, read
        // whole by whoever needs it.
        let skip_to = match self.cursor {
            None => Some(cursor_of(&ack["present"]).seq),
            Some(_) => None,
        };
        let run = cursor_of(&ack["next"]).run;
        if self.cursor.is_some() && ack["lost"].is_string() {
            self.resync(&ack["lost"]);
        }
        match skip_to {
            // From the node's present on: what came before went by the
            // event stream.
            Some(present) => self.taken_after.store(present, Ordering::SeqCst),
            // Another run, read from its first fact.
            None if ack["lost"] == "restarted" => self.taken_after.store(0, Ordering::SeqCst),
            None => {}
        }
        self.cursor = Some(match (self.cursor, skip_to) {
            (Some(c), None) if c.run == run => c,
            (_, seq) => helios_inspect::RelayCursor {
                run,
                seq: seq.unwrap_or(0),
            },
        });
        client
            .set_read_timeout(Some(self.timing.silence))
            .map_err(unreachable)?;
        *self.closer.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(client.closer().map_err(unreachable)?);
        let Some(relay) = self.hooks.relay() else {
            return Ok(());
        };
        let source = Origin::process(format!("{RELAYED}{}", self.link.name));
        loop {
            if self.stopped() {
                return Ok(());
            }
            let line = current_kinds(client.read().map_err(unreachable)?);
            if let Some(e) = line.get("relayed") {
                let seq = e["seq"].as_u64().unwrap_or(0);
                if skip_to.is_some_and(|s| seq <= s) {
                    continue;
                }
                let mut entry = RelayedEntry::new(
                    seq,
                    e["kind"].as_str().unwrap_or_default(),
                    e["text"].as_str().unwrap_or_default(),
                );
                entry.cause = e["cause"].as_u64();
                if let Err(refused) = relay.ingest(&source, entry) {
                    log::warn!(target: "helios", "node `{}`: {refused}", self.link.name);
                }
                if let Some(c) = self.cursor.as_mut() {
                    c.seq = seq;
                }
            } else if line["lost"].is_string() {
                self.resync(&line["lost"]);
                self.cursor = Some(cursor_of(&line["next"]));
            } else if let (Some(seq), Some(c)) = (line["heartbeat"].as_u64(), self.cursor.as_mut())
            {
                c.seq = c.seq.max(seq);
            }
        }
    }

    /// The node's facts have a hole: every listener reads its state
    /// again.
    fn resync(&self, lost: &Value) {
        let reason = match lost.as_str() {
            Some("restarted") => "the node restarted",
            _ => "the node's facts were missed",
        };
        self.events.resync(&self.link.name, reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_job_ids_and_links_parse() {
        assert_eq!(node_job("box", "batch-1"), "node:box/batch-1");
        assert_eq!(
            split_node_job("node:box/export-2"),
            Some(("box", "export-2"))
        );
        assert_eq!(split_node_job("export-2"), None);
        let link = NodeLink::parse("box=/run/node.sock,/run/batch.sock", None).unwrap();
        assert_eq!(link.name, "box");
        assert_eq!(link.control, Some(Address::Unix("/run/batch.sock".into())));
        assert!(
            NodeLink::parse("box=/run/node.sock", None)
                .unwrap()
                .control
                .is_none()
        );
        assert!(NodeLink::parse("/run/node.sock", None).is_err());
        assert!(NodeLink::parse("a/b=/run/node.sock", None).is_err());
        assert!(
            NodeLink::parse("box=tcp://127.0.0.1:7000", None)
                .unwrap_err()
                .contains("token")
        );
    }
}
