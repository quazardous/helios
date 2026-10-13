//! The jobs of a connected node, followed on the desktop.
//!
//! A [`Relay`] follows one node (a `helios-node` the desktop is told
//! about, [`NodeLink`]) on a thread of its own, off the UI thread:
//!
//! - it reads the node's jobs whole (`describe jobs`), then subscribes to
//!   its `job.*` events after that point (`subscribe`, pushed by the node,
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
//!   forgotten (`node.jobs_forgotten`, which the shell reads to take them
//!   out of the jobs centre). A node that restarted (another instance)
//!   leaves its old jobs failed.
//!
//! Each change of the link is said on the bus too: `node.link` with the
//! node's name, `up`, and why it dropped.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helios_core::Bus;
use helios_core::jobs::{
    CANCELLED, CANCELLING, FAILED, FINISHED, ITEM_FAILED, LOG, PROGRESS, STARTED,
};
use helios_inspect::control::ControlRequest;
use helios_inspect::{Address, Client, Closer, Request};
use serde_json::{Value, json};

/// What a node's job id starts with on the desktop.
pub const PREFIX: &str = "node:";
/// The bus event listing a lost node's jobs to forget (`node`, `ids`).
pub const FORGOTTEN: &str = "node.jobs_forgotten";
/// The bus event saying a link went up or down (`node`, `up`, `error`).
pub const LINK: &str = "node.link";
/// egui-pop's jobs events Helios's own jobs never send.
const PAUSED: &str = "job.paused";
const RESUMED: &str = "job.resumed";
/// The source of the events a relay publishes.
const SOURCE: &str = "relay";
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
/// for an event a relay does not carry.
fn job_kind(kind: &str) -> Option<&'static str> {
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
    /// Dropped with the relay: its thread stops.
    _stop: Sender<()>,
    closer: Arc<Mutex<Option<Closer>>>,
}

impl Relay {
    /// Starts following `link`, republishing on `bus`.
    pub fn start(link: NodeLink, bus: Arc<Bus>, timing: Timing) -> Result<Relay, String> {
        link.check()?;
        let (stop_tx, stop) = channel();
        let closer: Arc<Mutex<Option<Closer>>> = Arc::default();
        let mut follow = Follow {
            link: link.clone(),
            bus,
            timing,
            stop,
            closer: closer.clone(),
            instance: None,
            jobs: Vec::new(),
            lost_at: None,
            up: false,
        };
        std::thread::Builder::new()
            .name(format!("helios-relay-{}", link.name))
            .spawn(move || follow.run())
            .map_err(|e| format!("cannot follow node `{}`: {e}", link.name))?;
        Ok(Relay {
            link,
            _stop: stop_tx,
            closer,
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
        if let Some(c) = self
            .closer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            c.close();
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
        let (ack, mut client) = helios_inspect::subscribe(&self.link.inspect, "job.", Some(seq))
            .map_err(unreachable)?;
        if ack["instance"] != jobs["instance"] || ack["gap"] == true {
            return Err("the node moved on while the desktop caught up".into());
        }
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
        loop {
            if self.stopped() {
                return Ok(());
            }
            let line = client.read().map_err(unreachable)?;
            if let Some(event) = line.get("event") {
                self.relay(event);
            }
        }
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
