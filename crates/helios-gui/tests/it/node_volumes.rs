//! The node watches the volumes, not the import window: a card plugged
//! in while no import window is open reaches the node's bus and raises
//! the card toast, the import window opened afterwards lists it, and its
//! removal is seen too; a backup disk plugged in with no window at all
//! is backed up by the node.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::kittest::Queryable as _;
use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_gui::node::LocalNode;
use helios_gui::node_jobs::Backups;
use helios_gui::node_volumes::{self, Change, VolumeWatch};
use helios_gui::{Shell, headless, helios_panels};
use helios_library::import::{self, Options, Placement};
use helios_shell::commands::Origin;
use helios_storage::strategy::{self, Scope, Strategy, Trigger};
use helios_storage::{
    Card, Identity, Role, Volume, VolumeId, VolumeKind, Volumes, detect, registry,
};
use helios_vault::Vault;
use helios_vault_sqlite::SqliteStore;
use serde_json::json;

/// The volumes plugged in, as a test plugs and unplugs them.
#[derive(Default)]
struct Plugged(Mutex<Vec<Volume>>);

impl Plugged {
    fn plug(&self, v: Volume) {
        self.0.lock().unwrap().push(v);
    }

    fn unplug(&self, id: &VolumeId) {
        self.0.lock().unwrap().retain(|v| &v.id != id);
    }
}

