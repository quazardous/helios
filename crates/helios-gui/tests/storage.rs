//! The Storage page (the Settings app's Storage and Backup scopes) and
//! offline photos, over a temporary vault and fake
//! volumes: the page lists an online and an offline volume, and saves a
//! new name and a role; a photo on an unplugged disk shows its "offline"
//! badge and says which disk to plug in; plugging it in clears the badge.
//! With a backup strategy: the import window's strategy line, each photo's
//! backup badge and the `backup:` filters, and a card short of its backups
//! said safe to format once a backup job ends.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, Queryable as _};
use helios_core::Bus;
use helios_gui::persist::Persist;
use helios_gui::shell::{NodeStorage, STORAGE_COMMAND};
use helios_gui::{Shell, headless, helios_panels};
use helios_library::backup;
use helios_library::import::{self, Options, Placement};
use helios_shell::commands::Origin;
use helios_storage::strategy::{self, Scope, Strategy, Trigger};
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
    link: Arc<NodeStorage>,
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
        let link = Arc::new(NodeStorage::new(persist.node(), plugged.clone(), "test"));
        assert!(
            link.settle(Duration::from_secs(10)),
            "read through the node"
        );
        shell.set_storage(link.clone());
        assert!(link.settle(Duration::from_secs(10)));
        let world = Self {
            dir,
            vault,
            persist,
            plugged,
            link,
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
        self.persist.frame_settled(h.state_mut(), now as f64);
        assert!(self.persist.take_locations_changed(), "the scan is heard");
        h.state_mut().refresh_volumes();
        self.persist.fill_library(h.state_mut()).unwrap();
        self.settle(h);
    }

    /// Waits for the Storage link's reads through the node, then shows
    /// them as a frame does.
    fn settle(&self, h: &mut Harness<'_, Shell>) {
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

/// The Storage page is the Settings app's Storage scope: `library.storage`
/// opens it on a volume, whose row says when and where it was last seen;
/// a role toggled and a name typed in place are saved, and "Forget…"
/// asks first.
#[test]
fn the_page_lists_online_and_offline_volumes_and_saves_edits() {
    let (mut world, shell) = World::new("page");
    let mut h = headless::harness(shell, vec2(1600.0, 900.0));
    // The backup disk goes: offline, last seen at the first scan.
    let studio = world.studio.clone();
    world.plug(&mut h, &[&studio], FIRST + DAY);
    h.state_mut()
        .run(
            STORAGE_COMMAND,
            json!({ "volume": "uuid:backup" }),
            Origin::User,
        )
        .unwrap();
    until(&mut h, "the page", |h| {
        h.query_by_label_contains("Last seen 1970-01-11 on studio-pc")
            .is_some()
    });
    assert!(h.state().storage_page_open());
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
    until(&mut h, "the table read again", |h| {
        h.query(name()).is_some()
    });
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

    // "Forget…" asks first, naming the disk.
    until(&mut h, "the actions", |h| {
        h.query_all_by_label("Forget…").count() == 2
    });
    h.query_all_by_label("Forget…").nth(1).unwrap().click();
    until(&mut h, "the confirmation", |h| {
        h.query_by_label("Forget what was seen on Backup?")
            .is_some()
    });
    h.state_mut()
        .run(
            "dialog.answer",
            json!({ "answer": "primary" }),
            Origin::User,
        )
        .unwrap();
    until(&mut h, "answered", |h| {
        h.query_by_label("Forget what was seen on Backup?")
            .is_none()
    });
}

/// Backup › Strategy edits the strategy in place: turned on, then one
/// of its triggers off, each saved at once.
#[test]
fn the_strategy_is_edited_in_the_settings_app() {
    let (world, shell) = World::new("strategy");
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h.state_mut()
        .run(
            "admin.open",
            json!({ "path": "backup/strategy" }),
            Origin::User,
        )
        .unwrap();
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
    world2.persist.frame_settled(h.state_mut(), 0.0);
    assert!(world2.persist.take_locations_changed());
    world2.persist.fill_library(h.state_mut()).unwrap();
    assert_eq!(badges(&h), ["missing"]);
}

/// Gives the known volume `id` the roles `roles`.
fn give_roles(vault: &Vault<SqliteStore>, id: &str, roles: &[Role]) {
    let mut v = known(vault, id);
    v.roles = roles.iter().copied().collect();
    registry::save(vault, &v).unwrap();
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The import window's strategy line: no strategy offers the Storage
/// page; a strategy names its disks, plugged in or not seen for days.
#[test]
fn the_import_windows_strategy_line_says_what_backups_the_photos_get() {
    let (world, shell) = World::new("line");
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    // None: "No backup set up:", its button opens the Storage page.
    h.state_mut()
        .run("library.importWindow", json!(null), Origin::User)
        .unwrap();
    h.run();
    assert_eq!(
        h.state().import_backup_line(),
        ("No backup set up:".to_string(), true)
    );
    let _ = h.get_by_label("No backup set up:");
    h.get_by_label("Storage…").click_accesskit();
    until(&mut h, "the Storage page", |h| {
        h.state().storage_page_open()
    });

    // Two copies on any backup disk, both plugged in.
    give_roles(&world.vault, "uuid:studio", &[Role::Backup]);
    give_roles(&world.vault, "uuid:backup", &[Role::Backup]);
    let two = Strategy {
        copies: 2,
        ..Strategy::default()
    };
    strategy::save(&world.vault, &Scope::Library, Some(&two)).unwrap();
    h.state_mut().refresh_volumes();
    world.settle(&mut h);
    assert_eq!(
        h.state().import_backup_line(),
        (
            "2 copies: Backup (plugged in), Studio (plugged in)".to_string(),
            false
        )
    );
    h.run();
    let _ = h.get_by_label("2 copies: Backup (plugged in), Studio (plugged in)");

    // The studio disk last seen three days ago, unplugged since.
    let (studio, backup) = (world.studio.clone(), world.backup.clone());
    world.plugged.set(&[&studio, &backup]);
    scan::scan(
        &world.vault,
        &world.plugged.volumes(),
        "studio-pc",
        now() - 3 * DAY - 60,
        None,
    )
    .unwrap();
    world.plugged.set(&[&backup]);
    h.state_mut().refresh_volumes();
    world.settle(&mut h);
    assert_eq!(
        h.state().import_backup_line().0,
        "2 copies: Backup (plugged in), Studio (not seen for 3 days)"
    );
}

/// Each photo's backups as a badge in the grid, once the library has a
/// strategy, and the `backup:` filters of the search field.
#[test]
fn photos_show_their_backups_and_the_search_filters_on_them() {
    let (mut world, mut shell) = World::new("backups");
    // A second photo, on the studio disk only.
    let other_file = world.dir.join("studio/other.jpg");
    std::fs::write(&other_file, b"another photo, on the studio disk").unwrap();
    let other = world.vault.import_file(&other_file, 2).unwrap().hex();
    scan::scan(
        &world.vault,
        &world.plugged.volumes(),
        "studio-pc",
        FIRST + 1,
        None,
    )
    .unwrap();
    give_roles(&world.vault, "uuid:backup", &[Role::Backup]);
    world.persist.fill_library(&mut shell).unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let photo = world.photo.clone();
    let badges = |h: &Harness<'_, Shell>, id: &str| h.state().library().badges_of(&h.ctx, id);
    // No strategy: no backup badge.
    assert!(badges(&h, &photo).is_empty(), "{:?}", badges(&h, &photo));

    let refresh = |h: &mut Harness<'_, Shell>, world: &World| {
        h.state_mut().refresh_volumes();
        world.persist.fill_library(h.state_mut()).unwrap();
        world.settle(h);
    };
    strategy::save(&world.vault, &Scope::Library, Some(&Strategy::default())).unwrap();
    refresh(&mut h, &world);
    // On the backup disk, not verified; the other on no backup disk.
    assert_eq!(badges(&h, &photo), ["unverified"]);
    assert_eq!(badges(&h, &other), ["no backup"]);

    // Verified: backed up.
    let id = helios_vault::ImageId::from_hex(&photo).unwrap();
    strategy::record_verified(
        &world.vault,
        &id,
        &VolumeId("uuid:backup".into()),
        Path::new("photo.jpg"),
        FIRST + 2,
    )
    .unwrap();
    refresh(&mut h, &world);
    assert_eq!(badges(&h, &photo), ["backed up"]);

    // The filters, run in the vault's query on the host's turn.
    let shown = |h: &mut Harness<'_, Shell>, world: &mut World, typed: &str| {
        h.state()
            .library_mut()
            .set_filter(egui_pop::views::library::filter::Filter::parse(typed));
        world.persist.frame_settled(h.state_mut(), 1.0);
        h.state().library().shown()
    };
    assert_eq!(
        shown(&mut h, &mut world, "backup:met"),
        std::slice::from_ref(&photo)
    );
    assert_eq!(
        shown(&mut h, &mut world, "backup:none"),
        std::slice::from_ref(&other)
    );
    assert_eq!(
        shown(&mut h, &mut world, "backup:short"),
        std::slice::from_ref(&other)
    );

    // Two copies asked: one is short.
    let two = Strategy {
        copies: 2,
        ..Strategy::default()
    };
    strategy::save(&world.vault, &Scope::Library, Some(&two)).unwrap();
    refresh(&mut h, &world);
    assert_eq!(badges(&h, &photo), ["backup short"]);
    let mut short = shown(&mut h, &mut world, "backup:short");
    short.sort();
    let mut both = vec![photo.clone(), other.clone()];
    both.sort();
    assert_eq!(short, both);
    assert!(shown(&mut h, &mut world, "backup:met").is_empty());
}

