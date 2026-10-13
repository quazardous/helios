//! The jobs a process runs now, folded from its bus: what a client that
//! lost the event stream reads to catch up (see `docs/jobs.md`).
//!
//! [`JobsView::follow`] folds every `job.*` event of a bus as it is
//! published: the running jobs with what they started with and their
//! progress so far, and the last jobs that ended with how. Its
//! [`Describe`] snapshot says up to which sequence number it is folded,
//! so a client reads it, then subscribes after that number and misses
//! nothing.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helios_core::jobs::{CANCELLED, CANCELLING, FAILED, FINISHED, PROGRESS, STARTED};
use helios_core::{Bus, Describe, Envelope};
use serde_json::{Map, Value, json};

/// How many ended jobs the view remembers.
pub const ENDED: usize = 256;

/// The running and recently ended jobs of a bus.
#[derive(Clone)]
pub struct JobsView {
    fold: Arc<(Mutex<Fold>, Condvar)>,
    instance: String,
}

#[derive(Default)]
struct Fold {
    /// The last event folded.
    seq: u64,
    /// In the order they started: a parent before its children.
    running: Vec<Running>,
    ended: VecDeque<Value>,
}

struct Running {
    id: String,
    started: Value,
    progress: Map<String, Value>,
    cancelling: bool,
}

impl Fold {
    fn apply(&mut self, e: &Envelope) {
        self.seq = self.seq.max(e.seq);
        if !e.kind.starts_with("job.") {
            return;
        }
        let Some(id) = e.payload["id"].as_str() else {
            return;
        };
        let at = self.running.iter().position(|r| r.id == id);
        match (e.kind, at) {
            (STARTED, at) => {
                let running = Running {
                    id: id.to_string(),
                    started: e.payload.clone(),
                    progress: Map::new(),
                    cancelling: false,
                };
                match at {
                    Some(i) => self.running[i] = running,
                    None => self.running.push(running),
                }
            }
            (PROGRESS, Some(i)) => {
                if let Some(fields) = e.payload.as_object() {
                    for (k, v) in fields {
                        self.running[i].progress.insert(k.clone(), v.clone());
                    }
                }
            }
            (CANCELLING, Some(i)) => self.running[i].cancelling = true,
            (FINISHED | FAILED | CANCELLED, at) => {
                if let Some(i) = at {
                    self.running.remove(i);
                }
                if self.ended.len() == ENDED {
                    self.ended.pop_front();
                }
                self.ended
                    .push_back(json!({ "kind": e.kind, "data": e.payload }));
            }
            // Log lines and failed items are not kept: the stream has them.
            _ => {}
        }
    }
}

impl JobsView {
    /// Folds the jobs of `bus`, from what its journal holds now and then
    /// as events are published, on a thread of its own that ends with
    /// the bus. `instance` names the process (see
    /// [`crate::Inspector::instance`]).
    pub fn follow(bus: &Arc<Bus>, instance: &str) -> Self {
        let fold: Arc<(Mutex<Fold>, Condvar)> = Arc::default();
        // Subscribed before reading the journal: an event published in
        // between is folded once, from one or the other.
        let rx = bus.subscribe();
        {
            let mut f = fold.0.lock().unwrap_or_else(PoisonError::into_inner);
            for e in bus.since(0) {
                f.apply(&e);
            }
        }
        let folding = fold.clone();
        std::thread::Builder::new()
            .name("helios-inspect-jobs".into())
            .spawn(move || {
                for e in rx {
                    let mut f = folding.0.lock().unwrap_or_else(PoisonError::into_inner);
                    if e.seq > f.seq {
                        f.apply(&e);
                    }
                    folding.1.notify_all();
                }
            })
            .expect("a thread for the jobs view");
        Self {
            fold,
            instance: instance.to_string(),
        }
    }

    /// Waits until the view has folded the event `seq`: whether it did
    /// within `timeout`.
    pub fn caught_up(&self, seq: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut f = self.fold.0.lock().unwrap_or_else(PoisonError::into_inner);
        while f.seq < seq {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            f = self
                .fold
                .1
                .wait_timeout(f, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

impl Describe for JobsView {
    /// `{"instance", "seq", "running": [{"started", "progress",
    /// "cancelling"}], "ended": [{"kind", "data"}]}`: `started` is the job's
    /// `job.started` data, `progress` its `job.progress` fields merged,
    /// `ended` its end event, the oldest first.
    fn describe(&self) -> Value {
        let f = self.fold.0.lock().unwrap_or_else(PoisonError::into_inner);
        let running: Vec<Value> = f
            .running
            .iter()
            .map(|r| {
                json!({
                    "started": r.started,
                    "progress": r.progress,
                    "cancelling": r.cancelling,
                })
            })
            .collect();
        json!({
            "instance": self.instance,
            "seq": f.seq,
            "running": running,
            "ended": f.ended,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::jobs::{Jobs, Progress, Start};

    #[test]
    fn running_jobs_keep_their_start_and_progress_and_ended_ones_how() {
        let bus = Arc::new(Bus::new());
        let jobs = Jobs::new(bus.clone());
        let batch = jobs.start(Start::new("batch-1", "Batch of 2", "batch").total(2));
        let view = JobsView::follow(&bus, "here");
        let first = jobs.start(Start::new("export-1", "Export a", "export").parent("batch-1", 1.0));
        first.progress(Progress::new().counts(1, 3).item("a"));
        first.progress(Progress::new().step("encode"));
        first.finished();
        let second =
            jobs.start(Start::new("export-2", "Export b", "export").parent("batch-1", 1.0));
        let last = bus.last_seq();
        // The thread folds behind the bus: wait for it to catch up.
        assert!(view.caught_up(last, Duration::from_secs(10)));
        let snapshot = view.describe();
        assert_eq!(snapshot["seq"], last);
        assert_eq!(snapshot["instance"], "here");
        let ids: Vec<&str> = snapshot["running"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["started"]["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["batch-1", "export-2"], "a parent before its children");
        assert_eq!(snapshot["running"][1]["started"]["parent"], "batch-1");
        assert_eq!(snapshot["ended"][0]["kind"], "job.finished");
        assert_eq!(snapshot["ended"][0]["data"]["id"], "export-1");
        drop((second, batch));
    }
}
