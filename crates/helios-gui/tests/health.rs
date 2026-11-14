//! The health check on the desktop: a check that finds a problem shows
//! the status bar's badge; the badge opens the Settings app at Health,
//! listing it; "Ignore" moves it last, marked ignored, and clears the
//! badge; "Show again" brings it back.
//!
//! Helios's own checks over a temporary library, fake volumes and cards:
//! the backup strategy set, the photos meeting it, the backup disks seen
//! lately, the copies verified, the cards safe to format, and the library
//! opening with room; each finding's fix runs its command. Then the GPU
//! in use (fake adapters), its kernels (a compile failure injected), the
//! plugins (fake ones: fine, of a wrong version, crashing) and the
//! catalogue snapshots. And no library folder chosen, until one is.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::core::indicator::{self, Tone};
use helios_core::Message;
use helios_gui::shell::HEALTH_BADGE;
use helios_gui::{Shell, headless, helios_panels};
use helios_health::backup::{Backup, CardShort};
use helios_health::checks::Database;
use helios_health::gpu::{Adapter, AdapterCheck, AdapterKind, Gpu, Kernel, Kernels};
use helios_health::plugins::{Plugin, Plugins, Status};
use helios_health::runner::{Runner, Schedule};
use helios_health::snapshots::{Snapshot, Snapshots, SnapshotsCheck};
use helios_health::{Check, Context, Declaration, Finding, Findings, Level, Registry};
use helios_shell::commands::Origin;
use helios_storage::strategy::{self, Scope, Strategy};
use helios_storage::{Identity, Role, Volume, VolumeId, VolumeKind, detect, registry, scan};
use helios_vault::Vault;
use helios_vault_sqlite::SqliteStore;
use serde_json::json;

/// A check that always finds the same problem.
struct Broken;

impl Check for Broken {
    fn id(&self) -> &'static str {
        "test.broken"
    }

    fn title(&self) -> Message {
        Message::plain("Broken")
    }

    fn declaration(&self) -> Declaration {
        Declaration::new("A flux capacitor").levels(&[Level::Problem])
    }

    fn run(&self, _: &Context) -> Vec<Finding> {
        vec![Finding::new(
            "test.broken",
            "",
            Level::Problem,
            Message::plain("The flux capacitor is unplugged"),
            Message::plain("Plug it back in"),
        )]
    }
}

fn shell() -> Shell {
    let mut registry = Registry::default();
    registry.add("test", Arc::new(Broken)).unwrap();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_health(Runner::new(
        registry,
        Context::default,
        Findings::default(),
        // At once, then not again during the test.
        Schedule::new(Instant::now(), Duration::ZERO, Duration::from_hours(1)),
    ));
    shell
}

fn badge(h: &Harness<'_, Shell>) -> Option<indicator::Record> {
    indicator::find(&h.ctx, HEALTH_BADGE).filter(|r| r.shown)
}

/// Steps frames until `done` holds, for ten seconds at most.
fn until(h: &mut Harness<'_, Shell>, what: &str, done: impl Fn(&Harness<'_, Shell>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("never got there: {what}");
}

#[test]
fn a_problem_shows_the_badge_the_page_lists_it_and_ignore_clears_it() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    until(&mut h, "the badge", |h| badge(h).is_some());
    let b = badge(&h).unwrap();
    assert!(
        b.name
            .as_deref()
            .is_some_and(|n| n.contains("Health check:"))
    );
    assert_eq!(b.value, Some(1.0));
    assert_eq!(b.tone, Tone::Danger, "a problem");
    assert!(!h.state().health_page_open(), "never a modal");

    // The badge opens the page, which lists the problem.
    let [x, y, w, hh] = b.rect;
    let centre = egui::pos2(x + w / 2.0, y + hh / 2.0);
    h.hover_at(centre);
    h.step();
    h.drag_at(centre);
    h.step();
    h.drop_at(centre);
    until(&mut h, "the page", |h| h.state().health_page_open());
    until(&mut h, "the finding", |h| {
        h.query_by_label_contains("The flux capacitor is unplugged")
            .is_some()
    });
    assert!(h.query_by_label_contains("Plug it back in").is_some());
    until(&mut h, "the summary", |h| {
        h.query_by_label_contains("Problems: 1").is_some()
    });

    // Ignore: marked ignored, and the badge goes.
    h.get_by_label("Ignore").click();
    until(&mut h, "the badge gone", |h| badge(h).is_none());
    until(&mut h, "said ignored", |h| {
        h.query_by_label_contains("Problems: 1").is_none()
            && h.query_by_label("Show again").is_some()
    });

    // Undone: back, with its badge.
    h.get_by_label("Show again").click();
    until(&mut h, "the badge back", |h| badge(h).is_some());
}

