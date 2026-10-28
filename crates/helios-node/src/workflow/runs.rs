//! The runs of a node process, and the runner that works them.
//!
//! A run keeps one journal (rows per item and queueing node), its item
//! documents and its run document: in the node's vault when it has one
//! (`durable`), else in memory. The runner works every active run each
//! tick (every 250 ms, and at once when a job ends):
//!
//! 1. leases past their due time are given back (`expire`);
//! 2. leases held by running jobs are renewed every 20 s, so a long export
//!    keeps its lease;
//! 3. for each queueing node, while it has fewer leases out than its
//!    concurrency, one item is claimed and becomes a scheduler job: an
//!    export renders a copy of the network taken at submission with the
//!    item's file as its source, and writes the output path decided at
//!    submission; the job is a child of the run's job;
//! 4. when the job ends, its lease is concluded: done, failed (retried by
//!    the node's retry, or for good when the file cannot be read as a
//!    picture or does not fit the network), or cancelled.
//!
//! Decision nodes have no worker yet. A decision the network had answered
//! when the run was submitted concludes `done` at once, the export
//! rendering that answer; one still unanswered would make every render
//! fail, so its rows wait for an answer instead (`asking`), holding the
//! export, until decision workers and their signals come.
//!
//! Every write is announced on the bus as `workflow.rows` (the rows it
//! changed), and a run's life as `workflow.run` (created, extended,
//! finished). A run is also a job (`job.*`), its exports its children;
//! cancelled there, its rows not concluded are cancelled and its exports
//! stopped.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use helios_core::jobs::{Jobs, Progress, Report, Start};
use helios_core::{Bus, Describe, Engine, FaultKind, Graph, Image, JobId, Message, NodeId, Params};
use helios_core::{ModuleKind, Scheduler};
use helios_decision::Mode;
use helios_export::Format;
use helios_inspect::control::{FileFormat, RunItem};
use helios_journal::{Journal, Lease, MemoryStore, Outcome, Row, Status};
use helios_vault::journal::VaultJournal;
use helios_vault_sqlite::SqliteStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Queue, QueueNode, SUBMIT};

/// The vault a node process keeps its runs in.
pub type SharedVault = Arc<helios_vault::Vault<SqliteStore>>;

/// The format of the run and item documents: raised by a change old code
/// cannot read.
pub const FORMAT: u64 = 1;

/// How often the runner works the runs when nothing happens.
const TICK: Duration = Duration::from_millis(250);

/// How often a lease held by a running job is renewed.
const RENEW_MS: u64 = 20_000;

/// The source of the bus events of the workflow level.
const SOURCE: &str = "workflow";

/// The runs' job ids in this node: `run-1`, `run-2`...
static NEXT_RUN_JOB: AtomicU64 = AtomicU64::new(1);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Where a run's rows are kept.
pub enum RunStore {
    Memory(MemoryStore),
    Vault(VaultJournal<SqliteStore>),
}

impl helios_journal::Store for RunStore {
    fn get(&self, item: &str, node: &str) -> Option<Row> {
        match self {
            RunStore::Memory(s) => s.get(item, node),
            RunStore::Vault(s) => s.get(item, node),
        }
    }

    fn put(&mut self, item: &str, node: &str, row: Row) {
        match self {
            RunStore::Memory(s) => s.put(item, node, row),
            RunStore::Vault(s) => s.put(item, node, row),
        }
    }

    fn remove(&mut self, item: &str, node: &str) {
        match self {
            RunStore::Memory(s) => s.remove(item, node),
            RunStore::Vault(s) => s.remove(item, node),
        }
    }

    fn rows(&self) -> Vec<(String, String, Row)> {
        match self {
            RunStore::Memory(s) => s.rows(),
            RunStore::Vault(s) => s.rows(),
        }
    }

    fn begin(&mut self) {
        match self {
            RunStore::Memory(s) => s.begin(),
            RunStore::Vault(s) => s.begin(),
        }
    }

    fn commit(&mut self) {
        match self {
            RunStore::Memory(s) => s.commit(),
            RunStore::Vault(s) => s.commit(),
        }
    }

    fn last_token(&self) -> u64 {
        match self {
            RunStore::Memory(s) => s.last_token(),
            RunStore::Vault(s) => s.last_token(),
        }
    }

    fn set_last_token(&mut self, token: u64) {
        match self {
            RunStore::Memory(s) => s.set_last_token(token),
            RunStore::Vault(s) => s.set_last_token(token),
        }
    }
}

/// What a run's exports write: part of the run's id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportSettings {
    /// The folder written to, absolute.
    pub out: PathBuf,
    pub format: FileFormat,
    pub quality: Option<u8>,
    /// The output file name: `{stem}` and `{ext}` are replaced.
    pub name: String,
    /// The node exported.
    pub node: String,
    /// The source given each item.
    pub source: String,
}

impl ExportSettings {
    fn format(&self) -> Format {
        match self.format {
            FileFormat::Jpeg => Format::Jpeg {
                quality: self.quality.unwrap_or(92),
            },
            FileFormat::Png => Format::Png { bits: 8 },
            FileFormat::Tiff => Format::Tiff,
        }
    }
}

/// An item document: what a run needs to do an item again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    /// The file at submission, absolute.
    pub path: PathBuf,
    /// Decided at submission, never again against the files on disk.
    pub output: PathBuf,
    /// Parameters changed for this item only, by node name.
    #[serde(default)]
    pub params: BTreeMap<String, Params>,
    /// Lower goes first; then the order of submission.
    #[serde(default)]
    pub priority: i64,
    pub order: u64,
}

