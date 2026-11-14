//! Per-provider limits on model calls: how many at once, how many a
//! minute, and how many remote calls one run may make.
//!
//! ```toml
//! [limits]
//! calls = 2                # parallel calls of a provider that sets none
//! run_remote_calls = 500   # remote calls one run may make (0: no ceiling)
//!
//! [limits.providers.cloud]
//! parallel = 4
//! per_minute = 50
//! ```
//!
//! A call over a provider's limit **waits** for a slot (or for the minute
//! to roll on), and stops waiting when its job is cancelled. A call past
//! its run's ceiling is **refused**: the run stops there and asks. A local
//! provider runs one vision call at a time unless it sets `parallel`
//! (most machines hold one vision model at once).

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::Capability;

/// The window of `per_minute`.
const MINUTE: Duration = Duration::from_mins(1);
/// How often a waiting call looks at its cancel token.
const POLL: Duration = Duration::from_millis(100);

/// One provider's own limits; unset fields take the defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLimits {
    /// Calls at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel: Option<u64>,
    /// Calls started in any 60 seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_minute: Option<u64>,
}

/// `[limits]` of `ai.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Parallel calls of a provider with no `parallel` of its own.
    #[serde(default = "two")]
    pub calls: u64,
    /// Remote calls one run may make; 0 for no ceiling.
    #[serde(default = "ceiling")]
    pub run_remote_calls: u64,
    /// By provider id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderLimits>,
}

fn two() -> u64 {
    2
}

fn ceiling() -> u64 {
    500
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            calls: two(),
            run_remote_calls: ceiling(),
            providers: BTreeMap::new(),
        }
    }
}

/// What applies to one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Rule {
    pub parallel: u64,
    /// 0: no rate limit.
    pub per_minute: u64,
}

impl Limits {
    /// The limits a call of `capability` to `provider` waits on.
    pub fn rule(&self, provider: &str, remote: bool, capability: Capability) -> Rule {
        let own = self.providers.get(provider);
        let parallel = own.and_then(|l| l.parallel).unwrap_or(
            if !remote && capability == Capability::Vision {
                1
            } else {
                self.calls
            },
        );
        Rule {
            parallel: parallel.max(1),
            per_minute: own.and_then(|l| l.per_minute).unwrap_or(0),
        }
    }
}

/// Time, so tests can move it.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

/// The machine's clock.
#[derive(Debug, Default)]
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// A clock that moves only when slept on.
#[derive(Debug)]
pub struct FakeClock {
    start: Instant,
    elapsed: Mutex<Duration>,
}

impl Default for FakeClock {
    fn default() -> Self {
        FakeClock {
            start: Instant::now(),
            elapsed: Mutex::default(),
        }
    }
}

impl FakeClock {
    /// Time since the clock was made.
    pub fn elapsed(&self) -> Duration {
        *self.elapsed.lock().expect("clock lock")
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.start + self.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        *self.elapsed.lock().expect("clock lock") += duration;
    }
}

#[derive(Debug, Default)]
struct Provider {
    active: u64,
    /// When the calls of the last minute started.
    starts: VecDeque<Instant>,
}

#[derive(Debug, Default)]
struct State {
    providers: BTreeMap<String, Provider>,
    /// Remote calls made, by run.
    runs: BTreeMap<String, u64>,
}

/// The limits in force: who is calling, and who waits.
pub struct Limiter {
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    freed: Condvar,
}

impl std::fmt::Debug for Limiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Limiter").finish_non_exhaustive()
    }
}

/// A run's share of remote calls, for [`Limiter::acquire`].
#[derive(Clone, Copy, Debug)]
pub struct RunCeiling<'a> {
    pub run: &'a str,
    /// 0: no ceiling.
    pub ceiling: u64,
}

/// A call in flight; its slot is given back when dropped.
#[derive(Debug)]
pub struct Permit {
    limiter: Arc<Limiter>,
    provider: String,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.limiter.lock();
        if let Some(p) = state.providers.get_mut(&self.provider) {
            p.active = p.active.saturating_sub(1);
        }
        drop(state);
        self.limiter.freed.notify_all();
    }
}