#[test]
fn the_health_page_is_a_command_and_check_now_runs_again() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    until(&mut h, "a first run", |h| {
        h.state()
            .health()
            .is_some_and(|r| r.findings().ran() && !r.running())
    });
    h.state_mut()
        .run(
            helios_gui::shell::HEALTH_COMMAND,
            serde_json::Value::Null,
            helios_shell::commands::Origin::User,
        )
        .unwrap();
    until(&mut h, "the page", |h| {
        h.query_by_label("Check now").is_some()
    });
    h.state_mut()
        .run(
            helios_gui::shell::HEALTH_CHECK_NOW,
            serde_json::Value::Null,
            Origin::User,
        )
        .unwrap();
    until(&mut h, "a second run", |h| {
        h.state().health().is_some_and(|r| r.runs() == 2)
    });
    // What the page reads, and the fix and Ignore it runs.
    let read = h
        .state_mut()
        .run(
            helios_gui::shell::HEALTH_FINDINGS,
            serde_json::Value::Null,
            Origin::User,
        )
        .unwrap();
    assert_eq!(read["findings"][0]["id"], "test.broken");
    assert_eq!(read["findings"][0]["tone"], "danger");
    assert_eq!(read["summary"]["tone"], "danger");
    h.state_mut()
        .run(
            helios_gui::shell::HEALTH_IGNORE,
            json!({ "id": "test.broken" }),
            Origin::User,
        )
        .unwrap();
    h.run();
    let read = h
        .state_mut()
        .run(
            helios_gui::shell::HEALTH_FINDINGS,
            serde_json::Value::Null,
            Origin::User,
        )
        .unwrap();
    assert_eq!(read["findings"][0]["ignored"], true);
    assert_eq!(read["summary"]["tone"], "success");
}

/// When the first scan ran.
const FIRST: u64 = 86_400 * 10 + 43_200;
const DAY: u64 = 86_400;
const GB: u64 = 1_000_000_000;

fn disk(id: &str, name: &str, mount: &Path) -> Volume {
    Volume {
        id: VolumeId(id.into()),
        identified_by: Identity::Uuid,
        name: name.into(),
        label: Some(name.into()),
        kind: VolumeKind::External,
        capacity: Some(1000 * GB),
        free: Some(500 * GB),
        mount: Some(mount.to_path_buf()),
        ..detect::weak(name, None)
    }
}

/// A library (`library.db` in a temporary folder) with one photo on the
/// "Backup" disk (its role set, its copy not verified yet), and the
/// "Studio" disk; both seen at `FIRST`.
struct Library {
    dir: PathBuf,
    file: PathBuf,
    vault: Arc<Vault<SqliteStore>>,
    studio: Volume,
    backup: Volume,
}

impl Library {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("helios-gui-health-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (dir.join("studio"), dir.join("backup"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let photo = b.join("photo.jpg");
        std::fs::write(&photo, b"not really a jpeg, enough for the vault").unwrap();
        let file = dir.join("library.db");
        let vault = Arc::new(Vault::new(SqliteStore::open(&file).unwrap()));
        vault.import_file(&photo, 1).unwrap();
        let studio = disk("uuid:studio", "Studio", &a);
        let mut backup = disk("uuid:backup", "Backup", &b);
        scan::scan(&vault, &[studio.clone(), backup.clone()], "pc", FIRST, None).unwrap();
        backup.roles.insert(Role::Backup);
        registry::save(&vault, &backup).unwrap();
        Self {
            dir,
            file,
            vault,
            studio,
            backup,
        }
    }

    fn strategy(&self, strategy: Option<&Strategy>) {
        strategy::save(&self.vault, &Scope::Library, strategy).unwrap();
    }

