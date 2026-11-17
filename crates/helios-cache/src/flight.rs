//! Single flight: concurrent requests for one key run one computation.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Why a merged request got no value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightError {
    /// The computation it waited for did not end within the request's
    /// deadline (it goes on for the others).
    Timeout,
    /// The computation it waited for ended without a value (it failed or
    /// panicked): the caller may try again.
    Abandoned,
}

enum State<V> {
    Running,
    Done(V),
    Abandoned,
}

struct Cell<V> {
    state: Mutex<State<V>>,
    done: Condvar,
}

/// Computations in flight, by key. The first request for a key computes;
/// the others wait for its value, each within its own deadline. A value
/// is handed out once to each waiter (cloned: an `Arc` for large ones)
/// and not kept: keeping it is the cache's work.
pub struct Flight<K, V> {
    cells: Mutex<HashMap<K, Arc<Cell<V>>>>,
    merged: AtomicU64,
}

impl<K, V> Default for Flight<K, V> {
    fn default() -> Self {
        Flight {
            cells: Mutex::new(HashMap::new()),
            merged: AtomicU64::new(0),
        }
    }
}

/// Ends the leader's flight, with a value or abandoned (a panic too).
struct Leader<'a, K: Clone + Eq + Hash, V: Clone> {
    flight: &'a Flight<K, V>,
    key: K,
    cell: Arc<Cell<V>>,
    value: Option<V>,
}

impl<K: Clone + Eq + Hash, V: Clone> Drop for Leader<'_, K, V> {
    fn drop(&mut self) {
        self.flight.end(&self.key, &self.cell, self.value.take());
    }
}

/// What [`Flight::begin`] gives: the computation to run, or one to wait
/// for.
pub enum Begin<K: Clone + Eq + Hash, V: Clone> {
    /// No computation for the key runs: this one leads it.
    Lead(Lead<K, V>),
    /// One runs: wait for its value.
    Join(Waiter<V>),
}

/// The computation of a key, led by whoever holds it, on any thread: its
/// value is handed to the waiters by [`Lead::finish`]. Dropped without a
/// value (a failure, a panic), it ends abandoned.
pub struct Lead<K: Clone + Eq + Hash, V: Clone> {
    flight: Arc<Flight<K, V>>,
    key: K,
    cell: Arc<Cell<V>>,
    value: Option<V>,
}

impl<K: Clone + Eq + Hash, V: Clone> Lead<K, V> {
    /// A waiter for this computation's value.
    pub fn waiter(&self) -> Waiter<V> {
        Waiter {
            cell: self.cell.clone(),
        }
    }

    /// Ends the computation: its waiters get `value`, or abandoned with
    /// `None`; the next request for the key computes again.
    pub fn finish(mut self, value: Option<V>) {
        self.value = value;
    }
}

impl<K: Clone + Eq + Hash, V: Clone> Drop for Lead<K, V> {
    fn drop(&mut self) {
        self.flight.end(&self.key, &self.cell, self.value.take());
    }
}

/// Waits for a computation in flight, as often as asked.
pub struct Waiter<V> {
    cell: Arc<Cell<V>>,
}

impl<V> Clone for Waiter<V> {
    fn clone(&self) -> Self {
        Waiter {
            cell: self.cell.clone(),
        }
    }
}

impl<V: Clone> Waiter<V> {
    /// The value, once the computation ended; `None` while it runs.
    pub fn try_get(&self) -> Option<Result<V, FlightError>> {
        match &*self
            .cell
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            State::Done(v) => Some(Ok(v.clone())),
            State::Abandoned => Some(Err(FlightError::Abandoned)),
            State::Running => None,
        }
    }

    /// The value, waited for at most `deadline`.
    ///
    /// # Errors
    /// [`FlightError::Timeout`] past the deadline;
    /// [`FlightError::Abandoned`] when the computation gave no value.
    pub fn wait(&self, deadline: Duration) -> Result<V, FlightError> {
        let until = Instant::now() + deadline;
        let mut state = self
            .cell
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            match &*state {
                State::Done(v) => return Ok(v.clone()),
                State::Abandoned => return Err(FlightError::Abandoned),
                State::Running => {}
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(FlightError::Timeout);
            }
            state = self
                .cell
                .done
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl<K: Clone + Eq + Hash, V: Clone> Flight<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many requests were merged into another's computation.
    pub fn merged(&self) -> u64 {
        self.merged.load(Ordering::Relaxed)
    }

    /// Ends `key`'s computation: out of the flight, its waiters woken
    /// with `value` (abandoned without one).
    fn end(&self, key: &K, cell: &Arc<Cell<V>>, value: Option<V>) {
        {
            let mut cells = self.cells.lock().unwrap_or_else(PoisonError::into_inner);
            if cells.get(key).is_some_and(|c| Arc::ptr_eq(c, cell)) {
                cells.remove(key);
            }
        }
        *cell.state.lock().unwrap_or_else(PoisonError::into_inner) = match value {
            Some(v) => State::Done(v),
            None => State::Abandoned,
        };
        cell.done.notify_all();
    }

    /// Starts a request for `key` without blocking: it leads the
    /// computation when none runs (computed anywhere, ended by
    /// [`Lead::finish`]), else it joins the running one (counted merged).
    pub fn begin(self: &Arc<Self>, key: &K) -> Begin<K, V> {
        let mut cells = self.cells.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cell) = cells.get(key) {
            self.merged.fetch_add(1, Ordering::Relaxed);
            return Begin::Join(Waiter { cell: cell.clone() });
        }
        let cell = Arc::new(Cell {
            state: Mutex::new(State::Running),
            done: Condvar::new(),
        });
        cells.insert(key.clone(), cell.clone());
        Begin::Lead(Lead {
            flight: self.clone(),
            key: key.clone(),
            cell,
            value: None,
        })
    }

    /// How many computations are running.
    pub fn running(&self) -> usize {
        self.cells
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// The value of `key`: computed by `compute` when no request for it
    /// runs, else the running one's, waited for at most `deadline`. The
    /// value, and whether it came from another request's computation.
    ///
    /// # Errors
    /// [`FlightError::Timeout`] past the deadline;
    /// [`FlightError::Abandoned`] when the computation waited for gave no
    /// value.
    pub fn run(
        &self,
        key: &K,
        deadline: Duration,
        compute: impl FnOnce() -> Option<V>,
    ) -> Result<(V, bool), FlightError> {
        let (cell, lead) = {
            let mut cells = self.cells.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(cell) = cells.get(key) {
                (cell.clone(), false)
            } else {
                let cell = Arc::new(Cell {
                    state: Mutex::new(State::Running),
                    done: Condvar::new(),
                });
                cells.insert(key.clone(), cell.clone());
                (cell, true)
            }
        };
        if lead {
            let mut leader = Leader {
                flight: self,
                key: key.clone(),
                cell,
                value: None,
            };
            leader.value = compute();
            return leader
                .value
                .clone()
                .map(|v| (v, false))
                .ok_or(FlightError::Abandoned);
        }
        self.merged.fetch_add(1, Ordering::Relaxed);
        Waiter { cell }.wait(deadline).map(|v| (v, true))
    }
}
