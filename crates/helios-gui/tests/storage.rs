//! The Storage page and offline photos, over a temporary vault and fake
//! volumes: the page lists an online and an offline volume, and saves a
//! new name and a role; a photo on an unplugged disk shows its "offline"
//! badge and says which disk to plug in; plugging it in clears the badge.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, Queryable as _};
use helios_core::Bus;
use helios_gui::persist::Persist;
use helios_gui::shell::{STORAGE_COMMAND, VaultStorage};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use helios_storage::{
    Identity, Role, Volume, VolumeId, VolumeKind, Volumes, detect, registry, scan,
};
use helios_vault::Vault;
use helios_vault_sqlite::SqliteStore;
use serde_json::json;

/// Volumes plugged in and out by the test.
#[derive(Default)]
struct Plugged(Mutex<Vec<Volume>>);

impl Volumes for Plugged {
    fn volumes(&self) -> Vec<Volume> {
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
    let dir =
        std::env::temp_dir().join(format!("helios-gui-storage-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A vault with one photo on the "Backup" disk, two disks known (the
/// "Studio" disk and "Backup"), and a shell over it.
struct World {
    dir: PathBuf,
    vault: Arc<Vault<SqliteStore>>,
    persist: Persist<SqliteStore>,
    plugged: Arc<Plugged>,
    studio: Volume,
    backup: Volume,
    photo: String,
}

impl World {
    fn new(name: &str) -> (Self, Shell) {
        let dir = folder(name);
        let (a, b) = (dir.join("studio"), dir.join("backup"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let file = b.join("photo.jpg");
        std::fs::write(&file, b"not really a jpeg, enough for the vault").unwrap();
        let persist = Persist::new(
            SqliteStore::open(&dir.join("vault.db")).unwrap(),
            Arc::new(Bus::new()),
        );
        let vault = persist.vault().clone();
        let photo = vault.import_file(&file, 1).unwrap().hex();
        let studio = disk("uuid:studio", "Studio", &a);
        let backup = disk("uuid:backup", "Backup", &b);
        let plugged = Arc::new(Plugged::default());
        plugged.set(&[&studio, &backup]);
        scan::scan(&vault, &plugged.volumes(), "studio-pc", FIRST, None).unwrap();
        let mut shell = Shell::new(helios_panels(), None);
        shell.set_storage(Arc::new(VaultStorage::new(
            vault.clone(),
            plugged.clone(),
            "test",
        )));
        let world = Self {
            dir,
            vault,
            persist,
            plugged,
            studio,
            backup,
            photo,
        };
        (world, shell)
    }

    /// Plugs in `volumes` (the others out) and scans, as the background
    /// scan does; the library follows as the desktop's frame does.
    fn plug(&mut self, h: &mut Harness<'_, Shell>, volumes: &[&Volume], now: u64) {
        self.plugged.set(volumes);
        scan::scan(&self.vault, &self.plugged.volumes(), "studio-pc", now, None).unwrap();
        self.persist.frame(h.state_mut(), now as f64);
        assert!(self.persist.take_locations_changed(), "the scan is heard");
        h.state_mut().refresh_volumes();
        self.persist.fill_library(h.state_mut()).unwrap();
        h.run();
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Steps frames until `done` holds, for ten seconds at most.
fn until(h: &mut Harness<'_, Shell>, what: &str, done: impl Fn(&Harness<'_, Shell>) -> bool) {
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

#[test]
fn the_page_lists_online_and_offline_volumes_and_saves_edits() {
    let (mut world, shell) = World::new("page");
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    // The backup disk goes: offline, last seen at the first scan.
    let studio = world.studio.clone();
    world.plug(&mut h, &[&studio], FIRST + DAY);
    h.state_mut()
        .run(STORAGE_COMMAND, json!(null), Origin::User)
        .unwrap();
    until(&mut h, "the page", |h| {
        h.query_by_label("Storage").is_some()
    });
    let listed: Vec<(String, bool)> = h
        .state()
        .storage_volumes()
        .iter()
        .map(|v| (v.name.clone(), v.online()))
        .collect();
    assert_eq!(
        listed,
        [("Studio".to_string(), true), ("Backup".to_string(), false)],
        "online first"
    );
    assert!(
        h.query_by_label_contains("Last seen 1970-01-11 on studio-pc")
            .is_some(),
        "the offline disk says when and where it was last seen"
    );
    assert!(h.query_by_label_contains("Photos on it: 1").is_some());

    // A role toggled is saved: the first "Backup" checkbox is the
    // studio disk's (listed first).
    h.query_all_by_role_and_label(egui::accesskit::Role::CheckBox, "Backup")
        .next()
        .expect("the studio disk's Backup role")
        .click();
    until(&mut h, "the role saved", |_| {
        known(&world.vault, "uuid:studio")
            .roles
            .contains(&Role::Backup)
    });

    // Renamed: typed in its name field, saved when it loses the focus.
    let name = || {
        By::new()
            .role(egui::accesskit::Role::TextInput)
            .value("Studio")
    };
    h.get(name()).focus();
    h.run();
    h.get(name()).type_text("-SSD");
    h.run();
    h.key_press(egui::Key::Enter);
    // Where the cursor lands on focus is egui's: the new name holds
    // what was typed.
    until(&mut h, "the name saved", |_| {
        known(&world.vault, "uuid:studio").name.contains("-SSD")
    });
    // Kept by the next scan.
    let studio = world.studio.clone();
    scan::scan(&world.vault, &[studio], "studio-pc", FIRST + 2 * DAY, None).unwrap();
    let saved = known(&world.vault, "uuid:studio");
    assert!(saved.name.contains("-SSD"));
    assert!(saved.roles.contains(&Role::Backup));
}

#[test]
fn an_offline_photo_shows_its_badge_and_plugging_its_disk_in_clears_it() {
    let (mut world2, mut shell) = World::new("badge");
    world2.persist.fill_library(&mut shell).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let photo = world2.photo.clone();
    let badges = |h: &Harness<'_, Shell>| h.state().library().badges_of(&h.ctx, &photo);
    assert!(badges(&h).is_empty(), "present: no badge");

    // Unplugged: the "offline" badge, its note, the plug-in message.
    let studio = world2.studio.clone();
    world2.plug(&mut h, &[&studio], FIRST + DAY);
    assert_eq!(badges(&h), ["offline"]);
    let note = h
        .state()
        .library()
        .presence_note(&photo)
        .map(|m| h.state().text(m));
    assert_eq!(
        note.as_deref(),
        Some("On Backup, last seen 1970-01-11 on studio-pc")
    );
    let refused = h
        .state()
        .library()
        .unavailable(&photo)
        .map(|m| h.state().text(m));
    assert_eq!(
        refused.as_deref(),
        Some("Plug in Backup to open this photo (last seen 1970-01-11 on studio-pc)")
    );

    // Plugged in again: present, the badge gone.
    let backup = world2.backup.clone();
    world2.plug(&mut h, &[&studio, &backup], FIRST + 2 * DAY);
    assert!(badges(&h).is_empty(), "plugged in: no badge");
    assert!(h.state().library().unavailable(&photo).is_none());

    // Its file deleted: the "missing" badge.
    std::fs::remove_file(world2.dir.join("backup/photo.jpg")).unwrap();
    world2.plugged.set(&[&studio, &backup]);
    scan::scan(
        &world2.vault,
        &world2.plugged.volumes(),
        "studio-pc",
        FIRST + 3 * DAY,
        None,
    )
    .unwrap();
    world2.persist.frame(h.state_mut(), 0.0);
    assert!(world2.persist.take_locations_changed());
    world2.persist.fill_library(h.state_mut()).unwrap();
    assert_eq!(badges(&h), ["missing"]);
}
