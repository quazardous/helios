//! The Storage page (the Settings app's Storage and Backup scopes) run
//! without the desktop, on the feature host (`helios_feature_host`): its
//! commands, its link to a node over a temporary vault and fake volumes,
//! its points on the application's bus, nothing else. The tests that
//! need the Library, the import window, the health check or the
//! desktop's dialogs stay on a whole desktop (helios-gui's
//! `tests/it/storage.rs`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{By, Queryable as _};
use egui_pop::api::AppHandle;
use egui_pop::api::admin::model::Manifest;
use egui_pop::api::bus::bus_contract::Bus as ContractBus;
use egui_pop::egui::{self, vec2};
use helios_core::Message;
use helios_feature_host::{self as feature_host, Feature, Hosted, ReadPolicies, Register};
use helios_inspect::client::Client;
use helios_storage::strategy::{self, Scope, Strategy, Trigger};
use helios_storage::{
    Identity, Role, Volume, VolumeId, VolumeKind, Volumes, detect, registry, scan,
};
use helios_vault::Vault;
use helios_vault_sqlite::SqliteStore;
use serde_json::json;

use helios_storage_ui::admin::{
    STRATEGY as BACKUP_STRATEGY_COMMAND, VOLUME_ROLE as VOLUME_ROLE_COMMAND,
    VOLUMES as VOLUMES_COMMAND, register_commands,
};
use helios_storage_ui::{NodeStorage, StorageLink, StoragePage, settings};

/// Volumes plugged in and out by the test, read as slowly as it says
/// (a network drive's free space).
#[derive(Default)]
struct Plugged(Mutex<Vec<Volume>>, Mutex<Duration>);

impl Volumes for Plugged {
    fn volumes(&self) -> Vec<Volume> {
        let slow = *self.1.lock().unwrap();
        std::thread::sleep(slow);
        self.0.lock().unwrap().clone()
    }
}

impl Plugged {
    fn set(&self, volumes: &[&Volume]) {
        *self.0.lock().unwrap() = volumes.iter().map(|v| (*v).clone()).collect();
    }
}

fn disk(id: &str, name: &str, mount: &Path) -> Volume {
    Volume {
        id: VolumeId(id.into()),
        identified_by: Identity::Uuid,
        name: name.into(),
        label: Some(name.into()),
        kind: VolumeKind::External,
        capacity: Some(1_000_000_000),
        free: Some(400_000_000),
        mount: Some(mount.to_path_buf()),
        ..detect::weak(name, None)
    }
}

/// When the first scan ran: noon of 1970-01-11 (UTC), that day in the
/// time zones within twelve hours of it.
const FIRST: u64 = 86_400 * 10 + 43_200;
const DAY: u64 = 86_400;

/// A fresh folder for one test.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "helios-gui-storage-feature-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The storage feature as the host runs it: the Storage page's state,
/// its link, its commands, its points.
struct Storage {
    page: StoragePage,
    link: Arc<NodeStorage>,
}

impl Feature for Storage {
    fn register(&mut self, register: &mut Register<'_>, i18n: &egui_pop::i18n::I18n) {
        let link: Arc<dyn StorageLink> = self.link.clone();
        register_commands(register, &link, i18n, (&self.page.owners, &self.page.cards));
    }

    /// The Storage and Backup scopes, with their pages and the backup
    /// wizard, as the desktop's manifest places them.
    fn settings(&self, m: Manifest, t: &dyn Fn(Message) -> String) -> Manifest {
        m.scope_at("storage", "Storage", "disc", 40)
            .scope_at("backup", "Backup", "lock", 50)
            .section("storage/volumes", "Volumes", |s| {
                settings::volumes_section(s, t, "library.forgetVolume");
            })
            .section("backup/strategy", "Strategy", |s| {
                settings::strategy_section(s, t);
            })
            .wizard(settings::backup_wizard(t))
    }

    fn reads(&self) -> ReadPolicies {
        helios_storage_ui::READ_POLICIES
    }

    fn connect(&mut self, _app: &AppHandle, bus: &dyn ContractBus) {
        helios_storage_ui::read::declare(bus);
        helios_storage_ui::backup_end::declare(bus);
        self.page.attach_link(self.link.clone(), bus);
    }

    fn frame(&mut self, app: &AppHandle) {
        self.page.follow_link(app);
    }
}

type H<'a> = Harness<'a, Hosted<Storage>>;