impl Item {
    fn title(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    /// Items are worked.
    Active,
    /// No row open and no item claimable: until items are added.
    Finished,
}

/// What the runner keeps in memory of a row, so watching a run reads no
/// row.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Seen {
    status: Status,
    since: u64,
    due: Option<u64>,
}

impl Seen {
    fn of(row: &Row) -> Self {
        Self {
            status: row.status,
            since: row.since,
            due: row.due,
        }
    }
}

/// Where an item stands at a queueing node.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Place {
    /// It has a row.
    Row(Seen),
    /// No row, and its parents are concluded (since when).
    Claimable(u64),
    /// No row, its parents not all concluded yet.
    Upstream,
    /// No row, and a failed or cancelled parent holds it for good.
    Held,
}

impl Place {
    /// Its name, as `workflow.items` filters and lists it: `waiting` for
    /// an item that can be claimed, as the design note says, so a row
    /// waiting for a person is `asking`.
    fn name(self) -> &'static str {
        match self {
            Place::Row(seen) if seen.status == Status::Waiting => "asking",
            Place::Row(seen) => seen.status.name(),
            Place::Claimable(_) => "waiting",
            Place::Upstream => "upstream",
            Place::Held => "held",
        }
    }

    /// Whether it can be claimed at `now`.
    fn claimable(self, now: u64) -> bool {
        match self {
            Place::Claimable(_) => true,
            Place::Row(s) => s.status == Status::Scheduled && s.due.is_none_or(|d| d <= now),
            _ => false,
        }
    }

    /// Whether nothing more will happen to it without someone acting.
    fn settled(self) -> bool {
        match self {
            Place::Row(s) => !s.status.open(),
            Place::Held => true,
            Place::Claimable(_) | Place::Upstream => false,
        }
    }
}

/// A run's job: the parent of its exports.
struct RunJob {
    report: Report,
    /// Asked to stop through the jobs (`job.cancel`).
    asked: Arc<AtomicBool>,
    /// Its rows were cancelled: it ends cancelled.
    cancelled: bool,
    /// The last progress reported: (done, failed, total).
    progress: (u64, u64, u64),
}

/// A run of the node process.
struct Run {
    id: String,
    title: String,
    created: u64,
    state: RunState,
    durable: bool,
    settings: ExportSettings,
    snapshot: Value,
    nodes: Vec<QueueNode>,
    journal: Journal<RunStore>,
    /// The network as it was at submission, which each export copies.
    graph: Graph,
    source: NodeId,
    export: NodeId,
    /// In the order they are worked.
    items: Vec<Item>,
    known: BTreeSet<String>,
    /// The rows, by node then item, as the runner last wrote or read them.
    seen: BTreeMap<String, BTreeMap<String, Seen>>,
    job: Option<RunJob>,
}

impl Run {
    fn seen(&self, node: &str, item: &str) -> Option<Seen> {
        self.seen.get(node).and_then(|m| m.get(item)).copied()
    }

    /// Where `item` stands at each queueing node, in the order of
    /// `self.nodes`. Reads no row.
    fn places(&self, item: &str) -> Vec<Place> {
        let mut out: Vec<Place> = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            if let Some(seen) = self.seen(&node.name, item) {
                out.push(Place::Row(seen));
                continue;
            }
            let (mut accepted, mut blocked, mut since) = (0, 0, 0);
            for p in &node.parents {
                let parent = if p == SUBMIT {
                    self.seen(SUBMIT, item).map_or(Place::Upstream, Place::Row)
                } else {
                    self.nodes
                        .iter()
                        .position(|n| &n.name == p)
                        .map_or(Place::Upstream, |i| out[i])
                };
                match parent {
                    Place::Row(s)
                        if matches!(s.status, Status::Done | Status::Skipped | Status::Omitted) =>
                    {
                        accepted += 1;
                        since = since.max(s.since);
                    }
                    Place::Row(s) if matches!(s.status, Status::Failed | Status::Cancelled) => {
                        blocked += 1;
                    }
                    Place::Held => blocked += 1,
                    _ => {}
                }
            }
            out.push(if accepted >= node.parents.len() {
                Place::Claimable(since)
            } else if blocked > 0 {
                Place::Held
            } else {
                Place::Upstream
            });
        }
        out
    }

    fn node_index(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.name == name)
    }

    /// The export node's index: the last queueing node.
    fn export_index(&self) -> usize {
        self.nodes
            .iter()
            .position(|n| n.queue == Queue::Export)
            .unwrap_or(self.nodes.len() - 1)
    }

    /// Up to `max` items claimable at node `index` at `now`, in the order
    /// they are worked.
    fn candidates(&self, index: usize, now: u64, max: usize) -> Vec<String> {
        self.items
            .iter()
            .filter(|item| self.places(&item.id)[index].claimable(now))
            .take(max)
            .map(|item| item.id.clone())
            .collect()
    }

    /// Whether no row is open and no item can be claimed.
    fn finished(&self) -> bool {
        self.items
            .iter()
            .all(|item| self.places(&item.id).iter().all(|p| p.settled()))
    }

    /// Items done (or skipped) and failed at the export.
    fn tally(&self) -> (u64, u64) {
        let export = self.export_index();
        let (mut done, mut failed) = (0, 0);
        for item in &self.items {
            match self.places(&item.id)[export] {
                Place::Row(s) if matches!(s.status, Status::Done | Status::Skipped) => done += 1,
                Place::Row(s) if s.status == Status::Failed => failed += 1,
                Place::Held => failed += 1,
                _ => {}
            }
        }
        (done, failed)
    }

    /// Reports the run's progress on its job, when it changed.
    fn report_progress(&mut self) {
        let (done, failed) = self.tally();
        let total = self.items.len() as u64;
        let Some(job) = &mut self.job else {
            return;
        };
        if job.progress != (done, failed, total) {
            job.progress = (done, failed, total);
            job.report
                .progress(Progress::new().counts(done, total).failed(failed));
        }
    }

    /// The run document.
    fn document(&self, owner: &str) -> Value {
        json!({
            "format": FORMAT,
            "title": self.title,
            "created": self.created,
            "owner": owner,
            "state": self.state,
            "workflow": self.nodes,
            "network": self.snapshot,
            "export": self.settings,
        })
    }
}

