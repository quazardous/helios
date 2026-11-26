//! The Queue panel's source: the workflow level's queues, as the
//! connected nodes run them, and the long jobs that are not runs, as
//! promises (see `crate::queue`).
//!
//! - **The model is the node's.** A queue is a run's rows at a queueing
//!   graph node (`helios_node::workflow::runs`): one row per item, claimed
//!   by lease, retried by the node's policy. The desktop keeps no queue of
//!   its own: it reads each connected node's runs once
//!   (`workflow.runs`, then `workflow.items` per queueing node, off the
//!   interface's thread), then follows them on the bus: the rows changed
//!   (`helios.workflow.rows`) and the runs' lives (`helios.workflow.run`),
//!   heard through [`crate::node_events::NodeEvents`]. A node whose
//!   events had a hole ([`crate::node_events::RESYNC`]) is read again.
//! - **Promise items.** A long job that is not a run (an export started
//!   here, the trash, a backup) shows as its promise
//!   (`helios_node_core::job_promises`, followed by
//!   [`crate::activity::Activity`]) at the graph node its job named
//!   (`node` in `helios.job.started`).
//! - **Job items.** A job of the desktop's jobs centre that named a graph
//!   node and has no promise of its own (each export of a batch: the
//!   promise is the batch's; a desktop without a node) shows as itself,
//!   as the jobs centre holds it; its Cancel is `job.cancel` too.
//! - **The verbs are bus commands.** Retry, Move to the top or the bottom
//!   and Cancel run [`RETRY`], [`PRIORITIZE`] and [`CANCEL`] ([`PAUSE`]
//!   and [`RESUME`] beside them), which send `workflow.retry`,
//!   `workflow.prioritize`, `workflow.cancel`, `workflow.pause` and
//!   `workflow.resume` to the node that runs the queue: an agent runs the
//!   same commands, and the rows come back as facts, for the panel and
//!   the agent alike. A promise item's Cancel is `job.cancel`, as the
//!   jobs centre's.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use egui_pop::api::host::Seq;
use egui_pop::api::jobs::JOBS;
use egui_pop::api::{AppHandle, Ctx, Plugin, PluginError};
use egui_pop::model::commands::CommandSpec;
use egui_pop::model::ids::{CommandId, PluginId};
use egui_pop::model::jobs::{Job, JobState, Request};
use helios_inspect::control::{ControlRequest, MoveTo, RunItem};
use helios_vault::promises::{Promise, State};
use serde_json::{Value, json};

use crate::activity::Followed;
use crate::jobs::Relays;
use crate::node_events::{Listener, NodeEvent, NodeEvents};
use crate::queue::{Action, ItemState, NodeQueue, Place, QueueItem, QueueSource};
use crate::relay::NodeLink;

/// The plugin declaring the queue verbs.
pub const PLUGIN: PluginId = PluginId::from_static("helios.workflow");
/// Cancels a run's items: `workflow.cancel` on the node running it.
pub const CANCEL: &str = "helios.workflow.cancel";
/// Retries a run's failed or cancelled items: `workflow.retry`.
pub const RETRY: &str = "helios.workflow.retry";
/// Moves a run's items to the top or the bottom of the claim order:
/// `workflow.prioritize`.
pub const PRIORITIZE: &str = "helios.workflow.prioritize";
/// Adds library photos (`images`, by id) to a run, or starts one, on
/// the node running it: `workflow.submit` with `image:` items and the
/// run's export settings (the same settings extend the same run).
pub const SUBMIT: &str = "helios.workflow.submit";
/// Stops a run claiming items: `workflow.pause`.
pub const PAUSE: &str = "helios.workflow.pause";
/// Claims a paused run's items again: `workflow.resume`.
pub const RESUME: &str = "helios.workflow.resume";
/// Reads a node's queue, a page of its items, as the Queue panel shows
/// it: read-only, for agents allowed to read only.
pub const LIST: &str = "helios.workflow.list";
/// The page [`LIST`] gives when `limit` is left out.
pub const LIST_PAGE: usize = 100;
/// The largest page [`LIST`] gives.
pub const LIST_MAX: usize = 1000;

/// A run's rows changed (`{ run, node, items }`).
pub const ROWS: &str = "helios.workflow.rows";
/// A run changed state (`{ run, title, state, … }`).
pub const RUN: &str = "helios.workflow.run";

/// The listener's id in [`NodeEvents::listeners`].
const LISTENER: &str = "helios.queue";
/// How many items of a run are read at a queueing node.
const PAGE: usize = 1000;
/// How long a node that could not be read waits before it is read again.
const AGAIN: Duration = Duration::from_secs(5);

/// One item's row at a queueing node, as last heard.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    /// Its place: `waiting` (claimable), `upstream`, `held`, `asking`,
    /// or a row status (`running`, `scheduled`, `done`, `failed`…).
    status: String,
    attempts: u32,
    /// Since when, in seconds since the epoch.
    since: f64,
    error: Option<String>,
}

