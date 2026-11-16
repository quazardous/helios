//! The health checks the node composes: each module adds its own, over
//! the state it reads itself (the health module lists none). The node
//! holds what they read from: the volumes mounted, the vault and its
//! database file, whose folder stands for the library's place, read once
//! per run and shared ([`HealthInputs`]); what only the desktop knows (a
//! setting, the cards an import left short, the catalogue's snapshots) is
//! handed to the module's `register` by the desktop.

pub mod snapshots;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_health::{Gather, Registry};
use helios_library::health::Database;
use helios_storage::health::backup::{Backup, CardShort, DESTINATION_DAYS};
use helios_storage::{Volume, Volumes};
use helios_vault::{Store, Vault};

use snapshots::{Catalogue, Snapshots};

/// The catalogue's snapshots of the vault's file, beside it and on the
/// volumes mounted, at a time in seconds since the epoch.
pub type SnapshotsOf = Box<dyn Fn(&Path, &[Volume], u64) -> Snapshots + Send + Sync>;

/// What the node's modules read their health from: the `volumes` of this
/// machine, the `vault` and its database `file` (each optional). The
/// volumes mounted are read once per run, shared by the modules.
pub struct HealthInputs<S: Store + 'static> {
    vault: Option<Arc<Vault<S>>>,
    file: Option<PathBuf>,
    mounted: Gather<Vec<Volume>>,
    disks: Gather<helios_storage::health::Disks>,
}

impl<S: Store + 'static> HealthInputs<S> {
    /// Over the `volumes` of this machine, the `vault` and its database
    /// `file` (each optional).
    pub fn new(
        volumes: Arc<dyn Volumes>,
        vault: Option<Arc<Vault<S>>>,
        file: Option<PathBuf>,
    ) -> Self {
        let file = file.and_then(|f| f.canonicalize().ok());
        let mounted = Gather::new(move |_| volumes.volumes());
        let disks = {
            let (mounted, vault) = (mounted.clone(), vault.clone());
            let library = file
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf);
            Gather::new(move |ctx| {
                helios_storage::health::disks(
                    mounted.read(ctx).to_vec(),
                    vault.as_deref(),
                    library.as_deref(),
                )
            })
        };
        Self {
            vault,
            file,
            mounted,
            disks,
        }
    }

    /// The node's own checks, with what a node without a window has: the
    /// storage, the library, the backups (the default destination days, no
    /// cards) and the catalogue (no snapshots yet), their listeners on
    /// `bus`, the node's.
    pub fn checks(&self, bus: Arc<dyn bus_contract::Bus>) -> Registry {
        let mut r = Registry::new(bus);
        self.register_storage(&mut r);
        self.register_library(&mut r);
        self.register_backup(&mut r, || DESTINATION_DAYS, Vec::new);
        self.register_catalogue(&mut r, None);
        r
    }

    /// The storage module's checks: the free space where Helios writes.
    pub fn register_storage(&self, r: &mut Registry) {
        helios_storage::health::register(r, &self.disks);
    }

    /// The library module's checks, over the vault, the volumes and what
    /// the SQLite driver reads of the vault's file (it opens, its
    /// integrity).
    pub fn register_library(&self, r: &mut Registry) {
        let (disks, vault, file) = (self.disks.clone(), self.vault.clone(), self.file.clone());
        let source = Gather::new(move |ctx| {
            let file = file.as_deref();
            helios_library::health::read(
                vault.as_deref(),
                disks.read(ctx).volumes.clone(),
                file,
                file.map(|f| Database {
                    opens: helios_vault_sqlite::health::opens(f),
                    problems: helios_vault_sqlite::health::quick_check(f),
                }),
            )
        });
        helios_library::health::register(r, &source);
    }

    /// The backup module's checks, over the vault's backups, the
    /// `health.destinationDays` setting and the cards an import left short,
    /// still inserted (each read at each run).
    pub fn register_backup(
        &self,
        r: &mut Registry,
        destination_days: impl Fn() -> u64 + Send + Sync + 'static,
        cards: impl Fn() -> Vec<CardShort> + Send + Sync + 'static,
    ) {
        let (mounted, vault) = (self.mounted.clone(), self.vault.clone());
        let source = Gather::new(move |ctx| {
            let vault = vault.as_deref()?;
            let mut b = Backup::read(vault, &mounted.read(ctx), now())
                .inspect_err(|e| log::warn!("health: backups: {e}"))
                .ok()?;
            b.destination_days = destination_days();
            b.cards = cards();
            Some(b)
        });
        helios_storage::health::backup::register(r, &source);
    }

    /// The catalogue's check, over the snapshots `snapshots` finds (none:
    /// there are none yet).
    pub fn register_catalogue(&self, r: &mut Registry, snapshots: Option<SnapshotsOf>) {
        let (mounted, file) = (self.mounted.clone(), self.file.clone());
        let source = Gather::new(move |ctx| Catalogue {
            vault: file.clone(),
            snapshots: file
                .as_deref()
                .zip(snapshots.as_ref())
                .map(|(f, snapshots)| snapshots(f, &mounted.read(ctx), now())),
        });
        snapshots::register(r, &source);
    }
}

