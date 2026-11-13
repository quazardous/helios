//! Running the checks: a background job (`health-<n>`, kind `health`)
//! runs the checks due on a thread of its own and keeps their findings,
//! per check. [`Schedule`] says which are due: every check a while after
//! start (never slowing it), then each as its declaration says: every
//! [`Declaration::every_minutes`](crate::Declaration::every_minutes)
//! (the schedule's own period when it says nothing: daily) and at once
//! on the [`Event`]s it names (a volume coming or going).
//!
//! A [`Runner`] is a handle: its clones share one schedule, one run at a
//! time and one set of findings, so the node that schedules the checks
//! and the windows that show them hold the same runner.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use helios_core::jobs::{Jobs, Progress, Start};

use crate::{Context, Event, Findings, Registry};

/// The job's kind, in the jobs centre.
pub const KIND: &str = "health";

/// When each check runs: all of them once `delay` after start, then each
/// every period it declares (`every` when it declares none), and at once
/// on the events it names (the volumes' generation changing: a volume
/// came or went).
#[derive(Clone, Debug)]
pub struct Schedule {
    started: Instant,
    delay: Duration,
    every: Duration,
    /// When each check last ran, by id.
    last: HashMap<String, Instant>,
    /// The checks an event asked for, not run yet (a run was under way).
    pending: BTreeSet<String>,
    volumes: Option<u64>,
}

impl Schedule {
    /// The desktop's: a first run 30 s after start, then daily.
    pub fn desktop(now: Instant) -> Self {
        Self::new(now, Duration::from_secs(30), Duration::from_hours(24))
    }

    /// A first run `delay` after `now`, then every `every` for the checks
    /// that declare no period.
    pub fn new(now: Instant, delay: Duration, every: Duration) -> Self {
        Self {
            started: now,
            delay,
            every,
            last: HashMap::new(),
            pending: BTreeSet::new(),
            volumes: None,
        }
    }

    /// The ids of `registry`'s checks due at `now`, the volumes'
    /// generation being `volumes` (none without a watch). They count as
    /// made only once [`Schedule::ran`] says so.
    pub fn due(&mut self, now: Instant, volumes: Option<u64>, registry: &Registry) -> Vec<String> {
        let moved = volumes.is_some() && self.volumes.is_some() && volumes != self.volumes;
        if volumes.is_some() {
            self.volumes = volumes;
        }
        let first = now >= self.started + self.delay;
        let mut due = Vec::new();
        for check in registry.checks() {
            let id = check.id();
            let declared = check.declaration();
            let Some(last) = self.last.get(id) else {
                // Never ran: with the first run (an event before it
                // changes nothing, the first run reads it anyway).
                if first {
                    due.push(id.to_owned());
                }
                continue;
            };
            if moved && declared.on.contains(&Event::Volumes) {
                self.pending.insert(id.to_owned());
            }
            if self.pending.contains(id) || now >= *last + declared.period().unwrap_or(self.every) {
                due.push(id.to_owned());
            }
        }
        due
    }

