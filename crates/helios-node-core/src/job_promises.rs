//! Every long job as a promise the node keeps (see `docs/protocol.md`,
//! "Promises"): what reports through [`helios_core::jobs`] on a bus (an
//! export, a batch, the trash, a backup, a catalogue snapshot, a
//! calibration, a workshop run, the capture and cull passes) is mirrored,
//! one promise per job, so every client of the node (the Activity focus,
//! an agent's `promises.list`) sees it. The `helios.job.*` events stay as they are.
//!
//! - **One promise per top-level job**: a job inside another (an export of
//!   a batch) is its parent's progress, a job naming the promise it is the
//!   work of (`promise` in `helios.job.started`: an import by rule) is that
//!   promise already; the background upkeep of
//!   [`QUIET`] is left out (the image map's passes stand as one promise of
//!   their own, [`Standing`]).
//! - **Its progress** (`done` of `total`, the item at hand as `note`) is
//!   written at most every [`EVERY`]; its end settles it: finished, `done`
//!   with `{job, done, total, failed: [{path, error}], skipped}`, the
//!   photos it named as it started (`images`, at most [`IMAGES_KEPT`]: a
//!   batch's, the trash's) and what
//!   the job finished with (its `result`: `folder`, `images`; its
//!   `actions`); failed, not retryable (the job is not a kind the node runs
//!   again); cancelled.
//! - **Cancel**: `promises.cancel` reaches the job through the jobs it was
//!   started on ([`crate::promises::Promises::cancel_through`]). A job that
//!   cannot be cancelled says so in its request (`cancellable: false`); a
//!   job relayed from another node that can be says `relayed: true`: the
//!   desktop following that node sends the cancel there.
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

use helios_core::Message;
use helios_core::bus::Envelope;
use helios_core::jobs::{
    CANCELLED, FAILED, FINISHED, ITEM_FAILED, PAUSED, PROGRESS, RESUMED, STARTED,
};
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

/// How many photos a job's settled promise names at most (`images`: what
/// "Show in Library" shows); a job on more names none.
pub const IMAGES_KEPT: usize = 500;

/// The key of a mirrored promise's request saying its job runs on
/// another node (`node:<name>/<id>`) and is cancelled there, through the
/// relay of the desktop following it.
pub const RELAYED: &str = "relayed";

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
    /// The photos it works on, when it named them (at most
    /// [`IMAGES_KEPT`]).
    images: Option<Value>,
    /// Progress heard and not written yet.
    dirty: bool,
    written: Instant,
    /// Its last word (started, progress, an item failed, paused, resumed).
    heard: Instant,
    /// Paused: it said why, never a stall.
    paused: bool,
    /// Said stalled in the log since its last word.
    flagged: bool,
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
                    m.heard = Instant::now();
                    m.paused = false;
                    m.flagged = false;
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
                    m.heard = Instant::now();
                    m.flagged = false;
                    m.failed
                        .push(json!({ "path": d["item"], "error": d["error"] }));
                }
            }
            PAUSED => {
                if let Some(m) = self.jobs.get_mut(id) {
                    m.heard = Instant::now();
                    m.paused = true;
                    m.flagged = false;
                    m.note = d["reason"].as_str().map(String::from);
                    m.dirty = true;
                }
            }
            RESUMED => {
                if let Some(m) = self.jobs.get_mut(id) {
                    m.heard = Instant::now();
                    m.paused = false;
                    m.flagged = false;
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
                    if let Some(images) = m.images {
                        result.insert("images".into(), images);
                    }
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
        // A job naming its promise (an import by rule) is that promise's
        // work: the promise already stands for it.
        if d["parent"].is_string()
            || d["promise"].is_string()
            || QUIET.contains(&kind)
            || self.jobs.contains_key(id)
        {
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
        // A job relayed from another node is cancelled on it, not here:
        // `relayed`, its desktop sends the cancel to that node.
        let stoppable = d["cancellable"].as_bool().unwrap_or(true);
        let relayed = id.starts_with("node:");
        let mut request = json!({ JOB: id, "cancellable": stoppable && !relayed });
        if relayed && stoppable {
            request[RELAYED] = json!(true);
        }
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
                        images: d["images"]
                            .as_array()
                            .filter(|i| !i.is_empty() && i.len() <= IMAGES_KEPT)
                            .map(|i| Value::Array(i.clone())),
                        dirty: false,
                        written: Instant::now(),
                        heard: Instant::now(),
                        paused: false,
                        flagged: false,
                    },
                );
            }
            Err(e) => warn(&format!("job `{id}` has no promise: {e}")),
        }
    }

    /// Writes the progress heard, at most every [`EVERY`] a job (`all`:
    /// whatever its last write).
    fn flush(&mut self, all: bool) {
        for (id, m) in &mut self.jobs {
            // A job running with no word past STALLED: said once in the
            // log, with the item it was on.
            if !m.paused && !m.flagged && m.heard.elapsed() >= STALLED {
                m.flagged = true;
                let on = m.note.as_deref().unwrap_or("no item said");
                log::warn!(
                    target: "helios::promises",
                    "job `{id}`: no progress for {} min: {on}",
                    m.heard.elapsed().as_secs() / 60
                );
            }
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
    /// The mirror's thread, and what tells its end: the sender its
    /// thread holds, dropped as it ends.
    thread: Mutex<Option<(JoinHandle<()>, Receiver<()>)>>,
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
        let (ending, ended) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("helios-job-promises".into())
            .spawn(move || {
                // Dropped as the thread ends, a panic included.
                let _ending = ending;
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
            .ok()
            .map(|t| (t, ended));
        Self {
            stop,
            thread: Mutex::new(thread),
        }
    }

    /// Stops mirroring once what was heard is written; waits for it, at
    /// most [`MIRROR_STOP`].
    pub fn stop(&self) {
        self.stop_within(MIRROR_STOP);
    }

    /// Stops mirroring once what was heard is written; waits for it, at
    /// most `deadline`: whether it stopped. Past it, the mirror's thread
    /// is left (logged): the jobs it did not settle stay unsettled in the
    /// vault, and the next start settles them `interrupted`, as after a
    /// crash ([`crate::promises`]).
    pub fn stop_within(&self, deadline: Duration) -> bool {
        self.stop.store(true, Ordering::SeqCst);
        let mut thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner);
        let Some((t, ended)) = thread.take() else {
            return true;
        };
        // Woken as the thread ends (its sender dropped), not polled.
        if let Err(RecvTimeoutError::Timeout) = ended.recv_timeout(deadline) {
            log::warn!(
                target: "helios::node",
                "the jobs' mirror did not stop within {} ms: left, its last jobs settle at the next start",
                deadline.as_millis()
            );
            // Kept, so that a later stop waits for it again.
            *thread = Some((t, ended));
            return false;
        }
        let _ = t.join();
        true
    }
}

/// How long the mirror's stop waits for its last writes.
pub const MIRROR_STOP: Duration = Duration::from_secs(2);

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

    fn name(&self) -> Message {
        Message::new("node-end-mirror", "Saving the progress of the jobs")
    }

    /// The mirror is stopped by [`NodeJobs::wait`]: it writes what it
    /// heard first.
    fn cancel_all(&self) {}

    fn wait(&self, deadline: Duration) -> bool {
        self.stop_within(deadline)
    }

    fn stop_within(&self) -> Duration {
        MIRROR_STOP
    }
}

