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
        self.flight
            .cells
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
        *self
            .cell
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = match self.value.take() {
            Some(v) => State::Done(v),
            None => State::Abandoned,
        };
        self.cell.done.notify_all();
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
        let until = Instant::now() + deadline;
        let mut state = cell.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match &*state {
                State::Done(v) => return Ok((v.clone(), true)),
                State::Abandoned => return Err(FlightError::Abandoned),
                State::Running => {}
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(FlightError::Timeout);
            }
            state = cell
                .done
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}
