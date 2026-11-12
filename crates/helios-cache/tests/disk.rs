//! The disk tier: entries kept across a reopening, least recently used
//! out over the budget, pruned once unused for the kind's age limit, and
//! older versions' folders deleted.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use helios_cache::{Disk, IMPORT_GLANCE, Scope, Store};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-cache-disk-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn open(root: &std::path::Path, limit: u64, now: SystemTime) -> Disk {
    Disk::open(root, IMPORT_GLANCE, Scope::Shared, limit, now).unwrap()
}

const DAY: Duration = Duration::from_hours(24);

#[test]
fn entries_survive_a_reopening_and_a_cache_folder_is_tagged() {
    let root = temp("reopen");
    let now = SystemTime::now();
    let disk = open(&root, 1 << 20, now);
    disk.put(b"card:a/IMG_0001.JPG", b"one").unwrap();
    disk.put(b"card:a/IMG_0002.JPG", b"two").unwrap();
    drop(disk);
    let disk = open(&root, 1 << 20, now);
    assert_eq!(disk.usage(), (6, 2));
    assert_eq!(
        disk.get(b"card:a/IMG_0001.JPG").as_deref(),
        Some(&b"one"[..])
    );
    assert_eq!(disk.get(b"card:a/IMG_0003.JPG"), None);
    assert!(root.join("CACHEDIR.TAG").exists());
    assert!(disk.dir().ends_with("import.glance/v2"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn entries_unused_for_the_kinds_age_limit_are_pruned_and_the_others_kept() {
    let root = temp("prune");
    let now = SystemTime::now();
    let disk = open(&root, 1 << 20, now);
    disk.put(b"old", b"seen 40 days ago").unwrap();
    disk.put(b"recent", b"seen 2 days ago").unwrap();
    let set = |key: &[u8], ago: Duration| {
        let f = std::fs::File::options()
            .write(true)
            .open(disk.file_of(key))
            .unwrap();
        f.set_modified(now - ago).unwrap();
    };
    set(b"old", 40 * DAY);
    set(b"recent", 2 * DAY);
    drop(disk);
    // Opened again: the entry unused for more than 30 days is gone.
    let disk = open(&root, 1 << 20, now);
    assert!(!disk.contains(b"old"));
    assert!(!disk.file_of(b"old").exists());
    assert!(disk.contains(b"recent"));
    // A clock 29 days later: the other one goes too.
    assert_eq!(disk.prune(now + 29 * DAY), 1);
    assert_eq!(disk.usage(), (0, 0));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn over_its_budget_the_least_recently_used_go_first() {
    let root = temp("budget");
    let disk = open(&root, 100, SystemTime::now());
    for i in 0..4u8 {
        disk.put(&[i], &[i; 30]).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    // 120 bytes over 100: down to 90, the oldest out first.
    assert!(!disk.contains(&[0]));
    assert!((1..4u8).all(|i| disk.contains(&[i])));
    assert_eq!(disk.usage(), (90, 3));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_older_versions_folder_is_deleted_and_clearing_empties_the_tier() {
    let root = temp("versions");
    let old = root.join("import.glance/v1/ab");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("x.entry"), b"old format").unwrap();
    let disk = open(&root, 1 << 20, SystemTime::now());
    assert!(!root.join("import.glance/v1").exists());
    disk.put(b"k", b"bytes").unwrap();
    assert_eq!(disk.clear(Some("thumb.256")), 0);
    assert_eq!(disk.clear(Some("import.glance")), 5);
    assert!(!disk.file_of(b"k").exists());
    let _ = std::fs::remove_dir_all(&root);
}
