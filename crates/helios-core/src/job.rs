//! Long operations reported as jobs on the bus, for a jobs centre to show:
//! `job.started`, `job.progress` (done of total, the item at hand),
//! then `job.finished` or `job.failed`.
//!
//! These are a library operation's jobs (an import, a scan), run on any
//! thread by whoever started them; the scheduler's render jobs announce
//! themselves with `scheduler.*` events of their own. A job's id is unique
//! in the process.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::bus::{Bus, Event};

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A job began.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Started {
    pub job: u64,
    /// What kind of job it is (`library.import`).
    pub kind: String,
    /// What to call it, in English.
    pub title: String,
    /// How many items it will go through, when known.
    pub total: Option<u64>,
}

impl Event for Started {
    const KIND: &'static str = "job.started";
}

/// A job moved on: `done` items of `total`, and the one it is on.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Progress {
    pub job: u64,
    pub kind: String,
    pub done: u64,
    pub total: Option<u64>,
    pub current: Option<String>,
}

impl Event for Progress {
    const KIND: &'static str = "job.progress";
}

/// A job ended, with what it did.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finished {
    pub job: u64,
    pub kind: String,
    pub summary: serde_json::Value,
}

impl Event for Finished {
    const KIND: &'static str = "job.finished";
}

/// A job stopped on an error.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Failed {
    pub job: u64,
    pub kind: String,
    pub error: String,
}

impl Event for Failed {
    const KIND: &'static str = "job.failed";
}

/// A running job: what it says goes on the bus, if it has one. Ending it
/// takes it, so a job ends once.
pub struct Job {
    id: u64,
    kind: String,
    total: Option<u64>,
    bus: Option<Arc<Bus>>,
}

impl Job {
    /// Starts a job and announces it.
    pub fn start(bus: Option<Arc<Bus>>, kind: &str, title: &str, total: Option<u64>) -> Job {
        let job = Job {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            kind: kind.into(),
            total,
            bus,
        };
        job.publish(&Started {
            job: job.id,
            kind: job.kind.clone(),
            title: title.into(),
            total,
        });
        job
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    fn publish<E: Event>(&self, event: &E) {
        if let Some(bus) = &self.bus {
            bus.publish(&self.kind, None, event);
        }
    }

    /// `done` items of the total are done; `current` is the one at hand.
    pub fn progress(&self, done: u64, current: Option<&str>) {
        self.publish(&Progress {
            job: self.id,
            kind: self.kind.clone(),
            done,
            total: self.total,
            current: current.map(String::from),
        });
    }

    pub fn finish(self, summary: serde_json::Value) {
        self.publish(&Finished {
            job: self.id,
            kind: self.kind.clone(),
            summary,
        });
    }

    pub fn fail(self, error: &str) {
        self.publish(&Failed {
            job: self.id,
            kind: self.kind.clone(),
            error: error.into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_announces_its_start_progress_and_end() {
        let bus = Arc::new(Bus::new());
        let job = Job::start(Some(bus.clone()), "test.job", "Testing", Some(2));
        let id = job.id();
        job.progress(1, Some("a"));
        job.finish(serde_json::json!({ "ok": true }));
        let events = bus.since(0);
        let kinds: Vec<&str> = events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["job.started", "job.progress", "job.finished"]);
        assert!(events.iter().all(|e| e.payload["job"] == id));
        assert_eq!(events[1].payload["done"], 1);
        assert_eq!(events[1].payload["total"], 2);
        assert_eq!(events[1].payload["current"], "a");
        let other = Job::start(None, "test.job", "Testing", None);
        assert_ne!(other.id(), id);
        other.fail("no bus: nothing said");
    }
}
