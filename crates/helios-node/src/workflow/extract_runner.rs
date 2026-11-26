//! The runner of extractor runs (`docs/design/extractors.md`, section 8):
//! an [`ExtractRun`] submitted to the node is worked on a thread of its
//! own, as a job, with its run document beside the export runs'.
//!
//! - **The run's id** follows from its nodes (extractor, version, inputs,
//!   overrides) and its images ([`run_id`]): submitting them again while
//!   the run is at work gives back that run.
//! - **The pre-run check** refuses a submission a node cannot be served
//!   for, before anything is entered ([`ExtractRun::submit`]).
//! - **The run document** `workflow/runs/<id>` says `kind: "extract"`, its
//!   nodes and extractors, its state and when it finished. The export
//!   runner leaves these documents to this one.
//! - **The job** (kind `extract`) is mirrored as a promise by the node
//!   (`helios_node_core::job_promises`), so the run shows in the Activity
//!   focus and in `promises.list`: its progress is the rows concluded of
//!   the rows to conclude (images × nodes), its failures counted; it
//!   settles with the run's counts. Cancelling the job (`job.cancel`,
//!   `promises.cancel`) stops the run after the row at hand and cancels
//!   the rows not concluded; what concluded stays.
//! - **Results** go to the vault's extract cache for the run
//!   (`vault.extract_cache(Some(run))`), or to memory without a vault.
//!
//! - **The verb** `workflow.extract` ([`specs`], [`ExtractRunner::request`])
//!   takes extractor uses (`id@version` from the runner's catalogue,
//!   inputs, overrides, after) and library photos (`image:<hash>`).
//! - **Listing**: the runs are listed by `workflow.runs` beside the export
//!   runs ([`ExtractRunner::summaries`], with `kind: "extract"` and per
//!   node counts) and their items by `workflow.items`
//!   ([`ExtractRunner::items`]), read from the run's rows, so the Queue
//!   panel shows them as it shows export runs.
//!
//! - **The engine**: the node process hands the runner an opener
//!   ([`ExtractRunner::set_opener`]): the AI service the desktop uses
//!   (`Ai::open` on the user's AI settings: its routes and overrides,
//!   retries, pre-run check and cache) and the library's previews
//!   ([`super::library_source::LibraryPreviews`]), opened the first time
//!   the verb is asked. A node with no library refuses the verb, saying so.
//!
//! Not yet: a run taken over after a restart.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use helios_ai::Ai;
use helios_ai::routing::Overrides;
use helios_command::schema::{or_null, string};
use helios_command::{CommandSpec, object};
use helios_core::Bus;
use helios_core::Message;
use helios_core::jobs::{Jobs, Progress, Report, Start};
use helios_extract::{Catalog, MemoryCache, ResultCache};
use helios_inspect::control::ExtractUse;
use helios_journal::{MemoryStore, Row, Status, Store};
use helios_vault::journal::VaultJournal;
use serde::Serialize;
use serde_json::{Value, json};

use super::SUBMIT;
use super::extract::{Ended, ExtractNode, ExtractRun, ImageSource, Tally, image_item, image_of};
use super::lifecycle::FORMAT;
use super::runs::{RunStore, SharedVault};

/// The `kind` of an extract run's document in `workflow/runs`.
pub const KIND: &str = "extract";

/// How long a run waiting for a retry sleeps before it looks again.
const WAIT: Duration = Duration::from_millis(250);

static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Where an extract run stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtractState {
    Active,
    Finished,
    Cancelled,
}

/// An extract run to submit.
pub struct ExtractSubmit {
    /// Its title in the Activity focus; "Extract from N photos" when not
    /// given.
    pub title: Option<String>,
    pub nodes: Vec<ExtractNode>,
    /// Library images by content hash, in the order they are worked.
    pub images: Vec<String>,
    pub ai: Arc<Ai>,
    /// Where each image sent is read.
    pub source: Arc<dyn ImageSource + Send + Sync>,
}