/// A lease held by a running job.
struct Held {
    run: String,
    lease: Lease,
    renewed: u64,
    /// The job found the item cannot be done: another attempt would not
    /// change it.
    permanent: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    runs: BTreeMap<String, Run>,
    /// By scheduler job id.
    leases: BTreeMap<u64, Held>,
}

/// What the runs need of the node process.
pub struct Context {
    pub bus: Arc<Bus>,
    pub engine: Arc<Engine>,
    pub scheduler: Arc<Scheduler>,
    pub jobs: Jobs,
    /// Where runs last; in memory without one.
    pub vault: Option<SharedVault>,
    /// The scheduler's workers: the export's concurrency.
    pub workers: usize,
    /// The node instance that owns the runs it creates.
    pub owner: String,
}

/// A batch to submit as a run (`workflow.submit`).
pub struct Submit<'a> {
    pub title: Option<&'a str>,
    pub items: &'a [RunItem],
    pub out: &'a Path,
    pub format: FileFormat,
    pub quality: Option<u8>,
    pub name: &'a str,
    pub node: Option<&'a str>,
    pub source: Option<&'a str>,
    pub again: bool,
}

/// The runs of a node process.
pub struct Workflows {
    ctx: Context,
    state: Mutex<State>,
    /// Faults injected through control (`fault`), for the next export of
    /// a run: its render works on a copy of the network, which a fault
    /// injected in the node's own graph does not reach.
    faults: Mutex<Vec<(String, FaultKind)>>,
}

fn not_absolute(path: &Path) -> Message {
    Message::new(
        "node-path-not-absolute",
        "{ $path }: give an absolute path, the node does not know where you are",
    )
    .with("path", path)
}

fn no_run(id: &str) -> Message {
    Message::new("workflow-no-run", "no run `{ $run }`").with("run", id)
}

fn row_json(item: &str, row: Option<&Row>) -> Value {
    match row {
        Some(r) => json!({
            "item": item,
            "status": r.status,
            "attempts": r.attempts,
            "since": r.since,
            "due": r.due,
            "error": r.error,
        }),
        None => json!({ "item": item, "status": null }),
    }
}

impl Workflows {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            state: Mutex::new(State::default()),
            faults: Mutex::new(Vec::new()),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("workflow lock")
    }

    /// Starts the runner: a thread working the runs every tick, and at
    /// once when a job ends.
    pub fn start(self: &Arc<Self>) {
        let this = self.clone();
        let events = self.ctx.bus.subscribe();
        std::thread::Builder::new()
            .name("helios-node-workflow".into())
            .spawn(move || {
                let mut last = std::time::Instant::now();
                loop {
                    match events.recv_timeout(TICK) {
                        Ok(e) if e.kind == "scheduler.job_ended" => this.job_ended(&e.payload),
                        Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                    if last.elapsed() >= TICK {
                        last = std::time::Instant::now();
                        let mut state = this.state();
                        this.tick(&mut state, now_ms());
                    }
                }
            })
            .expect("a thread for the workflow runner");
    }

    /// Makes the next export of a run fail once at node `name`, as
    /// `fault` does the node's own renders.
    pub fn inject_fault(&self, name: &str, kind: FaultKind) {
        self.faults
            .lock()
            .expect("faults lock")
            .push((name.to_string(), kind));
    }

    /// Persists documents in the vault, when the runs last.
    fn persist(&self, writes: &[helios_vault::workflow::WorkflowWrite]) {
        if let Some(vault) = &self.ctx.vault {
            // As the journal's store does: a vault that cannot keep a run
            // stops the node rather than let it run on what it did not keep.
            vault
                .set_workflow_documents(writes)
                .unwrap_or_else(|e| panic!("workflow documents: {e}"));
        }
    }

    fn persist_run(&self, run: &Run) {
        self.persist(&[(
            "runs".into(),
            run.id.clone(),
            Some(run.document(&self.ctx.owner)),
        )]);
    }

    /// Reads the rows of `items` at `node` again, keeps them in memory,
    /// and announces them (`workflow.rows`).
    fn refresh(&self, run: &mut Run, node: &str, items: &[String]) {
        if items.is_empty() {
            return;
        }
        let mut rows = Vec::with_capacity(items.len());
        for item in items {
            let row = run.journal.row(item, node);
            let seen = run.seen.entry(node.to_string()).or_default();
            match &row {
                Some(r) => seen.insert(item.clone(), Seen::of(r)),
                None => seen.remove(item),
            };
            rows.push(row_json(item, row.as_ref()));
        }
        self.ctx.bus.publish_value(
            SOURCE,
            None,
            "workflow.rows",
            json!({ "run": run.id, "node": node, "items": rows }),
        );
    }

    /// [`Workflows::refresh`] for rows written at several nodes.
    fn refresh_pairs(&self, run: &mut Run, pairs: Vec<(String, String)>) {
        let mut by_node: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (item, node) in pairs {
            by_node.entry(node).or_default().push(item);
        }
        for (node, items) in by_node {
            self.refresh(run, &node, &items);
        }
    }

    /// The running rows the runner knows of, which an expiry may have
    /// given back.
    fn refresh_running(&self, run: &mut Run) {
        let running: Vec<(String, String)> = run
            .seen
            .iter()
            .flat_map(|(node, items)| {
                items
                    .iter()
                    .filter(|(_, s)| s.status == Status::Running)
                    .map(|(item, _)| (item.clone(), node.clone()))
            })
            .collect();
        self.refresh_pairs(run, running);
    }

    fn announce(&self, run: &Run, state: &str, extra: Value) {
        let mut payload = json!({ "run": run.id, "title": run.title, "state": state });
        if let (Some(p), Some(e)) = (payload.as_object_mut(), extra.as_object()) {
            p.extend(e.clone());
        }
        self.ctx
            .bus
            .publish_value(SOURCE, None, "workflow.run", payload);
    }

    fn start_job(&self, run: &mut Run) {
        let id = format!("run-{}", NEXT_RUN_JOB.fetch_add(1, Ordering::Relaxed));
        let total = run.items.len() as u64;
        let report = self
            .ctx
            .jobs
            .start(Start::new(&id, &run.title, "workflow").total(total));
        let asked = Arc::new(AtomicBool::new(false));
        {
            let asked = asked.clone();
            report.on_cancel(move || asked.store(true, Ordering::SeqCst));
        }
        run.job = Some(RunJob {
            report,
            asked,
            cancelled: false,
            progress: (0, 0, total),
        });
    }
}