    /// What a run reads with `mounted` plugged in at `now`.
    fn context(&self, mounted: &[&Volume], now: u64) -> Context {
        let mounted: Vec<Volume> = mounted.iter().map(|v| (*v).clone()).collect();
        Context {
            volumes: mounted.clone(),
            library: Some(self.dir.clone()),
            vault: Some(self.file.clone()),
            backup: Some(Backup::read(&self.vault, &mounted, now).unwrap()),
            database: Some(database(&self.file)),
            ..Context::default()
        }
    }
}

/// What the node reads of the database `file`, as its health inputs do.
fn database(file: &Path) -> Database {
    Database {
        opens: helios_vault_sqlite::health::opens(file),
        problems: helios_vault_sqlite::health::quick_check(file),
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A shell running Helios's checks over `ctx`, its Health page open
/// once the first run is in.
fn checked(ctx: Context) -> Harness<'static, Shell> {
    checked_by(Registry::builtin(), ctx)
}

/// A shell running `registry`'s checks over `ctx`, its Health page open
/// once the first run is in.
fn checked_by(registry: Registry, ctx: Context) -> Harness<'static, Shell> {
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_health(Runner::new(
        registry,
        move || ctx.clone(),
        Findings::default(),
        Schedule::new(Instant::now(), Duration::ZERO, Duration::from_hours(1)),
    ));
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    until(&mut h, "a first run", |h| {
        h.state()
            .health()
            .is_some_and(|r| r.findings().ran() && !r.running())
    });
    h.state_mut()
        .run(
            helios_gui::shell::HEALTH_COMMAND,
            serde_json::Value::Null,
            Origin::User,
        )
        .unwrap();
    until(&mut h, "the page", |h| {
        h.query_by_label("Check now").is_some()
    });
    h
}

/// The finding `id` of the last run: its level, message and fix.
fn finding(h: &Harness<'_, Shell>, id: &str) -> Option<Finding> {
    let runner = h.state().health()?;
    let findings = runner.findings();
    findings.shown().into_iter().find(|f| f.id == id).cloned()
}

/// Runs `id`'s fix, as its "Fix…" button does.
fn fix(h: &mut Harness<'_, Shell>, id: &str) {
    let f = finding(h, id).expect(id).fix.expect("a fix");
    h.state_mut().run(&f.command, f.args, Origin::User).unwrap();
    h.run();
}

#[test]
fn no_strategy_is_a_warning_whose_fix_opens_backup_strategy() {
    let lib = Library::new("none");
    let mut h = checked(lib.context(&[&lib.studio, &lib.backup], FIRST));
    until(&mut h, "the finding", |h| {
        h.query_by_label("No backup is set up").is_some()
    });
    let f = finding(&h, "backup.strategy").unwrap();
    assert_eq!(f.level, Level::Warning);
    // Without a strategy, the other backup checks say nothing.
    for id in [
        "backup.met",
        "backup.destinations-seen",
        "backup.unverified",
    ] {
        assert!(finding(&h, id).is_none(), "{id}");
    }
    assert!(!h.state().storage_page_open());
    fix(&mut h, "backup.strategy");
    assert!(h.state().storage_page_open(), "the strategy editor's page");
    let admin = h
        .state()
        .app()
        .handle()
        .service(&egui_pop::api::admin::ADMIN)
        .unwrap();
    assert_eq!(
        admin.shown(),
        (
            Some("backup".into()),
            Some("strategy".into()),
            String::new()
        )
    );

    // Set: fine.
    lib.strategy(Some(&Strategy::default()));
    let h = checked(lib.context(&[&lib.studio, &lib.backup], FIRST));
    assert_eq!(finding(&h, "backup.strategy").unwrap().level, Level::Ok);
}

