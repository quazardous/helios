//! The scheduler: every unit of background work goes through it.
//!
//! A job is named, gets an id, can be cancelled, and is announced on the
//! bus when it is queued, starts and ends. The scheduler knows at any time
//! whether anything is queued or running, and anyone can wait until it is
//! idle instead of sleeping (ROADMAP section 2.8, rule 3).
//!
//! A job declares up front what it needs: a priority, and amounts of named
//! resources (GPU memory, concurrent raw decodes, AI calls, an export
//! destination) whose capacity the scheduler limits. A job starts only when
//! all its needs are available, acquired together under one lock, so two
//! jobs can never hold part of what the other waits for (no deadlock), and
//! everything is released when the job ends. Among the jobs that can start,
//! the highest priority goes first, then the oldest.
//!
//! Jobs run on a fixed pool of threads. Remote nodes, leases that expire
//! when a node dies, and the measured-speed auto mode come later.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::bus::{Bus, Event};
use crate::describe::Describe;
use crate::message::Message;

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

/// Which work goes first when several can start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// Work nobody is waiting for: exports, batches.
    Batch,
    /// Work someone will probably need soon: the next images of a cull.
    Prefetch,
    /// Work someone is waiting for right now: the image on screen.
    #[default]
    Interactive,
}

/// What a job declares before it runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct JobSpec {
    pub name: String,
    pub priority: Priority,
    /// Amounts of named resources, held for the whole job.
    pub needs: BTreeMap<String, u64>,
}

impl JobSpec {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    pub fn needs(mut self, resource: &str, amount: u64) -> Self {
        *self.needs.entry(resource.into()).or_default() += amount;
        self
    }
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

/// What a job runs. It returns a message for people when it fails.
pub type Work = Box<dyn FnOnce(&CancelToken) -> Result<(), Message> + Send>;

#[derive(Serialize)]
pub struct JobQueued {
    pub job: JobId,
    pub name: String,
    pub priority: Priority,
    pub needs: BTreeMap<String, u64>,
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
    /// Why it failed, in English.
    pub error: Option<String>,
    /// Why it failed, by key, when the job said it with one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
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

/// The scheduler was paused or resumed.
#[derive(Serialize)]
pub struct Paused {
    pub paused: bool,
}

impl Event for Paused {
    const KIND: &'static str = "scheduler.paused";
}

impl Event for Idle {
    const KIND: &'static str = "scheduler.idle";
}

#[derive(Clone, Debug, Serialize)]
struct JobInfo {
    id: JobId,
    #[serde(flatten)]
    spec: JobSpec,
    state: JobState,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<Message>,
}

struct Pending {
    info: JobInfo,
    work: Work,
    token: CancelToken,
    /// The bus event that caused this job: what a waiter follows.
    cause: u64,
}

/// A limited resource: how much exists, how much running jobs hold.
#[derive(Clone, Copy, Debug, Default, Serialize)]
struct Resource {
    capacity: u64,
    used: u64,
}

#[derive(Default)]
struct State {
    next: u64,
    queue: VecDeque<Pending>,
    running: BTreeMap<JobId, (JobInfo, CancelToken)>,
    resources: BTreeMap<String, Resource>,
    recent: VecDeque<JobInfo>,
    done: u64,
    failed: u64,
    cancelled: u64,
    shutdown: bool,
    /// No job starts while paused; running jobs finish.
    paused: bool,
}

impl State {
    fn idle(&self) -> bool {
        self.queue.is_empty() && self.running.is_empty()
    }

    fn fits(&self, spec: &JobSpec) -> bool {
        spec.needs.iter().all(|(name, amount)| {
            self.resources
                .get(name)
                .is_some_and(|r| r.used + amount <= r.capacity)
        })
    }

    /// The queued job to start now: the highest priority among those whose
    /// needs are all available, the oldest first.
    fn next_runnable(&self) -> Option<usize> {
        if self.paused {
            return None;
        }
        let mut best: Option<usize> = None;
        for (i, p) in self.queue.iter().enumerate() {
            let better =
                best.is_none_or(|b| p.info.spec.priority > self.queue[b].info.spec.priority);
            if better && self.fits(&p.info.spec) {
                best = Some(i);
            }
        }
        best
    }