/// Resolving what a submission names in the network.
fn names(graph: &Graph) -> BTreeMap<String, NodeId> {
    graph
        .ids()
        .filter_map(|id| Some((graph.name(id).ok()?.to_string(), id)))
        .collect()
}

/// The source a run gives each item: the one named, else the network's
/// only source.
fn pick_source(graph: &Graph, named: Option<&str>) -> Result<String, Message> {
    if let Some(name) = named {
        return Ok(name.to_string());
    }
    let sources: Vec<String> = graph
        .ids()
        .filter(|id| matches!(graph.manifest(*id), Ok(None)))
        .filter_map(|id| graph.name(id).ok().map(str::to_string))
        .collect();
    match sources.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(Message::new("batch-no-source", "the network has no source")),
        many => Err(Message::new(
            "batch-several-sources",
            "the network has several sources ({ $names }): say --source NAME",
        )
        .with("names", many.join(", "))),
    }
}

/// The node a run exports: the one named, else the network's first
/// export, else its only end (a module no other reads, decisions aside),
/// else the source itself.
fn pick_export(
    graph: &Graph,
    named: Option<&str>,
    first_export: Option<&str>,
    source: &str,
) -> Result<String, Message> {
    if let Some(name) = named.or(first_export) {
        return Ok(name.to_string());
    }
    let read: BTreeSet<NodeId> = graph
        .ids()
        .flat_map(|id| graph.input_nodes(id).unwrap_or_default())
        .flatten()
        .collect();
    let ends: Vec<String> = graph
        .ids()
        .filter(|id| !read.contains(id))
        .filter(|id| matches!(graph.manifest(*id), Ok(Some(m)) if m.kind != ModuleKind::Decision))
        .filter_map(|id| graph.name(id).ok().map(str::to_string))
        .collect();
    match ends.as_slice() {
        [one] => Ok(one.clone()),
        [] => Ok(source.to_string()),
        many => Err(Message::new(
            "batch-several-ends",
            "the network ends in several nodes ({ $names }): say --node NAME",
        )
        .with("names", many.join(", "))),
    }
}