#[test]
fn photos_short_of_their_copies_offer_back_up_now_or_the_storage_page() {
    let lib = Library::new("met");
    lib.strategy(Some(&Strategy::default()));
    // The backup disk plugged in: "Back up now".
    let mut h = checked(lib.context(&[&lib.studio, &lib.backup], FIRST));
    until(&mut h, "the finding", |h| {
        h.query_by_label("Photos short of their backups: 1")
            .is_some()
    });
    let f = finding(&h, "backup.met").unwrap();
    assert_eq!(f.level, Level::Warning);
    assert_eq!(f.fix.unwrap().command, "library.backupNow");

    // None plugged in: the Storage page.
    let mut h = checked(lib.context(&[&lib.studio], FIRST));
    let f = finding(&h, "backup.met").unwrap();
    assert_eq!(f.fix.unwrap().command, "library.storage");
    fix(&mut h, "backup.met");
    assert!(h.state().storage_page_open());

    // Verified: met.
    let photo = lib.vault.images().unwrap()[0].id;
    strategy::record_verified(
        &lib.vault,
        &photo,
        &lib.backup.id,
        Path::new("photo.jpg"),
        FIRST,
    )
    .unwrap();
    let h = checked(lib.context(&[&lib.studio, &lib.backup], FIRST));
    assert_eq!(finding(&h, "backup.met").unwrap().level, Level::Ok);
}

#[test]
fn a_backup_disk_away_too_long_asks_to_be_plugged_in() {
    let lib = Library::new("seen");
    lib.strategy(Some(&Strategy::default()));
    // Away 20 days: past the 14 by default.
    let mut h = checked(lib.context(&[&lib.studio], FIRST + 20 * DAY));
    until(&mut h, "the finding", |h| {
        h.query_by_label("Backup was last seen 20 days ago")
            .is_some()
    });
    let f = finding(&h, "backup.destinations-seen/uuid:backup").unwrap();
    assert_eq!(f.level, Level::Warning);
    let x = f.fix.unwrap();
    assert_eq!(x.command, "library.storage");
    assert_eq!(x.args["volume"], "uuid:backup");
    assert_eq!(
        helios_gui::i18n::text(&h.ctx, x.label),
        "Plug in Backup to back up"
    );
    fix(&mut h, "backup.destinations-seen/uuid:backup");
    assert!(h.state().storage_page_open());

    // Ten days, or a longer setting: fine.
    let h = checked(lib.context(&[&lib.studio], FIRST + 10 * DAY));
    assert_eq!(
        finding(&h, "backup.destinations-seen").unwrap().level,
        Level::Ok
    );
    let mut ctx = lib.context(&[&lib.studio], FIRST + 20 * DAY);
    ctx.backup.as_mut().unwrap().destination_days = 30;
    let h = checked(ctx);
    assert!(finding(&h, "backup.destinations-seen/uuid:backup").is_none());
}

#[test]
fn unverified_copies_offer_back_up_now() {
    let lib = Library::new("unverified");
    lib.strategy(Some(&Strategy::default()));
    let mut h = checked(lib.context(&[&lib.studio, &lib.backup], FIRST));
    until(&mut h, "the finding", |h| {
        h.query_by_label("Photos with backup copies not verified: 1")
            .is_some()
    });
    let f = finding(&h, "backup.unverified").unwrap();
    assert_eq!(f.level, Level::Warning);
    assert_eq!(f.fix.unwrap().command, "library.backupNow");
}

#[test]
fn a_card_short_of_its_backups_is_not_safe_to_format() {
    let lib = Library::new("cards");
    lib.strategy(Some(&Strategy::default()));
    let mut ctx = lib.context(&[&lib.studio, &lib.backup], FIRST);
    ctx.backup.as_mut().unwrap().cards = vec![
        CardShort {
            label: "EOS_DIGITAL".into(),
            short: 3,
        },
        CardShort {
            label: "SAFE_CARD".into(),
            short: 0,
        },
    ];
    let mut h = checked(ctx);
    until(&mut h, "the finding", |h| {
        h.query_by_label("EOS_DIGITAL is not safe to format yet")
            .is_some()
    });
    assert!(
        h.query_by_label_contains("Photos on it short of their backups: 3")
            .is_some()
    );
    assert!(finding(&h, "backup.cards/SAFE_CARD").is_none());
    let f = finding(&h, "backup.cards/EOS_DIGITAL").unwrap();
    assert_eq!(f.fix.unwrap().command, "library.backupNow");
}

