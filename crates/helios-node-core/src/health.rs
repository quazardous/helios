//! What the health check reads, gathered by the node: the volumes mounted
//! (with the roles the vault gives them), the vault's file, whose folder
//! stands for the library's place, the library's backups, and what only
//! the process running the node knows (a setting, the cards an import
//! left short, the GPU, the plugins, the catalogue's snapshots), each
//! given by a provider. The desktop fills the providers from its settings,
//! import window and plugin broker; the headless node fills those it has.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_health::Context;
use helios_health::backup::{Backup, CardShort, DESTINATION_DAYS};
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
            changed_outside: self.vault.as_deref().and_then(|v| {
                v.changed_outside_images()
                    .inspect_err(|e| log::warn!("health: changed outside: {e}"))
                    .ok()
                    .map(|l| l.len())
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
}