/// Submission.
impl Workflows {
    /// Submits a batch: creates the run, or extends the one with the same
    /// network and settings. Answers the run's id, its workflow, the
    /// counts (new, already there, merged) and each item's output path.
    pub fn submit(
        &self,
        graph: &Graph,
        decisions: &BTreeMap<String, Mode>,
        first_export: Option<&str>,
        ask: &Submit<'_>,
    ) -> Result<Value, Message> {
        if !ask.out.is_absolute() {
            return Err(not_absolute(ask.out));
        }
        if ask.items.is_empty() {
            return Err(Message::new(
                "workflow-no-items",
                "a run needs at least one item",
            ));
        }
        if let Some(item) = ask.items.iter().find(|i| !i.path().is_absolute()) {
            return Err(not_absolute(item.path()));
        }
        let ids = names(graph);
        let source = pick_source(graph, ask.source)?;
        let export = pick_export(graph, ask.node, first_export, &source)?;
        let node_id = |name: &str| {
            ids.get(name).copied().ok_or_else(|| {
                Message::new("node-no-node", "no node named `{ $name }`").with("name", name)
            })
        };
        let (source_id, export_id) = (node_id(&source)?, node_id(&export)?);
        let derived = super::derive(
            &super::graph_nodes(graph, decisions),
            &export,
            self.ctx.workers,
        )
        .map_err(Message::plain)?;
        let settings = ExportSettings {
            out: ask.out.to_path_buf(),
            format: ask.format,
            quality: ask.quality,
            name: ask.name.to_string(),
            node: export,
            source,
        };
        let snapshot = super::snapshot(graph);
        let now = now_ms();
        let id = super::run_id(
            &snapshot,
            &serde_json::to_value(&settings).expect("settings serialize"),
            ask.again.then_some(now),
        );
        std::fs::create_dir_all(ask.out).map_err(|e| {
            Message::new("batch-cannot-create", "cannot create { $path }: { $error }")
                .with("path", ask.out)
                .with("error", e.to_string())
        })?;

        let mut state = self.state();
        let created = if state.runs.contains_key(&id) {
            false
        } else {
            let run = match self.open(&id, graph, source_id, export_id, now)? {
                Some(run) => run,
                None => self.create(
                    &id,
                    ask.title,
                    graph,
                    (source_id, export_id),
                    settings,
                    snapshot,
                    derived,
                    now,
                )?,
            };
            state.runs.insert(id.clone(), run);
            true
        };
        let run = state.runs.get_mut(&id).expect("the run was just put");
        let (new, already, merged) = self.add_items(run, ask, now);
        if new > 0 && run.state == RunState::Finished {
            run.state = RunState::Active;
        }
        if run.state == RunState::Active && run.job.is_none() {
            self.start_job(run);
        }
        self.persist_run(run);
        self.announce(
            run,
            if created { "created" } else { "extended" },
            json!({ "new": new, "already": already, "merged": merged }),
        );
        let outputs: Vec<Value> = ask
            .items
            .iter()
            .filter_map(|i| {
                let id = super::item_id(i.path());
                let item = run.items.iter().find(|it| it.id == id)?;
                Some(json!({ "item": item.id, "path": item.path, "output": item.output }))
            })
            .collect();
        let answer = json!({
            "run": run.id,
            "title": run.title,
            "durable": run.durable,
            "created": created,
            "new": new,
            "already": already,
            "merged": merged,
            "workflow": run.nodes,
            "items": outputs,
        });
        self.tick(&mut state, now);
        Ok(answer)
    }

    /// A run of the vault that this process has not opened yet: its
    /// documents read, every running row given back (no lease lives across
    /// a restart).
    fn open(
        &self,
        id: &str,
        graph: &Graph,
        source: NodeId,
        export: NodeId,
        now: u64,
    ) -> Result<Option<Run>, Message> {
        let Some(vault) = &self.ctx.vault else {
            return Ok(None);
        };
        let doc = vault
            .workflow_document("runs", id)
            .map_err(Message::plain)?;
        let Some(doc) = doc else {
            return Ok(None);
        };
        if doc["format"].as_u64() != Some(FORMAT) {
            return Err(Message::new(
                "workflow-newer-format",
                "run `{ $run }` was kept by a newer Helios: it cannot be worked here",
            )
            .with("run", id));
        }
        let unreadable = |e: serde_json::Error| {
            Message::plain(format!("run `{id}`: its document cannot be read: {e}"))
        };
        let nodes: Vec<QueueNode> =
            serde_json::from_value(doc["workflow"].clone()).map_err(unreadable)?;
        let settings: ExportSettings =
            serde_json::from_value(doc["export"].clone()).map_err(unreadable)?;
        let mut items: Vec<Item> = vault
            .workflow_documents(&format!("{id}/items"))
            .map_err(Message::plain)?
            .into_iter()
            .map(|(_, d)| serde_json::from_value(d))
            .collect::<Result<_, _>>()
            .map_err(unreadable)?;
        items.sort_by_key(|i| (i.priority, i.order));
        let workflow = super::journal_workflow(&nodes).map_err(Message::plain)?;
        let journal = Journal::new(
            workflow,
            RunStore::Vault(VaultJournal::new(vault.clone(), id)),
        );
        journal.recover(now);
        let mut run = Run {
            id: id.to_string(),
            title: doc["title"].as_str().unwrap_or(id).to_string(),
            created: doc["created"].as_u64().unwrap_or(now),
            state: RunState::Active,
            durable: true,
            settings,
            snapshot: doc["network"].clone(),
            known: items.iter().map(|i| i.id.clone()).collect(),
            items,
            nodes,
            journal,
            graph: graph.clone(),
            source,
            export,
            seen: BTreeMap::new(),
            job: None,
        };
        let names: Vec<String> = std::iter::once(SUBMIT.to_string())
            .chain(run.nodes.iter().map(|n| n.name.clone()))
            .collect();
        for node in names {
            let ids: Vec<String> = run.items.iter().map(|i| i.id.clone()).collect();
            for item in ids {
                if let Some(row) = run.journal.row(&item, &node) {
                    run.seen
                        .entry(node.clone())
                        .or_default()
                        .insert(item, Seen::of(&row));
                }
            }
        }
        Ok(Some(run))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "what a new run is made of, one by one"
    )]
    fn create(
        &self,
        id: &str,
        title: Option<&str>,
        graph: &Graph,
        (source, export): (NodeId, NodeId),
        settings: ExportSettings,
        snapshot: Value,
        nodes: Vec<QueueNode>,
        now: u64,
    ) -> Result<Run, Message> {
        let workflow = super::journal_workflow(&nodes).map_err(Message::plain)?;
        let store = match &self.ctx.vault {
            Some(vault) => RunStore::Vault(VaultJournal::new(vault.clone(), id)),
            None => RunStore::Memory(MemoryStore::default()),
        };
        Ok(Run {
            id: id.to_string(),
            title: title.map_or_else(
                || format!("Export to {}", settings.out.display()),
                str::to_string,
            ),
            created: now,
            state: RunState::Active,
            durable: self.ctx.vault.is_some(),
            settings,
            snapshot,
            nodes,
            journal: Journal::new(workflow, store),
            graph: graph.clone(),
            source,
            export,
            items: Vec::new(),
            known: BTreeSet::new(),
            seen: BTreeMap::new(),
            job: None,
        })
    }

    /// Adds the items of a submission the run does not have, their output
    /// paths decided now, then enters them at `submit`. Returns how many
    /// were new, already there, and given twice (merged).
    fn add_items(&self, run: &mut Run, ask: &Submit<'_>, now: u64) -> (usize, usize, usize) {
        let (mut already, mut merged) = (0, 0);
        let mut asked = BTreeSet::new();
        let mut new: Vec<(String, &RunItem)> = Vec::new();
        for item in ask.items {
            let id = super::item_id(item.path());
            if !asked.insert(id.clone()) {
                merged += 1;
            } else if run.known.contains(&id) {
                already += 1;
            } else {
                new.push((id, item));
            }
        }
        if new.is_empty() {
            return (0, already, merged);
        }
        let taken: BTreeSet<PathBuf> = run.items.iter().map(|i| i.output.clone()).collect();
        let paths: Vec<PathBuf> = new.iter().map(|(_, i)| i.path().to_path_buf()).collect();
        let outputs = crate::batch::output_paths(
            &paths,
            &run.settings.out,
            &run.settings.name,
            crate::batch::extension(run.settings.format),
            |p| taken.contains(p) || p.exists(),
        );
        let first = run.items.iter().map(|i| i.order + 1).max().unwrap_or(0);
        let added: Vec<Item> = new
            .iter()
            .zip(outputs)
            .enumerate()
            .map(|(n, ((id, item), output))| Item {
                id: id.clone(),
                path: item.path().to_path_buf(),
                output,
                params: item.params(),
                priority: 0,
                order: first + n as u64,
            })
            .collect();
        let writes: Vec<_> = added
            .iter()
            .map(|item| {
                (
                    format!("{}/items", run.id),
                    item.id.clone(),
                    Some(serde_json::to_value(item).expect("items serialize")),
                )
            })
            .collect();
        self.persist(&writes);
        let ids: Vec<String> = added.iter().map(|i| i.id.clone()).collect();
        for item in added {
            run.known.insert(item.id.clone());
            run.items.push(item);
        }
        run.items.sort_by_key(|i| (i.priority, i.order));
        // Entered at `submit`: a lease taken and concluded at once.
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        if let Ok(lease) = run.journal.claim(SUBMIT, refs.len(), &refs, now)
            && let Err(e) = run.journal.conclude(&lease, Outcome::Done, now)
        {
            eprintln!("helios-node: run {}: {e}", run.id);
        }
        self.refresh(run, SUBMIT, &ids);
        if let Some(job) = &mut run.job {
            job.progress.2 = run.items.len() as u64;
        }
        (ids.len(), already, merged)
    }
}

