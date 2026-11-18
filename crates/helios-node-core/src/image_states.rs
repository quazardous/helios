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
//! the router runs call it, and the router keeps the small states it
//! produced (the preview) and engine holds ([`helios_core::Hold`]) on the
//! steps it wants kept (a photo's full-size step), never copies: those
//! bytes stay counted once, in the engine's cache. Viewport tiles stay
//! direct engine asks.
//!
//! The ready states live within a RAM budget ([`ImageStates::set_budget`],
//! [`default_budget`]): past it, the states of least value go first, a
//! cache-service eviction (`docs/design/cache.md`, "Eviction": last use
//! plus what a miss would cost per byte, [`helios_cache::Lru`]), never a
//! pinned one ([`ImageStates::pin`]: the shown photo's full-size step).
//! The budget is passed only by pinned states and by the state that just
//! landed, for as long as it takes to evict others.

use std::any::Any;
use std::collections::{BinaryHeap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helios_cache::{Begin, Flight, FlightError, Lead, Lru, Waiter};
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
    /// A ready state was let go to stay within the budget (or cleared):
    /// the next ask computes it again.
    pub const EVICTED: &str = "image.state.evicted";
    /// Every fact, in lifecycle order.
    pub const ALL: [&str; 6] = [REQUESTED, COMPUTING, READY, FAILED, CANCELLED, EVICTED];
}

/// The router's ready states as the cache service counts them: the bytes
/// they own (previews). The engine steps they hold stay counted under
/// [`helios_cache::RENDER_STEP`], once.
pub const KIND: helios_cache::Kind = helios_cache::Kind {
    id: "image.state",
    title: "Image states",
    ..helios_cache::RENDER_STEP
};

/// What a photo can be computed into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateKind {
    /// The whole image at a small size, its longest side `size` pixels:
    /// what the scopes measure and the navigator draws.
    Preview,
    /// The full-size steps (the demosaic's output) kept in the engine's
    /// cache: a [`helios_core::Hold`], its `development` the steps' keys.
    /// The fit, a zoom and the preview read them instead of demosaicing
    /// again.
    FullSize,
}

impl StateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StateKind::Preview => "preview",
            StateKind::FullSize => "full_size",
        }
    }
}

/// One state of one photo.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

/// A computed state and what it weighs.
pub struct Ready {
    pub value: Value,
    /// The bytes it takes, counted against the budget.
    pub bytes: u64,
    /// The bytes live in another cache (an engine hold): counted against
    /// this budget, not again in the cache service's usage.
    pub held: bool,
}

impl Ready {
    /// A state owning its `bytes`.
    pub fn new(value: impl Any + Send + Sync, bytes: u64) -> Ready {
        Ready {
            value: Arc::new(value),
            bytes,
            held: false,
        }
    }

    /// A state holding `bytes` of another cache's (an engine hold).
    pub fn held(value: impl Any + Send + Sync, bytes: u64) -> Ready {
        Ready {
            held: true,
            ..Ready::new(value, bytes)
        }
    }
}

/// How a state is computed: called on a pool thread, under the shared
/// cancel token ([`cancel::current`] inside it).
pub type Compute = Arc<dyn Fn() -> Result<Ready, StateError> + Send + Sync>;

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
    /// Ready states let go to stay within the budget, or cleared.
    pub evicted: u64,
}

/// What the ready states take, against the budget.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    /// Ready states kept.
    pub entries: u64,
    /// Their bytes, those held in the engine's cache included.
    pub bytes: u64,
    /// Of them, the bytes held in another cache.
    pub held: u64,
    /// States pinned (never evicted).
    pub pinned: u64,
    /// The budget, `None` without a limit.
    pub budget: Option<u64>,
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
    /// What a ready or evicted state weighs.
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<u64>,
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

/// A ready state.
struct Kept {
    value: Value,
    held: bool,
}

#[derive(Default)]
struct Table {
    jobs: HashMap<StateKey, Arc<Job>>,
    /// Ready states.
    ready: HashMap<StateKey, Kept>,
    /// Their eviction order and bytes: last use (nanoseconds since the
    /// router was made) plus what a miss would cost.
    order: Lru<StateKey>,
    /// Keys never evicted, with how many pins hold each.
    pins: HashMap<StateKey, usize>,
    queue: BinaryHeap<Queued>,
    seq: u64,
    closed: bool,
}

