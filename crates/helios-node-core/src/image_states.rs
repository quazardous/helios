//! The image-state router: one owner for each photo's computed states.
//!
//! A consumer asks for "photo X, in development D, in state S at size Z"
//! ([`StateKey`]) and gets a [`Handle`] it can wait on, poll, or drop. The
//! router:
//!
//! - runs one computation for every ask of a key in flight (single flight,
//!   [`helios_cache::Flight`]), and counts the asks it merged;
//! - shares the cancel: a computation runs under a token that is cancelled
//!   only once every asker cancelled (dropping a handle cancels its ask),
//!   and a cancelled computation is never kept, so the next ask computes
//!   again;
//! - runs the computations on a bounded pool, highest [`Priority`] first
//!   (Interactive / Prefetch / Batch: shown / near / background); an ask
//!   of higher priority joining a waiting computation raises it;
//! - says each transition on the bus as an `image.state.*` fact
//!   ([`facts`]), never pixels: those travel by handle.
//!
//! The engine stays a provider with its own step cache: the computations
//! the router runs call it, and the router keeps only the small states it
//! produced (the preview), not copies of the engine's steps. Viewport
//! tiles stay direct engine asks.

use std::any::Any;
use std::collections::{BinaryHeap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use helios_cache::{Begin, Flight, FlightError, Lead, Waiter};
use helios_core::{Bus, ContentHash, Priority, cancel};
use serde::Serialize;

/// The source the router's facts carry on the bus.
pub const SOURCE: &str = "image_states";

/// The facts the router publishes, one per transition of a state.
pub mod facts {
    /// A state nobody computes yet was asked for.
    pub const REQUESTED: &str = "image.state.requested";
    /// Its computation started.
    pub const COMPUTING: &str = "image.state.computing";
    /// It is ready.
    pub const READY: &str = "image.state.ready";
    /// Its computation failed (the error in `reason`).
    pub const FAILED: &str = "image.state.failed";
    /// Every asker cancelled before it was ready: nothing is kept.
    pub const CANCELLED: &str = "image.state.cancelled";
    /// Every fact, in lifecycle order.
    pub const ALL: [&str; 5] = [REQUESTED, COMPUTING, READY, FAILED, CANCELLED];
}

/// What a photo can be computed into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateKind {
    /// The whole image at a small size, its longest side `size` pixels:
    /// what the scopes measure and the navigator draws.
    Preview,
}

impl StateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StateKind::Preview => "preview",
        }
    }
}

/// One state of one photo.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StateKey {
    /// The photo, as the facts name it.
    pub photo: String,
    /// The development (variant, edits): the content its pixels come from,
    /// e.g. the engine's key of the node shown at the state's size.
    pub development: ContentHash,
    pub state: StateKind,
    /// The size asked, in the state's own unit (the preview's longest side).
    pub size: u32,
}

/// A computed state, shared by handle.
pub type Value = Arc<dyn Any + Send + Sync>;

/// Why a state has no value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateError {
    /// Every asker cancelled (or this handle did).
    Cancelled,
    /// The computation failed.
    Failed(String),
    /// The wait ran past its deadline (the computation goes on).
    Timeout,
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::Cancelled => f.write_str(cancel::CANCELLED),
            StateError::Failed(why) => f.write_str(why),
            StateError::Timeout => f.write_str("timed out"),
        }
    }
}

impl std::error::Error for StateError {}

impl From<helios_core::Error> for StateError {
    fn from(e: helios_core::Error) -> Self {
        if e.is_cancelled() {
            StateError::Cancelled
        } else {
            StateError::Failed(e.to_string())
        }
    }
}

type Outcome = Result<Value, StateError>;

/// How a state is computed: called on a pool thread, under the shared
/// cancel token ([`cancel::current`] inside it).
pub type Compute = Arc<dyn Fn() -> Outcome + Send + Sync>;

/// What the router did, since it was made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counters {
    /// Asks received.
    pub asks: u64,
    /// Asks merged into a computation in flight.
    pub merged: u64,
    /// Asks answered by a ready state.
    pub ready_hits: u64,
    /// Computations run.
    pub computed: u64,
    /// Computations cancelled (before or while running).
    pub cancelled: u64,
}

/// One state's entry on the bus.
#[derive(Serialize)]
struct Fact<'a> {
    photo: &'a str,
    development: String,
    state: StateKind,
    size: u32,
    priority: Priority,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// A computation asked for, waiting or running.
