//! The scheduler: every unit of background work goes through it.
//!
//! A job is named, gets an id, can be cancelled, and is announced on the
//! bus when it is queued, starts and ends. The scheduler knows at any time
//! whether anything is queued or running, and anyone can wait until it is
//! idle instead of sleeping (ROADMAP section 2.8, rule 3).
//!
//! This first version runs jobs on a fixed pool of threads, in submission
//! order. Declared needs (GPU memory, AI calls, destinations), priorities and
//! remote nodes come later.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::bus::{Bus, Event};
use crate::describe::Describe;

const SOURCE: &str = "scheduler";

/// How many finished jobs `describe()` keeps.
const RECENT: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct JobId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

/// Lets a running job notice it was cancelled. Jobs check it between steps;
/// nothing is interrupted behind their back.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// What a job runs. It returns an error message when it fails.
pub type Work = Box<dyn FnOnce(&CancelToken) -> Result<(), String> + Send>;

#[derive(Serialize)]
pub struct JobQueued {
    pub job: JobId,
    pub name: String,
}

impl Event for JobQueued {
    const KIND: &'static str = "scheduler.job_queued";
}

#[derive(Serialize)]
pub struct JobStarted {
    pub job: JobId,
    pub name: String,
}

impl Event for JobStarted {
    const KIND: &'static str = "scheduler.job_started";
}

#[derive(Serialize)]
pub struct JobEnded {
    pub job: JobId,
    pub name: String,
    pub state: JobState,
    pub error: Option<String>,
}

impl Event for JobEnded {
    const KIND: &'static str = "scheduler.job_ended";
}

/// Nothing is queued or running any more.
#[derive(Serialize)]
pub struct Idle {
    pub done: u64,
    pub failed: u64,
    pub cancelled: u64,
}

impl Event for Idle {
    const KIND: &'static str = "scheduler.idle";
}

#[derive(Clone, Debug, Serialize)]
struct JobInfo {
    id: JobId,
    name: String,
    state: JobState,
    error: Option<String>,
}

struct Pending {
    info: JobInfo,
    work: Work,
    token: CancelToken,
    /// The bus event that caused this job: what a waiter follows.
    cause: u64,
}

#[derive(Default)]
struct State {
    next: u64,
    queue: VecDeque<Pending>,
    running: BTreeMap<JobId, (JobInfo, CancelToken)>,
    recent: VecDeque<JobInfo>,
    done: u64,
    failed: u64,
    cancelled: u64,
    shutdown: bool,
}

impl State {
    fn idle(&self) -> bool {
        self.queue.is_empty() && self.running.is_empty()
    }

    fn finish(&mut self, info: JobInfo) {
        match info.state {
            JobState::Done => self.done += 1,
            JobState::Failed => self.failed += 1,
            JobState::Cancelled => self.cancelled += 1,
            JobState::Queued | JobState::Running => unreachable!("only ended jobs finish"),
        }
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(info);
    }
}

struct Shared {
    bus: Arc<Bus>,
    state: Mutex<State>,
    /// Signalled when a job is queued (workers) and when the scheduler
    /// becomes idle (waiters).
    changed: Condvar,
}

pub struct Scheduler {
    shared: Arc<Shared>,
    workers: Vec<thread::JoinHandle<()>>,
}