    fn acquire(&mut self, spec: &JobSpec) {
        for (name, amount) in &spec.needs {
            self.resources.get_mut(name).expect("checked by fits").used += amount;
        }
    }

    fn release(&mut self, spec: &JobSpec) {
        for (name, amount) in &spec.needs {
            if let Some(r) = self.resources.get_mut(name) {
                r.used -= amount;
            }
        }
    }

    /// Why a job can never start, if so.
    fn impossible(&self, spec: &JobSpec) -> Option<String> {
        spec.needs
            .iter()
            .find_map(|(name, amount)| match self.resources.get(name) {
                None => Some(format!(
                    "needs `{name}`, which this scheduler does not have"
                )),
                Some(r) if *amount > r.capacity => Some(format!(
                    "needs {amount} of `{name}`, whose capacity is {}",
                    r.capacity
                )),
                Some(_) => None,
            })
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
    /// Signalled when a job is queued, when resources are released (workers)
    /// and when the scheduler becomes idle (waiters).
    changed: Condvar,
}

impl Shared {
    fn ended(&self, state: &mut State, info: JobInfo, cause: u64) {
        self.bus.publish(
            SOURCE,
            Some(cause),
            &JobEnded {
                job: info.id,
                name: info.spec.name.clone(),
                state: info.state,
                error: info.error.clone(),
                message: info.message.clone(),
            },
        );
        state.finish(info);
        if state.idle() {
            self.bus.publish(
                SOURCE,
                None,
                &Idle {
                    done: state.done,
                    failed: state.failed,
                    cancelled: state.cancelled,
                },
            );
        }
        self.changed.notify_all();
    }
}

pub struct Scheduler {
    shared: Arc<Shared>,
    workers: Vec<thread::JoinHandle<()>>,
}

impl Scheduler {
    /// A scheduler with `workers` threads (0: one per available CPU) and no
    /// limited resource.
    pub fn new(bus: Arc<Bus>, workers: usize) -> Self {
        Self::with_resources(bus, workers, &[])
    }

