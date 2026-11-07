//! The backup job end to end, on temporary folders standing for disks:
//! verified copies recorded and counted, never overwriting, never written
//! to a card or a volume without a role, paused while the destination is
//! unplugged, run after an import, and a card safe to format only once
//! its photos meet the strategy.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use helios_core::Bus;
use helios_core::jobs::Jobs;
use helios_library::backup;
use helios_library::import::{self, Options, Placement};
use helios_storage::strategy::{self, Scope, Strategy, Trigger};
use helios_storage::{Identity, Role, Volume, VolumeId, VolumeKind, Volumes, detect, registry};
use helios_vault::{ImageId, MemoryStore, Presence, Vault};
use serde_json::Value;

struct Place(PathBuf);

impl Place {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("helios-backup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The volumes plugged in, changed by the test.
#[derive(Default)]
struct Plugged(Mutex<Vec<Volume>>);

impl Volumes for Plugged {
    fn volumes(&self) -> Vec<Volume> {
        self.0.lock().unwrap().clone()
    }
}

impl Plugged {
    fn pull(&self, id: &str) {
        self.0.lock().unwrap().retain(|v| v.id.as_str() != id);
    }

    fn plug(&self, v: Volume) {
        self.0.lock().unwrap().push(v);
    }
}

fn disk(id: &str, mount: &Path, roles: &[Role]) -> Volume {
    std::fs::create_dir_all(mount).unwrap();
    Volume {
        id: VolumeId(id.into()),
        identified_by: Identity::Uuid,
        kind: VolumeKind::External,
        mount: Some(mount.to_path_buf()),
        roles: roles.iter().copied().collect(),
        ..detect::weak(id, Some(1_000_000))
    }
}

/// Records the volumes in the registry with their roles.
fn register(vault: &Vault<MemoryStore>, volumes: &[Volume]) {
    registry::record(vault, volumes, "studio", 10, None).unwrap();
    for v in volumes {
        registry::save(vault, v).unwrap();
    }
}

fn photo(path: &Path, seed: u8) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes: Vec<u8> = (0..20_000u32)
        .map(|i| (i as u8).wrapping_mul(seed))
        .collect();
    std::fs::write(path, bytes).unwrap();
}

fn import_into(
    vault: &Vault<MemoryStore>,
    jobs: &Jobs,
    sources: &[PathBuf],
    library: &Path,
) -> import::Report {
    let options = Options {
        placement: Placement::Copy {
            into: Some(library.to_path_buf()),
        },
        signals: false,
        ..Options::default()
    };
    import::import(vault, jobs, sources, &options, 100, &mut |_: &Value| {}).unwrap()
}

/// The world of a test: a library disk, a backup disk, photos imported.
struct World {
    p: Place,
    vault: Vault<MemoryStore>,
    bus: Arc<Bus>,
    jobs: Jobs,
    plugged: Plugged,
    backup: Volume,
}

fn world(name: &str, photos: u8) -> World {
    let p = Place::new(name);
    let vault = Vault::new(MemoryStore::default());
    let bus = Arc::new(Bus::new());
    let jobs = Jobs::new(bus.clone());
    let lib = disk("uuid:lib", &p.join("libdisk"), &[Role::Library]);
    let backup = disk("uuid:a", &p.join("A"), &[Role::Backup]);
    register(&vault, &[lib.clone(), backup.clone()]);
    let plugged = Plugged::default();
    plugged.plug(lib);
    plugged.plug(backup.clone());
    for k in 0..photos {
        photo(&p.join(&format!("roll/p{k}.raf")), k + 1);
    }
    if photos > 0 {
        import_into(&vault, &jobs, &[p.join("roll")], &p.join("libdisk/Library"));
    }
    World {
        p,
        vault,
        bus,
        jobs,
        plugged,
        backup,
    }
}

fn library_strategy(vault: &Vault<MemoryStore>, triggers: &[Trigger]) -> Strategy {
    let s = Strategy {
        triggers: triggers.iter().copied().collect::<BTreeSet<_>>(),
        ..Strategy::default()
    };
    strategy::save(vault, &Scope::Library, Some(&s)).unwrap();
    s
}

fn status_of(vault: &Vault<MemoryStore>, id: &ImageId) -> strategy::BackupStatus {
    let s = strategy::load(vault, &Scope::Library).unwrap();
    let all = strategy::statuses(vault, &registry::known(vault).unwrap(), s.as_ref()).unwrap();
    all[id].0
}

#[test]
fn a_backup_copy_is_verified_recorded_and_counted() {
    let w = world("verified", 2);
    library_strategy(&w.vault, &[Trigger::ByHand]);
    let ids: Vec<ImageId> = w
        .vault
        .images()
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(status_of(&w.vault, &ids[0]).have, 0);
    let reports = backup::run(&w.vault, &w.jobs, &w.plugged, Trigger::ByHand, 500, None).unwrap();
    let [report] = reports.as_slice() else {
        panic!("one destination: {reports:?}")
    };
    assert_eq!((report.copied, report.failed), (2, 0), "{report:?}");
    for id in &ids {
        let locations = w.vault.locations(id).unwrap();
        let library = locations
            .iter()
            .find(|l| l.path.starts_with(w.p.join("libdisk/Library")))
            .unwrap();
        let copy = locations
            .iter()
            .find(|l| l.volume.as_ref() == Some(&w.backup.id))
            .expect("the copy recorded");
        let in_library = library
            .path
            .strip_prefix(w.p.join("libdisk/Library"))
            .unwrap();
        assert_eq!(
            copy.rel_path.as_deref(),
            Some(Path::new("Helios Backup").join(in_library).as_path())
        );
        assert_eq!(
            (copy.presence, copy.last_verified),
            (Presence::Present, 500)
        );
        assert_eq!(import::hash_file(&copy.path).unwrap(), *id);
        let s = status_of(&w.vault, id);
        assert!(s.met() && s.have == 1, "{s:?}");
    }
    // No part file left behind, nothing pending, no job when run again.
    assert!(!w.p.join("A/Helios Backup/.helios-part").exists());
    let again = backup::run(&w.vault, &w.jobs, &w.plugged, Trigger::ByHand, 600, None).unwrap();
    assert!(again.is_empty(), "{again:?}");
}

#[test]
fn a_backup_never_overwrites_a_file() {
    let w = world("overwrite", 1);
    library_strategy(&w.vault, &[Trigger::ByHand]);
    let id = w.vault.images().unwrap()[0].id;
    let library = w.vault.locations(&id).unwrap()[0].path.clone();
    let in_library = library
        .strip_prefix(w.p.join("libdisk/Library"))
        .unwrap()
        .to_path_buf();
    // Another file already at the copy's place.
    let taken = w.p.join("A/Helios Backup").join(&in_library);
    std::fs::create_dir_all(taken.parent().unwrap()).unwrap();
    std::fs::write(&taken, b"someone else's").unwrap();
    let reports = backup::backup(
        &w.vault,
        &w.jobs,
        &w.plugged,
        &w.backup.id,
        &[id],
        500,
        None,
    )
    .unwrap();
    assert_eq!(reports.copied, 1, "{reports:?}");
    assert_eq!(std::fs::read(&taken).unwrap(), b"someone else's");
    let copy = w
        .vault
        .locations(&id)
        .unwrap()
        .into_iter()
        .find(|l| l.volume.as_ref() == Some(&w.backup.id))
        .unwrap();
    assert_ne!(copy.path, taken);
    assert!(
        copy.path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("-1")
    );
    assert_eq!(import::hash_file(&copy.path).unwrap(), id);
}

#[test]
fn a_backup_is_refused_on_a_card_or_a_volume_without_role() {
    let w = world("refused", 1);
    let id = w.vault.images().unwrap()[0].id;
    let none = disk("uuid:none", &w.p.join("N"), &[]);
    let mut card = disk("uuid:card", &w.p.join("EOS"), &[Role::Backup]);
    card.kind = VolumeKind::Removable;
    card.has_dcim = true;
    std::fs::create_dir_all(w.p.join("EOS/DCIM")).unwrap();
    register(&w.vault, &[none.clone(), card.clone()]);
    w.plugged.plug(none.clone());
    w.plugged.plug(card.clone());
    for v in [&none, &card] {
        let e = backup::backup(&w.vault, &w.jobs, &w.plugged, &v.id, &[id], 500, None);
        assert!(e.is_err(), "{e:?}");
    }
    assert!(!w.p.join("N/Helios Backup").exists());
    assert!(!w.p.join("EOS/Helios Backup").exists());
    // A strategy naming them by volume finds no destination either.
    let s = Strategy {
        destinations: vec![
            strategy::Destination::Volume(none.id.clone()),
            strategy::Destination::Volume(card.id.clone()),
        ],
        ..Strategy::default()
    };
    strategy::save(&w.vault, &Scope::Library, Some(&s)).unwrap();
    assert!(backup::run(&w.vault, &w.jobs, &w.plugged, Trigger::ByHand, 500, None).is_err());
}

/// Waits up to `seconds` for an event `kind` on `bus`: its payload.
fn event(bus: &Bus, kind: &str, seconds: u64) -> Option<Value> {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        if let Some(e) = bus.since(0).into_iter().find(|e| e.kind == kind) {
            return Some(e.payload.clone());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

#[test]
fn a_backup_pauses_while_its_destination_is_unplugged() {
    let w = world("pause", 2);
    library_strategy(&w.vault, &[Trigger::ByHand]);
    let ids: Vec<ImageId> = w
        .vault
        .images()
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    // Unplugged before it starts: it waits for it.
    w.plugged.pull("uuid:a");
    let cancel = Arc::new(AtomicBool::new(false));
    let (report, paused) = std::thread::scope(|scope| {
        let running = scope.spawn(|| {
            backup::backup(
                &w.vault,
                &w.jobs,
                &w.plugged,
                &w.backup.id,
                &ids,
                500,
                Some(cancel.clone()),
            )
        });
        let paused = event(&w.bus, "job.paused", 30);
        if paused.is_some() {
            // Back, at another mount point.
            let mut back = w.backup.clone();
            back.mount = Some(w.p.join("A2"));
            std::fs::rename(w.p.join("A"), w.p.join("A2")).unwrap();
            w.plugged.plug(back);
        }
        if paused.is_none() || event(&w.bus, "job.finished", 30).is_none() {
            // Never hang: a job that did not pause or end is stopped.
            cancel.store(true, Ordering::SeqCst);
        }
        (running.join().unwrap(), paused)
    });
    let paused = paused.expect("the backup paused");
    assert_eq!(paused["reason"], "Plug in uuid:a to go on");
    let report = report.unwrap();
    assert_eq!((report.copied, report.cancelled), (2, false), "{report:?}");
    assert!(event(&w.bus, "job.resumed", 1).is_some());
    for id in &ids {
        let copy = w
            .vault
            .locations(id)
            .unwrap()
            .into_iter()
            .find(|l| l.volume.as_ref() == Some(&w.backup.id))
            .unwrap();
        assert!(copy.path.starts_with(w.p.join("A2")), "{copy:?}");
    }
}

#[test]
fn a_paused_backup_can_be_cancelled() {
    let w = world("pause-cancel", 1);
    let ids: Vec<ImageId> = w
        .vault
        .images()
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    w.plugged.pull("uuid:a");
    let report = std::thread::scope(|scope| {
        let running = scope
            .spawn(|| backup::backup(&w.vault, &w.jobs, &w.plugged, &w.backup.id, &ids, 500, None));
        let started = event(&w.bus, "job.started", 30).expect("started");
        let paused = event(&w.bus, "job.paused", 30);
        w.jobs.cancel(started["id"].as_str().unwrap()).unwrap();
        assert!(paused.is_some());
        running.join().unwrap()
    });
    assert!(report.unwrap().cancelled);
}

#[test]
fn an_import_triggers_a_backup_when_the_strategy_says_so() {
    let w = world("on-import", 0);
    photo(&w.p.join("roll/a.raf"), 3);
    let library = w.p.join("libdisk/Library");
    // Without "on import": nothing.
    library_strategy(&w.vault, &[Trigger::ByHand]);
    let imported = import_into(&w.vault, &w.jobs, &[w.p.join("roll")], &library);
    let none = backup::after_import(&w.vault, &w.jobs, &w.plugged, &imported, 500).unwrap();
    assert!(none.is_empty());
    // With it: the new photo is copied.
    library_strategy(&w.vault, &[Trigger::OnImport]);
    photo(&w.p.join("roll2/b.raf"), 5);
    let imported = import_into(&w.vault, &w.jobs, &[w.p.join("roll2")], &library);
    let done = backup::after_import(&w.vault, &w.jobs, &w.plugged, &imported, 500).unwrap();
    assert_eq!(done.len(), 1);
    // Both photos were below the strategy: both copied.
    assert_eq!(done[0].copied, 2, "{done:?}");
    // The destination unplugged: no job, no error.
    w.plugged.pull("uuid:a");
    photo(&w.p.join("roll3/c.raf"), 7);
    let imported = import_into(&w.vault, &w.jobs, &[w.p.join("roll3")], &library);
    assert!(
        backup::after_import(&w.vault, &w.jobs, &w.plugged, &imported, 500)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_card_is_safe_to_format_once_its_photos_meet_the_strategy() {
    let w = world("format", 0);
    let card = w.p.join("EOS_DIGITAL");
    photo(&card.join("DCIM/100CANON/a.raf"), 9);
    let library = w.p.join("libdisk/Library");
    library_strategy(&w.vault, &[Trigger::ByHand]);
    let report = import_into(&w.vault, &w.jobs, std::slice::from_ref(&card), &library);
    let [first] = report.cards.as_slice() else {
        panic!("one card: {report:?}")
    };
    assert_eq!((first.missing, first.short, first.want), (0, 1, 1));
    assert!(!first.safe_to_format);
    assert_eq!(
        first.said(),
        "EOS_DIGITAL: in the Library, 1 photo short of its backups: keep the card"
    );
    backup::run(&w.vault, &w.jobs, &w.plugged, Trigger::ByHand, 500, None).unwrap();
    let report = import_into(&w.vault, &w.jobs, std::slice::from_ref(&card), &library);
    assert!(report.cards[0].safe_to_format, "{report:?}");
    assert_eq!(
        report.cards[0].said(),
        "EOS_DIGITAL: safe to format (library + 1 backup, verified)"
    );
    // Without a strategy, the library copy is enough.
    strategy::save(&w.vault, &Scope::Library, None).unwrap();
    photo(&card.join("DCIM/100CANON/b.raf"), 11);
    let report = import_into(&w.vault, &w.jobs, std::slice::from_ref(&card), &library);
    assert!(report.cards[0].safe_to_format, "{report:?}");
    assert_eq!(report.cards[0].said(), "EOS_DIGITAL: safe to format");
}