impl Volumes for Plugged {
    fn volumes(&self) -> Vec<Volume> {
        self.0.lock().unwrap().clone()
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("helios-node-volumes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Helios's one bus, as `main` builds it.
fn hooks() -> Arc<dyn egui_pop::api::bus::bus_contract::Bus> {
    Arc::new(egui_pop::api::bus::egui_pop_hooks::Kernel::new())
}

fn node(dir: &Path) -> LocalNode<SqliteStore> {
    LocalNode::open(
        SqliteStore::open(&dir.join("vault.db")).unwrap(),
        Arc::new(Bus::new()),
        Duration::from_hours(1),
    )
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_mins(1);
    while !done() {
        assert!(Instant::now() < deadline, "never got there in 60 s: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The volume events the watch published so far.
fn published(watch: &VolumeWatch) -> Vec<Change> {
    watch.bus().since(0).iter().filter_map(Change::of).collect()
}

#[test]
fn a_card_plugged_in_with_the_import_window_closed_is_seen_by_the_node() {
    let dir = scratch("card");
    let node = node(&dir);
    let plugged = Arc::new(Plugged::default());
    let mut shell = Shell::new(helios_panels(), None);
    let watch = node.watch_volumes(plugged.clone(), shell.app().handle().bus());
    shell.follow_volumes(watch.clone());
    let mut h = headless::harness(shell, egui::vec2(1280.0, 800.0));
    h.run();
    until("the watch's first look", || watch.listed());
    assert!(!h.state().import_window_open());

    // A card plugged in: on the node's bus with its identity, then a
    // toast in the main window.
    let root = dir.join("EOS_DIGITAL");
    std::fs::create_dir_all(root.join("DCIM")).unwrap();
    let card = Card {
        label: "EOS_DIGITAL".into(),
        root: root.clone(),
        capacity: Some(64_000_000_000),
    };
    let volume = card.volume();
    plugged.plug(volume.clone());
    until("the card on the node's bus", || {
        published(&watch).iter().any(|c| {
            matches!(c, Change::Mounted(m)
                if m.id == volume.id && m.card.as_ref() == Some(&card) && !m.at_start)
        })
    });
    until("the card's toast", || {
        h.run();
        h.state()
            .notes
            .iter()
            .any(|n| n.contains("EOS_DIGITAL") && n.contains("inserted"))
    });
    assert_eq!(h.state().import_cards(), vec![card.clone()]);

    // The import window opened afterwards lists it.
    h.state_mut()
        .run("library.importWindow", json!(null), Origin::User)
        .unwrap();
    h.step();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !h.state().import_read() {
        assert!(Instant::now() < deadline, "the sources not read in 20 s");
        std::thread::sleep(Duration::from_millis(20));
        h.step();
    }
    for _ in 0..3 {
        h.run();
    }
    assert!(h.state().import_window_open());
    // An item says its name; its size is on its detail line.
    let _ = h.get_by_role_and_label(egui::accesskit::Role::Button, "EOS_DIGITAL");

    // Unplugged: on the bus, and gone from the window's list.
    plugged.unplug(&volume.id);
    until("the card gone", || {
        h.step();
        h.state().import_cards().is_empty()
    });
    assert!(
        published(&watch)
            .iter()
            .any(|c| matches!(c, Change::Unmounted(u) if u.id == volume.id && u.mount == root))
    );
    // Heard on Helios's bus by the Storage reads, the cards and the
    // health check.
    let heard = watch.heard();
    let gone = heard
        .iter()
        .find(|h| h.point == "volume.unmounted" && h.id == volume.id)
        .expect("volume.unmounted passed");
    assert_eq!(
        gone.ran,
        [
            node_volumes::READS,
            node_volumes::CARDS,
            node_volumes::HEALTH
        ]
    );
    drop(h);
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
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

/// Photos with a copy on `disk`.
fn backed_up(vault: &Vault<SqliteStore>, disk: &str) -> usize {
    vault
        .images()
        .unwrap()
        .iter()
        .filter(|i| {
            vault
                .locations(&i.id)
                .unwrap()
                .iter()
                .any(|l| l.volume.as_ref().is_some_and(|v| v.as_str() == disk))
        })
        .count()
}

#[test]
fn a_backup_disk_plugged_in_with_no_window_is_backed_up_by_the_node() {
    let dir = scratch("backup");
    let node = node(&dir);
    let vault = node.vault();
    let jobs = Jobs::new(Arc::new(Bus::new()));
    let lib = disk("uuid:lib", &dir.join("libdisk"), &[Role::Library]);
    let backup = disk("uuid:backup", &dir.join("backup"), &[Role::Backup]);
    registry::record(vault, &[lib.clone(), backup.clone()], "studio", 10, None).unwrap();
    for v in [&lib, &backup] {
        registry::save(vault, v).unwrap();
    }
    let roll = dir.join("roll");
    std::fs::create_dir_all(&roll).unwrap();
    for k in 0..2u8 {
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
        &jobs,
        &[roll],
        &options,
        100,
        &mut |_: &serde_json::Value| {},
    )
    .unwrap();
    let on_connect = Strategy {
        triggers: [Trigger::OnConnect].into(),
        ..Strategy::default()
    };
    strategy::save(vault, &Scope::Library, Some(&on_connect)).unwrap();

    // Only the library disk is plugged in; the node watches, no window.
    let plugged = Arc::new(Plugged::default());
    plugged.plug(lib);
    let errors: Arc<Mutex<Vec<String>>> = Arc::default();
    let said = errors.clone();
    let backups = Backups::new(vault.clone(), jobs, plugged.clone(), move |e| {
        said.lock().unwrap().push(e);
    })
    .adopted_by(&node.link());
    let hooks = hooks();
    let watch = node.watch_volumes(plugged.clone(), hooks.clone());
    backups.follow(&*hooks);
    until("the watch's first look", || watch.listed());
    assert_eq!(backed_up(vault, "uuid:backup"), 0, "nothing at start");

    plugged.plug(backup);
    until("both photos backed up", || {
        backed_up(vault, "uuid:backup") == 2
    });
    assert!(
        errors.lock().unwrap().is_empty(),
        "{:?}",
        errors.lock().unwrap()
    );
    assert!(
        published(&watch)
            .iter()
            .any(|c| matches!(c, Change::Mounted(m) if m.id.as_str() == "uuid:backup")),
        "on the node's bus"
    );
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Volumes plugged in, as a test plugs them, read slowly by the volume
/// scan's own thread: a scan that takes its time, as on a large library.
/// Whoever acts on a disk plugged in must not depend on the scan being
/// quick.
struct SlowScan {
    plugged: Arc<Plugged>,
    scan_delay: Mutex<Duration>,
}

impl Volumes for SlowScan {
    fn volumes(&self) -> Vec<Volume> {
        if std::thread::current().name() == Some("helios-volume-scan") {
            let delay = *self.scan_delay.lock().unwrap();
            std::thread::sleep(delay);
        }
        self.plugged.volumes()
    }
}

/// Where the photos' copies on `disk` are, as the vault says.
fn presence_on(vault: &Vault<SqliteStore>, disk: &str) -> Vec<helios_vault::Presence> {
    vault
        .all_locations()
        .unwrap()
        .into_iter()
        .filter(|(_, l)| l.volume.as_ref().is_some_and(|v| v.as_str() == disk))
        .map(|(_, l)| l.presence)
        .collect()
}

/// The library disk plugged in again while the backup disk is there: the
/// scan says its copies are present before the backup on connect looks
/// for them, so the backup copies both photos at once, however slow the
/// scan's own thread is.
#[test]
fn a_disk_plugged_in_is_scanned_before_its_backup_starts() {
    let dir = scratch("scan-first");
    let node = node(&dir);
    let vault = node.vault();
    let jobs = Jobs::new(Arc::new(Bus::new()));
    let lib = disk("uuid:lib", &dir.join("libdisk"), &[Role::Library]);
    let backup = disk("uuid:backup", &dir.join("backup"), &[Role::Backup]);
    registry::record(vault, &[lib.clone(), backup.clone()], "studio", 10, None).unwrap();
    for v in [&lib, &backup] {
        registry::save(vault, v).unwrap();
    }
    let roll = dir.join("roll");
    std::fs::create_dir_all(&roll).unwrap();
    for k in 0..2u8 {
        let bytes: Vec<u8> = (0..20_000u32)
            .map(|i| (i as u8).wrapping_mul(k + 3))
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
        &jobs,
        &[roll],
        &options,
        100,
        &mut |_: &serde_json::Value| {},
    )
    .unwrap();
    let on_connect = Strategy {
        triggers: [Trigger::OnConnect].into(),
        ..Strategy::default()
    };
    strategy::save(vault, &Scope::Library, Some(&on_connect)).unwrap();

    // Both disks plugged in; the first scan links the copies to the
    // library disk.
    let plugged = Arc::new(Plugged::default());
    plugged.plug(lib.clone());
    plugged.plug(backup);
    let volumes = Arc::new(SlowScan {
        plugged: plugged.clone(),
        scan_delay: Mutex::new(Duration::ZERO),
    });
    let scans = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = scans.clone();
    let scanner = node.library().scan_volumes(
        volumes.clone(),
        "studio".into(),
        Duration::from_hours(1),
        move |outcome| {
            outcome.unwrap();
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
    );
    let scanned = || scans.load(std::sync::atomic::Ordering::SeqCst);
    until("the first scan", || scanned() >= 1);
    assert_eq!(
        presence_on(vault, "uuid:lib"),
        [helios_vault::Presence::Present; 2],
        "linked to the library disk"
    );
    let errors: Arc<Mutex<Vec<String>>> = Arc::default();
    let said = errors.clone();
    let backups = Backups::new(vault.clone(), jobs, plugged.clone(), move |e| {
        said.lock().unwrap().push(e);
    })
    .adopted_by(&node.link());
    // Wired as the desktop wires them: listeners of the volume points on
    // Helios's one bus.
    let hooks = hooks();
    node_volumes::listen_scan(&*hooks, scanner).unwrap();
    backups.follow(&*hooks);
    let watch = node.watch_volumes(plugged.clone(), hooks.clone());
    until("the watch's first look", || watch.listed());

    // The library disk unplugged: its copies offline once scanned.
    plugged.unplug(&lib.id);
    until("the unplugged disk scanned", || {
        presence_on(vault, "uuid:lib") == [helios_vault::Presence::Offline; 2]
    });
    let before = scanned();
    assert_eq!(backed_up(vault, "uuid:backup"), 0, "nothing backed up yet");

    // From now on the scan's own thread is slow; the library disk is
    // plugged in again.
    *volumes.scan_delay.lock().unwrap() = Duration::from_millis(500);
    plugged.plug(lib);
    until("the plugged disk scanned", || scanned() > before);
    until("the plugged disk's listeners ran", || {
        watch
            .heard()
            .iter()
            .any(|h| h.point == "volume.mounted" && h.id.as_str() == "uuid:lib")
    });
    let heard = watch.heard();
    let plugged_in = heard
        .iter()
        .find(|h| h.point == "volume.mounted" && h.id.as_str() == "uuid:lib")
        .unwrap();
    assert_eq!(
        plugged_in.ran,
        [node_volumes::SCAN, node_volumes::BACKUPS],
        "scanned, then backed up"
    );
    assert!(
        helios_gui::node::NodeJobs::wait(&*backups, Duration::from_mins(1)),
        "the backup ended"
    );
    assert_eq!(
        backed_up(vault, "uuid:backup"),
        2,
        "the backup on connect found the copies the scan had just set present: {:?}",
        errors.lock().unwrap()
    );
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The listeners of `volume.mounted` and `volume.unmounted` as the
/// desktop registers them, in the order they run: a reviewed dump
/// (`tests/bus/volumes.txt`). The cards first, then the scan, then
/// whoever reads where the copies are.
#[test]
fn the_volume_points_listeners_run_in_a_declared_order() {
    let dir = scratch("order");
    let node = node(&dir);
    let plugged = Arc::new(Plugged::default());
    let mut shell = Shell::new(helios_panels(), None);
    let bus = shell.app().handle().bus();
    let watch = node.watch_volumes(plugged.clone(), bus.clone());
    shell.follow_volumes(watch.clone());
    let scanner = node.library().scan_volumes(
        plugged.clone(),
        "studio".into(),
        Duration::from_hours(1),
        |_| {},
    );
    node_volumes::listen_scan(&*bus, scanner).unwrap();
    let jobs = Jobs::new(Arc::new(Bus::new()));
    let backups = Backups::new(node.vault().clone(), jobs, plugged, |_| {});
    let backups = Arc::new(backups);
    backups.follow(&*bus);
    node_volumes::listen_mounted(&*bus, node_volumes::MAP, "helios.map", |_| {}).unwrap();
    let mut h = headless::harness(shell, egui::vec2(1280.0, 800.0));
    h.run();
    assert_eq!(
        node_volumes::order(&*bus),
        include_str!("../bus/volumes.txt")
    );
    drop(h);
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