impl Scheduler {
    /// A scheduler with `workers` threads (0: one per available CPU).
    pub fn new(bus: Arc<Bus>, workers: usize) -> Self {
        let workers = if workers == 0 {
            thread::available_parallelism().map_or(1, |n| n.get())
        } else {
            workers
        };
        let shared = Arc::new(Shared {
            bus,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        let handles = (0..workers)
            .map(|i| {
                let shared = shared.clone();
                thread::Builder::new()
                    .name(format!("helios-worker-{i}"))
                    .spawn(move || worker(&shared))
                    .expect("spawn a worker")
            })
            .collect();
        Self {
            shared,
            workers: handles,
        }
    }

    /// Queues a named job; returns its id.
    pub fn submit(
        &self,
        name: &str,
        work: impl FnOnce(&CancelToken) -> Result<(), String> + Send + 'static,
    ) -> JobId {
        let mut state = self.shared.state.lock().expect("scheduler lock");
        state.next += 1;
        let id = JobId(state.next);
        let cause = self.shared.bus.publish(
            SOURCE,
            None,
            &JobQueued {
                job: id,
                name: name.into(),
            },
        );
        let info = JobInfo {
            id,
            name: name.into(),
            state: JobState::Queued,
            error: None,
        };
        state.queue.push_back(Pending {
            info,
            work: Box::new(work),
            token: CancelToken::default(),
            cause,
        });
        self.shared.changed.notify_all();
        id
    }

    /// Cancels a job: a queued job never starts, a running job sees its
    /// token cancelled. Returns false if the job is unknown or already ended.
    pub fn cancel(&self, id: JobId) -> bool {
        let mut state = self.shared.state.lock().expect("scheduler lock");
        if let Some(i) = state.queue.iter().position(|p| p.info.id == id) {
            let pending = state.queue.remove(i).expect("position is valid");
            let info = JobInfo {
                state: JobState::Cancelled,
                ..pending.info
            };
            self.shared.bus.publish(
                SOURCE,
                Some(pending.cause),
                &JobEnded {
                    job: id,
                    name: info.name.clone(),
                    state: JobState::Cancelled,
                    error: None,
                },
            );
            state.finish(info);
            announce_idle(&self.shared, &state);
            return true;
        }
        if let Some((_, token)) = state.running.get(&id) {
            token.cancel();
            return true;
        }
        false
    }

    pub fn is_idle(&self) -> bool {
        self.shared.state.lock().expect("scheduler lock").idle()
    }

    /// Waits until nothing is queued or running; false if `timeout` passed
    /// first. A timeout too long to represent waits without limit.
    pub fn wait_until_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now().checked_add(timeout);
        let mut state = self.shared.state.lock().expect("scheduler lock");
        while !state.idle() {
            state = match deadline {
                None => self.shared.changed.wait(state).expect("scheduler lock"),
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return false;
                    }
                    self.shared
                        .changed
                        .wait_timeout(state, left)
                        .expect("scheduler lock")
                        .0
                }
            };
        }
        true
    }

    /// The state of a job still known to the scheduler (queued, running or recent).
    pub fn job(&self, id: JobId) -> Option<JobState> {
        let state = self.shared.state.lock().expect("scheduler lock");
        if state.queue.iter().any(|p| p.info.id == id) {
            return Some(JobState::Queued);
        }
        if state.running.contains_key(&id) {
            return Some(JobState::Running);
        }
        state.recent.iter().find(|j| j.id == id).map(|j| j.state)
    }
}

impl Drop for Scheduler {
    /// Stops the workers once the jobs already queued have run.
    fn drop(&mut self) {
        self.shared.state.lock().expect("scheduler lock").shutdown = true;
        self.shared.changed.notify_all();
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

fn announce_idle(shared: &Shared, state: &State) {
    if state.idle() {
        shared.bus.publish(
            SOURCE,
            None,
            &Idle {
                done: state.done,
                failed: state.failed,
                cancelled: state.cancelled,
            },
        );
        shared.changed.notify_all();
    }
}

fn worker(shared: &Shared) {
    loop {
        let pending = {
            let mut state = shared.state.lock().expect("scheduler lock");
            loop {
                if let Some(p) = state.queue.pop_front() {
                    break p;
                }
                if state.shutdown {
                    return;
                }
                state = shared.changed.wait(state).expect("scheduler lock");
            }
        };
        let Pending {
            mut info,
            work,
            token,
            cause,
        } = pending;
        info.state = JobState::Running;
        {
            let mut state = shared.state.lock().expect("scheduler lock");
            state.running.insert(info.id, (info.clone(), token.clone()));
            shared.bus.publish(
                SOURCE,
                Some(cause),
                &JobStarted {
                    job: info.id,
                    name: info.name.clone(),
                },
            );
        }
        let result = work(&token);
        let mut state = shared.state.lock().expect("scheduler lock");
        state.running.remove(&info.id);
        (info.state, info.error) = match result {
            _ if token.is_cancelled() => (JobState::Cancelled, None),
            Ok(()) => (JobState::Done, None),
            Err(e) => (JobState::Failed, Some(e)),
        };
        shared.bus.publish(
            SOURCE,
            Some(cause),
            &JobEnded {
                job: info.id,
                name: info.name.clone(),
                state: info.state,
                error: info.error.clone(),
            },
        );
        state.finish(info);
        announce_idle(shared, &state);
    }
}

impl Describe for Scheduler {
    fn describe(&self) -> serde_json::Value {
        let state = self.shared.state.lock().expect("scheduler lock");
        serde_json::json!({
            "idle": state.idle(),
            "workers": self.workers.len(),
            "queued": state.queue.iter().map(|p| &p.info).collect::<Vec<_>>(),
            "running": state.running.values().map(|(info, _)| info).collect::<Vec<_>>(),
            "recent": state.recent,
            "totals": { "done": state.done, "failed": state.failed, "cancelled": state.cancelled },
        })
    }
}