    /// Counts a run of the checks `ids` made at `now`.
    pub fn ran(&mut self, ids: &[String], now: Instant) {
        for id in ids {
            self.pending.remove(id);
            self.last.insert(id.clone(), now);
        }
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

    /// Starts a run of the checks the schedule says are due, if any
    /// (each frame of a window, and the node's own clock); whether one
    /// started. Checks due while a run is under way wait for the next
    /// tick.
    pub fn tick(&self, now: Instant, volumes: Option<u64>) -> bool {
        let mut schedule = self.schedule.lock().unwrap_or_else(PoisonError::into_inner);
        let due = schedule.due(now, volumes, &self.registry);
        if due.is_empty() || !self.start(Some(&due)) {
            return false;
        }
        schedule.ran(&due, now);
        true
    }

    /// "Check now": every check at once (none while a run is under way).
    pub fn run_now(&self) -> bool {
        if !self.start(None) {
            return false;
        }
        let all: Vec<String> = self
            .registry
            .checks()
            .iter()
            .map(|c| c.id().to_owned())
            .collect();
        self.schedule
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .ran(&all, Instant::now());
        true
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

    /// Starts a run of the checks `only` names (all of them for none);
    /// false when one is under way, or none starts any more.
    fn start(&self, only: Option<&[String]>) -> bool {
        if self.stopped.load(Ordering::SeqCst) || !self.running.take() {
            return false;
        }
        let runs = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
        let checks: Vec<(String, Arc<dyn crate::Check>)> = self
            .registry
            .owned()
            .filter(|(_, c)| only.is_none_or(|ids| ids.iter().any(|id| id == c.id())))
            .map(|(owner, c)| (owner.to_owned(), c.clone()))
            .collect();
        let whole = only.is_none();
        let ids: BTreeSet<String> = self
            .registry
            .checks()
            .iter()
            .map(|c| c.id().to_owned())
            .collect();
        let jobs = self
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let report = jobs.as_ref().map(|jobs| {
            jobs.start(
                Start::new(&format!("health-{runs}"), "Health check", KIND)
                    .total(checks.len() as u64)
                    .not_cancellable(),
            )
        });
        let (context, findings, running, waker) = (
            self.context.clone(),
            self.findings.clone(),
            self.running.clone(),
            self.waker.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("helios-health".into())
            .spawn(move || {
                let ctx = context();
                let total = checks.len() as u64;
                let mut found = Vec::with_capacity(checks.len());
                for (i, (owner, check)) in checks.iter().enumerate() {
                    if let Some(r) = &report {
                        r.progress(Progress::new().counts(i as u64, total).item(check.id()));
                    }
                    let these = crate::run_one(check, &ctx);
                    if let Some(r) = &report {
                        r.log(format!("{}: {} findings", check.id(), these.len()));
                    }
                    found.push((owner, check.id(), these));
                }
                {
                    let mut kept = findings.lock().unwrap_or_else(PoisonError::into_inner);
                    for (owner, check, these) in found {
                        kept.put(owner, check, these);
                    }
                    if whole {
                        kept.retain(|check| ids.contains(check));
                    }
                }
                if let Some(r) = &report {
                    r.progress(Progress::new().counts(total, total));
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
    use std::sync::atomic::AtomicUsize;

    use helios_core::Message;

    use super::*;
    use crate::{Check, Declaration, Finding, Level};

    /// A fake module's check: declared as `declaration` says, it counts
    /// its runs and finds one ok finding saying how many.
    struct Counted {
        id: &'static str,
        declaration: Declaration,
        runs: Arc<AtomicUsize>,
    }

    impl Check for Counted {
        fn id(&self) -> &str {
            self.id
        }
        fn title(&self) -> Message {
            Message::plain(self.id)
        }
        fn declaration(&self) -> Declaration {
            self.declaration.clone()
        }
        fn run(&self, _: &Context) -> Vec<Finding> {
            let n = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
            vec![Finding::new(
                self.id,
                "",
                Level::Ok,
                Message::plain(format!("run {n}")),
                Message::plain(""),
            )]
        }
    }

    /// A periodic check (every 10 minutes), an event one (on a volume
    /// change) and one declaring neither, each owned by its fake module,
    /// with their run counters.
    fn fakes() -> (Registry, [Arc<AtomicUsize>; 3]) {
        let runs: [Arc<AtomicUsize>; 3] = Default::default();
        let declared = Declaration::new("A fake module's state").levels(&[Level::Ok]);
        let mut r = Registry::default();
        for (owner, id, declaration, runs) in [
            ("clock", "clock.tick", declared.clone().every(10), &runs[0]),
            (
                "disks",
                "disks.seen",
                declared.clone().on(&[Event::Volumes]),
                &runs[1],
            ),
            ("plain", "plain.daily", declared, &runs[2]),
        ] {
            r.add(
                owner,
                Arc::new(Counted {
                    id,
                    declaration,
                    runs: runs.clone(),
                }),
            )
            .unwrap();
        }
        (r, runs)
    }

    #[test]
    fn the_first_run_waits_then_each_check_runs_by_its_declaration() {
        let (registry, _) = fakes();
        let t0 = Instant::now();
        let at = |secs| t0 + Duration::from_secs(secs);
        let mut sch = Schedule::new(t0, Duration::from_secs(30), Duration::from_hours(24));
        let due = |sch: &mut Schedule, secs, volumes| {
            let due = sch.due(at(secs), volumes, &registry);
            sch.ran(&due, at(secs));
            due
        };
        assert!(due(&mut sch, 1, Some(0)).is_empty(), "never at start");
        assert_eq!(
            due(&mut sch, 30, Some(0)),
            ["clock.tick", "disks.seen", "plain.daily"]
        );
        assert!(due(&mut sch, 31, Some(0)).is_empty());
        assert_eq!(due(&mut sch, 40, Some(1)), ["disks.seen"], "a volume came");
        assert!(due(&mut sch, 41, Some(1)).is_empty());
        assert_eq!(
            due(&mut sch, 30 + 600, Some(1)),
            ["clock.tick"],
            "its period"
        );
        assert_eq!(
            due(&mut sch, 40 + 86_400, Some(1)),
            ["clock.tick", "disks.seen", "plain.daily"],
            "daily for the others"
        );
        // Due but not run (a run under way): still due at the next tick.
        let asked = sch.due(at(86_500), Some(2), &registry);
        assert_eq!(asked, ["disks.seen"]);
        assert_eq!(sch.due(at(86_501), Some(2), &registry), ["disks.seen"]);
        // Without a watch, only the clock.
        let mut sch = Schedule::new(t0, Duration::ZERO, Duration::from_hours(1));
        assert_eq!(due(&mut sch, 0, None).len(), 3);
        assert!(due(&mut sch, 5, None).is_empty());
    }

    #[test]
    fn a_fake_modules_periodic_and_event_checks_run_when_they_should() {
        let (registry, runs) = fakes();
        let t0 = Instant::now();
        let at = |secs| t0 + Duration::from_secs(secs);
        let runner = Runner::new(
            registry,
            Context::default,
            Findings::default(),
            Schedule::new(t0, Duration::ZERO, Duration::from_hours(24)),
        );
        let counts = || runs.each_ref().map(|r| r.load(Ordering::SeqCst));
        let tick = |secs, volumes| {
            let started = runner.tick(at(secs), volumes);
            assert!(runner.wait(Duration::from_secs(10)), "the run ends");
            started
        };
        assert!(tick(0, Some(0)), "the first run");
        assert_eq!(counts(), [1, 1, 1]);
        assert!(!tick(60, Some(0)), "nothing due");
        assert!(tick(120, Some(1)), "a volume came");
        assert_eq!(counts(), [1, 2, 1], "the event check only");
        assert!(tick(600, Some(1)));
        assert_eq!(counts(), [2, 2, 1], "the periodic check only");
        // Each source keeps its own findings: the event check's second
        // run is still there after the periodic one's.
        {
            let findings = runner.findings();
            let said: Vec<(&str, &str)> = findings
                .sources()
                .iter()
                .map(|s| (s.owner.as_str(), s.findings[0].message.english.as_str()))
                .collect();
            assert_eq!(
                said,
                [("clock", "run 2"), ("disks", "run 2"), ("plain", "run 1")]
            );
        }
        assert!(tick(86_400, Some(1)));
        assert_eq!(counts(), [3, 3, 2], "a day later, all of them");
        assert!(runner.run_now(), "check now");
        assert!(runner.wait(Duration::from_secs(10)));
        assert_eq!(counts(), [4, 4, 3], "check now runs every check");
        assert_eq!(runner.runs(), 5);
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