/// Why a call cannot start now.
enum Wait {
    Slot,
    Rate(Duration),
}

impl Limiter {
    pub fn new(clock: Arc<dyn Clock>) -> Arc<Limiter> {
        Arc::new(Limiter {
            clock,
            state: Mutex::default(),
            freed: Condvar::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("limiter lock")
    }

    /// Takes a slot of `provider` under `rule`, waiting while it is full or
    /// its minute is spent. `waiting` is told when the call starts and
    /// stops waiting. A remote call of a run past its ceiling is refused at
    /// once; `cancelled` saying so ends the wait.
    pub fn acquire(
        self: &Arc<Self>,
        provider: &str,
        rule: Rule,
        run: Option<RunCeiling<'_>>,
        cancelled: &dyn Fn() -> bool,
        waiting: &dyn Fn(bool),
    ) -> Result<Permit, String> {
        let mut waited = false;
        let result = loop {
            if cancelled() {
                break Err("cancelled while waiting for a call slot".to_string());
            }
            let mut state = self.lock();
            if let Some(r) = run
                && r.ceiling > 0
                && state.runs.get(r.run).copied().unwrap_or(0) >= r.ceiling
            {
                break Err(format!(
                    "the run `{}` reached its ceiling of {} remote calls \
                     (limits.run_remote_calls in the AI settings)",
                    r.run, r.ceiling
                ));
            }
            let now = self.clock.now();
            match Self::wait_for(&mut state, provider, rule, now) {
                None => {
                    let p = state.providers.entry(provider.to_string()).or_default();
                    p.active += 1;
                    if rule.per_minute > 0 {
                        p.starts.push_back(now);
                    }
                    if let Some(r) = run {
                        *state.runs.entry(r.run.to_string()).or_default() += 1;
                    }
                    break Ok(Permit {
                        limiter: self.clone(),
                        provider: provider.to_string(),
                    });
                }
                Some(wait) => {
                    if !waited {
                        waited = true;
                        waiting(true);
                    }
                    match wait {
                        Wait::Slot => {
                            let _ = self.freed.wait_timeout(state, POLL);
                        }
                        Wait::Rate(left) => {
                            drop(state);
                            self.clock.sleep(left.min(POLL));
                        }
                    }
                }
            }
        };
        if waited {
            waiting(false);
        }
        result
    }

    fn wait_for(state: &mut State, provider: &str, rule: Rule, now: Instant) -> Option<Wait> {
        let p = state.providers.entry(provider.to_string()).or_default();
        if p.active >= rule.parallel {
            return Some(Wait::Slot);
        }
        while p
            .starts
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= MINUTE)
        {
            p.starts.pop_front();
        }
        if rule.per_minute > 0 && p.starts.len() as u64 >= rule.per_minute {
            let first = *p.starts.front().expect("full, so not empty");
            return Some(Wait::Rate(
                MINUTE.saturating_sub(now.saturating_duration_since(first)),
            ));
        }
        None
    }

    /// Calls of `provider` in flight.
    pub fn active(&self, provider: &str) -> u64 {
        self.lock().providers.get(provider).map_or(0, |p| p.active)
    }

    /// Remote calls `run` has made.
    pub fn run_calls(&self, run: &str) -> u64 {
        self.lock().runs.get(run).copied().unwrap_or(0)
    }

    /// Forgets a run's count (the run ended, or its ceiling was raised).
    pub fn forget_run(&self, run: &str) {
        self.lock().runs.remove(run);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use super::*;

    fn never() -> bool {
        false
    }

    fn rule(parallel: u64, per_minute: u64) -> Rule {
        Rule {
            parallel,
            per_minute,
        }
    }

    #[test]
    fn the_rate_limit_spaces_calls_on_a_fake_clock() {
        let clock = Arc::new(FakeClock::default());
        let limiter = Limiter::new(clock.clone());
        let waits = AtomicU32::new(0);
        let mut started = Vec::new();
        for _ in 0..5 {
            let permit = limiter
                .acquire("cloud", rule(4, 2), None, &never, &|w| {
                    if w {
                        waits.fetch_add(1, Ordering::SeqCst);
                    }
                })
                .unwrap();
            started.push(clock.elapsed());
            drop(permit);
        }
        // Two in the first minute, two in the second, one in the third.
        assert_eq!(started[0], Duration::ZERO);
        assert_eq!(started[1], Duration::ZERO);
        assert!(
            started[2] >= MINUTE && started[2] < MINUTE + POLL,
            "{started:?}"
        );
        assert_eq!(started[3], started[2]);
        assert!(started[4] >= 2 * MINUTE, "{started:?}");
        assert_eq!(waits.load(Ordering::SeqCst), 2, "the 3rd and 5th waited");
    }

    #[test]
    fn a_run_past_its_ceiling_is_refused_and_counts_are_per_run() {
        let limiter = Limiter::new(Arc::new(FakeClock::default()));
        let run = |id| {
            Some(RunCeiling {
                run: id,
                ceiling: 2,
            })
        };
        for _ in 0..2 {
            limiter
                .acquire("cloud", rule(8, 0), run("a"), &never, &|_| {})
                .unwrap();
        }
        let refused = limiter
            .acquire("cloud", rule(8, 0), run("a"), &never, &|_| {})
            .unwrap_err();
        assert!(refused.contains("ceiling of 2 remote calls"), "{refused}");
        assert!(
            limiter
                .acquire("cloud", rule(8, 0), run("b"), &never, &|_| {})
                .is_ok()
        );
        assert_eq!(limiter.run_calls("a"), 2);
        limiter.forget_run("a");
        assert_eq!(limiter.run_calls("a"), 0);
    }

    #[test]
    fn a_cancelled_wait_ends_and_a_dropped_permit_frees_its_slot() {
        let limiter = Limiter::new(Arc::new(RealClock));
        let held = limiter
            .acquire("ollama", rule(1, 0), None, &never, &|_| {})
            .unwrap();
        assert_eq!(limiter.active("ollama"), 1);
        let token = AtomicBool::new(false);
        let cancelled = std::thread::scope(|s| {
            let waiter = s.spawn(|| {
                limiter
                    .acquire(
                        "ollama",
                        rule(1, 0),
                        None,
                        &|| token.load(Ordering::SeqCst),
                        &|_| {},
                    )
                    .map(drop)
            });
            std::thread::sleep(Duration::from_millis(50));
            token.store(true, Ordering::SeqCst);
            waiter.join().unwrap()
        });
        assert!(cancelled.unwrap_err().contains("cancelled"));
        drop(held);
        assert_eq!(limiter.active("ollama"), 0);
    }

    #[test]
    fn local_vision_runs_one_at_a_time_unless_the_provider_says() {
        let mut limits = Limits::default();
        assert_eq!(limits.rule("ollama", false, Capability::Vision).parallel, 1);
        assert_eq!(limits.rule("ollama", false, Capability::Chat).parallel, 2);
        assert_eq!(limits.rule("cloud", true, Capability::Vision).parallel, 2);
        limits.providers.insert(
            "ollama".into(),
            ProviderLimits {
                parallel: Some(3),
                per_minute: Some(10),
            },
        );
        assert_eq!(
            limits.rule("ollama", false, Capability::Vision),
            rule(3, 10)
        );
        let read: Limits = toml::from_str(
            "calls = 3\nrun_remote_calls = 0\n[providers.cloud]\nparallel = 4\nper_minute = 50\n",
        )
        .unwrap();
        assert_eq!(read.rule("cloud", true, Capability::Chat), rule(4, 50));
        assert_eq!(read.run_remote_calls, 0);
        // The older file, `calls` alone, still reads.
        let old: Limits = toml::from_str("calls = 2\n").unwrap();
        assert_eq!(old, Limits::default());
    }
}
