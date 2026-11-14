//! Every long job as a promise the node keeps (see `docs/protocol.md`,
//! "Promises"): what reports through [`helios_core::jobs`] on a bus (an
//! export, a batch, the trash, a backup, a catalogue snapshot, a
//! calibration, a workshop run, the capture and cull passes) is mirrored,
//! one promise per job, so every client of the node (the Activity focus,
//! an agent's `promises.list`) sees it. The `job.*` events stay as they are.
//!
//! - **One promise per top-level job**: a job inside another (an export of
//!   a batch) is its parent's progress; the background upkeep of
//!   [`QUIET`] is left out (the image map's passes stand as one promise of
//!   their own, [`Standing`]).
//! - **Its progress** (`done` of `total`, the item at hand as `note`) is
//!   written at most every [`EVERY`]; its end settles it: finished, `done`
//!   with `{job, done, total, failed: [{path, error}], skipped}` and what
//!   the job finished with (its `result`: `folder`, `images`; its
//!   `actions`); failed, not retryable (the job is not a kind the node runs
//!   again); cancelled.
//! - **Cancel**: `promises.cancel` reaches the job through the jobs it was
//!   started on ([`crate::promises::Promises::cancel_through`]). A job that
//!   cannot be cancelled says so in its request (`cancellable: false`).
//! - **Kept**: the newest [`KEPT`] settled mirrored promises; older ones
//!   are forgotten. A job still running when the node stops settles
//!   `failed` ([`INTERRUPTED`]).
//!
//! [`Standing`] is a promise with no end and no total, for the work that
//! runs as long as the node does (the image map, the thumbnail re-check):
//! running while it works, settled `done` when it stops.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use helios_core::bus::Envelope;
use helios_core::jobs::{CANCELLED, FAILED, FINISHED, ITEM_FAILED, PAUSED, PROGRESS, STARTED};
use helios_vault::promises::{Filter, INTERRUPTED, State};
use helios_vault::{Store, Vault};
use serde_json::{Map, Value, json};

use crate::node::NodeJobs;

/// The kinds of jobs not mirrored: the node's background upkeep (the
/// image map's passes, the stacks' suggestions) and the import window's
/// listings of its sources.
pub const QUIET: &[&str] = &["map", "stacks.suggest", "importer.scan"];

/// How often a running job's progress is written at most.
pub const EVERY: Duration = Duration::from_millis(250);

/// How many settled mirrored promises are kept.
pub const KEPT: usize = 100;

/// The key of a mirrored promise's request naming its job.
pub const JOB: &str = "job";

/// The key of a standing promise's request.
pub const STANDING: &str = "standing";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn warn(e: &str) {
    log::warn!(target: "helios::promises", "{e}");
}

/// Whether promise request `request` stands for work with no end
/// ([`Standing`]): it shows, but never counts as running in the status bar.
pub fn is_standing(request: &Value) -> bool {
    request[STANDING].as_bool().unwrap_or(false)
}

/// A job followed, and its promise.
struct Mirrored {
    promise: String,
    done: u64,
    total: Option<u64>,
    note: Option<String>,
    failed: Vec<Value>,
    skipped: u64,
    /// Progress heard and not written yet.
    dirty: bool,
    written: Instant,
}

/// The jobs followed and their promises, on the mirror's thread.
struct Mirror<S: Store> {
    vault: Arc<Vault<S>>,
    jobs: BTreeMap<String, Mirrored>,
}

impl<S: Store> Mirror<S> {
    fn hear(&mut self, e: &Envelope) {
        let d = &e.payload;
        let Some(id) = d["id"].as_str() else {
            return;
        };
        match e.kind {
            STARTED => self.started(id, d),
            PROGRESS => {
                if let Some(m) = self.jobs.get_mut(id) {
                    if let Some(done) = d["done"].as_u64() {
                        m.done = done;
                    }
                    if let Some(total) = d["total"].as_u64() {
                        m.total = Some(total);
                    }
                    if let Some(skipped) = d["skipped"].as_u64() {
                        m.skipped = skipped;
                    }
                    if let Some(item) = d["item"].as_str() {
                        m.note = Some(item.to_string());
                    }
                    m.dirty = true;
                }
            }
            ITEM_FAILED => {
                if let Some(m) = self.jobs.get_mut(id) {
                    m.failed
                        .push(json!({ "path": d["item"], "error": d["error"] }));
                }
            }
            PAUSED => {
                if let Some(m) = self.jobs.get_mut(id) {
                    m.note = d["reason"].as_str().map(String::from);
                    m.dirty = true;
                }
            }
            FINISHED => {
                if let Some(m) = self.jobs.remove(id) {
                    let mut result = Map::new();
                    result.insert(JOB.into(), json!(id));
                    result.insert("done".into(), json!(m.done));
                    result.insert("total".into(), json!(m.total));
                    result.insert("failed".into(), json!(m.failed));
                    result.insert("skipped".into(), json!(m.skipped));
                    if let Value::Object(made) = &d["result"] {
                        result.extend(made.clone());
                    }
                    if d["actions"].is_array() {
                        result.insert("actions".into(), d["actions"].clone());
                    }
                    self.settle(
                        &m.promise,
                        State::Done {
                            result: Value::Object(result),
                        },
                    );
                }
            }
            FAILED => {
                if let Some(m) = self.jobs.remove(id) {
                    let reason = d["error"].as_str().unwrap_or("failed").to_string();
                    self.settle(
                        &m.promise,
                        State::Failed {
                            reason,
                            retryable: false,
                        },
                    );
                }
            }
            CANCELLED => {
                if let Some(m) = self.jobs.remove(id) {
                    self.settle(&m.promise, State::Cancelled);
                }
            }
            _ => {}
        }
    }