struct Job {
    key: StateKey,
    /// Cancelled once every asker is.
    askers: cancel::Token,
    priority: Mutex<Priority>,
    started: AtomicBool,
    compute: Compute,
    /// Ends the flight; taken by the worker that finishes it.
    lead: Mutex<Option<Lead<StateKey, Outcome>>>,
}

impl Job {
    fn priority(&self) -> Priority {
        *self.priority.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A job in the pool's queue, highest priority first, then oldest.
struct Queued {
    priority: Priority,
    seq: u64,
    job: Arc<Job>,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        (self.priority, self.seq) == (other.priority, other.seq)
    }
}
impl Eq for Queued {}
impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Queued {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority
            .cmp(&other.priority)
            .then(other.seq.cmp(&self.seq))
    }
}

#[derive(Default)]
struct Table {
    jobs: HashMap<StateKey, Arc<Job>>,
    /// Ready states, with when each was last used.
    ready: HashMap<StateKey, (Value, u64)>,
    queue: BinaryHeap<Queued>,
    seq: u64,
    closed: bool,
}

struct Inner {
    bus: Arc<Bus>,
    flight: Arc<Flight<StateKey, Outcome>>,
    table: Mutex<Table>,
    work: Condvar,
    /// How many ready states are kept (the least recently used go first).
    keep: usize,
    asks: AtomicU64,
    ready_hits: AtomicU64,
    computed: AtomicU64,
    cancelled: AtomicU64,
}

/// The router. Cheap to clone: clones share it. Its pool's threads end
/// once the last clone is dropped.
#[derive(Clone)]
pub struct ImageStates {
    inner: Arc<Inner>,
    /// What its pool's thread names start with.
    prefix: Arc<str>,
    _pool: Arc<Pool>,
}

/// Closes the queue when the last router clone goes.
struct Pool(Arc<Inner>);

impl Drop for Pool {
    fn drop(&mut self) {
        self.0.table().closed = true;
        self.0.work.notify_all();
    }
}

impl fmt::Debug for ImageStates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageStates")
            .field("counters", &self.counters())
            .finish()
    }
}

/// Routers made so far, to name their threads apart.
static ROUTERS: AtomicU64 = AtomicU64::new(0);

/// How many computations the desktop's router runs at once: the
/// interactive state and one more (the heavy steps inside, the demosaic,
/// spread over every core already).
pub const THREADS: usize = 2;

/// How many ready states a router keeps by default.
pub const KEEP: usize = 16;

impl ImageStates {
    /// A router publishing on `bus`, running at most `threads` computations
    /// at once.
    pub fn new(bus: Arc<Bus>, threads: usize) -> Self {
        let inner = Arc::new(Inner {
            bus,
            flight: Arc::default(),
            table: Mutex::default(),
            work: Condvar::new(),
            keep: KEEP,
            asks: AtomicU64::new(0),
            ready_hits: AtomicU64::new(0),
            computed: AtomicU64::new(0),
            cancelled: AtomicU64::new(0),
        });
        let prefix: Arc<str> =
            format!("helios-states-{}.", ROUTERS.fetch_add(1, Ordering::Relaxed)).into();
        for i in 0..threads.max(1) {
            let inner = inner.clone();
            std::thread::Builder::new()
                .name(format!("{prefix}{i}"))
                .spawn(move || inner.work())
                .expect("a thread for the image-state router");
        }
        ImageStates {
            _pool: Arc::new(Pool(inner.clone())),
            inner,
            prefix,
        }
    }

    /// What the names of its pool's threads start with (`helios-states-`,
    /// then a number per router): its trace spans carry them.
    pub fn threads_prefix(&self) -> &str {
        &self.prefix
    }

    /// Asks for `key` at `priority`, computed by `compute` unless it is
    /// ready or in flight. The ask lasts as long as the handle.
    pub fn ask(&self, key: StateKey, priority: Priority, compute: Compute) -> Handle {
        let token = cancel::Token::new();
        let found = self.inner.ask(&key, priority, &compute, &token);
        Handle {
            router: self.inner.clone(),
            key,
            priority,
            compute,
            token,
            found,
        }
    }

    /// What the router did so far.
    pub fn counters(&self) -> Counters {
        Counters {
            asks: self.inner.asks.load(Ordering::SeqCst),
            merged: self.inner.flight.merged(),
            ready_hits: self.inner.ready_hits.load(Ordering::SeqCst),
            computed: self.inner.computed.load(Ordering::SeqCst),
            cancelled: self.inner.cancelled.load(Ordering::SeqCst),
        }
    }

    /// The states ready now, and those asked and not ready yet.
    pub fn list(&self) -> (Vec<StateKey>, Vec<StateKey>) {
        let table = self.inner.table();
        (
            table.ready.keys().cloned().collect(),
            table.jobs.keys().cloned().collect(),
        )
    }
}

