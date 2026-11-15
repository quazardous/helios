//! The Settings app's reads through egui-pop's read kernel
//! (`egui_pop::api::reads`): the Storage page's "In use" (`cache.stats`)
//! and "Volumes" (`library.volumes`, `library.backupStrategy`) are read
//! away from the frame, kept by command and arguments, shown at once
//! from what was last read and read again in the background; a refresh
//! never blanks them.
//!
//! - The transport ([`RegistryReads`]) calls the read-only command's own
//!   handler from Helios's registry on the kernel's thread, so the
//!   command stays the one place its answer is written (agents and the
//!   palette run the same).
//! - Each read's policy ([`POLICIES`]): how long it stays fresh, and
//!   which node events make it out of date.
//! - Invalidation: the node-event relay's listener `reads` (on
//!   `storage.*`, `library.*`, `volume.*` and `cache.*`) hands each node
//!   fact to the kernel at every frame; `node.resync` (a hole in a node's
//!   events) makes every value out of date. The storage link's own
//!   changes (an edit saved, a read after a scan) reach it as
//!   `storage.volumes.changed` ([`Shell::read_fact`]).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use egui_pop::api::reads::{ReadKernel, ReadPolicy, ReadTransport};
use helios_shell::commands::{Origin, Registry};
use serde_json::Value;

use super::Shell;

/// The reads the kernel serves: each command, how long its value stays
/// fresh (`None`: until an event), and the events that make it out of
/// date.
pub const POLICIES: &[(&str, Option<Duration>, &[&str])] = &[
    // The caches' sizes move all the time: short, read again while shown.
    (
        crate::caches::STATS_COMMAND,
        Some(Duration::from_secs(5)),
        &["cache.*"],
    ),
    // The volumes: free space read again now and then; a disk coming or
    // going, a scan or an edit says so at once.
    (
        super::storage_admin::VOLUMES,
        Some(Duration::from_secs(30)),
        &["storage.*", "library.*"],
    ),
    (
        super::storage_admin::STRATEGY,
        Some(Duration::from_secs(30)),
        &["storage.*", "library.*"],
    ),
];

/// Reads a read-only command through its handler in Helios's registry,
/// held only to take the handler.
pub struct RegistryReads {
    registry: Arc<Mutex<Registry>>,
}

impl RegistryReads {
    pub fn new(registry: Arc<Mutex<Registry>>) -> Self {
        Self { registry }
    }
}

impl ReadTransport for RegistryReads {
    fn serves(&self, command: &str) -> bool {
        POLICIES.iter().any(|(c, _, _)| *c == command)
    }

    fn read(&self, command: &str, args: &Value) -> Result<Value, String> {
        let handler = self
            .registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .reader(command)
            .ok_or_else(|| format!("no read-only command `{command}`"))?;
        handler(args, &Origin::User)
    }
}

/// The kernel over `registry`, with [`POLICIES`].
pub fn kernel(registry: Arc<Mutex<Registry>>) -> Arc<ReadKernel> {
    let k = ReadKernel::new(Arc::new(RegistryReads::new(registry)));
    for (command, fresh, events) in POLICIES {
        let mut p = ReadPolicy::new().invalidated_by(events);
        p.fresh_for = *fresh;
        k.policy(command, p);
    }
    Arc::new(k)
}

/// The node facts the reads listen to.
pub const LISTENS: &[&str] = &["storage.*", "library.*", "volume.*", "cache.*"];

/// The fact the shell gives the kernel when the storage link read
/// something new (a disk came or went, a scan, an edit saved).
pub const VOLUMES_CHANGED: &str = "storage.volumes.changed";

impl Shell {
    /// Attaches the read kernel to the Settings app, once.
    pub(super) fn attach_reads(&mut self) {
        let Some(admin) = self.core.app.handle().service(&egui_pop::api::admin::ADMIN) else {
            return;
        };
        if admin.reads().is_none() {
            admin.attach_reads(kernel(self.core.registry.clone()));
        }
        if self.core.storage.reads.is_none() {
            self.core.storage.reads = Some(self.core.node_events.listen("reads", LISTENS));
        }
    }

    /// Each frame: the node facts heard since reach the kernel.
    pub(super) fn reads_frame(&self) {
        let Some(listener) = &self.core.storage.reads else {
            return;
        };
        let Some(k) = self.reads() else {
            return;
        };
        for e in listener.try_iter() {
            if e.is_resync() {
                k.invalidate_all();
            } else {
                k.event(e.event.kind);
            }
        }
    }

    /// A fact the reads follow (`storage.volumes.changed`): the values
    /// it makes out of date are read again, shown meanwhile. The
    /// adapter the node-event relay's listener will call.
    pub fn read_fact(&self, name: &str) {
        if let Some(k) = self
            .core
            .app
            .handle()
            .service(&egui_pop::api::admin::ADMIN)
            .and_then(|a| a.reads())
        {
            k.event(name);
        }
    }

    /// The read kernel the Settings app reads through.
    pub fn reads(&self) -> Option<Arc<ReadKernel>> {
        self.core
            .app
            .handle()
            .service(&egui_pop::api::admin::ADMIN)
            .and_then(|a| a.reads())
    }
}