    fn started(&mut self, id: &str, d: &Value) {
        let kind = d["kind"].as_str().unwrap_or("job");
        if d["parent"].is_string() || QUIET.contains(&kind) || self.jobs.contains_key(id) {
            return;
        }
        // A job a promise's own work runs under is that promise already.
        let open = Filter {
            states: vec!["pending".into(), "running".into()],
            owner: Some(id.into()),
            kind: None,
        };
        if self.vault.promises(&open).is_ok_and(|p| !p.is_empty()) {
            return;
        }
        let title = d["title"].as_str().unwrap_or(id);
        // A job relayed from another node is cancelled on it, not here.
        let cancellable = d["cancellable"].as_bool().unwrap_or(true) && !id.starts_with("node:");
        let request = json!({ JOB: id, "cancellable": cancellable });
        let total = d["total"].as_u64();
        let created = self
            .vault
            .create_promise(kind, id, title, request, now())
            .and_then(|p| {
                self.vault.set_promise_live(&p.id, true);
                self.vault.progress_promise(&p.id, 0, total, None, now())
            });
        match created {
            Ok(p) => {
                self.jobs.insert(
                    id.to_string(),
                    Mirrored {
                        promise: p.id,
                        done: 0,
                        total,
                        note: None,
                        failed: Vec::new(),
                        skipped: 0,
                        dirty: false,
                        written: Instant::now(),
                    },
                );
            }
            Err(e) => warn(&format!("job `{id}` has no promise: {e}")),
        }
    }

    /// Writes the progress heard, at most every [`EVERY`] a job (`all`:
    /// whatever its last write).
    fn flush(&mut self, all: bool) {
        for m in self.jobs.values_mut() {
            if m.dirty && (all || m.written.elapsed() >= EVERY) {
                if let Err(e) = self.vault.progress_promise(
                    &m.promise,
                    m.done,
                    m.total,
                    m.note.as_deref(),
                    now(),
                ) {
                    warn(&e);
                }
                m.dirty = false;
                m.written = Instant::now();
            }
        }
    }

    fn settle(&self, promise: &str, outcome: State) {
        if let Err(e) = self.vault.settle_promise(promise, outcome, now()) {
            warn(&e);
        }
        self.vault.set_promise_live(promise, false);
        self.prune();
    }

    /// Forgets the settled mirrored promises past the newest [`KEPT`].
    fn prune(&self) {
        let Ok(all) = self.vault.promises(&Filter::default()) else {
            return;
        };
        let old = all
            .iter()
            .filter(|p| p.state.is_settled() && p.request[JOB].is_string())
            .skip(KEPT);
        for p in old {
            if let Err(e) = self.vault.forget_promise(&p.id) {
                warn(&e);
            }
        }
    }

    /// The node stops: the jobs still running settle failed.
    fn interrupt(&mut self) {
        for (_, m) in std::mem::take(&mut self.jobs) {
            self.settle(
                &m.promise,
                State::Failed {
                    reason: INTERRUPTED.into(),
                    retryable: false,
                },
            );
        }
    }
}

