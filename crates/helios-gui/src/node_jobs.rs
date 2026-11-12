//! The backups and the health check as the local node's jobs
//! ([`crate::node::NodeJobs`], adopted with [`crate::node::Link::adopt`]),
//! beside the exports ([`crate::export`]).
//!
//! - [`Backups`] runs the library's backups (`helios_library::backup`) on
//!   threads of the node's, never a window's: what the strategy's triggers
//!   start (after an import, a backup disk plugged in) and what a window's
//!   command asks (`library.backupNow`). Each run is a `backup` job in the
//!   jobs API (the Queue panel). Closing the window that started one leaves
//!   it running; the quit question counts it; the node's stop waits for
//!   it, or cancels it and waits for it to stop (a run stops before its
//!   next photo) when the quit was confirmed as cancelling the jobs, so its
//!   last write reaches the vault before the vault closes.
//! - [`Health`] keeps the health check's schedule on a thread of the
//!   node's: a run a while after start, then daily, with or without a
//!   window, each run a `health` job in the jobs API. The windows hold the
//!   same runner ([`helios_health::runner::Runner`] is a handle) to show its
//!   findings and add their own triggers (a volume coming or going, "Check
//!   now"). The checks only read: a quit does not ask about them, and the
//!   node's stop lets a run under way end before the vault closes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helios_core::jobs::Jobs;
use helios_health::runner::Runner;
use helios_library::backup::{self, Gate};
use helios_library::import::Report as Imported;
use helios_storage::Volumes;
use helios_storage::strategy::{self, Scope, Trigger};
use helios_vault::{Store, Vault};

use crate::node::{Link, NodeJobs};

/// How long the node's stop waits for a cancelled backup to stop: it
/// stops before its next photo, so one copy at most.
const CANCEL_WAIT: Duration = Duration::from_secs(30);

/// Runs counted, and their ends heard.
#[derive(Default)]
struct Runs {
    count: Mutex<usize>,
    ended: Condvar,
}

impl Runs {
    fn add(&self, n: isize) {
        let mut count = self.count.lock().unwrap_or_else(PoisonError::into_inner);
        *count = count.saturating_add_signed(n);
        self.ended.notify_all();
    }

    fn wait(&self, deadline: Duration) -> bool {
        let count = self.count.lock().unwrap_or_else(PoisonError::into_inner);
        let (count, _) = self
            .ended
            .wait_timeout_while(count, deadline, |c| *c > 0)
            .unwrap_or_else(PoisonError::into_inner);
        *count == 0
    }
}

/// The library's backups, run by the node.
pub struct Backups<S: Store> {
    vault: Arc<Vault<S>>,
    jobs: Jobs,
    volumes: Arc<dyn Volumes>,
    /// One run at a time: a run asked while one goes on is dropped (the
    /// one going on catches up).
    gate: Gate,
    /// Set by the node's cancel: the runs stop before their next photo,
    /// and no run starts any more.
    cancel: Arc<AtomicBool>,
    runs: Runs,
    /// Where a run's error goes (the desktop's log and toasts).
    warn: Box<dyn Fn(String) + Send + Sync>,
}