#[test]
fn the_library_opens_and_its_disk_needs_room() {
    let lib = Library::new("library");
    let mut h = checked(lib.context(&[&lib.studio, &lib.backup], FIRST));
    until(&mut h, "the finding", |h| {
        h.query_by_label("The library opens and has room").is_some()
    });

    // Its disk at 1 GB free: a warning, the fix on the Storage page.
    let mut home = disk("uuid:home", "Home", &lib.dir);
    home.free = Some(GB);
    let mut h = checked(lib.context(&[&home], FIRST));
    until(&mut h, "the finding", |h| {
        h.query_by_label("The library's disk is almost full")
            .is_some()
    });
    let x = finding(&h, "library.default").unwrap().fix.unwrap();
    assert_eq!(x.command, "library.storage");
    assert_eq!(x.args["volume"], "uuid:home");

    // A file that is no library: a problem, its folder offered.
    let mut ctx = lib.context(&[&lib.studio], FIRST);
    let bad = lib.dir.join("broken.db");
    std::fs::write(&bad, vec![0x5a; 8192]).unwrap();
    ctx.database = Some(database(&bad));
    ctx.vault = Some(bad);
    let h = checked(ctx);
    let f = finding(&h, "library.default").unwrap();
    assert_eq!(f.level, Level::Problem);
    let x = f.fix.unwrap();
    assert_eq!(x.command, "library.openFolder");
    assert_eq!(x.args["path"], json!(lib.dir.display().to_string()));
}

/// Only `check`.
fn only(check: Arc<dyn Check>) -> Registry {
    let mut r = Registry::default();
    r.add("test", check).unwrap();
    r
}

fn adapter(name: &str, kind: AdapterKind) -> Adapter {
    Adapter {
        name: name.into(),
        driver: "Mesa 25.2".into(),
        backend: "Vulkan".into(),
        kind,
    }
}

#[test]
fn a_software_adapter_beside_a_gpu_is_a_warning_whose_fix_is_the_docs() {
    let gpu = |used| Gpu {
        adapters: vec![
            adapter("llvmpipe (LLVM 19.1.7, 256 bits)", AdapterKind::Software),
            adapter("AMD Radeon RX 7600", AdapterKind::Discrete),
        ],
        used,
        why_cpu: None,
    };
    let ctx = Context {
        gpu: Some(gpu(Some(0))),
        ..Context::default()
    };
    let mut h = checked_by(only(Arc::new(AdapterCheck)), ctx);
    until(&mut h, "the finding", |h| {
        h.query_by_label("Helios renders on a software adapter, llvmpipe (LLVM 19.1.7, 256 bits)")
            .is_some()
    });
    assert!(
        h.query_by_label_contains("AMD Radeon RX 7600 (Vulkan, Mesa 25.2) is present")
            .is_some()
    );
    let f = finding(&h, "gpu.adapter").unwrap();
    assert_eq!(f.level, Level::Warning);
    let x = f.fix.unwrap();
    assert_eq!(x.command, "links.open");
    // The command is there; a dry run opens nothing.
    let mut args = x.args.clone();
    args["dry_run"] = json!(true);
    h.state_mut().run(&x.command, args, Origin::User).unwrap();

    // On the GPU: fine, its driver said.
    let ctx = Context {
        gpu: Some(gpu(Some(1))),
        ..Context::default()
    };
    let mut h = checked_by(only(Arc::new(AdapterCheck)), ctx);
    until(&mut h, "the finding", |h| {
        h.query_by_label("Helios renders on AMD Radeon RX 7600")
            .is_some()
    });
    assert_eq!(finding(&h, "gpu.adapter").unwrap().level, Level::Ok);

    // The CPU while a GPU is there: a warning.
    let ctx = Context {
        gpu: Some(Gpu {
            why_cpu: Some("no device".into()),
            ..gpu(None)
        }),
        ..Context::default()
    };
    let mut h = checked_by(only(Arc::new(AdapterCheck)), ctx);
    until(&mut h, "the finding", |h| {
        h.query_by_label("Helios renders on the CPU").is_some()
    });
    assert_eq!(finding(&h, "gpu.adapter").unwrap().level, Level::Warning);
}