/// The id of the run of `nodes` over `images`.
pub fn run_id(nodes: &[ExtractNode], images: &[String]) -> String {
    let nodes: Vec<Value> = nodes
        .iter()
        .map(|n| {
            json!({
                "name": n.name,
                "after": n.after,
                "extractor": n.def.id,
                "version": n.def.version,
                "input": n.input,
                "overrides": serde_json::to_value(&n.overrides).unwrap_or(Value::Null),
            })
        })
        .collect();
    let canonical = json!({ "nodes": nodes, "images": images });
    let bytes = serde_json::to_vec(&canonical).expect("JSON values serialize");
    format!("{KIND}-{}", &blake3::hash(&bytes).to_hex()[..16])
}

/// The verb submitting an extract run, declared here (registered by
/// [`crate::commands::registry`]).
pub fn specs() -> Vec<CommandSpec> {
    let node = object(&[
        ("name", string(), true),
        (
            "extractor",
            json!({ "type": "string", "pattern": "^[^@]+@[0-9]+$",
                    "description": "the extractor and its version: id@version" }),
            true,
        ),
        (
            "after",
            json!({ "type": "array", "items": string() }),
            false,
        ),
        ("input", json!({ "type": "object" }), false),
        ("overrides", json!({ "type": "object" }), false),
    ]);
    let image = json!({ "type": "string", "pattern": "^image:[0-9a-f]{64}$",
                        "description": "a library photo: image: and its content hash" });
    vec![
        CommandSpec::new(
            "workflow.extract",
            "Run extractors over library photos",
            "Workflow",
        )
        .params(object(&[
            ("title", or_null(string()), false),
            (
                "nodes",
                json!({ "type": "array", "items": node, "minItems": 1 }),
                true,
            ),
            (
                "items",
                json!({ "type": "array", "items": image, "minItems": 1 }),
                true,
            ),
        ])),
    ]
}

/// What the runner keeps of a run.
struct Held {
    title: String,
    created: u64,
    nodes: Vec<Value>,
    images: usize,
    job: String,
    status: Arc<Mutex<(ExtractState, Tally, Option<u64>)>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// The run's item ids, in the order they are worked.
    items: Vec<String>,
    /// Each node's name and the nodes it waits for.
    graph: Vec<(String, Vec<String>)>,
    /// Another handle on the run's rows, read for its listing while its
    /// thread works it.
    rows: Mutex<RunStore>,
}

/// Where an item stands at a node, as `workflow.items` names it.
fn place(rows: &RunStore, item: &str, node: &str, parents: &[String], now: u64) -> &'static str {
    match rows.get(item, node) {
        Some(Row {
            status: Status::Scheduled,
            due,
            ..
        }) if due.is_none_or(|d| d <= now) => "waiting",
        Some(r) => r.status.name(),
        None => {
            let parent = |p: &String| rows.get(item, p).map(|r| r.status);
            let statuses: Vec<Option<Status>> = parents.iter().map(parent).collect();
            if statuses.iter().all(|s| {
                s.is_some_and(|s| matches!(s, Status::Done | Status::Skipped | Status::Omitted))
            }) {
                "waiting"
            } else if statuses
                .iter()
                .any(|s| s.is_some_and(|s| matches!(s, Status::Failed | Status::Cancelled)))
            {
                "held"
            } else {
                "upstream"
            }
        }
    }
}

impl Held {
    fn document(&self, owner: &str) -> Value {
        let (state, tally, finished) = *self.status.lock().unwrap_or_else(PoisonError::into_inner);
        json!({
            "format": FORMAT,
            "kind": KIND,
            "title": self.title,
            "created": self.created,
            "owner": owner,
            "state": state,
            "finished": finished,
            "nodes": self.nodes,
            "images": self.images,
            "tally": tally,
        })
    }
}