/// How long a work may go without a word while it says it works before
/// it is flagged: a log line ([`StandingWatch::check`] for a standing
/// work, which also says it in its promise's note; the mirror for a job).
pub const STALLED: Duration = Duration::from_mins(5);

/// What a standing work last said, shared with its watch.
struct Said {
    done: u64,
    note: Option<String>,
    /// Waiting (nothing to do, another job, a pause): never a stall.
    waiting: bool,
    /// Its last word, working or waiting.
    heard: Instant,
    /// The last write of its promise.
    written: Option<Instant>,
    /// Flagged as stalled since its last word.
    flagged: bool,
}

/// A promise with no end and no total: work that runs as long as the node
/// does. Running from [`Standing::start`] until dropped, then settled
/// `done`. Each start forgets the earlier ones of its kind.
///
/// The work says what it does ([`Standing::working`]: the item at hand
/// and how many so far) or what it waits on ([`Standing::waiting`]); the
/// promise's note shows it, so the Activity focus never shows a standing
/// work running with nothing said. A watch ([`Standing::watch`]) flags it
/// when it says it works and then says nothing past a deadline.
pub struct Standing<S: Store> {
    vault: Arc<Vault<S>>,
    id: Option<String>,
    kind: String,
    said: Arc<Mutex<Said>>,
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
        Self {
            vault,
            id,
            kind: kind.into(),
            said: Arc::new(Mutex::new(Said {
                done: 0,
                note: None,
                waiting: false,
                heard: Instant::now(),
                written: None,
                flagged: false,
            })),
        }
    }

    /// The promise's id, when it was made.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// What it does now (`done` so far, a note), written at once.
    pub fn progress(&self, done: u64, note: Option<&str>) {
        if let Some(id) = &self.id
            && let Err(e) = self.vault.progress_promise(id, done, None, note, now())
        {
            warn(&e);
        }
    }

    /// Working on `item`, `done` so far: written at most every [`EVERY`].
    pub fn working(&self, done: u64, item: &str) {
        self.say(done, item, false);
    }

    /// Waiting on `why`, `done` so far: written at once when it changes.
    pub fn waiting(&self, done: u64, why: &str) {
        self.say(done, why, true);
    }

    fn say(&self, done: u64, note: &str, waiting: bool) {
        let write = {
            let mut said = self.said.lock().unwrap_or_else(PoisonError::into_inner);
            said.heard = Instant::now();
            if said.flagged {
                said.flagged = false;
                log::info!(target: "helios::promises", "{}: going again: {note}", self.kind);
            }
            let changed = said.note.as_deref() != Some(note) || said.waiting != waiting;
            let due = said.written.is_none_or(|w| w.elapsed() >= EVERY);
            // A new wait is written at once; the item at hand and the
            // count at most every EVERY.
            let write = (changed && (waiting || due)) || (said.done != done && due);
            if write {
                said.done = done;
                said.note = Some(note.to_string());
                said.waiting = waiting;
                said.written = Some(Instant::now());
            }
            write
        };
        if write {
            self.progress(done, Some(note));
        }
    }

    /// A watch on it, for whoever checks it is alive.
    pub fn watch(&self) -> StandingWatch<S> {
        StandingWatch {
            vault: self.vault.clone(),
            id: self.id.clone(),
            kind: self.kind.clone(),
            said: self.said.clone(),
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

/// A watch on a [`Standing`] work: [`StandingWatch::check`] flags it when
/// it said it works and has said nothing since, past a deadline.
pub struct StandingWatch<S: Store> {
    vault: Arc<Vault<S>>,
    id: Option<String>,
    kind: String,
    said: Arc<Mutex<Said>>,
}

impl<S: Store> StandingWatch<S> {
    /// Flags the work when it has said nothing for `after` while working
    /// (once until its next word): a warning in the log and its promise's
    /// note saying for how long and on what. Waiting is never a stall: it
    /// said what it waits on. Whether it is flagged now.
    pub fn check(&self, after: Duration) -> bool {
        let (done, note) = {
            let mut said = self.said.lock().unwrap_or_else(PoisonError::into_inner);
            if said.flagged {
                return true;
            }
            if said.waiting || said.heard.elapsed() < after {
                return false;
            }
            said.flagged = true;
            let secs = said.heard.elapsed().as_secs();
            let on = said.note.clone().unwrap_or_else(|| "starting".into());
            let note = format!("No progress for {} min {} s: {on}", secs / 60, secs % 60);
            log::warn!(target: "helios::promises", "{}: {note}", self.kind);
            said.note = Some(note.clone());
            (said.done, note)
        };
        if let Some(id) = &self.id
            && let Err(e) = self
                .vault
                .progress_promise(id, done, None, Some(&note), now())
        {
            warn(&e);
        }
        true
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
    fn a_job_a_promise_stands_for_is_not_mirrored_twice_and_a_job_names_its_photos() {
        let (vault, jobs, mirror) = mirrored();
        let by_rule = jobs.start(Start::new("import-7", "Import a card", "import").promise("p-1"));
        let export = jobs.start(
            Start::new("export-2", "Export 2 photos", "batch").images(["a".into(), "b".into()]),
        );
        by_rule.finished();
        export.finished();
        let p = until(&vault, "export-2", |p| p.state.is_settled());
        let State::Done { result } = p.state else {
            panic!("{p:?}")
        };
        // The photos it named: what "Show in Library" shows.
        assert_eq!(result["images"], json!(["a", "b"]));
        mirror.stop();
        let all = vault.promises(&Filter::default()).unwrap();
        assert_eq!(all.len(), 1, "{all:?}");
        assert_eq!(all[0].request[JOB], "export-2");
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

    /// The note of standing promise `standing`.
    fn note_of(vault: &Vault<MemoryStore>, standing: &Standing<MemoryStore>) -> (u64, String) {
        match vault
            .promise(standing.id().unwrap())
            .unwrap()
            .unwrap()
            .state
        {
            State::Running {
                done,
                note: Some(note),
                ..
            } => (done, note),
            other => panic!("not running with a note: {other:?}"),
        }
    }

    #[test]
    fn a_standing_work_says_what_it_waits_on_and_is_flagged_when_silent_while_working() {
        let vault = Arc::new(Vault::new(MemoryStore::default()));
        let map = Standing::start(vault.clone(), "map", "Image map");
        let watch = map.watch();
        // A wait is written at once.
        map.waiting(0, "Up to date");
        assert_eq!(note_of(&vault, &map), (0, "Up to date".into()));
        // Waiting is never a stall, however long.
        std::thread::sleep(Duration::from_millis(30));
        assert!(!watch.check(Duration::from_millis(10)));
        // Working, then silent past the deadline: flagged once, the note
        // naming the item it is on.
        std::thread::sleep(EVERY);
        map.working(3, "/photos/a.raw");
        assert_eq!(note_of(&vault, &map), (3, "/photos/a.raw".into()));
        assert!(!watch.check(Duration::from_mins(1)), "heard just now");
        std::thread::sleep(Duration::from_millis(30));
        assert!(watch.check(Duration::from_millis(10)));
        let (done, note) = note_of(&vault, &map);
        assert_eq!(done, 3);
        assert!(
            note.starts_with("No progress for 0 min") && note.ends_with(": /photos/a.raw"),
            "{note}"
        );
        // Its next word clears the flag and is written.
        map.waiting(4, "Waiting for the import job to end");
        assert!(!watch.check(Duration::from_mins(1)));
        assert_eq!(
            note_of(&vault, &map),
            (4, "Waiting for the import job to end".into())
        );
    }
}