/// The runner.
impl Workflows {
    fn tick(&self, state: &mut State, now: u64) {
        let ids: Vec<String> = state.runs.keys().cloned().collect();
        for id in ids {
            self.tick_run(state, &id, now);
        }
    }

    fn tick_run(&self, state: &mut State, id: &str, now: u64) {
        let State { runs, leases } = state;
        let Some(run) = runs.get_mut(id) else {
            return;
        };
        if run.state != RunState::Active {
            return;
        }
        if run
            .job
            .as_ref()
            .is_some_and(|j| j.asked.load(Ordering::SeqCst) && !j.cancelled)
        {
            self.cancel_run(run, leases, now);
        }
        if run.journal.expire(now) > 0 {
            self.refresh_running(run);
        }
        for held in leases.values_mut().filter(|h| h.run == run.id) {
            if now >= held.renewed + RENEW_MS && run.journal.renew(&held.lease, now).is_ok() {
                held.renewed = now;
            }
        }
        let cancelled = run.job.as_ref().is_some_and(|j| j.cancelled);
        for index in 0..run.nodes.len() {
            if cancelled {
                break;
            }
            let node = run.nodes[index].clone();
            match node.queue {
                // No worker for decisions yet: an answer the network had
                // at submission is the item's; without one, the item waits
                // for it (a render of an unanswered decision fails).
                Queue::Decision { .. } => {
                    let items = run.candidates(index, now, usize::MAX);
                    if items.is_empty() {
                        continue;
                    }
                    let answered = names(&run.graph)
                        .get(&node.name)
                        .is_some_and(|id| run.graph.is_answered(*id).unwrap_or(false));
                    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
                    if let Ok(lease) = run.journal.claim(&node.name, refs.len(), &refs, now) {
                        let written = if answered {
                            run.journal.conclude(&lease, Outcome::Done, now)
                        } else {
                            run.journal.wait(&lease, now)
                        };
                        if let Err(e) = written {
                            eprintln!("helios-node: run {}: {e}", run.id);
                        }
                        self.refresh(run, &node.name, &lease.items);
                    }
                }
                Queue::Export => loop {
                    let out = leases
                        .values()
                        .filter(|h| h.lease.node == node.name)
                        .count();
                    if out >= node.concurrency {
                        break;
                    }
                    let items = run.candidates(index, now, 8);
                    if items.is_empty() {
                        break;
                    }
                    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
                    let Ok(lease) = run.journal.claim(&node.name, 1, &refs, now) else {
                        break;
                    };
                    if lease.items.is_empty() {
                        break;
                    }
                    self.refresh(run, &node.name, &lease.items);
                    let item = run
                        .items
                        .iter()
                        .find(|i| i.id == lease.items[0])
                        .cloned()
                        .expect("a claimed item is the run's");
                    let (job, permanent) = self.start_export(run, &item, &node.name);
                    leases.insert(
                        job.0,
                        Held {
                            run: run.id.clone(),
                            lease,
                            renewed: now,
                            permanent,
                        },
                    );
                },
            }
        }
        run.report_progress();
        if run.finished() && !leases.values().any(|h| h.run == run.id) {
            self.finish(run);
        }
    }