/// A card left in the reader short of its backups: once a backup job
/// ends with its photos backed up, a toast says it is safe to format.
#[test]
fn a_card_short_of_its_backups_is_said_safe_once_a_backup_ends() {
    let (world, shell) = World::new("card-safe");
    give_roles(&world.vault, "uuid:studio", &[Role::Library]);
    give_roles(&world.vault, "uuid:backup", &[Role::Backup]);
    let one = Strategy {
        triggers: [Trigger::ByHand].into(),
        ..Strategy::default()
    };
    strategy::save(&world.vault, &Scope::Library, Some(&one)).unwrap();
    let card = world.dir.join("EOS_DIGITAL");
    let dcim = card.join("DCIM/100CANON");
    std::fs::create_dir_all(&dcim).unwrap();
    std::fs::write(dcim.join("IMG_0001.JPG"), b"a photo on the card, to copy").unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let jobs = h.state().jobs();
    let options = Options {
        placement: Placement::Copy {
            into: Some(world.dir.join("studio/Library")),
        },
        signals: false,
        ..Options::default()
    };
    let report = import::import(
        world.vault.as_ref(),
        &jobs,
        std::slice::from_ref(&card),
        &options,
        FIRST + 10,
        &mut |_: &serde_json::Value| {},
    )
    .unwrap();
    assert_eq!(report.cards.len(), 1, "{report:?}");
    assert!(!report.cards[0].safe_to_format, "{report:?}");
    h.state()
        .import_sink()
        .send((json!({}), json!({ "ended": report })))
        .unwrap();
    until(&mut h, "the card waits for its backups", |h| {
        h.state().cards_waiting_for_backups() == ["EOS_DIGITAL"]
    });

    backup::run(
        world.vault.as_ref(),
        &jobs,
        world.plugged.as_ref(),
        Trigger::ByHand,
        FIRST + 20,
        None,
    )
    .unwrap();
    let said = "EOS_DIGITAL is safe to format: every photo has its backups";
    until(&mut h, "the toast", |h| {
        h.state().notes.iter().any(|n| n == said)
    });
    assert!(h.state().cards_waiting_for_backups().is_empty());
}

