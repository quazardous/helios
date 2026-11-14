//! What the health check reads, gathered by the node: the volumes mounted
//! (with the roles the vault gives them), the vault's file, whose folder
//! stands for the library's place, what the SQLite driver reads of that
//! file (it opens, its integrity), the library's backups, and what only
//! the process running the node knows (a setting, the cards an import
//! left short, the GPU, the plugins, the catalogue's snapshots), each
//! given by a provider. The desktop fills the providers from its settings,
//! import window and plugin broker; the headless node fills those it has.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_health::Context;
use helios_health::backup::{Backup, CardShort, DESTINATION_DAYS};
use helios_health::checks::Database;
use helios_health::gpu::Gpu;
use helios_health::plugins::Plugin;
use helios_health::snapshots::Snapshots;
use helios_storage::{Volume, Volumes, registry};
use helios_vault::{Store, Vault};

type Provider<T> = Box<dyn Fn() -> T + Send + Sync>;
type SnapshotsOf = Box<dyn Fn(&Path, &[Volume], u64) -> Snapshots + Send + Sync>;

/// The health check's inputs. [`HealthInputs::new`] gives the defaults a
/// node without a window has (the default destination days, no cards, no
/// GPU, no plugins, no snapshots); each `with_` method replaces one.
pub struct HealthInputs<S: Store + 'static> {
    volumes: Arc<dyn Volumes>,
    vault: Option<Arc<Vault<S>>>,
    file: Option<PathBuf>,
    destination_days: Provider<u64>,
    cards: Provider<Vec<CardShort>>,
    gpu: Provider<Option<Gpu>>,
    plugins: Provider<Vec<Plugin>>,
    snapshots: Option<SnapshotsOf>,
}

