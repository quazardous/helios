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
//! Not yet: the node verb, the run listed in `workflow.runs` and the
//! Queue panel, a run taken over after a restart.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use helios_ai::Ai;
use helios_core::Bus;
use helios_core::jobs::{Jobs, Progress, Report, Start};
use helios_extract::{MemoryCache, ResultCache};
use helios_journal::MemoryStore;
use helios_vault::journal::VaultJournal;
use serde::Serialize;
use serde_json::{Value, json};

use super::extract::{Ended, ExtractNode, ExtractRun, ImageSource, Tally};
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

/// What the runner keeps of a run.
struct Held {
    title: String,
    created: u64,
    nodes: Vec<Value>,
    images: usize,
    job: String,
    status: Arc<Mutex<(ExtractState, Tally, Option<u64>)>>,
    thread: Mutex<Option<JoinHandle<()>>>,
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
}

impl ExtractRunner {
    pub fn new(bus: Arc<Bus>, jobs: Jobs, vault: Option<SharedVault>, owner: &str) -> Self {
        Self {
            bus,
            jobs,
            vault,
            owner: owner.into(),
            runs: Mutex::new(BTreeMap::new()),
        }
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
        let store = match &self.vault {
            Some(vault) => RunStore::Vault(VaultJournal::new(vault.clone(), &id)),
            None => RunStore::Memory(MemoryStore::default()),
        };
        let now = now_ms();
        let nodes: Vec<Value> = ask
            .nodes
            .iter()
            .map(|n| json!({ "name": n.name, "after": n.after, "extractor": n.def.id, "version": n.def.version }))
            .collect();
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
