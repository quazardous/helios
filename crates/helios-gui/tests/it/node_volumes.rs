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
use helios_gui::node_volumes::{Change, VolumeWatch};
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
    let watch = node.watch_volumes(plugged.clone());
    let mut shell = Shell::new(helios_panels(), None);
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
    let watch = node.watch_volumes(plugged.clone());
    backups.follow(&watch);
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