/// A vault with one photo on the "Backup" disk, two disks known (the
/// "Studio" disk and "Backup"), and the storage feature over it.
struct World {
    dir: PathBuf,
    vault: Arc<Vault<SqliteStore>>,
    _library: helios_node_core::desktop::Library<SqliteStore>,
    _node: Arc<Client>,
    plugged: Arc<Plugged>,
    link: Arc<NodeStorage>,
    studio: Volume,
}

impl World {
    fn new(name: &str, size: egui::Vec2) -> (Self, H<'static>) {
        let dir = folder(name);
        let (a, b) = (dir.join("studio"), dir.join("backup"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let file = b.join("photo.jpg");
        std::fs::write(&file, b"not really a jpeg, enough for the vault").unwrap();
        let library = helios_node_core::desktop::Library::on_store(
            SqliteStore::open(&dir.join("vault.db")).unwrap(),
            Arc::new(helios_core::Bus::new()),
        );
        let vault = library.vault().clone();
        let node = Arc::new(Client::in_process(Arc::new(library.service("test"))));
        vault.import_file(&file, 1).unwrap();
        let studio = disk("uuid:studio", "Studio", &a);
        let backup = disk("uuid:backup", "Backup", &b);
        let plugged = Arc::new(Plugged::default());
        plugged.set(&[&studio, &backup]);
        scan::scan(&vault, &plugged.volumes(), "studio-pc", FIRST, None).unwrap();
        let link = Arc::new(NodeStorage::new(&node, plugged.clone(), "test"));
        assert!(
            link.settle(Duration::from_secs(10)),
            "read through the node"
        );
        let feature = Storage {
            page: StoragePage::default(),
            link: link.clone(),
        };
        let h = feature_host::harness(feature, size);
        assert!(link.settle(Duration::from_secs(10)));
        let world = Self {
            dir,
            vault,
            _library: library,
            _node: node,
            plugged,
            link,
            studio,
        };
        (world, h)
    }

    /// Plugs in `volumes` (the others out) and scans, as the background
    /// scan does; the Storage page reads them again.
    fn plug(&mut self, h: &mut H<'_>, volumes: &[&Volume], now: u64) {
        self.plugged.set(volumes);
        scan::scan(&self.vault, &self.plugged.volumes(), "studio-pc", now, None).unwrap();
        refresh_volumes(h);
        self.settle(h);
    }

    /// Waits for the Storage link's reads through the node, then shows
    /// them as a frame does.
    fn settle(&self, h: &mut H<'_>) {
        assert!(
            self.link.settle(Duration::from_secs(10)),
            "the node answers"
        );
        h.run();
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The volumes read again, as a scan's `library.changed` asks.
fn refresh_volumes(h: &mut H<'_>) {
    let bus = h.state().handle().bus();
    h.state().feature.page.refresh_volumes(&*bus);
}

/// Steps frames until `done` holds, for ten seconds at most.
fn until(h: &mut H<'_>, what: &str, done: impl Fn(&H<'_>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("never got there: {what}");
}

fn known(vault: &Vault<SqliteStore>, id: &str) -> Volume {
    registry::known(vault)
        .unwrap()
        .into_iter()
        .find(|v| v.id.as_str() == id)
        .unwrap()
}

/// Runs a command as the user does.
fn run(h: &mut H<'_>, id: &str, args: serde_json::Value) -> Result<serde_json::Value, String> {
    h.state_mut().run(id, args)
}

/// The Studio disk's row: its name, edited in place.
fn studio_row(h: &H<'_>) -> bool {
    h.query(
        By::new()
            .role(egui::accesskit::Role::TextInput)
            .value("Studio"),
    )
    .is_some()
}

/// The read kernel the Settings app reads through.
fn reads(h: &H<'_>) -> Arc<egui_pop::api::reads::ReadKernel> {
    h.state()
        .handle()
        .service(&egui_pop::api::admin::ADMIN)
        .and_then(|a| a.reads())
        .expect("the read kernel")
}

/// The page's values are read away from the frame (the read kernel, the
/// storage link's thread): with the volumes slow to read, opening the
/// page does not wait for them, and a disk unplugged updates its row
/// while the rows read before stay on the page.
#[test]
fn the_page_draws_its_labels_at_once_and_a_refresh_never_blanks_it() {
    let (world, mut h) = World::new("kernel", vec2(1600.0, 900.0));
    h.run();
    *world.plugged.1.lock().unwrap() = Duration::from_millis(1500);
    let started = Instant::now();
    run(&mut h, "admin.open", json!({ "path": "storage/volumes" })).unwrap();
    h.step();
    h.step();
    assert!(
        started.elapsed() < Duration::from_millis(1200),
        "the frame waited for the volumes: {:?}",
        started.elapsed()
    );
    // Its labels, drawn from the page's declaration whatever was read.
    until(&mut h, "the labels", |h| {
        h.query_all_by_label("Volumes").next().is_some()
            && h.query_all_by_label("Space").next().is_some()
    });
    until(&mut h, "the rows", |h| {
        studio_row(h) && h.query_by_label_contains("2 online, 0 offline").is_some()
    });
    // The backup disk goes: its row says so, the page never blanks
    // while the volumes are read again.
    let studio = world.studio.clone();
    world.plugged.set(&[&studio]);
    scan::scan(
        &world.vault,
        std::slice::from_ref(&studio),
        "studio-pc",
        FIRST + DAY,
        None,
    )
    .unwrap();
    refresh_volumes(&mut h);
    assert!(world.link.settle(Duration::from_secs(10)));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        h.step();
        assert!(studio_row(&h), "a refresh blanked the volumes");
        assert!(
            h.query_all_by_label("Volumes").next().is_some(),
            "nor its status's label"
        );
        if h.query_by_label_contains("1 online, 1 offline").is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "the unplugged disk never showed");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        h.query_by_label_contains("Last seen 1970-01-11 on studio-pc")
            .is_some(),
        "the backup disk's row is updated"
    );
    assert!(reads(&h).fetches() >= 2, "read again after the event");
}

/// Backup › Strategy edits the strategy in place: turned on, then one
/// of its triggers off, each saved at once.
#[test]
fn the_strategy_is_edited_in_the_settings_app() {
    let (world, mut h) = World::new("strategy", vec2(1280.0, 800.0));
    h.run();
    run(&mut h, "admin.open", json!({ "path": "backup/strategy" })).unwrap();
    let on = "Keep verified backup copies of every photo";
    until(&mut h, "the strategy", |h| h.query_by_label(on).is_some());
    h.get_by_label(on).click();
    until(&mut h, "turned on", |_| {
        strategy::load(&world.vault, &Scope::Library)
            .unwrap()
            .is_some()
    });
    until(&mut h, "its parts", |h| {
        h.query_by_label("After each import").is_some()
    });
    h.get_by_label("After each import").click();
    until(&mut h, "the trigger off", |_| {
        strategy::load(&world.vault, &Scope::Library)
            .unwrap()
            .is_some_and(|s| !s.triggers.contains(&Trigger::OnImport))
    });
}

#[test]
fn the_storage_scope_reads_the_volumes_and_the_strategy_from_commands() {
    let (mut world, mut h) = World::new("admin", vec2(1280.0, 900.0));
    let studio = world.studio.clone();
    world.plug(&mut h, &[&studio], FIRST + DAY);

    // Declared read-only to egui-pop: what the Settings app may read.
    let app = h.state().handle().clone();
    let spec = |id: &str| {
        app.command(&egui_pop::model::ids::CommandId::new(id).unwrap())
            .unwrap()
    };
    assert!(spec(VOLUMES_COMMAND).read_only);
    assert!(spec(BACKUP_STRATEGY_COMMAND).read_only);
    assert!(!spec(VOLUME_ROLE_COMMAND).read_only);

    // The volumes as rows: online first, with their state's tone.
    let answer = run(&mut h, VOLUMES_COMMAND, json!(null)).unwrap();
    let rows = answer["volumes"].as_array().unwrap();
    let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
    assert_eq!(names, ["Studio", "Backup"]);
    assert_eq!(rows[0]["state"], "online");
    assert_eq!(rows[1]["state"], "offline");
    assert!(rows[1]["tone"].is_null());
    assert_eq!(rows[1]["photos"], 1);

    // A role given through its command is saved.
    run(
        &mut h,
        VOLUME_ROLE_COMMAND,
        json!({ "volume": "uuid:studio", "role": "backup" }),
    )
    .unwrap();
    assert!(world.link.settle(Duration::from_secs(10)));
    assert!(
        known(&world.vault, "uuid:studio")
            .roles
            .contains(&Role::Backup)
    );

    // The strategy: none, then the default one, its disk plugged in.
    let none = run(&mut h, BACKUP_STRATEGY_COMMAND, json!(null)).unwrap();
    assert_eq!(none["set"], false);
    strategy::save(&world.vault, &Scope::Library, Some(&Strategy::default())).unwrap();
    refresh_volumes(&mut h);
    world.settle(&mut h);
    let set = run(&mut h, BACKUP_STRATEGY_COMMAND, json!(null)).unwrap();
    assert_eq!(set["copies"], 1);
    assert_eq!(set["online"], 1, "the studio disk takes copies now");
    assert_eq!(set["summary"]["tone"], "success");

    // The Settings app's Storage scope shows the table from the command.
    run(&mut h, "admin.open", json!({ "path": "storage/volumes" })).unwrap();
    until(&mut h, "the volume table", |h| {
        studio_row(h) && h.query_by_label("offline").is_some()
    });
    h.run();
}

/// The backup wizard, opened by "Set up backups…" on Backup › Strategy:
/// Next waits for the disks to copy to, the off-site step comes only for
/// two copies or more, and finishing sets the whole strategy through
/// `library.setUpBackups`.
#[test]
fn the_backup_wizard_sets_up_the_strategy() {
    use egui::accesskit::Role as A11y;
    use egui_kittest::kittest::NodeT as _;

    let (mut world, mut h) = World::new("wizard", vec2(1280.0, 900.0));
    let studio = world.studio.clone();
    world.plug(&mut h, &[&studio], FIRST + DAY);
    run(&mut h, "admin.open", json!({ "path": "backup/strategy" })).unwrap();
    until(&mut h, "the strategy page", |h| {
        h.query_by_label("Set up backups…").is_some()
    });
    h.get_by_label("Set up backups…").click();
    until(&mut h, "the wizard", |h| {
        h.query_by_role_and_label(A11y::ListItem, "Step 1 of 5, Copies")
            .is_some()
    });
    // Two copies by default: Next, then the disks are required.
    h.get_by_label("Next").click();
    until(&mut h, "the disks step", |h| {
        h.query_all_by_label("Copy to: required.").next().is_some()
    });
    assert!(h.get_by_label("Next").accesskit_node().is_disabled());
    let state = |h: &mut H<'_>, cmd: &str, args: serde_json::Value| {
        let out = run(h, cmd, args);
        h.run();
        out
    };
    assert_eq!(
        state(&mut h, "admin.wizard.next", json!({})).unwrap_err(),
        "Copy to: required."
    );
    // One copy: no off-site step.
    let s = state(
        &mut h,
        "admin.wizard.set",
        json!({ "values": { "destination": "backup", "copies": 1 } }),
    )
    .unwrap();
    assert_eq!(s["count"], 3, "{s}");
    let s = state(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "copies", "value": 3 }),
    )
    .unwrap();
    assert_eq!(s["count"], 4);
    let s = state(&mut h, "admin.wizard.next", json!({})).unwrap();
    assert_eq!(s["step"], "off-site");
    state(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "offSite", "value": true }),
    )
    .unwrap();
    state(
        &mut h,
        "admin.wizard.set",
        json!({ "key": "onConnect", "value": false }),
    )
    .unwrap();
    let s = state(&mut h, "admin.wizard.go", json!({ "step": "summary" })).unwrap();
    assert_eq!(s["canFinish"], true);
    until(&mut h, "the summary", |h| {
        h.query_by_role_and_label(A11y::Group, "Copy to: Any disk with the backup role")
            .is_some()
    });
    // Finishing is the button's click, as a user does it: the toast the
    // refused Next left in the corner keeps off the footer.
    let app = h.state().handle().clone();
    let toasts = app.service(&egui_pop::api::toasts::TOASTS).unwrap();
    assert!(!toasts.shown().is_empty(), "the refused Next's toast");
    h.get_by_label("Use this strategy").click();
    until(&mut h, "the strategy saved", |_| {
        strategy::load(&world.vault, &Scope::Library)
            .unwrap()
            .is_some()
    });
    let s = strategy::load(&world.vault, &Scope::Library)
        .unwrap()
        .unwrap();
    assert_eq!(s.copies, 3);
    assert!(s.off_site_required);
    assert_eq!(s.destinations, [strategy::Destination::Role(Role::Backup)]);
    assert!(s.triggers.contains(&Trigger::OnImport));
    assert!(!s.triggers.contains(&Trigger::OnConnect));
    // Opened from the Settings app: back on its page.
    assert!(egui_pop::api::admin::is_open(&app));
    // An agent finishes the same wizard with its command.
    state(
        &mut h,
        "admin.wizard.open",
        json!({ "id": "backup.setup", "values": { "copies": 1, "destination": "archive" } }),
    )
    .unwrap();
    let s = state(&mut h, "admin.wizard.go", json!({ "step": "summary" })).unwrap();
    assert_eq!(s["canFinish"], true, "{s}");
    let done = state(&mut h, "admin.wizard.finish", json!({})).unwrap();
    assert_eq!(done["command"], "library.setUpBackups", "{done}");
    until(&mut h, "the strategy set again", |_| {
        strategy::load(&world.vault, &Scope::Library)
            .unwrap()
            .is_some_and(|s| s.copies == 1)
    });
    let s = strategy::load(&world.vault, &Scope::Library)
        .unwrap()
        .unwrap();
    assert_eq!(s.destinations, [strategy::Destination::Role(Role::Archive)]);
    assert!(!s.off_site_required);
}

