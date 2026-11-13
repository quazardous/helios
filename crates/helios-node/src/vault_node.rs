//! The node of `helios-node --vault`: one [`LocalNode`] owns the vault
//! (the same node the desktop embeds, `helios_node_core`), and the
//! process's other users of it (the settings' layers, the edit, the
//! workflow runs) borrow its handle instead of opening the file again.
//!
//! As it opens, the node starts the work that outlives a window, as the
//! desktop's does: it watches the machine's volumes, runs the backups
//! (a backup disk of the strategy plugged in starts one) and keeps the
//! health check's schedule, each run a job on the node's jobs (`job.*`
//! on its bus). Without a window, the check's inputs keep a windowless
//! node's defaults ([`HealthInputs::new`]: no cards, no GPU, no plugins).
//!
//! Imports done through the node ([`Importer`], the control endpoint's
//! `library.import`) publish their report on its bus
//! ([`Imported`], `library.imported`); the node hears it and starts the
//! strategy's after-import backup, as the desktop does after its imports.
//!
//! [`VaultNode::stop`] is the process's end (SIGTERM, Ctrl-C): the
//! volume watch stops, running backups are cancelled (the copy under way
//! is recorded first), a health run under way ends, then the vault is
//! flushed and marked closed cleanly.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use helios_core::jobs::Jobs;
use helios_core::{Bus, Envelope, Event};
use helios_health::runner::{Runner, Schedule};
use helios_health::{Findings, Registry};
use helios_library::import::{self, Report};
use helios_node_core::health::HealthInputs;
use helios_node_core::jobs::{Backups, Health};
use helios_node_core::node::{FLUSH_EVERY, Link, LocalNode};
use helios_node_core::settings::NodeSettings;
use helios_node_core::volumes::VolumeWatch;
use helios_storage::Volumes;
use helios_storage::strategy::Trigger;
use helios_vault::Vault;
use helios_vault_sqlite::SqliteStore;
use serde::Serialize;
use serde_json::Value;

/// How often the node looks whether a health run is due (the desktop's
/// pace).
pub const HEALTH_CLOCK: Duration = Duration::from_secs(10);

/// Who publishes the node's own events on its bus.
pub const SOURCE: &str = "node";

/// An import done through the node ended: its report (the files it
/// added, located, whether it was cancelled...). The node's after-import
/// backup starts on it.
#[derive(Serialize)]
pub struct Imported<'a> {
    #[serde(flatten)]
    pub report: &'a Report,
}

impl Event for Imported<'_> {
    const KIND: &'static str = "library.imported";
}

/// What the after-import backup needs of an [`Imported`] event's
/// payload: what came in, and whether the import was cancelled.
fn imported(payload: &Value) -> Report {
    let count = |key: &str| payload[key].as_u64().unwrap_or(0);
    Report {
        added: count("added"),
        located: count("located"),
        cancelled: payload["cancelled"].as_bool().unwrap_or(false),
        ..Report::default()
    }
}

/// Imports into the node's vault, each announced on its bus
/// ([`Imported`]). A handle: the control endpoint keeps one.
#[derive(Clone)]
pub struct Importer {
    vault: Arc<Vault<SqliteStore>>,
    jobs: Jobs,
    bus: Arc<Bus>,
    link: Link,
}

impl Importer {
    /// The node's vault, for its library commands ([`crate::library`]).
    pub fn vault(&self) -> &Arc<Vault<SqliteStore>> {
        &self.vault
    }

    /// The node's bus.
    pub fn bus(&self) -> &Arc<Bus> {
        &self.bus
    }

    /// Imports `sources` (`helios_library::import`), the run a job on the
    /// node's jobs, each file's line to `emit`; then announces the report.
    /// Refused once the node is stopping.
    pub fn import(
        &self,
        sources: &[PathBuf],
        options: &import::Options,
        emit: &mut impl FnMut(&Value),
    ) -> Result<Report, String> {
        if self.link.quitting() || self.link.stopped() {
            return Err("the node is stopping: no import starts now".into());
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let report = import::import(&self.vault, &self.jobs, sources, options, now, emit)?;
        self.bus
            .publish(SOURCE, None, &Imported { report: &report });
        Ok(report)
    }
}

/// Hears the imports announced on the bus and starts the after-import
/// backup of each, until stopped.
struct AfterImports {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AfterImports {
    /// How often the listener looks whether it must stop.
    const PACE: Duration = Duration::from_millis(100);

