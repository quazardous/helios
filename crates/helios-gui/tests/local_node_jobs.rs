//! The backups and the health check are the local node's jobs, not a
//! window's: a backup started from a window finishes once that window
//! closed, another remaining; the health check runs with no window at all
//! and reports in the jobs API; quitting during a backup asks, and the
//! confirmed quit cancels it before the node closes the vault cleanly.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use egui_kittest::kittest::Queryable as _;
use helios_core::Bus;
use helios_core::jobs::{self, Jobs};
use helios_gui::node::{LocalNode, NodeJobs as _};
use helios_gui::node_jobs::{self, Backups, Health};
use helios_gui::{Shell, headless, helios_panels};
use helios_library::import::{self, Options, Placement};
use helios_shell::commands::Origin;
use helios_storage::strategy::{self, Scope, Strategy, Trigger};
use helios_storage::{Identity, Role, Volume, VolumeId, VolumeKind, Volumes, detect, registry};
use helios_vault::Vault;
use helios_vault_sqlite::SqliteStore;
use serde_json::json;

type Errors = Arc<Mutex<Vec<String>>>;

/// The disks plugged in, and a hold: once a backup job runs, the next
/// look at the disks waits until the test lets it go on, or until the
/// node cancels the backups. A backup is thus caught running.
struct Held {
    disks: Vec<Volume>,
    jobs: Jobs,
    open: AtomicBool,
    backups: OnceLock<Weak<Backups<SqliteStore>>>,
}

impl Volumes for Held {
    fn volumes(&self) -> Vec<Volume> {
        let deadline = Instant::now() + Duration::from_mins(1);
        let backing_up = || {
            self.jobs
                .cancellable()
                .iter()
                .any(|id| id.starts_with("backup-"))
        };
        let cancelling = || {
            self.backups
                .get()
                .and_then(Weak::upgrade)
                .is_some_and(|b| b.cancelling())
        };
        while backing_up()
            && !self.open.load(Ordering::SeqCst)
            && !cancelling()
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.disks.clone()
    }
}

fn disk(id: &str, mount: &Path, roles: &[Role]) -> Volume {
    std::fs::create_dir_all(mount).unwrap();
    Volume {
        id: VolumeId(id.into()),
        identified_by: Identity::Uuid,
        name: id.into(),
        kind: VolumeKind::External,
        capacity: Some(1_000_000_000),
        free: Some(400_000_000),
        mount: Some(mount.to_path_buf()),
        roles: roles.iter().copied().collect(),
        ..detect::weak(id, None)
    }
}

/// A library of photos on a library disk, a backup disk, a strategy run
/// by hand, the vault owned by a local node.
struct Library {
    dir: PathBuf,
    node: Option<LocalNode<SqliteStore>>,
    held: Arc<Held>,
}