/// The Storage page reaches the vault through the desktop's node, off
/// the frame: while another writer holds the vault file locked, a role
/// given, a strategy set and the volumes read again cost no frame; once it
/// lets go, they are kept and shown.
#[test]
fn a_vault_held_locked_does_not_delay_the_storage_pages_frames() {
    use helios_vault::Store as _;

    let (world, mut h) = World::new("held", vec2(1280.0, 800.0));
    h.run();

    // Another writer holds the vault's file.
    let mut other = SqliteStore::open(&world.dir.join("vault.db")).unwrap();
    other.begin().unwrap();
    other
        .set_document("library/elsewhere", "held", Some(&json!(true)))
        .unwrap();
    let started = Instant::now();
    run(
        &mut h,
        VOLUME_ROLE_COMMAND,
        json!({ "volume": "uuid:studio", "role": "backup" }),
    )
    .unwrap();
    run(
        &mut h,
        "library.setUpBackups",
        json!({ "copies": 1, "destination": "backup" }),
    )
    .unwrap();
    refresh_volumes(&mut h);
    let bus = h.state().handle().bus();
    h.state().feature.page.refresh_backups(&*bus);
    for _ in 0..3 {
        h.run();
    }
    let slowest = started.elapsed();
    assert!(
        world.link.busy(),
        "the writes wait for the vault, not the frame"
    );
    within(
        "the frames with the vault held",
        slowest,
        Duration::from_secs(1),
    );
    // Shown at once all the same.
    let page = &h.state().feature.page;
    assert!(page.strategy().is_some());
    assert!(
        page.volumes()
            .iter()
            .any(|v| v.id.as_str() == "uuid:studio" && v.roles.contains(&Role::Backup)),
        "{:?}",
        page.volumes()
    );

    // Let go: kept.
    other.rollback().unwrap();
    drop(other);
    world.settle(&mut h);
    assert!(
        known(&world.vault, "uuid:studio")
            .roles
            .contains(&Role::Backup)
    );
    assert!(
        strategy::load(&world.vault, &Scope::Library)
            .unwrap()
            .is_some()
    );
}