#[test]
fn a_kernel_that_does_not_compile_falls_back_to_the_cpu_and_compiles_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let kernels = Kernels::new(move || {
        c.fetch_add(1, Ordering::SeqCst);
        vec![
            Kernel {
                module: "exposure".into(),
                error: None,
            },
            Kernel {
                module: "sigmoid".into(),
                error: Some("kernel `main`: did not compile (injected)".into()),
            },
        ]
    });
    let ctx = Context {
        gpu: Some(Gpu {
            adapters: vec![adapter(
                "Microsoft Basic Render Driver",
                AdapterKind::Software,
            )],
            used: Some(0),
            why_cpu: None,
        }),
        ..Context::default()
    };
    let mut h = checked_by(only(Arc::new(kernels)), ctx);
    until(&mut h, "the finding", |h| {
        h.query_by_label("sigmoid falls back to the CPU").is_some()
    });
    assert!(
        h.query_by_label_contains("did not compile (injected)")
            .is_some()
    );
    assert!(finding(&h, "gpu.kernels/exposure").is_none());
    assert_eq!(
        finding(&h, "gpu.kernels/sigmoid").unwrap().level,
        Level::Warning
    );
    // Checked again: the same adapter and driver, nothing compiled again.
    h.state_mut()
        .run(
            helios_gui::shell::HEALTH_CHECK_NOW,
            serde_json::Value::Null,
            Origin::User,
        )
        .unwrap();
    until(&mut h, "a second run", |h| {
        h.state().health().is_some_and(|r| !r.running()) && calls.load(Ordering::SeqCst) == 1
    });
    for _ in 0..10 {
        h.step();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "compiled once per adapter");
}

fn plugin(id: &str, name: &str, status: Status, log: &[&str]) -> Plugin {
    Plugin {
        id: id.into(),
        name: name.into(),
        version: "1.2.0".into(),
        dir: std::env::temp_dir().join(id),
        status,
        log: log.iter().map(|l| (*l).to_string()).collect(),
    }
}

#[test]
fn each_plugin_answers_or_says_why_with_its_fix() {
    let ctx = Context {
        plugins: vec![
            plugin("org.example.fine", "Fine", Status::Answers, &[]),
            plugin(
                "org.example.old",
                "Old",
                Status::Unsupported(vec![
                    "interface `importer` version 9 is not supported (supported: [1, 2])".into(),
                ]),
                &[],
            ),
            plugin(
                "org.example.crash",
                "Crash",
                Status::Crashed("plugin `org.example.crash` did not initialize".into()),
                &["Traceback (most recent call last)", "ValueError: boom"],
            ),
        ],
        ..Context::default()
    };
    let mut h = checked_by(only(Arc::new(Plugins)), ctx);
    until(&mut h, "the findings", |h| {
        h.query_by_label("Crash does not start").is_some()
    });
    assert!(h.query_by_label("Fine answers").is_some());
    assert!(h.query_by_label("Old does not suit this Helios").is_some());
    assert!(
        h.query_by_label_contains("ValueError: boom").is_some(),
        "its log"
    );
    assert!(
        h.query_by_label_contains("version 9 is not supported")
            .is_some()
    );
    assert_eq!(
        finding(&h, "plugins.org.example.fine").unwrap().level,
        Level::Ok
    );
    let old = finding(&h, "plugins.org.example.old").unwrap();
    assert_eq!(old.level, Level::Problem);
    assert_eq!(old.fix.unwrap().command, "library.openFolder");
    let crash = finding(&h, "plugins.org.example.crash").unwrap();
    assert_eq!(crash.level, Level::Problem);
    let x = crash.fix.unwrap();
    assert_eq!(x.command, "plugin.disable");
    assert_eq!(x.args["plugin"], "org.example.crash");
}

#[test]
fn without_catalogue_snapshots_one_line_says_none_yet() {
    let lib = Library::new("snapshots");
    let ctx = lib.context(&[&lib.studio], FIRST);
    let mut h = checked_by(only(Arc::new(SnapshotsCheck)), ctx.clone());
    until(&mut h, "the finding", |h| {
        h.query_by_label("No catalogue snapshots yet").is_some()
    });
    assert_eq!(finding(&h, "catalogue.snapshots").unwrap().level, Level::Ok);

    // Once snapshots exist, an old one is a warning with its folder.
    let day = 86_400;
    let file = lib.dir.join("snapshots/catalogue-1.sqlite");
    let ctx = Context {
        snapshots: Some(Snapshots {
            list: vec![Snapshot {
                file: file.clone(),
                taken: FIRST,
                verified: Some(FIRST),
                restorable: true,
            }],
            now: FIRST + 30 * day,
            max_age_days: helios_health::snapshots::MAX_AGE_DAYS,
        }),
        ..ctx
    };
    let mut h = checked_by(only(Arc::new(SnapshotsCheck)), ctx);
    until(&mut h, "the finding", |h| {
        h.query_by_label("The last catalogue snapshot is 30 days old")
            .is_some()
    });
    let f = finding(&h, "catalogue.snapshots").unwrap();
    assert_eq!(f.level, Level::Warning);
    assert_eq!(f.fix.unwrap().command, "library.openFolder");
}