/// A run, as the desktop knows it.
#[derive(Clone, Debug, Default)]
struct RunView {
    title: String,
    state: String,
    /// When it was created, in seconds since the epoch.
    created: f64,
    /// Its items in claim order.
    order: Vec<String>,
    titles: BTreeMap<String, String>,
    /// Its queueing graph nodes.
    nodes: BTreeSet<String>,
    /// What its exports write (`out`, `format`, `quality`, `name`, the
    /// export `node`, the `source`), as `workflow.runs` says it.
    export: Value,
    /// Rows by graph node, then item.
    rows: BTreeMap<String, BTreeMap<String, Row>>,
}

/// What a panel item acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    /// An item of a run on a connected node.
    Item {
        node: String,
        run: String,
        item: String,
    },
    /// A job, through its promise (when it has one: a held promise
    /// resumes through it).
    Job {
        job: String,
        promise: Option<String>,
    },
}

/// A node read whole, or why it could not be.
type Read = Result<Vec<(Value, Vec<Value>)>, String>;

/// Where a node's reading is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reading {
    Asked,
    Done,
    /// Failed: read again after [`AGAIN`].
    Failed(Instant),
}

/// The Queue panel's source over the workflow level (see the module).
#[derive(Default)]
pub struct WorkflowQueue {
    relays: Option<Relays>,
    listener: Option<Listener>,
    followed: Option<Followed>,
    runs: BTreeMap<(String, String), RunView>,
    reading: BTreeMap<String, Reading>,
    /// Nodes read off the interface's thread, waiting to be taken in.
    inbox: Arc<Mutex<Vec<(String, Read)>>>,
    targets: BTreeMap<String, Target>,
    /// The journal read up to here, for the jobs' graph nodes.
    seen: Seq,
    /// Each job's graph node, as its `helios.job.started` named it.
    job_nodes: BTreeMap<String, String>,
    /// The promises as last taken in, to tell a change.
    promises: Vec<(String, u64, &'static str, bool)>,
    /// The jobs centre's jobs that named a graph node, as last taken in
    /// (id, state, done), to tell a change.
    jobs: Vec<(String, JobState, u64)>,
}

impl std::fmt::Debug for WorkflowQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowQueue")
            .field("runs", &self.runs.len())
            .finish_non_exhaustive()
    }
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Milliseconds since the epoch, as the node writes them, in seconds.
fn seconds(ms: &Value) -> Option<f64> {
    ms.as_f64().map(|ms| ms / 1000.0)
}