/// The extract runs of a node process.
pub struct ExtractRunner {
    bus: Arc<Bus>,
    jobs: Jobs,
    vault: Option<SharedVault>,
    /// The node instance that owns the runs it creates.
    owner: String,
    runs: Mutex<BTreeMap<String, Arc<Held>>>,
    /// The extractors the verb names.
    catalog: Catalog,
    /// The AI and image source the verb's runs are worked with, once the
    /// node hands them over.
    engine: Mutex<Slot>,
}

/// The AI a run calls, and where it reads its images.
pub type Engine = (Arc<Ai>, Arc<dyn ImageSource + Send + Sync>);

/// Opens the engine the first time the verb is asked (the AI service
/// starts its threads then, not with every node).
pub type Opener = Box<dyn Fn() -> Result<Engine, String> + Send>;

/// The runner's engine: none (with the reason), to open, or open.
enum Slot {
    None(String),
    Lazy(Opener),
    Ready(Engine),
}

impl Slot {
    /// The engine, opened now if it was not yet; the reason it cannot be.
    fn get(&mut self) -> Result<Engine, String> {
        let engine = match self {
            Slot::Ready(engine) => return Ok(engine.clone()),
            Slot::None(reason) => return Err(reason.clone()),
            Slot::Lazy(open) => open()?,
        };
        *self = Slot::Ready(engine.clone());
        Ok(engine)
    }
}

impl ExtractRunner {
    pub fn new(bus: Arc<Bus>, jobs: Jobs, vault: Option<SharedVault>, owner: &str) -> Self {
        Self {
            bus,
            jobs,
            vault,
            owner: owner.into(),
            runs: Mutex::new(BTreeMap::new()),
            catalog: builtin_catalog(),
            engine: Mutex::new(Slot::None(
                "this node opens no AI: extract runs cannot be worked here".into(),
            )),
        }
    }

    /// The extractors the verb may name: the built-ins unless given.
    #[must_use]
    pub fn with_catalog(mut self, catalog: Catalog) -> Self {
        self.catalog = catalog;
        self
    }

    /// Hands the runner the AI and image source the verb's runs use.
    pub fn set_engine(&self, ai: Arc<Ai>, source: Arc<dyn ImageSource + Send + Sync>) {
        *self.engine.lock().unwrap_or_else(PoisonError::into_inner) = Slot::Ready((ai, source));
    }

    /// Hands the runner what opens its AI and image source the first time
    /// the verb is asked; an opening that fails refuses that request with
    /// its reason, and is tried again on the next.
    pub fn set_opener(&self, open: Opener) {
        *self.engine.lock().unwrap_or_else(PoisonError::into_inner) = Slot::Lazy(open);
    }

    /// Says why the verb's runs cannot be worked by this node (no
    /// library, say).
    pub fn set_unavailable(&self, reason: &str) {
        *self.engine.lock().unwrap_or_else(PoisonError::into_inner) = Slot::None(reason.into());
    }

    /// `workflow.extract`: each use resolved in the catalogue, the photos
    /// taken as `image:<hash>`, then [`ExtractRunner::submit`].
    pub fn request(
        self: &Arc<Self>,
        title: Option<&str>,
        uses: &[ExtractUse],
        items: &[String],
    ) -> Result<Value, Message> {
        let (ai, source) = self
            .engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get()
            .map_err(Message::plain)?;
        let nodes = uses
            .iter()
            .map(|u| self.node(u))
            .collect::<Result<Vec<_>, String>>()
            .map_err(Message::plain)?;
        let images = items
            .iter()
            .map(|i| {
                image_of(i)
                    .map(str::to_string)
                    .ok_or_else(|| format!("`{i}` is not a library photo (image:<hash>)"))
            })
            .collect::<Result<Vec<_>, String>>()
            .map_err(Message::plain)?;
        self.submit(ExtractSubmit {
            title: title.map(str::to_string),
            nodes,
            images,
            ai,
            source,
        })
        .map_err(Message::plain)
    }