/// A snapshot taken by hand waits for a writer holding the catalogue on
/// a thread of its own, never on the frame that asked; taken once it
/// lets go.
#[test]
fn a_snapshot_by_hand_waits_for_a_held_catalogue_off_the_frame() {
    use helios_gui::catalogue;
    use helios_vault::Store as _;
    let lib = Library::new("snapshot-held");
    let bus = Arc::new(helios_core::Bus::new());
    let node = Arc::new(helios_inspect::client::Client::in_process(Arc::new(
        helios_node_core::endpoint::VaultService::new(
            lib.vault.clone(),
            bus.clone(),
            helios_core::jobs::Jobs::new(bus),
            "test",
        ),
    )));
    let mut other = helios_vault_sqlite::SqliteStore::open(&lib.file).unwrap();
    other.begin().unwrap();
    other
        .set_document("library/elsewhere", "held", Some(&serde_json::json!(true)))
        .unwrap();
    let (send, taken) = std::sync::mpsc::channel();
    let started = std::time::Instant::now();
    catalogue::take_by_hand(&node, &lib.file, 3, move |t| {
        let _ = send.send(t);
    })
    .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "the caller waited {:?}",
        started.elapsed()
    );
    other.rollback().unwrap();
    drop(other);
    let (file, _) = taken
        .recv_timeout(std::time::Duration::from_secs(20))
        .unwrap()
        .unwrap();
    assert!(file.is_file(), "{file:?}");
}

#[test]
fn catalogue_snapshots_taken_fill_the_check_with_their_copies_on_backup_disks() {
    use helios_gui::catalogue;
    let lib = Library::new("snapshots-taken");
    lib.strategy(Some(&Strategy::default()));
    let mounted = [lib.studio.clone(), lib.backup.clone()];
    // The strategy and the disks read through the library's node.
    let bus = Arc::new(helios_core::Bus::new());
    let node = helios_inspect::client::Client::in_process(Arc::new(
        helios_node_core::endpoint::VaultService::new(
            lib.vault.clone(),
            bus.clone(),
            helios_core::jobs::Jobs::new(bus),
            "test",
        ),
    ));
    let (taken, copies) = catalogue::take(&node, &mounted, &lib.file, "by-hand", 3).unwrap();
    assert_eq!(
        copies,
        [lib.dir
            .join("backup/Helios Backup/catalogue/library")
            .join(taken.file_name().unwrap())]
    );
    // Listed next to the catalogue, then on the backup disk.
    let listed = catalogue::list(&lib.file, &mounted);
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].file, taken);
    assert_eq!(listed[0].disk, None);
    assert_eq!(listed[1].disk.as_deref(), Some("Backup"));
    let check = |now: u64| {
        let ctx = Context {
            vault: Some(lib.file.clone()),
            snapshots: Some(catalogue::health(&lib.file, &mounted, now)),
            ..Context::default()
        };
        SnapshotsCheck.run(&ctx).remove(0)
    };
    let when = listed[0].taken;
    let found = check(when + 60);
    assert_eq!(found.level, Level::Ok);
    assert_eq!(found.message.key, "health-snapshots-ok");
    assert_eq!(check(when + 10 * 86_400).level, Level::Warning);
    // A newest snapshot that no longer reads back cannot be restored.
    for l in &listed {
        std::fs::write(&l.file, b"broken").unwrap();
    }
    let found = check(when + 60);
    assert_eq!(found.level, Level::Problem);
    assert_eq!(found.message.key, "health-snapshots-broken");
}

/// A fake module's check finding `message` at `level`.
struct Said {
    id: &'static str,
    level: Level,
    message: &'static str,
}