#[test]
fn the_storage_scope_reads_the_volumes_and_the_strategy_from_commands() {
    use helios_gui::shell::{BACKUP_STRATEGY_COMMAND, VOLUME_ROLE_COMMAND, VOLUMES_COMMAND};

    let (mut world, shell) = World::new("admin");
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    let studio = world.studio.clone();
    world.plug(&mut h, &[&studio], FIRST + DAY);

    // Declared read-only to egui-pop: what the Settings app may read.
    let app = h.state().app().handle().clone();
    let spec = |id: &str| {
        app.command(&egui_pop::model::ids::CommandId::new(id).unwrap())
            .unwrap()
    };
    assert!(spec(VOLUMES_COMMAND).read_only);
    assert!(spec(BACKUP_STRATEGY_COMMAND).read_only);
    assert!(!spec(VOLUME_ROLE_COMMAND).read_only);

    // The volumes as rows: online first, with their state's tone.
    let answer = h
        .state_mut()
        .run(VOLUMES_COMMAND, json!(null), Origin::User)
        .unwrap();
    let rows = answer["volumes"].as_array().unwrap();
    let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
    assert_eq!(names, ["Studio", "Backup"]);
    assert_eq!(rows[0]["state"], "online");
    assert_eq!(rows[1]["state"], "offline");
    assert!(rows[1]["tone"].is_null());
    assert_eq!(rows[1]["photos"], 1);

    // A role given through its command is saved.
    h.state_mut()
        .run(
            VOLUME_ROLE_COMMAND,
            json!({ "volume": "uuid:studio", "role": "backup" }),
            Origin::User,
        )
        .unwrap();
    assert!(world.link.settle(Duration::from_secs(10)));
    assert!(
        known(&world.vault, "uuid:studio")
            .roles
            .contains(&Role::Backup)
    );

    // The strategy: none, then the default one, its disk plugged in.
    let none = h
        .state_mut()
        .run(BACKUP_STRATEGY_COMMAND, json!(null), Origin::User)
        .unwrap();
    assert_eq!(none["set"], false);
    strategy::save(&world.vault, &Scope::Library, Some(&Strategy::default())).unwrap();
    h.state_mut().refresh_volumes();
    world.settle(&mut h);
    let set = h
        .state_mut()
        .run(BACKUP_STRATEGY_COMMAND, json!(null), Origin::User)
        .unwrap();
    assert_eq!(set["copies"], 1);
    assert_eq!(set["online"], 1, "the studio disk takes copies now");
    assert_eq!(set["summary"]["tone"], "success");

    // The Settings app's Storage scope shows the table from the command.
    h.state_mut()
        .run(
            "admin.open",
            json!({ "path": "storage/volumes" }),
            Origin::User,
        )
        .unwrap();
    until(&mut h, "the volume table", |h| {
        h.query(
            By::new()
                .role(egui::accesskit::Role::TextInput)
                .value("Studio"),
        )
        .is_some()
            && h.query_by_label("offline").is_some()
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

    let (mut world, shell) = World::new("wizard");
    let mut h = headless::harness(shell, vec2(1280.0, 900.0));
    let studio = world.studio.clone();
    world.plug(&mut h, &[&studio], FIRST + DAY);
    h.state_mut()
        .run(
            "admin.open",
            json!({ "path": "backup/strategy" }),
            Origin::User,
        )
        .unwrap();
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
    let state = |h: &mut Harness<'_, Shell>, cmd: &str, args: serde_json::Value| {
        let out = h.state_mut().run(cmd, args, Origin::User);
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
    let app = h.state().app().handle().clone();
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
    use helios_gui::shell::VOLUME_ROLE_COMMAND;
    use helios_vault::Store as _;

    let (world, shell) = World::new("held");
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();

    // Another writer holds the vault's file.
    let mut other = SqliteStore::open(&world.dir.join("vault.db")).unwrap();
    other.begin().unwrap();
    other
        .set_document("library/elsewhere", "held", Some(&json!(true)))
        .unwrap();
    let started = Instant::now();
    h.state_mut()
        .run(
            VOLUME_ROLE_COMMAND,
            json!({ "volume": "uuid:studio", "role": "backup" }),
            Origin::User,
        )
        .unwrap();
    h.state_mut()
        .run(
            "library.setUpBackups",
            json!({ "copies": 1, "destination": "backup" }),
            Origin::User,
        )
        .unwrap();
    h.state_mut().refresh_volumes();
    h.state_mut().refresh_backups();
    for _ in 0..3 {
        h.run();
    }
    let slowest = started.elapsed();
    assert!(
        world.link.busy(),
        "the writes wait for the vault, not the frame"
    );
    assert!(
        slowest < Duration::from_secs(1),
        "the frames waited {slowest:?} for the vault"
    );
    // Shown at once all the same.
    assert!(h.state().storage_strategy().is_some());
    assert!(
        h.state().storage_volumes()[0].roles.contains(&Role::Backup),
        "{:?}",
        h.state().storage_volumes()
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
