//! Background work as people follow it: `job.*` events on the [`Bus`].
//!
//! The [`scheduler`](crate::scheduler) runs work; this module says how it
//! goes, in the events egui-pop's jobs model folds (its jobs centre, the
//! status bar's jobs item, a job's log console): a job starts with a title
//! and what it will count, reports its progress (done of total, the current
//! item, the step), writes log lines, and ends finished, failed or
//! cancelled. Whatever runs long publishes through it: an export, a batch,
//! the GPU calibration, an import.
//!
//! | Event | Data |
//! |---|---|
//! | `job.started` | `id`, `title`, `kind`, `total`, `steps` (`[{id, title, weight}]`), `parent`, `weight`, `cancellable`, `node` |
//! | `job.progress` | `id`, `done`, `total`, `failed`, `skipped`, `item`, `item_progress`, `step`, `step_done`, `step_total`, `estimate` |
//! | `job.item_failed` | `id`, `item`, `error` |
//! | `job.log` | `id`, `text` |
//! | `job.cancelling` | `id` |
//! | `job.finished`, `job.failed`, `job.cancelled` | `id`, `kind`, `seconds`; `error` when failed |
//!
//! A job may name the graph node it works for (`node`, [`Start::node`]):
//! an export names the node it renders. The desktop's Queue panel lists a
//! node's jobs by it.
//!
//! A job is cancelled by its id ([`Jobs::cancel`], what the `job.cancel`
//! command runs): the job gave the way when it started
//! ([`Report::on_cancel`]), usually the scheduler's cancel of its job.
//!
//! ```
//! use std::sync::Arc;
//! use helios_core::Bus;
//! use helios_core::jobs::{Jobs, Progress, Start};
//!
//! let bus = Arc::new(Bus::new());
//! let jobs = Jobs::new(bus.clone());
//! let job = jobs.start(Start::new("import-1", "Import 3 photos", "import").total(3));
//! job.progress(Progress::new().counts(1, 3).item("a.raw"));
//! job.log("a.raw: imported");
//! job.finished();
//! let kinds: Vec<&str> = bus.since(0).iter().map(|e| e.kind).collect();
//! assert_eq!(kinds, ["job.started", "job.progress", "job.log", "job.finished"]);
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use serde_json::{Map, Value as Json, json};

use crate::bus::Bus;

/// The bus source of the job events.
pub const SOURCE: &str = "jobs";

pub const STARTED: &str = "job.started";
pub const PROGRESS: &str = "job.progress";
pub const ITEM_FAILED: &str = "job.item_failed";
pub const LOG: &str = "job.log";
pub const CANCELLING: &str = "job.cancelling";
pub const FINISHED: &str = "job.finished";
pub const FAILED: &str = "job.failed";
pub const CANCELLED: &str = "job.cancelled";

type Cancel = Arc<dyn Fn() + Send + Sync>;

/// Where jobs report: a bus, and the way to cancel each running job.
/// Cheap to clone; clones share the jobs.
#[derive(Clone)]
pub struct Jobs {
    bus: Arc<Bus>,
    cancels: Arc<Mutex<BTreeMap<String, Cancel>>>,
}

impl fmt::Debug for Jobs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Jobs")
            .field("cancellable", &self.cancellable())
            .finish_non_exhaustive()
    }
}

impl Jobs {
    /// Jobs reporting on `bus`.
    pub fn new(bus: Arc<Bus>) -> Self {
        Self {
            bus,
            cancels: Arc::default(),
        }
    }

    /// The bus the events go on.
    pub fn bus(&self) -> &Arc<Bus> {
        &self.bus
    }

