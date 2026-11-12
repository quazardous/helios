//! Running the checks: one background job (`health-<n>`, kind `health`)
//! runs every check of the registry on a thread of its own and keeps
//! their findings. [`Schedule`] says when: a while after start (never
//! slowing it), then daily, and when the volumes mounted change.
//!
//! A [`Runner`] is a handle: its clones share one schedule, one run at a
//! time and one set of findings, so the node that schedules the checks
//! and the windows that show them hold the same runner.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use helios_core::jobs::{Jobs, Progress, Start};

use crate::{Context, Findings, Registry};

/// The job's kind, in the jobs centre.
pub const KIND: &str = "health";

/// When the checks run: once `delay` after start, then every `every`,
/// and when the volumes' generation changes (a volume came or went).
#[derive(Clone, Debug)]
pub struct Schedule {
    started: Instant,
    delay: Duration,
    every: Duration,
    last: Option<Instant>,
    volumes: Option<u64>,
}

impl Schedule {
    /// The desktop's: a first run 30 s after start, then daily.
    pub fn desktop(now: Instant) -> Self {
        Self::new(now, Duration::from_secs(30), Duration::from_hours(24))
    }

    pub fn new(now: Instant, delay: Duration, every: Duration) -> Self {
        Self {
            started: now,
            delay,
            every,
            last: None,
            volumes: None,
        }
    }

    /// Whether a run is due at `now`, the volumes' generation being
    /// `volumes` (none without a watch); a due run counts as made.
    pub fn due(&mut self, now: Instant, volumes: Option<u64>) -> bool {
        let moved = volumes.is_some() && self.volumes.is_some() && volumes != self.volumes;
        if volumes.is_some() {
            self.volumes = volumes;
        }
        let due = match self.last {
            None => now >= self.started + self.delay,
            Some(last) => moved || now >= last + self.every,
        };
        if due {
            self.last = Some(now);
        }
        due
    }

    /// Counts a run made now (asked by hand), so the next waits again.
    pub fn ran(&mut self, now: Instant) {
        self.last = Some(now);
    }
}

type Waker = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;

/// Whether a run is under way, and its end heard by [`Runner::wait`].
#[derive(Default)]
struct Running {
    on: Mutex<bool>,
    ended: Condvar,
}

impl Running {
    /// Takes the run; false when one is under way.
    fn take(&self) -> bool {
        let mut on = self.on.lock().unwrap_or_else(PoisonError::into_inner);
        !std::mem::replace(&mut *on, true)
    }

    fn end(&self) {
        *self.on.lock().unwrap_or_else(PoisonError::into_inner) = false;
        self.ended.notify_all();
    }

    fn on(&self) -> bool {
        *self.on.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Runs the checks in the background and keeps what they found; its
/// clones share all of it.
#[derive(Clone)]
pub struct Runner {
    registry: Registry,
    context: Arc<dyn Fn() -> Context + Send + Sync>,
    findings: Arc<Mutex<Findings>>,
    jobs: Arc<Mutex<Option<Jobs>>>,
    running: Arc<Running>,
    waker: Waker,
    schedule: Arc<Mutex<Schedule>>,
    runs: Arc<AtomicU64>,
    /// No run starts any more (its owner stops).
    stopped: Arc<AtomicBool>,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("registry", &self.registry)
            .field("runs", &self.runs())
            .finish_non_exhaustive()
    }
}

impl Runner {
    /// Runs `registry`'s checks over what `context` gathers (called on
    /// the job's thread), keeping the findings beside `findings`' ignored
    /// list.
    pub fn new(
        registry: Registry,
        context: impl Fn() -> Context + Send + Sync + 'static,
        findings: Findings,
        schedule: Schedule,
    ) -> Self {
        Self {
            registry,
            context: Arc::new(context),
            findings: Arc::new(Mutex::new(findings)),
            jobs: Arc::default(),
            running: Arc::default(),
            waker: Arc::default(),
            schedule: Arc::new(Mutex::new(schedule)),
            runs: Arc::default(),
            stopped: Arc::default(),
        }
    }

    /// Reports each run as a job there (the jobs centre).
    pub fn report_to(&self, jobs: Jobs) {
        *self.jobs.lock().unwrap_or_else(PoisonError::into_inner) = Some(jobs);
    }