struct Inner {
    bus: Arc<Bus>,
    flight: Arc<Flight<StateKey, Outcome>>,
    table: Mutex<Table>,
    work: Condvar,
    /// The eviction order's clock starts here.
    start: Instant,
    /// Bytes the ready states may take; [`NO_BUDGET`] for no limit.
    budget: AtomicU64,
    asks: AtomicU64,
    ready_hits: AtomicU64,
    computed: AtomicU64,
    cancelled: AtomicU64,
    evicted: AtomicU64,
    /// Keeps the router attached to the cache service.
    store: Mutex<Option<Arc<dyn helios_cache::Store>>>,
}

/// [`Inner::budget`]'s value for no limit.
const NO_BUDGET: u64 = u64::MAX;

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

/// The budget's share of the machine's memory by default: 12.5 %.
pub const DEFAULT_SHARE: f64 = 12.5;

/// The default budget's floor and ceiling: 1 GiB (the shown photo's
/// full-size step at 24 MP is about 0.4 GB, 1 GB at 61 MP) and 4 GiB.
pub const MIN_BUDGET: u64 = 1 << 30;
pub const MAX_BUDGET: u64 = 4 << 30;

/// The budget by default: [`DEFAULT_SHARE`] of `memory` (bytes; `None`:
/// not known), within [`MIN_BUDGET`] and [`MAX_BUDGET`]. On 16 GB, 2 GiB:
/// the shown 24 MP photo's full-size step and three or four more.
pub fn default_budget(memory: Option<u64>) -> u64 {
    memory.map_or(MIN_BUDGET, |m| {
        ((m as f64 * DEFAULT_SHARE / 100.0) as u64).clamp(MIN_BUDGET, MAX_BUDGET)
    })
}

impl ImageStates {
    /// A router publishing on `bus`, running at most `threads` computations
    /// at once.
    pub fn new(bus: Arc<Bus>, threads: usize) -> Self {
        let inner = Arc::new(Inner {
            bus,
            flight: Arc::default(),
            table: Mutex::default(),
            work: Condvar::new(),
            start: Instant::now(),
            budget: AtomicU64::new(default_budget(helios_cache::physical_memory())),
            asks: AtomicU64::new(0),
            ready_hits: AtomicU64::new(0),
            computed: AtomicU64::new(0),
            cancelled: AtomicU64::new(0),
            evicted: AtomicU64::new(0),
            store: Mutex::default(),
        });
        let store: Arc<dyn helios_cache::Store> = Arc::new(StateStore(Arc::downgrade(&inner)));
        helios_cache::Service::global().attach(Arc::downgrade(&store));
        *inner.store.lock().unwrap_or_else(PoisonError::into_inner) = Some(store);
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
            evicted: self.inner.evicted.load(Ordering::SeqCst),
        }
    }

    /// The bytes the ready states may take (`None`: no limit); past it,
    /// the states of least value go at once.
    pub fn set_budget(&self, budget: Option<u64>) {
        self.inner
            .budget
            .store(budget.unwrap_or(NO_BUDGET), Ordering::SeqCst);
        let mut table = self.inner.table();
        self.inner.settle(&mut table);
    }

    /// The budget, `None` without a limit.
    pub fn budget(&self) -> Option<u64> {
        self.inner.budget()
    }

    /// What the ready states take now.
    pub fn usage(&self) -> Usage {
        self.inner.usage()
    }

    /// Keeps `key` from eviction until the pin drops: the shown photo's
    /// full-size step. A key not ready yet is kept once it lands.
    #[must_use = "the state is evictable again when the pin drops"]
    pub fn pin(&self, key: &StateKey) -> StatePin {
        *self.inner.table().pins.entry(key.clone()).or_default() += 1;
        StatePin {
            router: self.inner.clone(),
            key: key.clone(),
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

/// A pin on one state: dropped, the state is evictable again.
pub struct StatePin {
    router: Arc<Inner>,
    key: StateKey,
}

impl StatePin {
    pub fn key(&self) -> &StateKey {
        &self.key
    }
}

impl fmt::Debug for StatePin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StatePin").field(&self.key).finish()
    }
}

impl Drop for StatePin {
    fn drop(&mut self) {
        let mut table = self.router.table();
        if let Some(n) = table.pins.get_mut(&self.key) {
            *n -= 1;
            if *n == 0 {
                table.pins.remove(&self.key);
            }
        }
        self.router.settle(&mut table);
    }
}

/// The router as the cache service sees it.
struct StateStore(std::sync::Weak<Inner>);

impl helios_cache::Store for StateStore {
    fn usage(&self) -> Vec<helios_cache::Usage> {
        let Some(inner) = self.0.upgrade() else {
            return Vec::new();
        };
        let usage = inner.usage();
        vec![helios_cache::Usage {
            scope: helios_cache::Scope::Shared,
            kind: KIND,
            tier: helios_cache::Tier::Ram,
            entries: usage.entries,
            bytes: usage.bytes - usage.held,
            pinned: usage.pinned,
            budget: usage.budget,
            shown: None,
        }]
    }