/// What an ask found.
enum Found {
    Ready(Value),
    Flying(Waiter<Outcome>),
}

impl Inner {
    fn table(&self) -> std::sync::MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn publish(
        &self,
        kind: &'static str,
        key: &StateKey,
        priority: Priority,
        reason: Option<String>,
    ) {
        let fact = Fact {
            photo: &key.photo,
            development: key.development.short(),
            state: key.state,
            size: key.size,
            priority,
            reason,
        };
        let payload = serde_json::to_value(&fact).expect("a fact serializes");
        self.bus.publish_value(SOURCE, None, kind, payload);
    }

    fn ask(
        &self,
        key: &StateKey,
        priority: Priority,
        compute: &Compute,
        token: &cancel::Token,
    ) -> Found {
        self.asks.fetch_add(1, Ordering::SeqCst);
        let mut table = self.table();
        table.seq += 1;
        let seq = table.seq;
        if let Some((value, used)) = table.ready.get_mut(key) {
            *used = seq;
            self.ready_hits.fetch_add(1, Ordering::SeqCst);
            return Found::Ready(value.clone());
        }
        // Every flight begins under the table's lock, so a key flying is a
        // key in `jobs`.
        match self.flight.begin(key) {
            Begin::Join(waiter) => {
                if let Some(job) = table.jobs.get(key).cloned() {
                    job.askers.join(token.clone());
                    let mut current = job.priority.lock().unwrap_or_else(PoisonError::into_inner);
                    if priority > *current && !job.started.load(Ordering::SeqCst) {
                        *current = priority;
                        drop(current);
                        table.queue.push(Queued { priority, seq, job });
                        self.work.notify_one();
                    }
                }
                Found::Flying(waiter)
            }
            Begin::Lead(lead) => {
                let waiter = lead.waiter();
                let askers = cancel::Token::shared();
                askers.join(token.clone());
                let job = Arc::new(Job {
                    key: key.clone(),
                    askers,
                    priority: Mutex::new(priority),
                    started: AtomicBool::new(false),
                    compute: compute.clone(),
                    lead: Mutex::new(Some(lead)),
                });
                table.jobs.insert(key.clone(), job.clone());
                table.queue.push(Queued { priority, seq, job });
                self.publish(facts::REQUESTED, key, priority, None);
                self.work.notify_one();
                Found::Flying(waiter)
            }
        }
    }

    /// A pool thread: runs the queue's jobs, highest priority first.
    fn work(&self) {
        loop {
            let job = {
                let mut table = self.table();
                loop {
                    if table.closed {
                        return;
                    }
                    match table.queue.pop() {
                        // An older entry of a job whose priority was
                        // raised, already taken.
                        Some(q) if q.job.started.swap(true, Ordering::SeqCst) => {}
                        Some(q) => break q.job,
                        None => {
                            table = self
                                .work
                                .wait(table)
                                .unwrap_or_else(PoisonError::into_inner);
                        }
                    }
                }
            };
            let outcome = if job.askers.is_cancelled() {
                Err(StateError::Cancelled)
            } else {
                self.publish(facts::COMPUTING, &job.key, job.priority(), None);
                self.computed.fetch_add(1, Ordering::SeqCst);
                let compute = job.compute.clone();
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    cancel::scope(&job.askers, || compute())
                }))
                .unwrap_or_else(|_| Err(StateError::Failed("the computation panicked".into())))
            };
            self.finish(&job, outcome);
        }
    }

    /// Ends a job: its value kept when ready, its fact said, its waiters
    /// handed the outcome. A cancelled or failed one is not kept: the next
    /// ask computes again.
    fn finish(&self, job: &Job, outcome: Outcome) {
        let mut table = self.table();
        table.jobs.remove(&job.key);
        let priority = job.priority();
        match &outcome {
            Ok(value) => {
                let seq = table.seq;
                table.ready.insert(job.key.clone(), (value.clone(), seq));
                while table.ready.len() > self.keep {
                    let oldest = table
                        .ready
                        .iter()
                        .min_by_key(|(_, (_, used))| *used)
                        .map(|(k, _)| k.clone());
                    match oldest {
                        Some(k) => table.ready.remove(&k),
                        None => break,
                    };
                }
                self.publish(facts::READY, &job.key, priority, None);
            }
            Err(StateError::Cancelled) => {
                self.cancelled.fetch_add(1, Ordering::SeqCst);
                self.publish(facts::CANCELLED, &job.key, priority, None);
            }
            Err(e) => self.publish(facts::FAILED, &job.key, priority, Some(e.to_string())),
        }
        if let Some(lead) = job
            .lead
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            lead.finish(Some(outcome));
        }
    }
}

