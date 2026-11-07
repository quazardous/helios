//! Running the checks: one background job (`health-<n>`, kind `health`)
//! runs every check of the registry on a thread of its own and keeps
//! their findings. [`Schedule`] says when: a while after start (never
//! slowing it), then daily, and when the volumes mounted change.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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

/// Runs the checks in the background and keeps what they found.
pub struct Runner {
    registry: Registry,
    context: Arc<dyn Fn() -> Context + Send + Sync>,
    findings: Arc<Mutex<Findings>>,
    jobs: Option<Jobs>,
    running: Arc<AtomicBool>,
    waker: Waker,
    schedule: Schedule,
    runs: u64,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("registry", &self.registry)
            .field("runs", &self.runs)
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
            jobs: None,
            running: Arc::default(),
            waker: Arc::default(),
            schedule,
            runs: 0,
        }
    }

    /// Reports each run as a job there (the jobs centre).
    pub fn report_to(&mut self, jobs: Jobs) {
        self.jobs = Some(jobs);
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
        self.runs
    }

    /// Whether a run is under way.
    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Each frame: starts a run when the schedule says one is due.
    pub fn tick(&mut self, now: Instant, volumes: Option<u64>) -> bool {
        self.schedule.due(now, volumes) && self.start()
    }

    /// "Check now": a run at once (none while one is under way).
    pub fn run_now(&mut self) -> bool {
        self.schedule.ran(Instant::now());
        self.start()
    }

    fn start(&mut self) -> bool {
        if self.running.swap(true, Ordering::SeqCst) {
            return false;
        }
        self.runs += 1;
        let report = self.jobs.as_ref().map(|jobs| {
            jobs.start(
                Start::new(&format!("health-{}", self.runs), "Health check", KIND)
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
                running.store(false, Ordering::SeqCst);
                if let Some(r) = &report {
                    r.progress(Progress::new().counts(checks.len() as u64, checks.len() as u64));
                    r.finished();
                }
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
            self.running.store(false, Ordering::SeqCst);
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
        let mut runner = Runner::new(
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