impl Check for Said {
    fn id(&self) -> &str {
        self.id
    }

    fn title(&self) -> Message {
        Message::plain(self.id)
    }

    fn declaration(&self) -> Declaration {
        Declaration::new("A fake module's state").levels(&[self.level])
    }

    fn run(&self, _: &Context) -> Vec<Finding> {
        vec![Finding::new(
            self.id,
            "",
            self.level,
            Message::plain(self.message),
            Message::plain(""),
        )]
    }
}

#[test]
fn the_page_groups_the_findings_by_their_owner() {
    let mut registry = Registry::default();
    let said = |id, level, message| Arc::new(Said { id, level, message }) as Arc<dyn Check>;
    // Owners the page has a group for (Helios's backups and GPU modules)
    // and one it has not (a test's).
    registry
        .add(
            "gpu",
            said("gpu.fake", Level::Ok, "The fake adapter renders"),
        )
        .unwrap();
    registry
        .add(
            "backup",
            said("backup.fake", Level::Warning, "The fake disk is away"),
        )
        .unwrap();
    registry.add("test", Arc::new(Broken)).unwrap();
    let mut h = checked_by(registry, Context::default());
    until(&mut h, "the findings", |h| {
        h.query_by_label_contains("The flux capacitor is unplugged")
            .is_some()
    });
    // A heading is its words alone; a finding's row holds its message.
    let heading = |label: &str| h.get_by_label(label).rect().center().y;
    let row = |label: &str| h.get_by_label_contains(label).rect().center().y;
    // Each finding under its owner's heading, the groups in the page's
    // order: Helios's modules, then the other checks.
    let order = [
        heading("Backups"),
        row("The fake disk is away"),
        heading("GPU"),
        row("The fake adapter renders"),
        heading("Other checks"),
        row("The flux capacitor is unplugged"),
    ];
    assert!(order.is_sorted(), "{order:?}");
    let read = h
        .state_mut()
        .run(
            helios_gui::shell::HEALTH_FINDINGS,
            serde_json::Value::Null,
            Origin::User,
        )
        .unwrap();
    assert_eq!(read["owners"]["backup"][0]["id"], "backup.fake");
    assert_eq!(read["owners"]["gpu"][0]["id"], "gpu.fake");
    assert_eq!(read["others"][0]["id"], "test.broken");
    // A module without findings has no rows, so no group.
    assert!(read["owners"].get("catalogue").is_none());
}

/// The library folder, kept in memory for the test.
struct Folder(Arc<std::sync::Mutex<Option<PathBuf>>>);

impl helios_gui::shell::LibraryFolder for Folder {
    fn get(&self) -> Result<Option<PathBuf>, String> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn set(&self, dir: &Path) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(dir.to_path_buf());
        Ok(())
    }
}

#[test]
fn no_library_folder_is_a_warning_until_one_is_chosen() {
    let folder = Arc::new(std::sync::Mutex::new(None));
    let read = folder.clone();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_import_library_folder(Arc::new(Folder(folder.clone())));
    shell.set_health(Runner::new(
        Registry::builtin(),
        move || Context {
            library_folder: Some(read.lock().unwrap().clone()),
            ..Context::default()
        },
        Findings::default(),
        Schedule::new(Instant::now(), Duration::ZERO, Duration::from_hours(1)),
    ));
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    until(&mut h, "the warning", |h| {
        finding(h, "library.folder").is_some_and(|f| f.level == Level::Warning)
    });
    let x = finding(&h, "library.folder").unwrap().fix.unwrap();
    assert_eq!(x.command, helios_health::checks::CHOOSE_LIBRARY_FOLDER);

    // Chosen (as the setup's answer or an agent's `path`): the checks
    // run again and the warning clears.
    let chosen = std::env::temp_dir().join(format!("helios-health-folder-{}", std::process::id()));
    h.state_mut()
        .run(
            &x.command,
            json!({ "path": chosen.display().to_string() }),
            Origin::User,
        )
        .unwrap();
    h.run();
    assert_eq!(folder.lock().unwrap().as_deref(), Some(chosen.as_path()));
    until(&mut h, "the folder chosen", |h| {
        finding(h, "library.folder").is_some_and(|f| f.level == Level::Ok)
    });
}