    fn node(&self, u: &ExtractUse) -> Result<ExtractNode, String> {
        let (id, version) = u
            .extractor
            .split_once('@')
            .and_then(|(id, v)| Some((id, v.parse::<u32>().ok()?)))
            .ok_or_else(|| format!("node `{}`: `{}` is not id@version", u.name, u.extractor))?;
        let def = self
            .catalog
            .get(id, version)
            .ok_or_else(|| format!("node `{}`: no extractor {id}@{version}", u.name))?
            .clone();
        let input = if u.input.is_null() {
            json!({})
        } else {
            u.input.clone()
        };
        let mut node = ExtractNode::new(&u.name, def, input);
        node.after = u.after.clone();
        if !u.overrides.is_null() {
            node.overrides = serde_json::from_value::<Overrides>(u.overrides.clone())
                .map_err(|e| format!("node `{}`: overrides: {e}", u.name))?;
        }
        Ok(node)
    }

    fn write(&self, id: &str, held: &Held) {
        if let Some(vault) = &self.vault
            && let Err(e) = vault.set_workflow_documents(&[(
                "runs".into(),
                id.into(),
                Some(held.document(&self.owner)),
            )])
        {
            log::warn!("extract run {id}: its document is not written: {e}");
        }
    }

    /// Checks and opens the run, writes its document, starts its job and
    /// the thread that works it. Answers `{run, job, total, already}`;
    /// `already` when the same run is still at work here.
    pub fn submit(self: &Arc<Self>, ask: ExtractSubmit) -> Result<Value, String> {
        let id = run_id(&ask.nodes, &ask.images);
        let mut runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = runs.get(&id) {
            let (state, tally, _) = *held.status.lock().unwrap_or_else(PoisonError::into_inner);
            if state == ExtractState::Active {
                return Ok(json!({
                    "run": id, "job": held.job, "total": tally.total, "already": true,
                }));
            }
        }
        let (store, rows) = match &self.vault {
            Some(vault) => (
                RunStore::Vault(VaultJournal::new(vault.clone(), &id)),
                RunStore::Vault(VaultJournal::new(vault.clone(), &id)),
            ),
            None => {
                let memory = MemoryStore::default();
                let shared = memory.share();
                (RunStore::Memory(memory), RunStore::Memory(shared))
            }
        };
        let now = now_ms();
        let nodes: Vec<Value> = ask
            .nodes
            .iter()
            .map(|n| json!({ "name": n.name, "after": n.after, "extractor": n.def.id, "version": n.def.version }))
            .collect();
        let graph: Vec<(String, Vec<String>)> = ask
            .nodes
            .iter()
            .map(|n| {
                let after = if n.after.is_empty() {
                    vec![SUBMIT.to_string()]
                } else {
                    n.after.clone()
                };
                (n.name.clone(), after)
            })
            .collect();
        let items: Vec<String> = ask.images.iter().map(|i| image_item(i)).collect();
        let mut run =
            ExtractRun::submit(&ask.ai, &self.bus, &id, ask.nodes, &ask.images, store, now)?;
        let tally = run.tally();
        let title = ask
            .title
            .unwrap_or_else(|| format!("Extract from {} photos", ask.images.len()));
        let job = format!("extract-{}", NEXT_JOB.fetch_add(1, Ordering::Relaxed));
        let report = Arc::new(
            self.jobs
                .start(Start::new(&job, &title, KIND).total(tally.total)),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let (stop, cancelled) = (stop.clone(), cancelled.clone());
            report.on_cancel(move || {
                cancelled.store(true, Ordering::SeqCst);
                stop.store(true, Ordering::SeqCst);
            });
        }
        let status = Arc::new(Mutex::new((ExtractState::Active, tally, None)));
        {
            let (report, status) = (report.clone(), status.clone());
            run.on_tally(move |t| {
                status.lock().unwrap_or_else(PoisonError::into_inner).1 = t;
                report.progress(
                    Progress::new()
                        .counts(t.done + t.failed, t.total)
                        .failed(t.failed),
                );
            });
        }
        let held = Arc::new(Held {
            title,
            created: now,
            nodes,
            images: ask.images.len(),
            job: job.clone(),
            status: status.clone(),
            thread: Mutex::new(None),
            items,
            graph,
            rows: Mutex::new(rows),
        });
        self.write(&id, &held);
        let this = self.clone();
        let (ai, source, worked) = (ask.ai, ask.source, held.clone());
        let run_id = id.clone();
        let thread = std::thread::Builder::new()
            .name("helios-node-extract".into())
            .spawn(move || {
                let flags = (stop.as_ref(), cancelled.as_ref());
                this.work(
                    &run_id,
                    &mut run,
                    &ai,
                    source.as_ref(),
                    flags,
                    &report,
                    &worked.status,
                );
                this.write(&run_id, &worked);
            })
            .map_err(|e| format!("extract run {id}: no thread to work it: {e}"))?;
        *held.thread.lock().unwrap_or_else(PoisonError::into_inner) = Some(thread);
        runs.insert(id.clone(), held);
        Ok(json!({ "run": id, "job": job, "total": tally.total, "already": false }))
    }