/// The node's mirror of a bus's jobs as promises: running until the node
/// stops (adopted by it, [`crate::node::Link::adopt`]) or it is dropped.
pub struct JobPromises {
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl JobPromises {
    /// Mirrors the jobs reported on `events` (a bus's subscription, taken
    /// before the first job starts) as promises of `vault`.
    pub fn start<S: Store + 'static>(vault: Arc<Vault<S>>, events: Receiver<Envelope>) -> Self
    where
        Vault<S>: Send + Sync,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::Builder::new()
            .name("helios-job-promises".into())
            .spawn(move || {
                let mut mirror = Mirror {
                    vault,
                    jobs: BTreeMap::new(),
                };
                loop {
                    match events.recv_timeout(EVERY / 2) {
                        Ok(e) => mirror.hear(&e),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    mirror.flush(false);
                    if stopping.load(Ordering::SeqCst) {
                        while let Ok(e) = events.try_recv() {
                            mirror.hear(&e);
                        }
                        break;
                    }
                }
                mirror.flush(true);
                mirror.interrupt();
            })
            .map_err(|e| warn(&format!("jobs are not mirrored as promises: {e}")))
            .ok();
        Self {
            stop,
            thread: Mutex::new(thread),
        }
    }

    /// Stops mirroring once what was heard is written; waits for it.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(t) = thread {
            let _ = t.join();
        }
    }
}

impl Drop for JobPromises {
    fn drop(&mut self) {
        self.stop();
    }
}

impl NodeJobs for JobPromises {
    /// None: the jobs it mirrors are their owners' to count.
    fn running(&self) -> Vec<String> {
        Vec::new()
    }

    fn cancel_all(&self) {
        self.stop();
    }

    fn wait(&self, _deadline: Duration) -> bool {
        self.stop();
        true
    }
}

/// A promise with no end and no total: work that runs as long as the node
/// does. Running from [`Standing::start`] until dropped, then settled
/// `done`. Each start forgets the earlier ones of its kind.
pub struct Standing<S: Store> {
    vault: Arc<Vault<S>>,
    id: Option<String>,
}

impl<S: Store> Standing<S> {
    /// A standing promise of `kind` for `label`, running from now on.
    pub fn start(vault: Arc<Vault<S>>, kind: &str, label: &str) -> Self {
        let earlier = Filter {
            kind: Some(kind.into()),
            ..Filter::default()
        };
        for p in vault.promises(&earlier).unwrap_or_default() {
            if !is_standing(&p.request) {
                continue;
            }
            if !p.state.is_settled() {
                let _ = vault.settle_promise(&p.id, State::Cancelled, now());
            }
            let _ = vault.forget_promise(&p.id);
        }
        let request = json!({ STANDING: true, "cancellable": false, "origin": "node" });
        let id = vault
            .create_promise(kind, &format!("{kind}-{{id}}"), label, request, now())
            .and_then(|p| {
                vault.set_promise_live(&p.id, true);
                vault.progress_promise(&p.id, 0, None, None, now())
            })
            .map(|p| p.id)
            .map_err(|e| warn(&format!("{kind}: no promise: {e}")))
            .ok();
        Self { vault, id }
    }

    /// The promise's id, when it was made.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// What it does now (`done` so far, a note).
    pub fn progress(&self, done: u64, note: Option<&str>) {
        if let Some(id) = &self.id
            && let Err(e) = self.vault.progress_promise(id, done, None, note, now())
        {
            warn(&e);
        }
    }
}