    /// A scheduler with `workers` threads and named resources of fixed capacity.
    pub fn with_resources(bus: Arc<Bus>, workers: usize, resources: &[(&str, u64)]) -> Self {
        let workers = if workers == 0 {
            thread::available_parallelism().map_or(1, std::num::NonZero::get)
        } else {
            workers
        };
        let state = State {
            resources: resources
                .iter()
                .map(|(name, capacity)| {
                    (
                        name.to_string(),
                        Resource {
                            capacity: *capacity,
                            used: 0,
                        },
                    )
                })
                .collect(),
            ..State::default()
        };
        let shared = Arc::new(Shared {
            bus,
            state: Mutex::new(state),
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

    /// Declares a resource if it does not exist yet (an existing one keeps
    /// its capacity). For resources known only at run time, such as an
    /// export destination with a capacity of one writer.
    pub fn ensure_resource(&self, name: &str, capacity: u64) {
        let mut state = self.shared.state.lock().expect("scheduler lock");
        state
            .resources
            .entry(name.to_string())
            .or_insert(Resource { capacity, used: 0 });
    }

    /// Queues an interactive job with no declared need; returns its id.
    pub fn submit(
        &self,
        name: &str,
        work: impl FnOnce(&CancelToken) -> Result<(), String> + Send + 'static,
    ) -> JobId {
        self.submit_spec(JobSpec::new(name), work)
    }

    /// Queues a job with its declared priority and needs. A job whose needs
    /// can never be met ends at once as failed, saying why.
    pub fn submit_spec(
        &self,
        spec: JobSpec,
        work: impl FnOnce(&CancelToken) -> Result<(), String> + Send + 'static,
    ) -> JobId {
        self.submit_message(spec, move |cancel| work(cancel).map_err(Message::plain))
    }

    /// [`Scheduler::submit_spec`] for work that says why it failed by key
    /// ([`Message`]): the job's end carries the message.
    pub fn submit_message(
        &self,
        spec: JobSpec,
        work: impl FnOnce(&CancelToken) -> Result<(), Message> + Send + 'static,
    ) -> JobId {
        let mut state = self.shared.state.lock().expect("scheduler lock");
        state.next += 1;
        let id = JobId(state.next);
        let cause = self.shared.bus.publish(
            SOURCE,
            None,
            &JobQueued {
                job: id,
                name: spec.name.clone(),
                priority: spec.priority,
                needs: spec.needs.clone(),
            },
        );
        if let Some(reason) = state.impossible(&spec) {
            let info = JobInfo {
                id,
                spec,
                state: JobState::Failed,
                error: Some(reason),
                message: None,
            };
            self.shared.ended(&mut state, info, cause);
            return id;
        }
        let info = JobInfo {
            id,
            spec,
            state: JobState::Queued,
            error: None,
            message: None,
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
            self.shared.ended(&mut state, info, pending.cause);
            return true;
        }
        if let Some((_, token)) = state.running.get(&id) {
            token.cancel();
            return true;
        }
        false
    }

    /// Stops starting jobs (running ones finish); queued jobs wait for `resume`.
    pub fn pause(&self) {
        let mut state = self.shared.state.lock().expect("scheduler lock");
        if !state.paused {
            state.paused = true;
            self.shared
                .bus
                .publish(SOURCE, None, &Paused { paused: true });
        }
    }

    pub fn resume(&self) {
        let mut state = self.shared.state.lock().expect("scheduler lock");
        if state.paused {
            state.paused = false;
            self.shared
                .bus
                .publish(SOURCE, None, &Paused { paused: false });
            self.shared.changed.notify_all();
        }
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

fn worker(shared: &Shared) {
    loop {
        // Pick a job, acquire its needs and mark it running under one lock:
        // the scheduler is never seen idle while a job is about to run.
        let (info, work, token, cause) = {
            let mut state = shared.state.lock().expect("scheduler lock");
            loop {
                if let Some(i) = state.next_runnable() {
                    let Pending {
                        mut info,
                        work,
                        token,
                        cause,
                    } = state.queue.remove(i).expect("index from the queue");
                    state.acquire(&info.spec);
                    info.state = JobState::Running;
                    state.running.insert(info.id, (info.clone(), token.clone()));
                    shared.bus.publish(
                        SOURCE,
                        Some(cause),
                        &JobStarted {
                            job: info.id,
                            name: info.spec.name.clone(),
                        },
                    );
                    break (info, work, token, cause);
                }
                if state.shutdown && state.queue.is_empty() {
                    return;
                }
                state = shared.changed.wait(state).expect("scheduler lock");
            }
        };
        let result = work(&token);
        let mut state = shared.state.lock().expect("scheduler lock");
        state.running.remove(&info.id);
        state.release(&info.spec);
        let mut info = info;
        (info.state, info.error, info.message) = match result {
            _ if token.is_cancelled() => (JobState::Cancelled, None, None),
            Ok(()) => (JobState::Done, None, None),
            Err(e) => (
                JobState::Failed,
                Some(e.english.clone()),
                e.is_keyed().then_some(e),
            ),
        };
        shared.ended(&mut state, info, cause);
    }
}

impl Describe for Scheduler {
    fn describe(&self) -> serde_json::Value {
        let state = self.shared.state.lock().expect("scheduler lock");
        let waiting: Vec<_> = state
            .queue
            .iter()
            .map(|p| serde_json::json!({ "job": &p.info, "can_start": state.fits(&p.info.spec) }))
            .collect();
        serde_json::json!({
            "idle": state.idle(),
            "paused": state.paused,
            "workers": self.workers.len(),
            "queued": waiting,
            "running": state.running.values().map(|(info, _)| info).collect::<Vec<_>>(),
            "resources": state.resources,
            "recent": state.recent,
            "totals": { "done": state.done, "failed": state.failed, "cancelled": state.cancelled },
        })
    }
}