fn library(name: &str, photos: u8, jobs: &Jobs) -> Library {
    let dir = std::env::temp_dir().join(format!("helios-local-jobs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A long schedule: only the stop flushes.
    let node = LocalNode::open(
        SqliteStore::open(&dir.join("vault.db")).unwrap(),
        Arc::new(Bus::new()),
        Duration::from_hours(1),
    );
    let vault = node.vault();
    let lib = disk("uuid:lib", &dir.join("libdisk"), &[Role::Library]);
    let backup = disk(BACKUP_DISK, &dir.join("backup"), &[Role::Backup]);
    registry::record(vault, &[lib.clone(), backup.clone()], "studio", 10, None).unwrap();
    for v in [&lib, &backup] {
        registry::save(vault, v).unwrap();
    }
    if photos > 0 {
        let roll = dir.join("roll");
        std::fs::create_dir_all(&roll).unwrap();
        for k in 0..photos {
            let bytes: Vec<u8> = (0..20_000u32)
                .map(|i| (i as u8).wrapping_mul(k + 1))
                .collect();
            std::fs::write(roll.join(format!("p{k}.raf")), bytes).unwrap();
        }
        let options = Options {
            placement: Placement::Copy {
                into: Some(dir.join("libdisk/Library")),
            },
            signals: false,
            ..Options::default()
        };
        import::import(
            vault,
            jobs,
            &[roll],
            &options,
            100,
            &mut |_: &serde_json::Value| {},
        )
        .unwrap();
    }
    let by_hand = Strategy {
        triggers: [Trigger::ByHand].into(),
        ..Strategy::default()
    };
    strategy::save(vault, &Scope::Library, Some(&by_hand)).unwrap();
    let held = Arc::new(Held {
        disks: vec![lib, backup],
        jobs: jobs.clone(),
        open: AtomicBool::new(false),
        backups: OnceLock::new(),
    });
    Library {
        dir,
        node: Some(node),
        held,
    }
}

const BACKUP_DISK: &str = "uuid:backup";

impl Library {
    fn node(&self) -> &LocalNode<SqliteStore> {
        self.node.as_ref().expect("the node")
    }

    /// The node's backups, their errors kept in `errors`.
    fn backups(&self, jobs: &Jobs, errors: &Errors) -> Arc<Backups<SqliteStore>> {
        let errors = errors.clone();
        let backups = Backups::new(
            self.node().vault().clone(),
            jobs.clone(),
            self.held.clone(),
            move |e| errors.lock().unwrap().push(e),
        )
        .adopted_by(&self.node().link());
        let _ = self.held.backups.set(Arc::downgrade(&backups));
        backups
    }

    /// The node stopped, then the vault opened again: closed cleanly.
    fn reopened(&mut self) -> Vault<SqliteStore> {
        self.node.take().expect("the node").stop();
        let store = SqliteStore::open(&self.dir.join("vault.db")).unwrap();
        assert!(!store.report().unclean_shutdown, "closed cleanly");
        Vault::new(store)
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        if let Some(node) = self.node.take() {
            node.stop();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Photos with a copy on the backup disk.
fn backed_up(vault: &Vault<SqliteStore>) -> usize {
    vault
        .images()
        .unwrap()
        .iter()
        .filter(|i| {
            vault
                .locations(&i.id)
                .unwrap()
                .iter()
                .any(|l| l.volume.as_ref().is_some_and(|v| v.as_str() == BACKUP_DISK))
        })
        .count()
}

fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_mins(1);
    while !done() {
        assert!(Instant::now() < deadline, "never got there in 60 s: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The ids of the jobs `kind` events (`job.finished`...) on `bus` name.
fn said(bus: &Bus, kind: &str) -> Vec<String> {
    bus.since(0)
        .into_iter()
        .filter(|e| e.source == jobs::SOURCE && e.kind == kind)
        .filter_map(|e| e.payload["id"].as_str().map(String::from))
        .collect()
}

/// A backup started by a window's command goes on once that window and
/// its handle are gone (another window, the node's other client,
/// remaining): it finishes, its copies recorded, in the jobs API.
#[test]
fn a_backup_started_from_a_window_finishes_after_that_window_closes() {
    let mut shell = Shell::new(helios_panels(), None);
    let jobs = shell.jobs();
    let mut lib = library("window", 2, &jobs);
    let link = lib.node().link();
    let (window, other) = (lib.node().connect(), lib.node().connect());
    shell.attach_node(link.clone());
    let errors = Errors::default();
    let backups = lib.backups(&jobs, &errors);
    node_jobs::register_back_up_now(&mut shell, &backups).unwrap();
    let mut h = headless::harness(shell, egui::vec2(800.0, 500.0));
    h.run();
    h.state_mut()
        .run("library.backupNow", json!(null), Origin::User)
        .unwrap();
    until("the backup runs", || !link.running_jobs().is_empty());
    let running = link.running_jobs();

    // The window closes: its harness, its client and its handle go.
    drop(h);
    drop(window);
    drop(backups);
    lib.held.open.store(true, Ordering::SeqCst);

    until("the backup ends", || link.running_jobs().is_empty());
    assert_eq!(
        said(jobs.bus(), jobs::FINISHED)
            .iter()
            .filter(|id| running.contains(id))
            .count(),
        1,
        "it finished"
    );
    assert_eq!(backed_up(lib.node().vault()), 2, "both photos copied");
    assert!(!link.stopped(), "the other window keeps the node");
    assert!(errors.lock().unwrap().is_empty(), "{errors:?}");

    link.quit();
    drop(other);
    assert!(
        link.wait_stopped(Duration::from_secs(10)),
        "the node stopped"
    );
    assert_eq!(backed_up(&lib.reopened()), 2, "kept");
}

/// The health check needs no window: the node keeps its schedule, and
/// its run reports in the jobs API; the node's stop ends the schedule.
#[test]
fn the_health_check_runs_with_no_window_and_reports_in_the_jobs_api() {
    use helios_health::runner::{Runner, Schedule};
    use helios_health::{Context, Findings, Registry};

    let bus = Arc::new(Bus::new());
    let jobs = Jobs::new(bus.clone());
    let mut lib = library("health", 0, &jobs);
    let link = lib.node().link();
    let runner = Runner::new(
        Registry::builtin(),
        Context::default,
        Findings::default(),
        Schedule::new(Instant::now(), Duration::ZERO, Duration::from_hours(24)),
    );
    runner.report_to(jobs.clone());
    let health = Health::adopted_by(runner, &link, Duration::from_millis(20));

    until("a run ends", || {
        !said(&bus, jobs::FINISHED).is_empty() && !health.runner().running()
    });
    assert_eq!(
        said(&bus, jobs::FINISHED),
        ["health-1"],
        "one run, by the node"
    );
    assert!(health.runner().findings().ran(), "its findings kept");
    assert!(
        link.running_jobs().is_empty(),
        "nothing a quit would cancel"
    );

    // No client ever connected: the node stops when the host says so.
    drop(lib.reopened());
    assert!(link.stopped());
    assert!(health.wait(Duration::ZERO), "nothing under way");
    assert!(!health.runner().run_now(), "no run once the node stopped");
}

/// Quit during a backup: the question comes first; confirmed, the node
/// cancels the backup, which stops before its next photo, then closes the
/// vault cleanly; nothing it wrote hit a closed vault.
#[test]
fn quitting_during_a_backup_asks_then_the_node_cancels_it_and_stops_cleanly() {
    let mut shell = Shell::new(helios_panels(), None);
    let jobs = shell.jobs();
    let mut lib = library("quit", 3, &jobs);
    let link = lib.node().link();
    let client = lib.node().connect();
    shell.attach_node(link.clone());
    let errors = Errors::default();
    let backups = lib.backups(&jobs, &errors);
    node_jobs::register_back_up_now(&mut shell, &backups).unwrap();
    let mut h = headless::harness(shell, egui::vec2(800.0, 500.0));
    h.ctx.set_embed_viewports(false);
    h.run();
    h.state_mut()
        .run("library.backupNow", json!(null), Origin::User)
        .unwrap();
    until("the backup runs", || !link.running_jobs().is_empty());
    let running = link.running_jobs();

    h.state_mut()
        .run("workbench.quit", json!(null), Origin::User)
        .unwrap();
    h.run_steps(2);
    assert!(!h.state().quitting(), "asked first");
    assert!(
        h.query_by_label_contains("jobs are running").is_some(),
        "the question about the running backup"
    );
    h.state_mut()
        .run("helios.quit.confirmed", json!(null), Origin::User)
        .unwrap();
    h.run_steps(2);
    assert!(h.state().quitting());
    assert_eq!(link.running_jobs(), running, "running until the stop");

    // The GUI ends; the node, its last client gone, cancels the backup
    // and closes the vault.
    drop(h);
    drop(client);
    assert!(
        link.wait_stopped(Duration::from_mins(1)),
        "the node stopped"
    );
    assert!(backups.cancelling());
    assert_eq!(said(jobs.bus(), jobs::CANCELLED), running, "cancelled");
    assert!(link.running_jobs().is_empty());
    assert!(errors.lock().unwrap().is_empty(), "{errors:?}");
    assert!(
        !backups.start(Trigger::ByHand, None),
        "none starts any more"
    );
    assert!(
        backed_up(&lib.reopened()) < 3,
        "stopped before the last photo"
    );
}