impl<S: Store + 'static> Backups<S>
where
    Self: Send + Sync,
{
    /// Backups of `vault`'s library to the disks `volumes` sees, each run
    /// a job on `jobs`; a run's error goes to `warn`.
    pub fn new(
        vault: Arc<Vault<S>>,
        jobs: Jobs,
        volumes: Arc<dyn Volumes>,
        warn: impl Fn(String) + Send + Sync + 'static,
    ) -> Self {
        Self {
            vault,
            jobs,
            volumes,
            gate: Gate::new(),
            cancel: Arc::default(),
            runs: Runs::default(),
            warn: Box::new(warn),
        }
    }

    /// Hands the backups to the node: they become its jobs. The windows
    /// keep the handle returned, to start runs.
    pub fn adopted_by(self, node: &Link) -> Arc<Self> {
        let backups = Arc::new(self);
        node.adopt(backups.clone());
        backups
    }

    /// Whether the node cancelled the backups (it is stopping).
    pub fn cancelling(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Backs up, on a thread of the node's, what is below the library's
    /// strategy when it runs on `trigger`, to each destination plugged in
    /// (`helios_library::backup::run`); an import's report makes it the
    /// after-import trigger. Whether a run was started (none once the
    /// node cancelled its jobs).
    pub fn start(self: &Arc<Self>, trigger: Trigger, imported: Option<Imported>) -> bool {
        if self.cancelling() {
            return false;
        }
        self.runs.add(1);
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("helios-backup".into())
            .spawn(move || {
                let cancel = Some(me.cancel.clone());
                let mounted = me.volumes.as_ref();
                let ran = me.gate.run(|| match &imported {
                    Some(report) => backup::after_import(
                        &me.vault,
                        &me.jobs,
                        mounted,
                        report,
                        seconds(),
                        cancel,
                    ),
                    None => backup::run(&me.vault, &me.jobs, mounted, trigger, seconds(), cancel),
                });
                if let Some(Err(e)) = ran {
                    (me.warn)(format!("backup: {e}"));
                }
                me.runs.add(-1);
            });
        if let Err(e) = spawned {
            (self.warn)(format!("backup: no thread to run on: {e}"));
            self.runs.add(-1);
            return false;
        }
        true
    }

    /// "Back up now" (`library.backupNow`): the errors a person must see
    /// first (no strategy, no disk plugged in) come back at once; the
    /// copies run as the node's job.
    pub fn back_up_now(self: &Arc<Self>) -> Result<(), String> {
        let s = strategy::load(&self.vault, &Scope::Library)?
            .ok_or("no backup strategy set: choose one on the Storage page")?;
        let volumes = helios_storage::registry::merged(&self.vault, &self.volumes.volumes())?;
        if strategy::destinations_online(&s, &volumes).is_empty() {
            return Err("no backup disk of the strategy is plugged in".into());
        }
        if !self.start(Trigger::ByHand, None) {
            return Err("Helios is quitting: no backup starts now".into());
        }
        Ok(())
    }
}

impl<S: Store + 'static> NodeJobs for Backups<S>
where
    Self: Send + Sync,
{
    /// The backup jobs running (`helios_library::backup` names them
    /// `backup-<n>`).
    fn running(&self) -> Vec<String> {
        let prefix = format!("{}-", backup::KIND);
        self.jobs
            .cancellable()
            .into_iter()
            .filter(|id| id.starts_with(&prefix))
            .collect()
    }

    /// The runs stop before their next photo, and none starts any more;
    /// waits for them, so that their last write reaches the vault before
    /// it closes.
    fn cancel_all(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        for id in self.running() {
            // A job that just ended refuses: nothing to do.
            let _ = self.jobs.cancel(&id);
        }
        if !self.runs.wait(CANCEL_WAIT) {
            log::warn!(target: "helios::node", "a cancelled backup did not stop in time");
        }
    }

    fn wait(&self, deadline: Duration) -> bool {
        self.runs.wait(deadline)
    }
}

/// Registers `library.backupNow` on `shell`: a window's command, the
/// copies run by the node's `backups`.
pub fn register_back_up_now<S: Store + 'static>(
    shell: &mut crate::Shell,
    backups: &Arc<Backups<S>>,
) -> Result<(), String>
where
    Backups<S>: Send + Sync,
{
    let backups = backups.clone();
    shell.register(
        helios_shell::commands::CommandSpec::new(
            crate::shell::BACKUP_NOW_COMMAND,
            "Back up now: copy the photos short of the library's backup strategy to its disks plugged in, verified, as a job",
            "Library",
        ),
        move |_, _| {
            backups.back_up_now()?;
            Ok(serde_json::json!({ "queued": true }))
        },
    )
}

fn seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The health check's schedule, kept by the node.
pub struct Health {
    runner: Runner,
    /// Set when the node stops: the clock's thread ends.
    stop: Arc<(Mutex<bool>, Condvar)>,
}

impl Health {
    /// The node keeps `runner`'s schedule from now on, looking every
    /// `every` whether a run is due (each run a job on the jobs the runner
    /// reports to). The windows keep a clone of `runner` to show its
    /// findings.
    pub fn adopted_by(runner: Runner, node: &Link, every: Duration) -> Arc<Self> {
        let stop: Arc<(Mutex<bool>, Condvar)> = Arc::default();
        let clock = (runner.clone(), stop.clone());
        let spawned = std::thread::Builder::new()
            .name("helios-health-clock".into())
            .spawn(move || {
                let (runner, stop) = clock;
                let (stopped, woken) = &*stop;
                let mut stopped = stopped.lock().unwrap_or_else(PoisonError::into_inner);
                while !*stopped {
                    runner.tick(Instant::now(), None);
                    stopped = woken
                        .wait_timeout_while(stopped, every, |s| !*s)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            });
        if let Err(e) = spawned {
            log::warn!(target: "helios::node", "health: no thread for its schedule: {e}");
        }
        let health = Arc::new(Self { runner, stop });
        node.adopt(health.clone());
        health
    }

    /// The runner, shared with the windows.
    pub fn runner(&self) -> &Runner {
        &self.runner
    }

    /// No run starts any more: the clock stops, and so does the runner.
    fn halt(&self) {
        self.runner.stop();
        let (stopped, woken) = &*self.stop;
        *stopped.lock().unwrap_or_else(PoisonError::into_inner) = true;
        woken.notify_all();
    }
}

impl NodeJobs for Health {
    /// None a quit would cancel: the checks only read.
    fn running(&self) -> Vec<String> {
        Vec::new()
    }

    /// The schedule stops; a run under way, which only reads, ends before
    /// the vault closes.
    fn cancel_all(&self) {
        self.halt();
        let _ = self.runner.wait(CANCEL_WAIT);
    }

    fn wait(&self, deadline: Duration) -> bool {
        // Only the node's stop waits: no run starts after it.
        self.halt();
        self.runner.wait(deadline)
    }
}
