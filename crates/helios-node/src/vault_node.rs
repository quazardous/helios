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
//! [`VaultNode::stop`] is the process's end (SIGTERM, Ctrl-C): the
//! volume watch stops, running backups are cancelled (the copy under way
//! is recorded first), a health run under way ends, then the vault is
//! flushed and marked closed cleanly.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_health::runner::{Runner, Schedule};
use helios_health::{Findings, Registry};
use helios_node_core::health::HealthInputs;
use helios_node_core::jobs::{Backups, Health};
use helios_node_core::node::{FLUSH_EVERY, Link, LocalNode};
use helios_node_core::volumes::VolumeWatch;
use helios_storage::Volumes;
use helios_vault::Vault;
use helios_vault_sqlite::SqliteStore;

/// How often the node looks whether a health run is due (the desktop's
/// pace).
pub const HEALTH_CLOCK: Duration = Duration::from_secs(10);

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
        let node = LocalNode::own(vault, bus, FLUSH_EVERY);
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

    /// The health check whose schedule the node keeps.
    pub fn health(&self) -> &Runner {
        self.health.runner()
    }

    /// Stops the node (the process ends): the running backups cancelled
    /// (their copy under way recorded), the health check's schedule
    /// stopped, then the vault flushed and marked closed cleanly. Waits
    /// for it.
    pub fn stop(self) {
        self.node.link().quit_cancelling_jobs();
        self.node.stop();
    }
}