/// How many times a handle that did not cancel asks again for a state whose
/// computation its other askers cancelled.
const MAX_ATTEMPTS: u32 = 8;

/// An ask: wait for its state, poll it, or drop it to withdraw.
pub struct Handle {
    router: Arc<Inner>,
    key: StateKey,
    priority: Priority,
    compute: Compute,
    token: cancel::Token,
    found: Found,
}

impl Handle {
    /// The state asked.
    pub fn key(&self) -> &StateKey {
        &self.key
    }

    /// Withdraws this ask: the computation stops once every asker did.
    pub fn cancel(&self) {
        self.token.cancel();
    }

    /// The state, waited for at most `timeout`.
    ///
    /// # Errors
    /// [`StateError::Timeout`] past the deadline, [`StateError::Cancelled`]
    /// when this ask was cancelled, [`StateError::Failed`] when the
    /// computation failed.
    pub fn wait(&mut self, timeout: Duration) -> Result<Value, StateError> {
        let deadline = std::time::Instant::now() + timeout;
        for _ in 0..MAX_ATTEMPTS {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let outcome = match &self.found {
                Found::Ready(value) => return Ok(value.clone()),
                Found::Flying(waiter) => waiter.wait(left),
            };
            match self.settle(outcome) {
                Some(answer) => return answer,
                None => self.again(),
            }
        }
        Err(StateError::Cancelled)
    }

    /// The state if it is there now; `None` while it is computed.
    pub fn try_get(&mut self) -> Option<Result<Value, StateError>> {
        let outcome = match &self.found {
            Found::Ready(value) => return Some(Ok(value.clone())),
            Found::Flying(waiter) => waiter.try_get()?,
        };
        let answer = self.settle(outcome);
        if answer.is_none() {
            self.again();
        }
        answer
    }

    /// The answer of a flight's outcome, or `None` when the others
    /// cancelled a computation this ask still wants.
    fn settle(&self, outcome: Result<Outcome, FlightError>) -> Option<Result<Value, StateError>> {
        match outcome {
            Ok(Ok(value)) => Some(Ok(value)),
            Ok(Err(StateError::Cancelled)) if !self.token.is_cancelled() => None,
            Ok(Err(e)) => Some(Err(e)),
            Err(FlightError::Timeout) => Some(Err(StateError::Timeout)),
            Err(FlightError::Abandoned) => Some(Err(StateError::Failed(
                "the computation was abandoned".into(),
            ))),
        }
    }