/// Now, in seconds since the epoch.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_health::runner::{Runner, Schedule};
    use helios_health::{Context, Findings, Level};
    use helios_vault_sqlite::SqliteStore;
    use std::time::{Duration, Instant};

    /// A bus of its own, as an application gives one.
    fn bus() -> Arc<dyn bus_contract::Bus> {
        Arc::new(egui_pop_hooks::Kernel::new())
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("helios-node-health-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The node's checks, each owned by its module, declared soundly.
    #[test]
    fn the_node_s_checks_are_declared_soundly_and_owned_by_their_modules() {
        let volumes: Arc<dyn Volumes> = Arc::new(Vec::<Volume>::new());
        let r = HealthInputs::<SqliteStore>::new(volumes, None, None).checks(bus());
        let owners: Vec<(&str, Vec<&str>)> = r
            .by_owner()
            .into_iter()
            .map(|(o, checks)| (o, checks.iter().map(|c| c.id()).collect()))
            .collect();
        assert_eq!(
            owners,
            [
                ("storage", vec!["storage.free-space"]),
                (
                    "library",
                    vec![
                        "vault.integrity",
                        "library.default",
                        "library.folder",
                        "library.changed-outside",
                        "library.raw-jpeg-pairs"
                    ]
                ),
                (
                    "backup",
                    vec![
                        "backup.strategy",
                        "backup.met",
                        "backup.destinations-seen",
                        "backup.unverified",
                        "backup.cards"
                    ]
                ),
                ("catalogue", vec!["catalogue.snapshots"]),
            ]
        );
        for check in r.checks() {
            assert_eq!(
                check.declaration().problems(check.id()),
                Vec::<String>::new()
            );
        }
    }

    /// With no window, the node's inputs and what the desktop would hand
    /// the backups (the days, the cards) reach the modules' checks, and a
    /// run of them comes through.
    #[test]
    fn a_health_check_runs_over_inputs_filled_by_hand() {
        let dir = scratch("inputs");
        let file = dir.join("vault.db");
        let vault = Arc::new(Vault::new(SqliteStore::open(&file).unwrap()));
        let volumes: Arc<dyn Volumes> = Arc::new(Vec::<Volume>::new());
        let inputs = HealthInputs::new(volumes, Some(vault), Some(file));
        let mut registry = Registry::new(bus());
        inputs.register_storage(&mut registry);
        inputs.register_library(&mut registry);
        inputs.register_backup(
            &mut registry,
            || 3,
            || {
                vec![CardShort {
                    label: "CARD".into(),
                    short: 2,
                }]
            },
        );
        inputs.register_catalogue(&mut registry, None);

        let found = registry.run(&Context::default());
        let level = |id: &str| found.iter().find(|f| f.id == id).map(|f| f.level);
        assert_eq!(level("vault.integrity"), Some(Level::Ok), "{found:?}");
        assert_eq!(
            level("backup.cards/CARD"),
            Some(Level::Warning),
            "{found:?}"
        );
        assert_eq!(
            level("catalogue.snapshots"),
            Some(Level::Ok),
            "none yet: {found:?}"
        );
        assert_eq!(level("library.changed-outside"), None, "none changed");

        let runner = Runner::new(
            registry,
            Context::default,
            Findings::default(),
            Schedule::desktop(Instant::now()),
        );
        assert!(runner.run_now(), "a run starts");
        assert!(runner.wait(Duration::from_secs(30)), "the run ends in 30 s");
        assert!(runner.findings().ran(), "the run's findings are in");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fresh library has no library folder chosen: a warning whose fix
    /// chooses one; once a folder is set, the next run says it is chosen.
    #[test]
    fn a_fresh_library_warns_until_a_folder_is_chosen() {
        use helios_library::health::{CHOOSE_LIBRARY_FOLDER, FOLDER_CHOSEN};

        let dir = scratch("folder");
        let file = dir.join("vault.db");
        let vault = Arc::new(Vault::new(SqliteStore::open(&file).unwrap()));
        let volumes: Arc<dyn Volumes> = Arc::new(Vec::<Volume>::new());
        let inputs = HealthInputs::new(volumes, Some(vault.clone()), Some(file));
        let mut registry = Registry::new(bus());
        inputs.register_library(&mut registry);
        let check = || {
            registry
                .run(&Context::default())
                .into_iter()
                .filter(|f| f.id == FOLDER_CHOSEN)
                .collect::<Vec<_>>()
        };

        let found = check();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].level, Level::Warning);
        assert_eq!(
            found[0].fix.as_ref().map(|f| f.command.as_str()),
            Some(CHOOSE_LIBRARY_FOLDER)
        );

        let photos = dir.join("Photos");
        std::fs::create_dir_all(&photos).unwrap();
        helios_library::import::set_library_root(&vault, &photos).unwrap();
        let found = check();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].level, Level::Ok);
        assert!(found[0].fix.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A vault file that is not a database is read by the driver and
    /// reported by the integrity and library checks as a problem.
    #[test]
    fn a_corrupt_vault_is_reported() {
        use helios_library::health::{LIBRARY, VAULT_INTEGRITY};

        let dir = scratch("corrupt");
        let file = dir.join("vault.db");
        std::fs::write(&file, vec![0x5a; 8192]).unwrap();
        let volumes: Arc<dyn Volumes> = Arc::new(Vec::<Volume>::new());
        let inputs = HealthInputs::<SqliteStore>::new(volumes, None, Some(file));
        let found: Vec<_> = inputs
            .checks(bus())
            .run(&Context::default())
            .into_iter()
            .filter(|f| f.id == VAULT_INTEGRITY || f.id == LIBRARY)
            .collect();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|f| f.level == Level::Problem), "{found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