/// Read again (a change, a node's `storage.*` event), the Storage page
/// never goes blank: frame by frame its rows stay, marked "Refreshing…"
/// while they are read, and are replaced in place.
#[test]
fn a_refresh_never_blanks_the_page() {
    let (world, mut h) = World::new("refresh", vec2(1600.0, 900.0));
    world.settle(&mut h);
    run(&mut h, "admin.open", json!({ "path": "storage/volumes" })).unwrap();
    until(&mut h, "the page", studio_row);
    let admin = h
        .state()
        .handle()
        .service(&egui_pop::api::admin::ADMIN)
        .unwrap();
    for round in 0..3 {
        refresh_volumes(&mut h);
        admin.refresh();
        for frame in 0..8 {
            h.step();
            assert!(
                studio_row(&h),
                "round {round}, frame {frame}: the rows left the page"
            );
        }
        world.settle(&mut h);
        assert!(studio_row(&h));
    }
    until(&mut h, "the mark gone", |h| {
        h.query_by_label_contains("Refreshing").is_none()
    });
}

/// Checks that `took` is under `budget`: fails over it, or warns on a
/// machine shared with other jobs (`HELIOS_SHARED_MACHINE`), as the
/// integration tests' `wall_clock::within` does.
fn within(what: &str, took: Duration, budget: Duration) {
    if took < budget {
        return;
    }
    let message = format!("{what}: took {took:?}, over {budget:?}");
    let shared =
        std::env::var_os("HELIOS_SHARED_MACHINE").is_some_and(|v| !v.is_empty() && v != "0");
    if shared {
        eprintln!("warning (shared machine, budget not enforced): {message}");
    } else {
        panic!("{message}");
    }
}