    fn start(heard: Receiver<Envelope>, backups: &Arc<Backups<SqliteStore>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (stopped, backups) = (stop.clone(), Arc::downgrade(backups));
        let thread = std::thread::Builder::new()
            .name("helios-after-import".into())
            .spawn(move || {
                while !stopped.load(Ordering::SeqCst) {
                    match heard.recv_timeout(Self::PACE) {
                        Ok(e) if e.kind == Imported::KIND => {
                            if let Some(backups) = backups.upgrade() {
                                backups.start(Trigger::OnImport, Some(imported(&e.payload)));
                            }
                        }
                        Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .map_err(|e| eprintln!("helios-node: no thread for the after-import backups: {e}"))
            .ok();
        Self { stop, thread }
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the node watches and when its health check runs.
pub struct Options {
    /// The volumes of the machine (`helios_storage::system()`).
    pub volumes: Arc<dyn Volumes>,
    /// When the health check runs.
    pub schedule: Schedule,
    /// How often a due health run is looked for.
    pub clock: Duration,
}

impl Options {
    /// The machine's volumes, the desktop's schedule (a run a while after
    /// start, then daily).
    pub fn system() -> Self {
        Self {
            volumes: helios_storage::system(),
            schedule: Schedule::desktop(Instant::now()),
            clock: HEALTH_CLOCK,
        }
    }
}

/// The vault's owner in `helios-node`, with its jobs running.
pub struct VaultNode {
    node: LocalNode<SqliteStore>,
    watch: VolumeWatch,
    backups: Arc<Backups<SqliteStore>>,
    health: Arc<Health>,
    importer: Importer,
    after_imports: AfterImports,
}

impl VaultNode {
    /// Takes `vault` (opened on `file`, its settings already read),
    /// announcing its changes on `bus`, and starts the node's jobs, each
    /// run reported on `jobs`.
    pub fn start(
        vault: Vault<SqliteStore>,
        file: &Path,
        bus: Arc<Bus>,
        jobs: &Jobs,
        options: Options,
    ) -> Self {
        // Heard from now on: no import announced before the listener runs.
        let heard = bus.subscribe();
        let node = LocalNode::own(vault, bus.clone(), FLUSH_EVERY);
        let link = node.link();
        let watch = node.watch_volumes(options.volumes.clone());
        let backups = Backups::new(
            node.vault().clone(),
            jobs.clone(),
            options.volumes.clone(),
            |e| eprintln!("helios-node: {e}"),
        )
        .adopted_by(&link);
        backups.follow(&watch);
        let after_imports = AfterImports::start(heard, &backups);
        let importer = Importer {
            vault: node.vault().clone(),
            jobs: jobs.clone(),
            bus,
            link: link.clone(),
        };
        let inputs = HealthInputs::new(
            options.volumes,
            Some(node.vault().clone()),
            Some(file.to_path_buf()),
        );
        let runner = Runner::new(
            Registry::builtin(),
            inputs.into_context(),
            Findings::default(),
            options.schedule,
        );
        runner.report_to(jobs.clone());
        let health = Health::adopted_by(runner, &link, options.clock);
        Self {
            node,
            watch,
            backups,
            health,
            importer,
            after_imports,
        }
    }

    /// The vault the node owns, for the process's users of it.
    pub fn vault(&self) -> &Arc<Vault<SqliteStore>> {
        self.node.vault()
    }

    /// The node's life: its jobs, whether it stopped.
    pub fn link(&self) -> Link {
        self.node.link()
    }

    /// The machine's volumes, as the node watches them.
    pub fn watch(&self) -> &VolumeWatch {
        &self.watch
    }

    /// The backups the node runs.
    pub fn backups(&self) -> &Arc<Backups<SqliteStore>> {
        &self.backups
    }

    /// Imports into the vault, each followed by the strategy's
    /// after-import backup.
    pub fn importer(&self) -> &Importer {
        &self.importer
    }

    /// The settings layers the node keeps (`user`, `project/<folder>`),
    /// each change announced on its bus.
    pub fn settings(&self) -> NodeSettings<SqliteStore> {
        self.node.settings()
    }

    /// The health check whose schedule the node keeps.
    pub fn health(&self) -> &Runner {
        self.health.runner()
    }

    /// Stops the node (the process ends): no import starts a backup any
    /// more, the running backups cancelled
    /// (their copy under way recorded), the health check's schedule
    /// stopped, then the vault flushed and marked closed cleanly. Waits
    /// for it.
    pub fn stop(mut self) {
        self.after_imports.stop();
        self.node.link().quit_cancelling_jobs();
        self.node.stop();
    }
}