    fn cancels(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Cancel>> {
        self.cancels.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a job: publishes `job.started`. The job reports through
    /// what this returns until it ends; dropped without an end, it fails.
    pub fn start(&self, start: Start) -> Report {
        let kind = start.kind.clone();
        self.bus.publish_value(SOURCE, None, STARTED, start.data());
        Report {
            jobs: self.clone(),
            id: start.id,
            kind,
            started: Instant::now(),
            ended: AtomicBool::new(false),
        }
    }

    /// Cancels a running job: publishes `job.cancelling`, then runs the
    /// job's cancel; the job says `job.cancelled` once it stopped. Refused
    /// for a job that is not running or cannot be cancelled.
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let cancel = self
            .cancels()
            .get(id)
            .cloned()
            .ok_or_else(|| format!("no running job `{id}` to cancel"))?;
        self.bus
            .publish_value(SOURCE, None, CANCELLING, json!({ "id": id }));
        cancel();
        Ok(())
    }

    /// The running jobs that can be cancelled, by id.
    pub fn cancellable(&self) -> Vec<String> {
        self.cancels().keys().cloned().collect()
    }
}

/// How a job starts: its id (unique among running jobs), its title for
/// people, its kind (`export`, `batch`, `calibration`, `import`) and what
/// it will count.
#[derive(Clone, Debug, PartialEq)]
pub struct Start {
    pub id: String,
    pub title: String,
    pub kind: String,
    /// How many items it will do; none: it cannot say (yet).
    pub total: Option<u64>,
    /// Its steps (`id`, `title`, weight): a render is decode 1, process 3,
    /// write 1; the overall estimate weighs them.
    pub steps: Vec<(String, String, f64)>,
    /// The job it is part of, and its weight there.
    pub parent: Option<(String, f64)>,
    pub cancellable: bool,
    /// The graph node it works for (an export: the node it renders).
    pub node: Option<String>,
}

impl Start {
    pub fn new(id: &str, title: &str, kind: &str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            kind: kind.into(),
            total: None,
            steps: Vec::new(),
            parent: None,
            cancellable: true,
            node: None,
        }
    }

    pub fn total(mut self, total: u64) -> Self {
        self.total = Some(total);
        self
    }

    pub fn step(mut self, id: &str, title: &str, weight: f64) -> Self {
        self.steps.push((id.into(), title.into(), weight));
        self
    }

    pub fn parent(mut self, id: &str, weight: f64) -> Self {
        self.parent = Some((id.into(), weight));
        self
    }

    /// The graph node it works for: its queue lists it.
    pub fn node(mut self, node: &str) -> Self {
        self.node = Some(node.into());
        self
    }

    /// It cannot be cancelled once started (no Cancel button).
    pub fn not_cancellable(mut self) -> Self {
        self.cancellable = false;
        self
    }

    fn data(&self) -> Json {
        let mut d = json!({
            "id": self.id,
            "title": self.title,
            "kind": self.kind,
            "cancellable": self.cancellable,
            "pausable": false,
            "retryable": false,
        });
        if let Some(t) = self.total {
            d["total"] = json!(t);
        }
        if !self.steps.is_empty() {
            d["steps"] = self
                .steps
                .iter()
                .map(|(id, title, weight)| json!({ "id": id, "title": title, "weight": weight }))
                .collect();
        }
        if let Some((parent, weight)) = &self.parent {
            d["parent"] = json!(parent);
            d["weight"] = json!(weight);
        }
        if let Some(node) = &self.node {
            d["node"] = json!(node);
        }
        d
    }
}

/// One progress report: only what is set is sent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress(Map<String, Json>);

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    fn set(mut self, key: &str, value: Json) -> Self {
        self.0.insert(key.into(), value);
        self
    }

    /// `done` items of `total`.
    pub fn counts(self, done: u64, total: u64) -> Self {
        self.set("done", json!(done)).set("total", json!(total))
    }

    /// `done` items, the total unknown.
    pub fn done(self, done: u64) -> Self {
        self.set("done", json!(done))
    }

    /// The item being worked on (a file name).
    pub fn item(self, item: &str) -> Self {
        self.set("item", json!(item))
    }

    /// How far the current item is, 0 to 1.
    pub fn item_progress(self, fraction: f64) -> Self {
        self.set("item_progress", json!(fraction.clamp(0.0, 1.0)))
    }

    /// The step under way (one of [`Start::step`]'s).
    pub fn step(self, step: &str) -> Self {
        self.set("step", json!(step))
    }

    /// How far the current step is, in its own counts.
    pub fn step_counts(self, done: u64, total: u64) -> Self {
        self.set("step_done", json!(done))
            .set("step_total", json!(total))
    }

    pub fn failed(self, failed: u64) -> Self {
        self.set("failed", json!(failed))
    }

    pub fn skipped(self, skipped: u64) -> Self {
        self.set("skipped", json!(skipped))
    }

    /// The job's own estimate of the whole, 0 to 1, when the counts do
    /// not say it.
    pub fn estimate(self, fraction: f64) -> Self {
        self.set("estimate", json!(fraction.clamp(0.0, 1.0)))
    }
}

/// A running job's voice: its progress, its log and its end. Shared
/// between threads behind an `Arc` if need be.
pub struct Report {
    jobs: Jobs,
    id: String,
    kind: String,
    started: Instant,
    ended: AtomicBool,
}