    /// Renders the item through a copy of the run's network, as a child
    /// of the run's job writing the item's output path.
    fn start_export(&self, run: &Run, item: &Item, node: &str) -> (JobId, Arc<AtomicBool>) {
        let mut graph = run.graph.clone();
        let faults: Vec<(String, FaultKind)> =
            std::mem::take(&mut *self.faults.lock().expect("faults lock"));
        let ids = names(&graph);
        for (name, kind) in faults {
            if let Some(id) = ids.get(&name) {
                self.ctx.engine.inject_fault(&graph, *id, kind);
            }
        }
        let permanent = Arc::new(AtomicBool::new(false));
        let (engine, source, export) = (self.ctx.engine.clone(), run.source, run.export);
        let (path, params, flag) = (item.path.clone(), item.params.clone(), permanent.clone());
        let render = move || -> Result<Image, String> {
            // What another attempt would not change.
            let for_good = |e: String| {
                flag.store(true, Ordering::SeqCst);
                e
            };
            let loaded = helios_load::load(&path).map_err(for_good)?;
            let mut cameras = BTreeMap::new();
            crate::load_source(&mut graph, &mut cameras, source, loaded)
                .map_err(|e| for_good(format!("the network cannot take the image: {e}")))?;
            for (name, values) in &params {
                let target = ids
                    .get(name)
                    .copied()
                    .ok_or_else(|| for_good(format!("no node named `{name}`")))?;
                for (param, value) in values.iter() {
                    graph
                        .set_param(target, param, value.clone())
                        .map_err(|e| for_good(format!("cannot set {name}.{param}: {e}")))?;
                }
            }
            let region = graph
                .extent(export)
                .map_err(|e| e.to_string())?
                .ok_or("the exported node has no extent")?;
            engine
                .render(&graph, export, "output", region)
                .map_err(|e| e.to_string())
        };
        let parent = run.job.as_ref().map(|j| j.report.id().to_string());
        let format = run.settings.format();
        let job = match &parent {
            Some(parent) => helios_export::submit_child(
                &self.ctx.scheduler,
                &self.ctx.jobs,
                parent,
                Some(node),
                &item.output,
                format,
                render,
            ),
            None => helios_export::submit_reported(
                &self.ctx.scheduler,
                &self.ctx.jobs,
                Some(node),
                &item.output,
                format,
                render,
            ),
        }
        .expect("an export job is always queued");
        (job, permanent)
    }

    /// Concludes the lease of a job that ended (`scheduler.job_ended`).
    fn job_ended(&self, payload: &Value) {
        let Some(job) = payload["job"].as_u64() else {
            return;
        };
        let mut state = self.state();
        let Some(held) = state.leases.remove(&job) else {
            return;
        };
        let now = now_ms();
        if let Some(run) = state.runs.get_mut(&held.run) {
            let lease = &held.lease;
            let error = payload["error"].as_str().unwrap_or("failed").to_string();
            let result = match payload["state"].as_str() {
                Some("done") => run.journal.conclude(lease, Outcome::Done, now),
                // Not a failure: the rows it still holds are cancelled.
                Some("cancelled") => {
                    let held_still: Vec<&str> = lease
                        .items
                        .iter()
                        .filter(|i| {
                            run.journal.row(i, &lease.node).is_some_and(|r| {
                                r.status == Status::Running && r.token == Some(lease.token)
                            })
                        })
                        .map(String::as_str)
                        .collect();
                    run.journal
                        .cancel(&held_still, Some(&lease.node), now)
                        .map(|_| ())
                }
                _ if held.permanent.load(Ordering::SeqCst) => {
                    run.journal
                        .conclude(lease, Outcome::FailedForGood(error), now)
                }
                _ => run.journal.conclude(lease, Outcome::Failed(error), now),
            };
            match result {
                // A lease given back meanwhile (expired, or its run
                // cancelled) writes nothing.
                Ok(()) | Err(helios_journal::Error::StaleLease { .. }) => {}
                Err(e) => eprintln!("helios-node: run {}: {e}", run.id),
            }
            self.refresh(run, &lease.node, &lease.items);
        }
        self.tick(&mut state, now);
    }

    /// Cancels a run asked to stop through its job: its exports are
    /// stopped and its rows not concluded cancelled.
    fn cancel_run(&self, run: &mut Run, leases: &BTreeMap<u64, Held>, now: u64) {
        for (job, _) in leases.iter().filter(|(_, h)| h.run == run.id) {
            self.ctx.scheduler.cancel(JobId(*job));
        }
        let ids: Vec<String> = run.items.iter().map(|i| i.id.clone()).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        match run.journal.cancel(&refs, None, now) {
            Ok(written) => self.refresh_pairs(run, written),
            Err(e) => eprintln!("helios-node: run {}: {e}", run.id),
        }
        if let Some(job) = &mut run.job {
            job.cancelled = true;
        }
    }