impl<S: Store> Drop for Standing<S> {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            if let Err(e) = self
                .vault
                .settle_promise(&id, State::Done { result: json!({}) }, now())
            {
                warn(&e);
            }
            self.vault.set_promise_live(&id, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use helios_core::Bus;
    use helios_core::jobs::{Jobs, Progress, Start};
    use helios_vault::MemoryStore;
    use helios_vault::promises::Promise;

    use super::*;
    use crate::promises::Promises;

    fn mirrored() -> (Arc<Vault<MemoryStore>>, Jobs, JobPromises) {
        let bus = Arc::new(Bus::new());
        let vault = Arc::new(Vault::new(MemoryStore::default()));
        let mirror = JobPromises::start(vault.clone(), bus.subscribe());
        (vault, Jobs::new(bus), mirror)
    }

    /// Waits until the promise of job `job` satisfies `ok`.
    fn until(vault: &Vault<MemoryStore>, job: &str, ok: impl Fn(&Promise) -> bool) -> Promise {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let found = vault
                .promises(&Filter {
                    owner: Some(job.into()),
                    ..Filter::default()
                })
                .unwrap();
            if let Some(p) = found.into_iter().next()
                && ok(&p)
            {
                return p;
            }
            assert!(Instant::now() < deadline, "job `{job}`: never got there");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_job_runs_as_a_promise_with_its_progress_and_settles_with_its_result() {
        let (vault, jobs, _mirror) = mirrored();
        let job = jobs.start(Start::new("trash-1", "Move 3 files to the trash", "trash").total(3));
        let p = until(&vault, "trash-1", |p| {
            matches!(p.state, State::Running { total: Some(3), .. })
        });
        assert_eq!(
            (p.kind.as_str(), p.label.as_str()),
            ("trash", "Move 3 files to the trash")
        );
        assert!(vault.promise_live(&p.id), "the node works on it");
        job.progress(Progress::new().counts(2, 3).item("b.raw"));
        job.item_failed("/photos/c.raw", "read-only");
        until(
            &vault,
            "trash-1",
            |p| matches!(&p.state, State::Running { done: 2, note: Some(n), .. } if n == "b.raw"),
        );
        job.finished_result(json!({ "folder": "/out" }));
        let p = until(&vault, "trash-1", |p| p.state.is_settled());
        let State::Done { result } = p.state else {
            panic!("{p:?}")
        };
        assert_eq!(result["folder"], "/out");
        assert_eq!(result["done"], 2);
        assert_eq!(result["failed"][0]["path"], "/photos/c.raw");
        assert!(!vault.promise_live(&p.id));
    }

    #[test]
    fn a_job_without_a_total_stays_without_one_and_a_failure_is_not_retryable() {
        let (vault, jobs, _mirror) = mirrored();
        let job = jobs.start(Start::new("snapshot-1", "Snapshot", "snapshot"));
        until(&vault, "snapshot-1", |p| {
            matches!(p.state, State::Running { total: None, .. })
        });
        job.failed("the disk is full");
        let p = until(&vault, "snapshot-1", |p| p.state.is_settled());
        assert_eq!(
            p.state,
            State::Failed {
                reason: "the disk is full".into(),
                retryable: false
            }
        );
    }

    #[test]
    fn children_and_quiet_jobs_are_left_out() {
        let (vault, jobs, mirror) = mirrored();
        let batch = jobs.start(Start::new("batch-1", "Export 2 photos", "batch").total(2));
        let child = jobs.start(Start::new("export-1", "Export a", "export").parent("batch-1", 1.0));
        let map = jobs.start(Start::new("map-1", "Image map", "map"));
        child.finished();
        map.finished();
        batch.finished_with(
            json!([{ "title": "Open folder", "command": "library.openFolder",
                                       "args": { "path": "/out" } }]),
        );
        let p = until(&vault, "batch-1", |p| p.state.is_settled());
        let State::Done { result } = p.state else {
            panic!("{p:?}")
        };
        assert_eq!(result["actions"][0]["args"]["path"], "/out");
        mirror.stop();
        let all = vault.promises(&Filter::default()).unwrap();
        assert_eq!(all.len(), 1, "{all:?}");
    }

    #[test]
    fn cancel_on_the_promise_cancels_the_job() {
        let bus = Arc::new(Bus::new());
        let vault = Arc::new(Vault::new(MemoryStore::default()));
        let _mirror = JobPromises::start(vault.clone(), bus.subscribe());
        let jobs = Jobs::new(bus.clone());
        let promises = Promises::new(vault.clone(), Jobs::new(Arc::new(Bus::new())));
        promises.cancel_through(jobs.clone());
        let job = Arc::new(jobs.start(Start::new("backup-1", "Back up", "backup").total(9)));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let stop = stop.clone();
            job.on_cancel(move || stop.store(true, Ordering::SeqCst));
        }
        let p = until(&vault, "backup-1", |p| !p.state.is_settled());
        assert_eq!(p.request["cancellable"], true);
        promises.cancel(&p.id).unwrap();
        assert!(stop.load(Ordering::SeqCst), "the job was asked to stop");
        job.cancelled();
        let p = until(&vault, "backup-1", |p| p.state.is_settled());
        assert_eq!(p.state, State::Cancelled);
    }

    #[test]
    fn a_job_running_when_the_node_stops_settles_interrupted() {
        let (vault, jobs, mirror) = mirrored();
        let _job = jobs.start(Start::new("render-1", "Render", "render"));
        until(&vault, "render-1", |p| !p.state.is_settled());
        mirror.stop();
        let p = until(&vault, "render-1", |p| p.state.is_settled());
        assert!(
            matches!(p.state, State::Failed { ref reason, retryable: false } if reason == INTERRUPTED)
        );
    }

    #[test]
    fn a_standing_promise_runs_without_a_total_until_dropped_and_replaces_the_last() {
        let vault = Arc::new(Vault::new(MemoryStore::default()));
        let first = Standing::start(vault.clone(), "map", "Image map");
        let id = first.id().unwrap().to_string();
        let p = vault.promise(&id).unwrap().unwrap();
        assert!(is_standing(&p.request));
        assert!(matches!(p.state, State::Running { total: None, .. }));
        drop(first);
        assert!(matches!(
            vault.promise(&id).unwrap().unwrap().state,
            State::Done { .. }
        ));
        let again = Standing::start(vault.clone(), "map", "Image map");
        assert!(
            vault.promise(&id).unwrap().is_none(),
            "the last one is forgotten"
        );
        assert_ne!(again.id(), Some(id.as_str()));
    }
}