impl fmt::Debug for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Report")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("ended", &self.ended.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl Report {
    pub fn id(&self) -> &str {
        &self.id
    }

    fn publish(&self, kind: &'static str, mut data: Json) {
        data["id"] = json!(self.id);
        self.jobs.bus.publish_value(SOURCE, None, kind, data);
    }

    /// How [`Jobs::cancel`] stops this job (the scheduler's cancel of its
    /// job, a flag the work checks between items), until it ends.
    pub fn on_cancel(&self, cancel: impl Fn() + Send + Sync + 'static) {
        if !self.ended.load(Ordering::SeqCst) {
            self.jobs
                .cancels()
                .insert(self.id.clone(), Arc::new(cancel));
        }
    }

    pub fn progress(&self, progress: Progress) {
        self.publish(PROGRESS, Json::Object(progress.0));
    }

    /// Lines of the job's log (its console).
    pub fn log(&self, text: impl AsRef<str>) {
        self.publish(LOG, json!({ "text": text.as_ref() }));
    }

    /// An item failed; the job goes on.
    pub fn item_failed(&self, item: &str, error: &str) {
        self.publish(ITEM_FAILED, json!({ "item": item, "error": error }));
    }

    /// The seconds since the job started.
    pub fn seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Ends the job once: later ends are ignored.
    fn end(&self, kind: &'static str, mut data: Json) {
        if self.ended.swap(true, Ordering::SeqCst) {
            return;
        }
        self.jobs.cancels().remove(&self.id);
        data["kind"] = json!(self.kind);
        data["seconds"] = json!(self.seconds());
        self.publish(kind, data);
    }

    /// It did what it was asked.
    pub fn finished(&self) {
        self.end(FINISHED, json!({}));
    }

    /// It did, and offers what to do next (`[{title, command, args}]`:
    /// "Show in folder").
    pub fn finished_with(&self, actions: Json) {
        self.end(FINISHED, json!({ "actions": actions }));
    }

    /// It failed, saying why.
    pub fn failed(&self, error: &str) {
        self.end(FAILED, json!({ "error": error }));
    }

    /// It stopped because it was cancelled.
    pub fn cancelled(&self) {
        self.end(CANCELLED, json!({}));
    }

    /// Whether it ended.
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }
}

impl Drop for Report {
    /// A job never left running in people's eyes: dropped without an
    /// end, it failed.
    fn drop(&mut self) {
        self.end(
            FAILED,
            json!({ "error": "stopped without saying how it ended" }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_reports_its_progress_and_ends_once() {
        let bus = Arc::new(Bus::new());
        let jobs = Jobs::new(bus.clone());
        let job = jobs.start(
            Start::new("e1", "Export", "export")
                .total(2)
                .step("render", "Render", 3.0)
                .node("exposure"),
        );
        job.progress(Progress::new().counts(1, 2).item("a.jpg").step("render"));
        job.item_failed("b.jpg", "disk full");
        job.failed("disk full");
        job.finished();
        drop(job);
        let events = bus.since(0);
        let kinds: Vec<&str> = events.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                "job.started",
                "job.progress",
                "job.item_failed",
                "job.failed"
            ]
        );
        assert_eq!(events[0].payload["total"], 2);
        assert_eq!(events[0].payload["steps"][0]["weight"], 3.0);
        assert_eq!(events[0].payload["node"], "exposure");
        assert_eq!(events[1].payload["item"], "a.jpg");
        assert_eq!(events[1].payload["id"], "e1");
        assert_eq!(events[3].payload["kind"], "export");
        assert!(events[3].payload["seconds"].as_f64().is_some());
    }

    #[test]
    fn a_job_is_cancelled_by_its_id_until_it_ends() {
        let bus = Arc::new(Bus::new());
        let jobs = Jobs::new(bus.clone());
        let job = jobs.start(Start::new("b1", "Batch", "batch"));
        let asked = Arc::new(AtomicBool::new(false));
        let flag = asked.clone();
        job.on_cancel(move || flag.store(true, Ordering::SeqCst));
        assert_eq!(jobs.cancellable(), ["b1"]);
        jobs.cancel("b1").unwrap();
        assert!(asked.load(Ordering::SeqCst));
        job.cancelled();
        assert!(jobs.cancel("b1").is_err(), "ended: nothing to cancel");
        let kinds: Vec<&str> = bus.since(0).iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["job.started", "job.cancelling", "job.cancelled"]);
    }

    #[test]
    fn a_job_dropped_without_an_end_failed() {
        let bus = Arc::new(Bus::new());
        drop(Jobs::new(bus.clone()).start(Start::new("x", "X", "test")));
        assert_eq!(bus.since(0).last().unwrap().kind, "job.failed");
    }
}