    /// Ends a run with nothing left to work: its job ends, it is
    /// announced, and its document says so.
    fn finish(&self, run: &mut Run) {
        run.state = RunState::Finished;
        let (done, failed) = run.tally();
        let total = run.items.len() as u64;
        let mut cancelled = false;
        if let Some(job) = run.job.take() {
            cancelled = job.cancelled;
            if job.cancelled {
                job.report.cancelled();
            } else if failed > 0 {
                job.report
                    .failed(&format!("{failed} of {total} items failed"));
            } else {
                job.report.finished();
            }
        }
        self.persist_run(run);
        self.announce(
            run,
            "finished",
            json!({ "done": done, "failed": failed, "total": total, "cancelled": cancelled }),
        );
    }
}

/// Monitoring.
impl Workflows {
    /// What a run says of itself, and per queueing node: the journal's
    /// counts by status (a row waiting for a person counted as `asking`),
    /// plus `waiting` (claimable now), `upstream`, `held`
    /// (behind a failure), the oldest waiting since, the concurrency and
    /// the leases out.
    fn summary(&self, run: &Run, leases: &BTreeMap<u64, Held>, now: u64) -> Value {
        let monitor = run.journal.monitor(now);
        let places: Vec<Vec<Place>> = run.items.iter().map(|i| run.places(&i.id)).collect();
        let nodes: serde_json::Map<String, Value> = run
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let mut out = monitor["nodes"][&node.name].clone();
                let at = places.iter().map(|p| p[index]);
                let count = |f: &dyn Fn(Place) -> bool| at.clone().filter(|p| f(*p)).count();
                let oldest_waiting = at
                    .clone()
                    .filter_map(|p| match p {
                        Place::Claimable(since) => Some(since),
                        Place::Row(s) if s.status == Status::Scheduled => s.due,
                        _ => None,
                    })
                    .min();
                if let Some(map) = out.as_object_mut() {
                    // The journal's `waiting` rows wait for a person: named
                    // `asking` here, `waiting` being what can be claimed.
                    let asking = map.remove("waiting").unwrap_or(json!(0));
                    map.insert("asking".into(), asking);
                    map.insert("waiting".into(), json!(count(&|p| p.claimable(now))));
                    map.insert("upstream".into(), json!(count(&|p| p == Place::Upstream)));
                    map.insert("held".into(), json!(count(&|p| p == Place::Held)));
                    map.insert("oldest_waiting_since".into(), json!(oldest_waiting));
                    map.insert("concurrency".into(), json!(node.concurrency));
                    map.insert(
                        "leases_out".into(),
                        json!(
                            leases
                                .values()
                                .filter(|h| h.run == run.id && h.lease.node == node.name)
                                .count()
                        ),
                    );
                    map.insert("queue".into(), json!(node.queue));
                    map.insert("parents".into(), json!(node.parents));
                }
                (node.name.clone(), out)
            })
            .collect();
        json!({
            "id": run.id,
            "title": run.title,
            "state": run.state,
            "durable": run.durable,
            "owner": self.ctx.owner,
            "created": run.created,
            "items": run.items.len(),
            "job": run.job.as_ref().map(|j| j.report.id()),
            "export": run.settings,
            "nodes": nodes,
        })
    }

    /// `workflow.runs`: every run with its counts.
    pub fn runs(&self) -> Value {
        let now = now_ms();
        let state = self.state();
        let runs: Vec<Value> = state
            .runs
            .values()
            .map(|r| self.summary(r, &state.leases, now))
            .collect();
        json!({ "runs": runs, "now": now })
    }

    /// `workflow.items`: one page of a run's items at a queueing node (the
    /// export when not given), in the order they are worked; `state` keeps
    /// those in one state (`waiting` counts retries due too).
    pub fn items(
        &self,
        run: &str,
        node: Option<&str>,
        filter: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, Message> {
        let now = now_ms();
        let state = self.state();
        let run = state.runs.get(run).ok_or_else(|| no_run(run))?;
        let index = match node {
            Some(name) => run.node_index(name).ok_or_else(|| {
                Message::new(
                    "workflow-no-node",
                    "run `{ $run }` does not queue at `{ $node }`",
                )
                .with("run", &run.id)
                .with("node", name)
            })?,
            None => run.export_index(),
        };
        let name = run.nodes[index].name.clone();
        let matching: Vec<(&Item, Place)> = run
            .items
            .iter()
            .map(|item| (item, run.places(&item.id)[index]))
            .filter(|(_, place)| match filter {
                None => true,
                // Retries due count as waiting too.
                Some("waiting") => place.claimable(now) || place.name() == "waiting",
                Some(state) => place.name() == state,
            })
            .collect();
        let page: Vec<Value> = matching
            .iter()
            .skip(offset)
            .take(limit)
            .map(|(item, place)| {
                let row = run.journal.row(&item.id, &name);
                json!({
                    "item": item.id,
                    "title": item.title(),
                    "path": item.path,
                    "output": item.output,
                    "state": place.name(),
                    "row": row.as_ref().map(|r| row_json(&item.id, Some(r))),
                    "error": row.and_then(|r| r.error),
                })
            })
            .collect();
        Ok(json!({
            "run": run.id,
            "node": name,
            "total": matching.len(),
            "offset": offset,
            "items": page,
            "now": now,
        }))
    }
}

/// `describe workflow`: the runs, per queueing node, with the node's clock
/// so a reader converts the times to its own.
impl Describe for Workflows {
    fn describe(&self) -> Value {
        let now = now_ms();
        let state = self.state();
        let runs: Vec<Value> = state
            .runs
            .values()
            .map(|r| self.summary(r, &state.leases, now))
            .collect();
        json!({
            "now": now,
            "owner": self.ctx.owner,
            "durable": self.ctx.vault.is_some(),
            "runs": runs,
        })
    }
}