impl<S: Store + 'static> HealthInputs<S> {
    /// Over the `volumes` of this machine, the `vault` and its database
    /// `file` (each optional).
    pub fn new(
        volumes: Arc<dyn Volumes>,
        vault: Option<Arc<Vault<S>>>,
        file: Option<PathBuf>,
    ) -> Self {
        Self {
            volumes,
            vault,
            file: file.and_then(|f| f.canonicalize().ok()),
            destination_days: Box::new(|| DESTINATION_DAYS),
            cards: Box::new(Vec::new),
            gpu: Box::new(|| None),
            plugins: Box::new(Vec::new),
            snapshots: None,
        }
    }

    /// The `health.destinationDays` setting, read at each run.
    #[must_use]
    pub fn with_destination_days(mut self, f: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        self.destination_days = Box::new(f);
        self
    }

    /// The cards an import left short, still inserted.
    #[must_use]
    pub fn with_cards(mut self, f: impl Fn() -> Vec<CardShort> + Send + Sync + 'static) -> Self {
        self.cards = Box::new(f);
        self
    }

    /// The adapters the process sees and the one it renders on.
    #[must_use]
    pub fn with_gpu(mut self, f: impl Fn() -> Option<Gpu> + Send + Sync + 'static) -> Self {
        self.gpu = Box::new(f);
        self
    }

    /// The plugins installed, each probed.
    #[must_use]
    pub fn with_plugins(mut self, f: impl Fn() -> Vec<Plugin> + Send + Sync + 'static) -> Self {
        self.plugins = Box::new(f);
        self
    }

    /// The catalogue's snapshots of the vault's file, beside it and on
    /// the volumes mounted, at a time in seconds since the epoch.
    #[must_use]
    pub fn with_snapshots(
        mut self,
        f: impl Fn(&Path, &[Volume], u64) -> Snapshots + Send + Sync + 'static,
    ) -> Self {
        self.snapshots = Some(Box::new(f));
        self
    }

    /// Gathers the check's context now (on the check's thread).
    pub fn context(&self) -> Context {
        let mounted = self.volumes.volumes();
        let mut volumes = mounted.clone();
        if let Some(known) = self.vault.as_deref().and_then(|v| registry::known(v).ok()) {
            for v in &mut volumes {
                if let Some(k) = known.iter().find(|k| k.id == v.id) {
                    v.roles.clone_from(&k.roles);
                    v.name.clone_from(&k.name);
                }
            }
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let backup = self.vault.as_deref().and_then(|v| {
            Backup::read(v, &mounted, now)
                .inspect_err(|e| log::warn!("health: backups: {e}"))
                .ok()
        });
        let backup = backup.map(|mut b| {
            b.destination_days = (self.destination_days)();
            b.cards = (self.cards)();
            b
        });
        let file = self.file.as_deref();
        Context {
            volumes,
            library: file.and_then(Path::parent).map(Path::to_path_buf),
            vault: self.file.clone(),
            backup,
            gpu: (self.gpu)(),
            plugins: (self.plugins)(),
            snapshots: file
                .zip(self.snapshots.as_ref())
                .map(|(f, snapshots)| snapshots(f, &mounted, now)),
            library_folder: self.vault.as_deref().and_then(|v| {
                helios_library::import::library_root(v)
                    .inspect_err(|e| log::warn!("health: library folder: {e}"))
                    .ok()
            }),
            changed_outside: self.vault.as_deref().and_then(|v| {
                v.changed_outside_images()
                    .inspect_err(|e| log::warn!("health: changed outside: {e}"))
                    .ok()
                    .map(|l| l.len())
            }),
            database: file.map(|f| Database {
                opens: helios_vault_sqlite::health::opens(f),
                problems: helios_vault_sqlite::health::quick_check(f),
            }),
        }
    }

    /// The context provider a [`helios_health::runner::Runner`] takes.
    pub fn into_context(self) -> impl Fn() -> Context + Send + Sync + 'static {
        move || self.context()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_health::runner::{Runner, Schedule};
    use helios_health::{Findings, Registry};
    use helios_vault_sqlite::SqliteStore;
    use std::time::{Duration, Instant};

    /// With no window, inputs filled by hand give the check its context,
    /// and a run of the built-in checks over it comes through.
    #[test]
    fn a_health_check_runs_over_inputs_filled_by_hand() {
        let dir = std::env::temp_dir().join(format!("helios-node-health-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("vault.db");
        let vault = Arc::new(Vault::new(SqliteStore::open(&file).unwrap()));
        let volumes: Arc<dyn Volumes> = Arc::new(Vec::<Volume>::new());
        let inputs = HealthInputs::new(volumes, Some(vault), Some(file.clone()))
            .with_destination_days(|| 3)
            .with_cards(|| {
                vec![CardShort {
                    label: "CARD".into(),
                    short: 2,
                }]
            });

        let context = inputs.context();
        let file = file.canonicalize().unwrap();
        assert_eq!(context.vault.as_deref(), Some(file.as_path()));
        assert_eq!(context.library.as_deref(), file.parent());
        let backup = context.backup.expect("the vault's backups");
        assert_eq!(backup.destination_days, 3);
        assert_eq!(backup.cards.len(), 1);
        assert!(context.gpu.is_none() && context.plugins.is_empty());
        assert!(context.snapshots.is_none());
        assert_eq!(context.changed_outside, Some(0));
        let database = context.database.as_ref().expect("the database read");
        assert_eq!(database.opens, Ok(()));
        assert_eq!(database.problems, Ok(Vec::new()));

        let runner = Runner::new(
            Registry::builtin(),
            inputs.into_context(),
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
        use helios_health::Check as _;
        use helios_health::Level;
        use helios_health::checks::{CHOOSE_LIBRARY_FOLDER, FOLDER_CHOSEN, FolderChosen};

        let dir =
            std::env::temp_dir().join(format!("helios-node-health-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("vault.db");
        let vault = Arc::new(Vault::new(SqliteStore::open(&file).unwrap()));
        let volumes: Arc<dyn Volumes> = Arc::new(Vec::<Volume>::new());
        let inputs = HealthInputs::new(volumes, Some(vault.clone()), Some(file));
        let check = |inputs: &HealthInputs<SqliteStore>| FolderChosen.run(&inputs.context());

        let found = check(&inputs);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].id, FOLDER_CHOSEN);
        assert_eq!(found[0].level, Level::Warning);
        assert_eq!(
            found[0].fix.as_ref().map(|f| f.command.as_str()),
            Some(CHOOSE_LIBRARY_FOLDER)
        );

        let photos = dir.join("Photos");
        helios_library::import::set_library_root(&vault, &photos).unwrap();
        let found = check(&inputs);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].level, Level::Ok);
        assert!(found[0].fix.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A vault file that is not a database is read by the driver and
    /// reported by the integrity and library checks as a problem.
    #[test]
    fn a_corrupt_vault_is_reported() {
        use helios_health::Level;
        use helios_health::checks::{LIBRARY, VAULT_INTEGRITY};

        let dir =
            std::env::temp_dir().join(format!("helios-node-health-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("vault.db");
        std::fs::write(&file, vec![0x5a; 8192]).unwrap();
        let volumes: Arc<dyn Volumes> = Arc::new(Vec::<Volume>::new());
        let inputs = HealthInputs::<SqliteStore>::new(volumes, None, Some(file));
        let context = inputs.context();
        let found: Vec<_> = Registry::builtin()
            .checks()
            .iter()
            .flat_map(|c| c.run(&context))
            .filter(|f| f.id == VAULT_INTEGRITY || f.id == LIBRARY)
            .collect();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|f| f.level == Level::Problem), "{found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