    /// Works `run` until it finishes or its job is cancelled, then ends
    /// the job and says where the run stands.
    /// `stop` is raised by a cancel, which `cancelled` says.
    #[allow(clippy::too_many_arguments)]
    fn work(
        &self,
        id: &str,
        run: &mut ExtractRun<RunStore>,
        ai: &Ai,
        source: &dyn ImageSource,
        (stop, cancelled): (&AtomicBool, &AtomicBool),
        report: &Report,
        status: &Mutex<(ExtractState, Tally, Option<u64>)>,
    ) {
        let memory = MemoryCache::default();
        let cache: Box<dyn ResultCache + '_> = match &self.vault {
            Some(vault) => Box::new(vault.extract_cache(Some(id))),
            None => Box::new(memory),
        };
        let ended = loop {
            match run.work(ai, cache.as_ref(), source, &self.bus, stop, &now_ms) {
                Ended::Waiting if !stop.load(Ordering::SeqCst) => std::thread::sleep(WAIT),
                Ended::Waiting => break Ended::Stopped,
                other => break other,
            }
        };
        let state = if ended == Ended::Stopped && cancelled.load(Ordering::SeqCst) {
            if let Err(e) = run.cancel(now_ms()) {
                log::warn!("extract run {id}: {e}");
            }
            ExtractState::Cancelled
        } else {
            ExtractState::Finished
        };
        let tally = run.tally();
        *status.lock().unwrap_or_else(PoisonError::into_inner) = (state, tally, Some(now_ms()));
        match state {
            ExtractState::Cancelled => report.cancelled(),
            _ => report.finished_result(json!({ "run": id, "tally": tally })),
        }
    }

    /// Cancels the run `id`'s job: the run stops after the row at hand
    /// and its rows not concluded are cancelled.
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
        let held = runs
            .get(id)
            .ok_or_else(|| format!("no extract run `{id}`"))?;
        self.jobs.cancel(&held.job)
    }

    /// Where the run `id` stands: `{run, title, state, tally, job}`.
    pub fn status(&self, id: &str) -> Option<Value> {
        let runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
        runs.get(id).map(|h| {
            let (state, tally, finished) = *h.status.lock().unwrap_or_else(PoisonError::into_inner);
            json!({
                "run": id, "title": h.title, "state": state, "tally": tally,
                "job": h.job, "finished": finished,
            })
        })
    }

    /// Whether `id` is one of this runner's runs.
    pub fn has(&self, id: &str) -> bool {
        self.runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(id)
    }

    /// The runs for `workflow.runs`, shaped as the export runs' summaries
    /// (`id`, `title`, `state`, `items`, `job`, `nodes` with per node
    /// counts by place), with `kind: "extract"`.
    pub fn summaries(&self) -> Vec<Value> {
        let now = now_ms();
        let runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
        runs.iter()
            .map(|(id, held)| {
                let (state, tally, finished) =
                    *held.status.lock().unwrap_or_else(PoisonError::into_inner);
                let rows = held.rows.lock().unwrap_or_else(PoisonError::into_inner);
                let nodes: serde_json::Map<String, Value> = held
                    .graph
                    .iter()
                    .map(|(name, parents)| {
                        let mut counts: BTreeMap<&str, u64> = Status::ALL
                            .iter()
                            .map(|s| (s.name(), 0))
                            .chain(["waiting", "upstream", "held"].map(|p| (p, 0)))
                            .collect();
                        for item in &held.items {
                            *counts
                                .entry(place(&rows, item, name, parents, now))
                                .or_default() += 1;
                        }
                        let mut out = json!(counts);
                        out["asking"] = json!(0);
                        out["leases_out"] = json!(0);
                        out["concurrency"] = json!(1);
                        out["queue"] = json!({ "kind": KIND });
                        out["parents"] = json!(parents);
                        (name.clone(), out)
                    })
                    .collect();
                json!({
                    "id": id,
                    "kind": KIND,
                    "title": held.title,
                    "state": state,
                    "durable": self.vault.is_some(),
                    "owner": self.owner,
                    "created": held.created,
                    "finished": finished,
                    "format": FORMAT,
                    "read_only": false,
                    "items": held.items.len(),
                    "job": held.job,
                    "export": Value::Null,
                    "extract": { "nodes": held.nodes, "tally": tally },
                    "nodes": nodes,
                })
            })
            .collect()
    }

    /// `workflow.items` for an extract run: one page of its items at
    /// `node` (its last node when not given), shaped as the export runs'
    /// pages. `None` when `run` is not one of this runner's.
    pub fn items(
        &self,
        run: &str,
        node: Option<&str>,
        filter: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Option<Result<Value, Message>> {
        let now = now_ms();
        let held = self
            .runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(run)?
            .clone();
        let at = match node {
            Some(name) => held.graph.iter().find(|(n, _)| n == name),
            None => held.graph.last(),
        };
        let Some((name, parents)) = at else {
            return Some(Err(Message::new(
                "workflow-no-node",
                "run `{ $run }` does not queue at `{ $node }`",
            )
            .with("run", run)
            .with("node", node.unwrap_or_default())));
        };
        let rows = held.rows.lock().unwrap_or_else(PoisonError::into_inner);
        let matching: Vec<(&String, &str)> = held
            .items
            .iter()
            .map(|item| (item, place(&rows, item, name, parents, now)))
            .filter(|(_, place)| filter.is_none_or(|f| f == *place))
            .collect();
        let page: Vec<Value> = matching
            .iter()
            .skip(offset)
            .take(limit)
            .map(|(item, place)| {
                let row = rows.get(item, name);
                json!({
                    "item": item,
                    "title": image_of(item).map(|h| &h[..h.len().min(12)]),
                    "path": Value::Null,
                    "output": Value::Null,
                    "state": place,
                    "row": row.as_ref().map(|r| json!({
                        "item": item,
                        "status": r.status,
                        "attempts": r.attempts,
                        "since": r.since,
                        "due": r.due,
                        "error": r.error,
                    })),
                    "error": row.and_then(|r| r.error),
                })
            })
            .collect();
        Some(Ok(json!({
            "run": run,
            "node": name,
            "total": matching.len(),
            "offset": offset,
            "items": page,
            "now": now,
        })))
    }

    /// Waits for the thread working the run `id` to end (a test, a node
    /// stopping).
    pub fn join(&self, id: &str) {
        let thread = {
            let runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
            runs.get(id).and_then(|h| {
                h.thread
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
            })
        };
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

/// Whether a `workflow/runs` document is an extract run's, which the
/// export runner leaves alone.
pub fn is_extract(doc: &Value) -> bool {
    doc["kind"] == KIND
}

/// The built-in extractors, as a catalogue.
fn builtin_catalog() -> Catalog {
    let mut catalog = Catalog::default();
    for def in helios_extract::builtins().extractors {
        if let Err(e) = catalog.add(def) {
            log::warn!("{e}");
        }
    }
    catalog
}