    /// Asks again, under the same token.
    fn again(&mut self) {
        self.found = self
            .router
            .ask(&self.key, self.priority, &self.compute, &self.token);
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    fn key(photo: &str) -> StateKey {
        StateKey {
            photo: photo.into(),
            development: ContentHash::of(b"seven"),
            state: StateKind::Preview,
            size: 256,
        }
    }

    fn value(n: u32) -> Compute {
        Arc::new(move || Ok(Arc::new(n) as Value))
    }

    fn number(v: &Value) -> u32 {
        *v.downcast_ref::<u32>().expect("a number")
    }

    const LONG: Duration = Duration::from_secs(20);

    /// A computation that runs until `go` is set or it is cancelled,
    /// counting its runs; `started` is met once it runs.
    fn held(calls: &Arc<AtomicUsize>, started: &Arc<Barrier>, go: &Arc<AtomicBool>) -> Compute {
        let (calls, started, go) = (calls.clone(), started.clone(), go.clone());
        Arc::new(move || {
            let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
            if first {
                started.wait();
            }
            let token = cancel::current();
            let until = Instant::now() + LONG;
            while first && !go.load(Ordering::SeqCst) {
                if token.is_cancelled() {
                    return Err(StateError::Cancelled);
                }
                assert!(Instant::now() < until, "never released");
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(Arc::new(1u32) as Value)
        })
    }

    #[test]
    fn concurrent_asks_of_one_state_share_one_computation() {
        let router = ImageStates::new(Arc::new(Bus::new()), 2);
        let (calls, started, go) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(Barrier::new(2)),
            Arc::new(AtomicBool::new(false)),
        );
        let compute = held(&calls, &started, &go);
        let mut a = router.ask(key("x"), Priority::Interactive, compute.clone());
        started.wait();
        let mut b = router.ask(key("x"), Priority::Prefetch, compute);
        go.store(true, Ordering::SeqCst);
        assert_eq!(number(&a.wait(LONG).unwrap()), 1);
        assert_eq!(number(&b.wait(LONG).unwrap()), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let c = router.counters();
        assert_eq!((c.asks, c.merged, c.computed), (2, 1, 1), "{c:?}");
        // Ready now: a third ask is answered at once.
        let mut again = router.ask(key("x"), Priority::Batch, value(9));
        assert_eq!(number(&again.try_get().unwrap().unwrap()), 1);
        assert_eq!(router.counters().ready_hits, 1);
    }

    #[test]
    fn one_asker_cancelling_leaves_the_result_to_the_other() {
        let router = ImageStates::new(Arc::new(Bus::new()), 1);
        let (calls, started, go) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(Barrier::new(2)),
            Arc::new(AtomicBool::new(false)),
        );
        let compute = held(&calls, &started, &go);
        let a = router.ask(key("x"), Priority::Interactive, compute.clone());
        started.wait();
        let mut b = router.ask(key("x"), Priority::Interactive, compute);
        drop(a);
        std::thread::sleep(Duration::from_millis(30));
        go.store(true, Ordering::SeqCst);
        assert_eq!(number(&b.wait(LONG).unwrap()), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "never stopped");
        assert_eq!(router.counters().cancelled, 0);
    }

    #[test]
    fn a_cancelled_state_is_not_kept_and_the_next_ask_computes_it() {
        let bus = Arc::new(Bus::new());
        let router = ImageStates::new(bus.clone(), 1);
        let (calls, started, go) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(Barrier::new(2)),
            Arc::new(AtomicBool::new(false)),
        );
        let compute = held(&calls, &started, &go);
        let mut a = router.ask(key("x"), Priority::Interactive, compute.clone());
        started.wait();
        a.cancel();
        assert_eq!(a.wait(LONG).unwrap_err(), StateError::Cancelled);
        assert_eq!(router.counters().cancelled, 1);
        assert!(router.list().0.is_empty(), "nothing ready kept");
        let mut b = router.ask(key("x"), Priority::Interactive, compute);
        assert_eq!(number(&b.wait(LONG).unwrap()), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let kinds: Vec<&str> = bus.since(0).into_iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                facts::REQUESTED,
                facts::COMPUTING,
                facts::CANCELLED,
                facts::REQUESTED,
                facts::COMPUTING,
                facts::READY
            ]
        );
    }

    #[test]
    fn the_pool_runs_the_highest_priority_first_and_a_joiner_raises_it() {
        let bus = Arc::new(Bus::new());
        let router = ImageStates::new(bus.clone(), 1);
        let (calls, started, go) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(Barrier::new(2)),
            Arc::new(AtomicBool::new(false)),
        );
        // The one thread is busy; three asks wait.
        let mut busy = router.ask(key("busy"), Priority::Batch, held(&calls, &started, &go));
        started.wait();
        let order = Arc::new(Mutex::new(Vec::new()));
        let noting = |name: &'static str| -> Compute {
            let order = order.clone();
            Arc::new(move || {
                order.lock().unwrap().push(name);
                Ok(Arc::new(0u32) as Value)
            })
        };
        let mut batch = router.ask(key("batch"), Priority::Batch, noting("batch"));
        let mut near = router.ask(key("near"), Priority::Prefetch, noting("near"));
        let mut raised = router.ask(key("raised"), Priority::Batch, noting("raised"));
        let mut shown = router.ask(key("raised"), Priority::Interactive, noting("raised"));
        go.store(true, Ordering::SeqCst);
        for h in [&mut busy, &mut batch, &mut near, &mut raised, &mut shown] {
            h.wait(LONG).unwrap();
        }
        assert_eq!(*order.lock().unwrap(), ["raised", "near", "batch"]);
    }

    #[test]
    fn a_failure_is_said_and_not_kept() {
        let bus = Arc::new(Bus::new());
        let router = ImageStates::new(bus.clone(), 1);
        let mut h = router.ask(
            key("x"),
            Priority::Interactive,
            Arc::new(|| Err(StateError::Failed("no pixels".into()))),
        );
        assert_eq!(
            h.wait(LONG).unwrap_err(),
            StateError::Failed("no pixels".into())
        );
        let failed = bus
            .since(0)
            .into_iter()
            .find(|e| e.kind == facts::FAILED)
            .expect("a failed fact");
        assert_eq!(failed.payload["reason"], "no pixels");
        assert_eq!(failed.payload["state"], "preview");
        assert_eq!(failed.payload["photo"], "x");
        let mut again = router.ask(key("x"), Priority::Interactive, value(3));
        assert_eq!(number(&again.wait(LONG).unwrap()), 3);
    }
}