    /// Called from the job's thread once a run's findings are in (to ask
    /// the interface for a frame).
    pub fn on_done(&self, wake: impl Fn() + Send + 'static) {
        *self.waker.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(wake));
    }

    pub fn registry_mut(&mut self) -> &mut Registry {
        &mut self.registry
    }

    /// The findings and the ignored list.
    pub fn findings(&self) -> MutexGuard<'_, Findings> {
        self.findings.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many runs started.
    pub fn runs(&self) -> u64 {
        self.runs.load(Ordering::SeqCst)
    }

    /// Whether a run is under way.
    pub fn running(&self) -> bool {
        self.running.on()
    }

    /// Starts a run when the schedule says one is due (each frame of a
    /// window, and the node's own clock).
    pub fn tick(&self, now: Instant, volumes: Option<u64>) -> bool {
        let due = self
            .schedule
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .due(now, volumes);
        due && self.start()
    }

    /// "Check now": a run at once (none while one is under way).
    pub fn run_now(&self) -> bool {
        self.schedule
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .ran(Instant::now());
        self.start()
    }

    /// No run starts from now on (the node stops); one under way ends.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    /// Waits until no run is under way, at most `deadline`; whether none is.
    pub fn wait(&self, deadline: Duration) -> bool {
        let on = self
            .running
            .on
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (on, _) = self
            .running
            .ended
            .wait_timeout_while(on, deadline, |on| *on)
            .unwrap_or_else(PoisonError::into_inner);
        !*on
    }

    fn start(&self) -> bool {
        if self.stopped.load(Ordering::SeqCst) || !self.running.take() {
            return false;
        }
        let runs = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
        let jobs = self
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let report = jobs.as_ref().map(|jobs| {
            jobs.start(
                Start::new(&format!("health-{runs}"), "Health check", KIND)
                    .total(self.registry.checks().len() as u64)
                    .not_cancellable(),
            )
        });
        let (registry, context, findings, running, waker) = (
            self.registry.clone(),
            self.context.clone(),
            self.findings.clone(),
            self.running.clone(),
            self.waker.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("helios-health".into())
            .spawn(move || {
                let ctx = context();
                let checks = registry.checks();
                let mut found = Vec::new();
                for (i, check) in checks.iter().enumerate() {
                    if let Some(r) = &report {
                        r.progress(
                            Progress::new()
                                .counts(i as u64, checks.len() as u64)
                                .item(check.id()),
                        );
                    }
                    let these = crate::run_one(check, &ctx);
                    if let Some(r) = &report {
                        r.log(format!("{}: {} findings", check.id(), these.len()));
                    }
                    found.extend(these);
                }
                findings
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .replace(found);
                if let Some(r) = &report {
                    r.progress(Progress::new().counts(checks.len() as u64, checks.len() as u64));
                    r.finished();
                }
                running.end();
                if let Some(wake) = waker
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                {
                    wake();
                }
            });
        if let Err(e) = spawned {
            log::warn!("health: no thread to run the checks: {e}");
            self.running.end();
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_run_waits_then_runs_daily_and_on_a_volume_change() {
        let t0 = Instant::now();
        let s = |secs| t0 + Duration::from_secs(secs);
        let mut sch = Schedule::new(t0, Duration::from_secs(30), Duration::from_hours(1));
        assert!(!sch.due(s(1), Some(0)), "never at start");
        assert!(sch.due(s(30), Some(0)));
        assert!(!sch.due(s(31), Some(0)));
        assert!(sch.due(s(40), Some(1)), "a volume came");
        assert!(!sch.due(s(41), Some(1)));
        assert!(sch.due(s(40 + 3600), Some(1)), "daily");
        // Without a watch, only the clock.
        let mut sch = Schedule::new(t0, Duration::ZERO, Duration::from_hours(1));
        assert!(sch.due(s(0), None));
        assert!(!sch.due(s(5), None));
    }

    #[test]
    fn a_run_keeps_its_findings_and_wakes() {
        use std::sync::mpsc;
        let runner = Runner::new(
            Registry::builtin(),
            Context::default,
            Findings::default(),
            Schedule::new(Instant::now(), Duration::ZERO, Duration::from_hours(1)),
        );
        let (tx, rx) = mpsc::channel();
        runner.on_done(move || {
            let _ = tx.send(());
        });
        assert!(runner.run_now());
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the run ends");
        assert!(runner.findings().ran());
        assert!(!runner.running());
    }
}