    fn clear(&self, kind: Option<&str>) -> u64 {
        if kind.is_some_and(|k| k != KIND.id) {
            return 0;
        }
        let Some(inner) = self.0.upgrade() else {
            return 0;
        };
        let mut guard = inner.table();
        let table = &mut *guard;
        let pins = &table.pins;
        let gone = table.order.clear(|k| pins.contains_key(k));
        inner.let_go(table, gone)
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

    /// Nanoseconds since the router was made: the eviction order's clock.
    fn now(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64
    }

    fn budget(&self) -> Option<u64> {
        match self.budget.load(Ordering::SeqCst) {
            NO_BUDGET => None,
            b => Some(b),
        }
    }

    fn usage(&self) -> Usage {
        let table = self.table();
        let held = table
            .ready
            .iter()
            .filter(|(_, kept)| kept.held)
            .filter_map(|(k, _)| table.order.size(k))
            .sum();
        Usage {
            entries: table.ready.len() as u64,
            bytes: table.order.bytes(),
            held,
            pinned: table.pins.len() as u64,
            budget: self.budget(),
        }
    }

    /// Evicts the ready states of least value, never a pinned one, until
    /// they fit the budget.
    fn settle(&self, table: &mut Table) {
        let Some(budget) = self.budget() else { return };
        let pins = &table.pins;
        let gone = table.order.evict(budget, |k| pins.contains_key(k));
        self.let_go(table, gone);
    }

    /// Drops the states taken out of the order (their values released: an
    /// engine hold unpins), says each: the bytes freed.
    fn let_go(&self, table: &mut Table, gone: Vec<(StateKey, u64)>) -> u64 {
        let mut freed = 0;
        for (key, bytes) in gone {
            table.ready.remove(&key);
            freed += bytes;
            self.evicted.fetch_add(1, Ordering::SeqCst);
            self.publish_with(facts::EVICTED, &key, Priority::Batch, None, Some(bytes));
        }
        freed
    }

    fn publish(
        &self,
        kind: &'static str,
        key: &StateKey,
        priority: Priority,
        reason: Option<String>,
    ) {
        self.publish_with(kind, key, priority, reason, None);
    }

    fn publish_with(
        &self,
        kind: &'static str,
        key: &StateKey,
        priority: Priority,
        reason: Option<String>,
        bytes: Option<u64>,
    ) {
        let fact = Fact {
            photo: &key.photo,
            development: key.development.short(),
            state: key.state,
            size: key.size,
            priority,
            reason,
            bytes,
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
        if let Some(kept) = table.ready.get(key) {
            let value = kept.value.clone();
            let now = self.now();
            table.order.touch(key, now);
            self.ready_hits.fetch_add(1, Ordering::SeqCst);
            return Found::Ready(value);
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
            let started = Instant::now();
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
            self.finish(&job, outcome, started.elapsed());
        }
    }

    /// Ends a job: its value kept when ready (the budget settled, which
    /// may let it go at once when it does not fit), its fact said, its
    /// waiters handed the outcome. A cancelled or failed one is not kept:
    /// the next ask computes again.
    fn finish(&self, job: &Job, outcome: Result<Ready, StateError>, took: Duration) {
        let mut table = self.table();
        table.jobs.remove(&job.key);
        let priority = job.priority();
        let outcome = outcome.map(|ready| {
            let bonus = helios_cache::Cost::Render.bonus(took.as_nanos() as u64, ready.bytes);
            let now = self.now();
            table.order.insert(job.key.clone(), ready.bytes, now, bonus);
            table.ready.insert(
                job.key.clone(),
                Kept {
                    value: ready.value.clone(),
                    held: ready.held,
                },
            );
            self.publish_with(facts::READY, &job.key, priority, None, Some(ready.bytes));
            self.settle(&mut table);
            ready.value
        });
        match &outcome {
            Ok(_) => {}
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
        Arc::new(move || Ok(Ready::new(n, 0)))
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
            Ok(Ready::new(1u32, 0))
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
                Ok(Ready::new(0u32, 0))
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

    /// A state of `bytes`, computed at once.
    fn weighing(bytes: u64) -> Compute {
        Arc::new(move || Ok(Ready::new(bytes, bytes)))
    }

    fn ready_now(router: &ImageStates, k: StateKey, compute: Compute) {
        router
            .ask(k, Priority::Interactive, compute)
            .wait(LONG)
            .expect("ready");
    }

    fn ready_keys(router: &ImageStates) -> Vec<String> {
        let mut keys: Vec<String> = router.list().0.into_iter().map(|k| k.photo).collect();
        keys.sort();
        keys
    }

    #[test]
    fn past_the_budget_the_least_recently_used_unpinned_states_go_first() {
        let bus = Arc::new(Bus::new());
        let router = ImageStates::new(bus.clone(), 1);
        router.set_budget(Some(300));
        let shown = router.pin(&key("shown"));
        // Used in this order, a few milliseconds apart (the eviction
        // order's clock), all of one cost.
        for name in ["shown", "a", "b"] {
            ready_now(&router, key(name), weighing(100));
            std::thread::sleep(Duration::from_millis(20));
        }
        // "a" used again: "b" is now the least recently used unpinned.
        ready_now(&router, key("a"), weighing(100));
        std::thread::sleep(Duration::from_millis(20));
        ready_now(&router, key("c"), weighing(100));
        assert_eq!(ready_keys(&router), ["a", "c", "shown"]);
        // "shown", the oldest, is pinned: "a" goes next.
        std::thread::sleep(Duration::from_millis(20));
        ready_now(&router, key("d"), weighing(100));
        assert_eq!(ready_keys(&router), ["c", "d", "shown"]);
        let usage = router.usage();
        assert_eq!((usage.bytes, usage.entries, usage.pinned), (300, 3, 1));
        assert_eq!(router.counters().evicted, 2);
        let evicted: Vec<String> = bus
            .since(0)
            .into_iter()
            .filter(|e| e.kind == facts::EVICTED)
            .map(|e| e.payload["photo"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(evicted, ["b", "a"]);
        // Unpinned, the shown state is the oldest: it goes once room is
        // needed.
        drop(shown);
        ready_now(&router, key("e"), weighing(100));
        assert_eq!(ready_keys(&router), ["c", "d", "e"]);
    }

    #[test]
    fn the_budget_is_never_passed_by_more_than_the_state_in_flight() {
        let router = ImageStates::new(Arc::new(Bus::new()), 2);
        let budget = 1_000;
        router.set_budget(Some(budget));
        let _shown = router.pin(&key("p0"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        for i in 0..40u64 {
            let (seen, router2) = (seen.clone(), router.clone());
            let bytes = 150 + (i % 4) * 50;
            let compute: Compute = Arc::new(move || {
                // While this one computes, what is kept fits the budget.
                seen.lock().unwrap().push(router2.usage().bytes);
                Ok(Ready::new(bytes, bytes))
            });
            handles.push(router.ask(key(&format!("p{i}")), Priority::Prefetch, compute));
        }
        for mut h in handles {
            h.wait(LONG).unwrap();
            assert!(router.usage().bytes <= budget, "{:?}", router.usage());
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 40);
        assert!(seen.iter().all(|b| *b <= budget), "{seen:?}");
        assert!(
            router.list().0.contains(&key("p0")),
            "the pinned state stays"
        );
    }

    #[test]
    fn a_pinned_state_too_big_for_the_budget_stays_and_the_others_go() {
        let router = ImageStates::new(Arc::new(Bus::new()), 1);
        router.set_budget(Some(100));
        ready_now(&router, key("small"), weighing(50));
        let _pin = router.pin(&key("big"));
        ready_now(&router, key("big"), weighing(400));
        assert_eq!(ready_keys(&router), ["big"]);
        // An unpinned state past the budget is handed to its asker, not
        // kept.
        ready_now(&router, key("other"), weighing(400));
        assert_eq!(ready_keys(&router), ["big"]);
    }

    #[test]
    fn the_cache_service_counts_the_owned_bytes_and_clearing_spares_pins() {
        let router = ImageStates::new(Arc::new(Bus::new()), 1);
        router.set_budget(None);
        let held: Compute = Arc::new(|| Ok(Ready::held(0u32, 700)));
        let _pin = router.pin(&key("held"));
        ready_now(&router, key("held"), held);
        ready_now(&router, key("own"), weighing(30));
        let usage = router.usage();
        assert_eq!((usage.bytes, usage.held), (730, 700));
        let store = StateStore(Arc::downgrade(&router.inner));
        let line = &helios_cache::Store::usage(&store)[0];
        assert_eq!(
            (line.kind.id, line.bytes, line.pinned),
            ("image.state", 30, 1)
        );
        assert_eq!(helios_cache::Store::clear(&store, Some("image.state")), 30);
        assert_eq!(ready_keys(&router), ["held"]);
    }
}