/// An item's title from its id (`path:/a/b.jpg`): its file name.
fn title_of(item: &str) -> String {
    let path = item.split_once(':').map_or(item, |(_, p)| p);
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn row_of(row: &Value, fallback_since: f64) -> Row {
    Row {
        status: row["status"].as_str().unwrap_or("waiting").to_string(),
        attempts: row["attempts"]
            .as_u64()
            .map_or(0, |a| u32::try_from(a).unwrap_or(u32::MAX)),
        since: seconds(&row["since"]).unwrap_or(fallback_since),
        error: row["error"].as_str().map(str::to_string),
    }
}

/// An item's state from its row's place, its run paused or not.
fn state_of(status: &str, paused: bool) -> ItemState {
    match status {
        "running" => ItemState::Running,
        "done" | "skipped" | "omitted" => ItemState::Done,
        "failed" => ItemState::Failed,
        "cancelled" => ItemState::Cancelled,
        "asking" => ItemState::Paused,
        _ if paused => ItemState::Paused,
        _ => ItemState::Waiting,
    }
}

/// What may be asked of an item of a run in `state`.
fn actions_of(state: ItemState) -> Vec<Action> {
    match state {
        ItemState::Waiting | ItemState::Paused => vec![Action::Top, Action::Bottom, Action::Cancel],
        ItemState::Running => vec![Action::Cancel],
        ItemState::Failed | ItemState::Cancelled => vec![Action::Retry],
        ItemState::Cancelling | ItemState::Done => Vec::new(),
    }
}

/// Reads a node's runs and their items, as `workflow.runs` and
/// `workflow.items` answer them.
fn read(link: &NodeLink) -> Read {
    let runs = crate::relay::request_at(link, &ControlRequest::WorkflowRuns)?;
    let mut out = Vec::new();
    for run in runs["runs"].as_array().into_iter().flatten() {
        let Some(id) = run["id"].as_str() else {
            continue;
        };
        // Another node's run, listed read-only: its items are read there.
        if run["read_only"] == true {
            continue;
        }
        let mut pages = Vec::new();
        for node in run["nodes"].as_object().into_iter().flat_map(|n| n.keys()) {
            pages.push(crate::relay::request_at(
                link,
                &ControlRequest::WorkflowItems {
                    run: id.to_string(),
                    node: Some(node.clone()),
                    state: None,
                    offset: 0,
                    limit: PAGE,
                },
            )?);
        }
        out.push((run.clone(), pages));
    }
    Ok(out)
}

impl WorkflowQueue {
    /// The queues of the nodes `relays` follows, heard through
    /// `events`, and the promises `followed` reads.
    pub fn new(events: &NodeEvents, relays: Relays, followed: Followed) -> Self {
        Self {
            relays: Some(relays),
            listener: Some(events.listen(LISTENER, &[ROWS, RUN])),
            followed: Some(followed),
            ..Self::default()
        }
    }

    /// Reads the nodes not read yet (or read again), off the interface's
    /// thread.
    fn read_nodes(&mut self, app: &AppHandle) {
        let Some(relays) = &self.relays else {
            return;
        };
        let links: Vec<NodeLink> = relays.borrow().values().map(|r| r.link().clone()).collect();
        for link in links {
            match self.reading.get(&link.name) {
                Some(Reading::Asked | Reading::Done) => continue,
                Some(Reading::Failed(at)) if at.elapsed() < AGAIN => continue,
                _ => {}
            }
            self.reading.insert(link.name.clone(), Reading::Asked);
            let (inbox, egui) = (self.inbox.clone(), app.egui().clone());
            let spawned = std::thread::Builder::new()
                .name("helios-queue-read".into())
                .spawn(move || {
                    let answer = read(&link);
                    inbox
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push((link.name, answer));
                    egui.request_repaint();
                });
            if let Err(e) = spawned {
                log::warn!(target: "helios", "the workflow queues are not read: {e}");
            }
        }
    }

    /// Takes in a node read whole: its runs replace what was known.
    fn take_read(&mut self, node: &str, runs: Vec<(Value, Vec<Value>)>) {
        self.runs.retain(|(n, _), _| n != node);
        for (summary, pages) in runs {
            let Some(id) = summary["id"].as_str() else {
                continue;
            };
            let created = seconds(&summary["created"]).unwrap_or_else(now);
            let mut view = RunView {
                title: summary["title"].as_str().unwrap_or(id).to_string(),
                state: summary["state"].as_str().unwrap_or_default().to_string(),
                created,
                export: summary["export"].clone(),
                ..RunView::default()
            };
            for page in pages {
                let Some(at) = page["node"].as_str() else {
                    continue;
                };
                view.nodes.insert(at.to_string());
                let rows = view.rows.entry(at.to_string()).or_default();
                for item in page["items"].as_array().into_iter().flatten() {
                    let Some(name) = item["item"].as_str() else {
                        continue;
                    };
                    if !view.titles.contains_key(name) {
                        view.order.push(name.to_string());
                        view.titles.insert(
                            name.to_string(),
                            item["title"]
                                .as_str()
                                .map_or_else(|| title_of(name), str::to_string),
                        );
                    }
                    let mut row = row_of(&item["row"], created);
                    // The item's place (`waiting`, `held`, `asking`…) over
                    // the row's bare status.
                    if let Some(place) = item["state"].as_str() {
                        row.status = place.to_string();
                    }
                    rows.insert(name.to_string(), row);
                }
            }
            self.runs.insert((node.to_string(), id.to_string()), view);
        }
    }

    /// Takes in a node's fact.
    fn hear(&mut self, e: &NodeEvent) {
        if e.is_resync() {
            self.reading.remove(&e.node);
            return;
        }
        let p = &e.event.payload;
        let Some(run) = p["run"].as_str() else {
            return;
        };
        let key = (e.node.clone(), run.to_string());
        match e.event.kind {
            ROWS => {
                let Some(at) = p["node"].as_str() else {
                    return;
                };
                let view = self.runs.entry(key).or_default();
                view.nodes.insert(at.to_string());
                for item in p["items"].as_array().into_iter().flatten() {
                    let Some(name) = item["item"].as_str() else {
                        continue;
                    };
                    if !view.titles.contains_key(name) {
                        view.order.push(name.to_string());
                        view.titles.insert(name.to_string(), title_of(name));
                    }
                    let rows = view.rows.entry(at.to_string()).or_default();
                    if item["status"].is_null() {
                        // No row any more: claimable again.
                        rows.insert(
                            name.to_string(),
                            Row {
                                status: "waiting".into(),
                                attempts: 0,
                                since: now(),
                                error: None,
                            },
                        );
                    } else {
                        let mut row = row_of(item, now());
                        // A row's `waiting` waits for a person.
                        if row.status == "waiting" {
                            row.status = "asking".into();
                        } else if row.status == "scheduled" {
                            row.status = "waiting".into();
                        }
                        rows.insert(name.to_string(), row);
                    }
                }
            }
            RUN => {
                let state = p["state"].as_str().unwrap_or_default();
                match state {
                    "forgotten" | "lost" => {
                        self.runs.remove(&key);
                    }
                    // New items or a run opened: read the node again.
                    "created" | "extended" | "opened" => {
                        self.reading.remove(&e.node);
                    }
                    "reordered" => {
                        if let Some(view) = self.runs.get_mut(&key) {
                            let moved: Vec<String> = p["items"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|i| i.as_str().map(str::to_string))
                                .collect();
                            view.order.retain(|i| !moved.contains(i));
                            if p["to"] == "top" {
                                view.order.splice(0..0, moved);
                            } else {
                                view.order.extend(moved);
                            }
                        }
                    }
                    _ => {
                        let view = self.runs.entry(key).or_default();
                        view.state = p["run_state"]
                            .as_str()
                            .unwrap_or(match state {
                                "paused" => "paused",
                                "finished" => "finished",
                                _ => "running",
                            })
                            .to_string();
                        if let Some(title) = p["title"].as_str() {
                            view.title = title.to_string();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The jobs started since the last call, and the graph node each
    /// named.
    fn follow_jobs(&mut self, app: &AppHandle) {
        for e in app.events_since(self.seen) {
            self.seen = e.seq;
            if e.name != helios_core::jobs::STARTED {
                continue;
            }
            if let (Some(id), Some(node)) = (e.data["id"].as_str(), e.data["node"].as_str()) {
                self.job_nodes.insert(id.to_string(), node.to_string());
            }
        }
    }

    /// Takes in what came: the nodes read, their facts, the jobs started;
    /// asks for the nodes to read. Whether anything was taken in.
    fn take_in(&mut self, app: &AppHandle) -> bool {
        let read: Vec<(String, Read)> =
            std::mem::take(&mut *self.inbox.lock().unwrap_or_else(PoisonError::into_inner));
        let mut changed = !read.is_empty();
        for (node, answer) in read {
            match answer {
                Ok(runs) => {
                    self.take_read(&node, runs);
                    self.reading.insert(node, Reading::Done);
                }
                Err(e) => {
                    log::debug!(target: "helios", "node `{node}`'s queues: {e}");
                    self.reading.insert(node, Reading::Failed(Instant::now()));
                }
            }
        }
        let heard: Vec<NodeEvent> = self
            .listener
            .as_ref()
            .map(|l| l.try_iter().collect())
            .unwrap_or_default();
        changed |= !heard.is_empty();
        for e in &heard {
            self.hear(e);
        }
        self.read_nodes(app);
        let before = self.job_nodes.len();
        self.follow_jobs(app);
        changed |= self.job_nodes.len() != before;
        if let Some(centre) = app.service(&JOBS) {
            let jobs: Vec<_> = centre
                .all()
                .iter()
                .filter(|j| self.job_nodes.contains_key(&j.id))
                .map(|j| (j.id.clone(), j.state, j.done))
                .collect();
            if jobs != self.jobs {
                self.jobs = jobs;
                changed = true;
            }
        }
        if let Some(followed) = &self.followed {
            let promises: Vec<_> = followed
                .promises()
                .iter()
                .map(|p| (p.id.clone(), p.updated, p.state.name(), p.cancel_asked))
                .collect();
            if promises != self.promises {
                self.promises = promises;
                changed = true;
            }
        }
        changed
    }

    /// A promise as an item, when its job named a graph node.
    fn promise_item(&self, p: &Promise, node: &str) -> Option<(QueueItem, Target)> {
        let job = p.request[helios_node_core::job_promises::JOB]
            .as_str()
            .unwrap_or(&p.owner);
        if self.job_nodes.get(job).is_none_or(|n| n != node) {
            return None;
        }
        let mut state = match &p.state {
            // Held: its work waits on the service (a quota, a sign-in),
            // before its first progress too.
            State::Pending | State::Running { .. } if p.paused.is_some() => ItemState::Paused,
            State::Pending => ItemState::Waiting,
            State::Running { .. } => ItemState::Running,
            State::Done { .. } => ItemState::Done,
            State::Failed { .. } => ItemState::Failed,
            State::Cancelled => ItemState::Cancelled,
        };
        if p.cancel_asked && state.active() {
            state = ItemState::Cancelling;
        }
        let cancellable = p.request["cancellable"].as_bool().unwrap_or(true)
            || p.request[helios_node_core::job_promises::RELAYED] == true;
        let mut actions = Vec::new();
        if state == ItemState::Paused {
            actions.push(Action::Resume);
        }
        if state.active() && state != ItemState::Cancelling && cancellable {
            actions.push(Action::Cancel);
        }
        let place = match crate::relay::split_node_job(job) {
            Some((node, _)) => Place::Node(node.to_string()),
            None => Place::Here,
        };
        let error = match &p.state {
            State::Failed { reason, .. } => Some(reason.clone()),
            _ => None,
        };
        let item = QueueItem {
            id: format!("promise:{}", p.id),
            title: p.label.clone(),
            state,
            since: p.updated as f64,
            attempts: p.attempts.max(1),
            place,
            error,
            paused: p.paused.clone(),
            actions,
        };
        let target = Target::Job {
            job: job.to_string(),
            promise: Some(p.id.clone()),
        };
        Some((item, target))
    }
}

/// A job's item state: started but without any progress yet, a job is
/// still waiting in its queue.
fn job_state(job: &Job) -> ItemState {
    match job.state {
        JobState::Running
            if job.step.is_none() && job.done == 0 && job.item.is_none() && job.phase.is_none() =>
        {
            ItemState::Waiting
        }
        JobState::Running => ItemState::Running,
        JobState::Paused => ItemState::Paused,
        JobState::Cancelling => ItemState::Cancelling,
        JobState::Done => ItemState::Done,
        JobState::Failed => ItemState::Failed,
        JobState::Cancelled => ItemState::Cancelled,
    }
}

/// A job of the jobs centre as an item, its id the job's; `offset` takes
/// the centre's clock to the wall clock.
fn job_item(job: &Job, offset: f64) -> QueueItem {
    let state = job_state(job);
    let place = match crate::relay::split_node_job(&job.id) {
        Some((node, _)) => Place::Node(node.to_string()),
        None => Place::Here,
    };
    let actions = if state.active() && state != ItemState::Cancelling && job.cancellable {
        vec![Action::Cancel]
    } else {
        Vec::new()
    };
    QueueItem {
        id: format!("job:{}", job.id),
        title: job.title.clone(),
        state,
        since: job.ended_at.unwrap_or(job.started_at) + offset,
        attempts: job.attempts,
        place,
        error: job.error.clone(),
        paused: None,
        actions,
    }
}

impl QueueSource for WorkflowQueue {
    fn queue(&mut self, app: &AppHandle, node: &str) -> Option<NodeQueue> {
        self.take_in(app);
        let mut items = Vec::new();
        let mut targets = BTreeMap::new();
        for ((host, run), view) in &self.runs {
            if !view.nodes.contains(node) {
                continue;
            }
            let paused = view.state == "paused";
            let rows = view.rows.get(node);
            for name in &view.order {
                let row = rows.and_then(|r| r.get(name));
                let state = state_of(row.map_or("waiting", |r| r.status.as_str()), paused);
                let id = format!("{host}/{run}/{name}");
                targets.insert(
                    id.clone(),
                    Target::Item {
                        node: host.clone(),
                        run: run.clone(),
                        item: name.clone(),
                    },
                );
                items.push(QueueItem {
                    id,
                    title: view
                        .titles
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| title_of(name)),
                    state,
                    since: row.map_or(view.created, |r| r.since),
                    attempts: row.map_or(0, |r| r.attempts),
                    place: Place::Node(host.clone()),
                    error: row.and_then(|r| r.error.clone()),
                    paused: None,
                    actions: actions_of(state),
                });
            }
        }
        // The jobs a promise stands for.
        let mut promised = BTreeSet::new();
        if let Some(followed) = &self.followed {
            // Oldest first, as the queue lists its items.
            for p in followed.promises().iter().rev() {
                if let Some((item, target)) = self.promise_item(p, node) {
                    if let Target::Job { job, .. } = &target {
                        promised.insert(job.clone());
                    }
                    targets.insert(item.id.clone(), target);
                    items.push(item);
                }
            }
        }
        // The jobs that named the node without a promise of their own.
        if let Some(centre) = app.service(&JOBS) {
            // The centre's clock to the wall clock the items are in.
            let offset = now() - centre.now();
            for job in centre.all() {
                if promised.contains(&job.id)
                    || self.job_nodes.get(&job.id).is_none_or(|n| n != node)
                {
                    continue;
                }
                let item = job_item(&job, offset);
                targets.insert(
                    item.id.clone(),
                    Target::Job {
                        job: job.id.clone(),
                        promise: None,
                    },
                );
                items.push(item);
            }
        }
        self.targets = targets;
        (!items.is_empty()).then(|| NodeQueue { items, now: now() })
    }

    fn now(&self, _app: &AppHandle) -> Option<f64> {
        Some(now())
    }

    fn due(&mut self, app: &AppHandle) -> bool {
        self.take_in(app)
    }

    fn submit_args(&self, node: &str) -> Option<Value> {
        // The run created last that queues at the node and says what it
        // writes.
        let ((host, _), view) = self
            .runs
            .iter()
            .filter(|(_, v)| v.nodes.contains(node) && v.export.is_object())
            .max_by(|a, b| a.1.created.total_cmp(&b.1.created))?;
        submit_args_of(host, &view.export)
    }

    fn act(&mut self, app: &AppHandle, id: &str, action: Action) {
        let Some(target) = self.targets.get(id).cloned() else {
            return;
        };
        match target {
            Target::Job { job, promise } => match action {
                Action::Cancel => egui_pop::api::jobs::ask(app, &job, Request::Cancel),
                // A held promise goes on now, through the node.
                Action::Resume => {
                    if let Some(id) = promise {
                        app.run(
                            CommandId::from_static(crate::activity::RESUME),
                            json!({ "id": id }),
                        );
                    }
                }
                _ => {}
            },
            Target::Item { node, run, item } => {
                let base = json!({ "node": node, "run": run, "items": [item] });
                let (command, args) = match action {
                    Action::Cancel => (CANCEL, base),
                    Action::Retry => (RETRY, base),
                    // A run's item is never held on its own: its run is.
                    Action::Resume => return,
                    Action::Top | Action::Bottom => {
                        let mut args = base;
                        args["to"] = json!(if action == Action::Top {
                            "top"
                        } else {
                            "bottom"
                        });
                        (PRIORITIZE, args)
                    }
                };
                app.run(CommandId::from_static(command), args);
            }
        }
    }
}

/// [`SUBMIT`]'s arguments (all but `images`) that extend a run of the
/// node `host` exporting with `export` (its `workflow.runs` settings).
fn submit_args_of(host: &str, export: &Value) -> Option<Value> {
    Some(json!({
        "node": host,
        "out": export["out"].as_str()?,
        "format": export["format"].as_str()?,
        "quality": export["quality"],
        "name": export["name"].as_str()?,
        "export": export["node"].as_str()?,
        "source": export["source"].as_str()?,
    }))
}

/// [`SUBMIT`]'s request from its arguments: each image an `image:` item.
fn submit_request(args: &Value) -> Result<ControlRequest, String> {
    let text = |k: &str| args[k].as_str().map(str::to_string);
    let images = args["images"].as_array().ok_or("`images` is missing")?;
    let items = images
        .iter()
        .map(|i| {
            i.as_str()
                .filter(|i| !i.is_empty())
                .map(|i| RunItem::Path(format!("{IMAGE}{i}").into()))
                .ok_or("`images` are library photo ids")
        })
        .collect::<Result<Vec<_>, _>>()?;
    if items.is_empty() {
        return Err("`images` is empty".into());
    }
    let format = serde_json::from_value(args["format"].clone())
        .map_err(|_| "`format` is jpeg, png or tiff")?;
    Ok(ControlRequest::WorkflowSubmit {
        title: None,
        items,
        out: text("out").ok_or("`out` is missing")?.into(),
        format,
        quality: args["quality"]
            .as_u64()
            .map(|q| u8::try_from(q.min(100)).unwrap_or(100)),
        name: text("name").unwrap_or_else(|| "{stem}.{ext}".into()),
        node: text("export"),
        source: text("source"),
        again: false,
        concurrency: None,
    })
}

/// The prefix of a library photo's item, as the node reads it
/// (`helios_node::workflow::extract::IMAGE`).
const IMAGE: &str = "image:";

/// Declares the queue verbs ([`CANCEL`], [`RETRY`], [`PRIORITIZE`],
/// [`PAUSE`], [`RESUME`]): each sends its `workflow.*` request to the
/// connected node named `node`, through its relay; and [`LIST`], which
/// reads the Queue panel's source.
pub struct Commands {
    pub relays: Relays,
    /// The Queue panel, whose source [`LIST`] reads.
    pub panel: std::rc::Rc<std::cell::RefCell<crate::queue::QueuePanel>>,
}

/// [`LIST`]'s arguments.
pub fn list_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "node": { "type": ["string", "null"], "description": "The graph node whose queue is read; the selected one when left out" },
            "from": { "type": "integer", "minimum": 0, "description": "The first item given, by its index in queue order" },
            "limit": { "type": "integer", "minimum": 1, "maximum": LIST_MAX, "description": "How many items, 100 when left out" },
        },
    })
}

/// An item's state, as [`LIST`] names it.
fn state_name(state: ItemState) -> &'static str {
    match state {
        ItemState::Waiting => "waiting",
        ItemState::Running => "running",
        ItemState::Paused => "paused",
        ItemState::Cancelling => "cancelling",
        ItemState::Done => "done",
        ItemState::Failed => "failed",
        ItemState::Cancelled => "cancelled",
    }
}

/// [`LIST`]'s answer: the summary of `node`'s queue and the page of its
/// items `args` asks (`from`, `limit`). Without `private`, what names a
/// file (an item's id and title, its error) is redacted: the states,
/// counts and places stay.
pub fn list_answer(
    node: Option<&str>,
    queue: Option<&NodeQueue>,
    args: &Value,
    private: bool,
) -> Value {
    let from = args["from"]
        .as_u64()
        .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let limit = args["limit"]
        .as_u64()
        .map_or(LIST_PAGE, |n| usize::try_from(n).unwrap_or(LIST_MAX))
        .clamp(1, LIST_MAX);
    let empty = NodeQueue::default();
    let queue = queue.unwrap_or(&empty);
    let hide = |s: &str| {
        if private {
            json!(s)
        } else {
            json!(crate::privacy::REDACTED)
        }
    };
    let items: Vec<Value> = queue
        .items
        .iter()
        .enumerate()
        .skip(from)
        .take(limit)
        .map(|(index, item)| {
            json!({
                "index": index,
                "id": hide(&item.id),
                "title": hide(&item.title),
                "state": state_name(item.state),
                "since": item.since,
                "attempts": item.attempts,
                "place": match &item.place {
                    Place::Here => "here".to_string(),
                    Place::Node(n) => n.clone(),
                },
                "error": item.error.as_deref().map_or(Value::Null, hide),
                "held": item.paused.as_ref().map(|p| p.reason.clone()),
                "actions": item.actions.iter().map(|a| a.name()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let s = queue.summary();
    let next = from.saturating_add(items.len());
    json!({
        "node": node,
        "total": queue.items.len(),
        "from": from,
        "items": items,
        "next": (next < queue.items.len()).then_some(next),
        "summary": {
            "waiting": s.waiting,
            "running": s.running,
            "done": s.done,
            "failed": s.failed,
            "cancelled": s.cancelled,
            "oldest_wait": s.oldest_wait,
        },
        "now": queue.now,
    })
}

/// The arguments every verb takes: the node, the run.
fn base_schema() -> serde_json::Map<String, Value> {
    let mut props = serde_json::Map::new();
    props.insert(
        "node".into(),
        json!({ "type": "string", "minLength": 1, "description": "The connected node running the run" }),
    );
    props.insert("run".into(), json!({ "type": "string", "minLength": 1 }));
    props
}

fn schema(extra: &[(&str, Value)], required: &[&str]) -> Value {
    let mut props = base_schema();
    for (k, v) in extra {
        props.insert((*k).into(), v.clone());
    }
    let mut req = vec!["node", "run"];
    req.extend_from_slice(required);
    json!({ "type": "object", "properties": props, "required": req })
}

/// A verb's request from its arguments.
fn request_of(command: &str, args: &Value) -> Result<ControlRequest, String> {
    let run = args["run"].as_str().ok_or("`run` is missing")?.to_string();
    let items = args["items"].as_array().map(|items| {
        items
            .iter()
            .filter_map(|i| i.as_str().map(str::to_string))
            .collect::<Vec<_>>()
    });
    let at = args["at"].as_str().map(str::to_string);
    Ok(match command {
        CANCEL => ControlRequest::WorkflowCancel {
            run,
            items,
            node: at,
        },
        RETRY => ControlRequest::WorkflowRetry {
            run,
            items,
            node: at,
        },
        PRIORITIZE => ControlRequest::WorkflowPrioritize {
            run,
            items: items.ok_or("`items` is missing")?,
            to: match args["to"].as_str() {
                Some("top") => MoveTo::Top,
                Some("bottom") => MoveTo::Bottom,
                _ => return Err("`to` is top or bottom".into()),
            },
        },
        PAUSE => ControlRequest::WorkflowPause { run },
        RESUME => ControlRequest::WorkflowResume { run },
        other => return Err(format!("`{other}` is not a queue verb")),
    })
}

impl Plugin for Commands {
    fn id(&self) -> PluginId {
        PLUGIN
    }

    fn title(&self) -> String {
        "Helios workflow queues".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        let items = json!({ "type": "array", "items": { "type": "string" },
            "description": "The items, by id (`path:…`) or absolute path; every item when left out" });
        let at = json!({ "type": "string",
            "description": "The queueing graph node; every one when left out" });
        let verbs: [(&'static str, &'static str, Value); 5] = [
            (
                CANCEL,
                "Cancels a workflow run's items on the node running it (`workflow.cancel`): their rows not concluded become cancelled",
                schema(&[("items", items.clone()), ("at", at.clone())], &[]),
            ),
            (
                RETRY,
                "Retries a workflow run's failed and cancelled items on the node running it (`workflow.retry`)",
                schema(&[("items", items.clone()), ("at", at)], &[]),
            ),
            (
                PRIORITIZE,
                "Moves a workflow run's items to the top or the bottom of its claim order (`workflow.prioritize`)",
                schema(
                    &[
                        ("items", items),
                        ("to", json!({ "enum": ["top", "bottom"] })),
                    ],
                    &["items", "to"],
                ),
            ),
            (
                PAUSE,
                "Pauses a workflow run on the node running it (`workflow.pause`): it claims nothing more",
                schema(&[], &[]),
            ),
            (
                RESUME,
                "Resumes a paused workflow run (`workflow.resume`)",
                schema(&[], &[]),
            ),
        ];
        let relays = self.relays.clone();
        ctx.command(
            CommandSpec::new(
                CommandId::from_static(SUBMIT),
                "Adds library photos to a workflow run on a connected node (`workflow.submit` with `image:` items): the run with the same network and export settings is extended, else one starts",
                "Queue",
            )
            .schema(json!({
                "type": "object",
                "properties": {
                    "node": { "type": "string", "minLength": 1, "description": "The connected node running the run" },
                    "images": { "type": "array", "items": { "type": "string" }, "description": "The library photos, by id" },
                    "out": { "type": "string", "description": "The folder written to, absolute" },
                    "format": { "enum": ["jpeg", "png", "tiff"] },
                    "quality": { "type": ["integer", "null"], "minimum": 1, "maximum": 100 },
                    "name": { "type": "string", "description": "The file name: {stem} and {ext} replaced" },
                    "export": { "type": "string", "description": "The graph node exported; the network's first export when left out" },
                    "source": { "type": "string", "description": "The source given each item; the network's only source when left out" },
                },
                "required": ["node", "images", "out", "format"],
            }))
            .hidden(),
            move |call| {
                let node = call.args["node"].as_str().ok_or("`node` is missing")?;
                let request = submit_request(call.args)?;
                relays
                    .borrow()
                    .get(node)
                    .ok_or_else(|| format!("node `{node}` is not followed"))?
                    .request(&request)
            },
        )?;
        let panel = self.panel.clone();
        ctx.command(
            CommandSpec::new(
                CommandId::from_static(LIST),
                "Reads a node's queue as the Queue panel shows it (`node`: the selected one when left out): its summary, then a page of its items in queue order (`from`, `limit`: 100, at most 1000), each with its state, since when, its attempts, where it runs and what may be asked of it; file names only for agents allowed private data",
                "Queue",
            )
            .schema(list_schema())
            .read_only()
            .hidden(),
            move |call| {
                let node = call.args["node"].as_str();
                // Asked while the panel draws (its own click), it is busy.
                let (node, queue) = panel
                    .try_borrow_mut()
                    .map_err(|_| "the Queue panel is busy: ask again".to_string())?
                    .read(call.app, node);
                let private = egui_pop_agent::grants::reads_private(call.app, call.origin);
                Ok(list_answer(node.as_deref(), queue.as_ref(), call.args, private))
            },
        )?;
        for (id, about, schema) in verbs {
            let relays = self.relays.clone();
            ctx.command(
                CommandSpec::new(CommandId::from_static(id), about, "Queue")
                    .schema(schema)
                    .hidden(),
                move |call| {
                    let node = call.args["node"].as_str().ok_or("`node` is missing")?;
                    let request = request_of(id, call.args)?;
                    relays
                        .borrow()
                        .get(node)
                        .ok_or_else(|| format!("node `{node}` is not followed"))?
                        .request(&request)
                },
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_s_place_maps_to_an_item_state() {
        assert_eq!(state_of("waiting", false), ItemState::Waiting);
        assert_eq!(state_of("held", false), ItemState::Waiting);
        assert_eq!(state_of("waiting", true), ItemState::Paused);
        assert_eq!(state_of("asking", false), ItemState::Paused);
        assert_eq!(state_of("running", true), ItemState::Running);
        assert_eq!(state_of("skipped", false), ItemState::Done);
        assert_eq!(state_of("failed", false), ItemState::Failed);
        assert_eq!(actions_of(ItemState::Failed), [Action::Retry]);
        assert_eq!(
            actions_of(ItemState::Waiting),
            [Action::Top, Action::Bottom, Action::Cancel]
        );
        assert_eq!(title_of("path:/photos/a.jpg"), "a.jpg");
    }

    #[test]
    fn the_list_gives_a_page_and_hides_file_names_without_private_data() {
        let item = |n: usize, state| QueueItem {
            id: format!("n1/r1/path:/photos/{n}.jpg"),
            title: format!("{n}.jpg"),
            state,
            since: 10.0,
            attempts: 1,
            place: Place::Node("n1".into()),
            error: (state == ItemState::Failed).then(|| "cannot read /photos".into()),
            paused: None,
            actions: actions_of(state),
        };
        let queue = NodeQueue {
            items: vec![
                item(0, ItemState::Done),
                item(1, ItemState::Failed),
                item(2, ItemState::Waiting),
            ],
            now: 20.0,
        };
        let page = list_answer(
            Some("export"),
            Some(&queue),
            &json!({ "from": 1, "limit": 1 }),
            true,
        );
        assert_eq!(page["total"], 3);
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["index"], 1);
        assert_eq!(page["items"][0]["state"], "failed");
        assert_eq!(page["items"][0]["title"], "1.jpg");
        assert_eq!(page["items"][0]["actions"], json!(["retry"]));
        assert_eq!(page["next"], 2);
        assert_eq!(page["summary"]["failed"], 1);
        assert!(
            egui_pop::core::schema::check(&list_schema(), &json!({ "from": 1, "limit": 1 }))
                .is_ok()
        );
        assert!(egui_pop::core::schema::check(&list_schema(), &json!({ "limit": 0 })).is_err());

        let hidden = list_answer(Some("export"), Some(&queue), &json!({}), false);
        let text = hidden.to_string();
        assert!(!text.contains("photos"), "{text}");
        assert_eq!(hidden["items"][2]["state"], "waiting");
        assert_eq!(hidden["next"], Value::Null);
        assert_eq!(list_answer(None, None, &json!({}), true)["total"], 0);
    }

    #[test]
    fn the_verbs_map_to_the_workflow_requests() {
        let args = json!({ "node": "n1", "run": "r1", "items": ["path:/a.jpg"], "to": "top" });
        assert_eq!(
            request_of(PRIORITIZE, &args).unwrap(),
            ControlRequest::WorkflowPrioritize {
                run: "r1".into(),
                items: vec!["path:/a.jpg".into()],
                to: MoveTo::Top,
            }
        );
        assert!(matches!(
            request_of(RETRY, &args).unwrap(),
            ControlRequest::WorkflowRetry {
                items: Some(_),
                node: None,
                ..
            }
        ));
        assert!(request_of(PRIORITIZE, &json!({ "run": "r1" })).is_err());
    }
}
