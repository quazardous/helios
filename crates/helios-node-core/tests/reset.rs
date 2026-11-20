//! A reset takes Helios's own folders and never a photo: a fixture home
//! with a library, its photos and sidecars is reset, and every photo's
//! hash is the same after.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use helios_node_core::reset::{self, Fate, Places, Scope, What};

/// A fixture home in the temporary folder, removed when dropped.
struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "helios-reset-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Home(dir)
    }

    fn places(&self, library: Option<&str>) -> Places {
        Places {
            config: self.0.join(".config/helios"),
            data: self.0.join(".local/share/helios"),
            state: self.0.join(".local/state/helios"),
            cache: self.0.join(".cache/helios"),
            library: library.map(|l| self.0.join(l)),
        }
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn has(&self, path: &str) -> bool {
        self.0.join(path).exists()
    }

    /// Helios's own files, as a launch leaves them.
    fn helios(&self) {
        self.write(".config/helios/helios.toml", b"[ui]\ntheme = \"dark\"\n");
        self.write(".config/helios/workspaces/main.json", b"{}");
        self.write(".config/helios/ai/ai.toml", b"");
        self.write(".local/share/helios/library.db", b"SQLite format 3\0");
        self.write(".local/share/helios/library.db-wal", b"wal");
        self.write(
            ".local/share/helios/library.db.snapshots/000001-v9-manual.db",
            b"snap",
        );
        self.write(
            ".local/share/helios/.helios/thumbs/ab/1.1.256.jpg",
            b"thumb",
        );
        self.write(".local/state/helios/logs/helios.log", b"log");
        self.write(".local/state/helios/crashes/1.txt", b"crash");
        self.write(".cache/helios/glances/x.bin", b"cache");
    }

    /// The photos: RAW + JPEG pairs, sidecars, a video; some in a folder
    /// named like Helios's.
    fn photos(&self) -> Vec<PathBuf> {
        for (i, f) in [
            "Pictures/2026/IMG_0001.CR3",
            "Pictures/2026/IMG_0001.JPG",
            "Pictures/2026/IMG_0001.CR3.xmp",
            "Pictures/2026/IMG_0002.NEF",
            "Pictures/2026/IMG_0002.xmp",
            "Pictures/helios/IMG_0003.DNG",
            "Pictures/Google Photos/ann/PXL_0004.jpg",
            "Pictures/2026/MOV_0005.MP4",
        ]
        .iter()
        .enumerate()
        {
            let bytes: Vec<u8> = (0..4096).map(|b| ((b * 31 + i * 7) % 251) as u8).collect();
            self.write(f, &bytes);
        }
        vec![self.0.join("Pictures")]
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Every file under the photo folders, by path, with its hash.
fn hashes(roots: &[PathBuf]) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    for root in roots {
        walk(root, &mut out);
    }
    out
}

fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            let hash = blake3::hash(&std::fs::read(&path).unwrap());
            out.insert(path, hash.to_hex().to_string());
        }
    }
}

#[test]
fn everything_goes_to_the_backup_and_every_photo_stays_byte_for_byte() {
    let home = Home::new("everything");
    home.helios();
    let roots = home.photos();
    let before = hashes(&roots);
    assert_eq!(before.len(), 8);
    let places = home.places(Some(".local/share/helios/library.db"));

    let plan = reset::plan(&places, Scope::Everything, &roots, "20261120-1200").unwrap();
    // The caches are removed first, the rest moved; the library's files
    // go with the data folder they are in.
    let listed: Vec<_> = plan.targets.iter().map(|t| (t.what, t.fate)).collect();
    assert_eq!(
        listed,
        [
            (What::Cache, Fate::Remove),
            (What::Thumbnails, Fate::Remove),
            (What::Config, Fate::Backup),
            (What::Data, Fate::Backup),
            (What::State, Fate::Backup),
        ]
    );
    let backup = home.0.join(".local/share/helios-reset-20261120-1200");
    assert_eq!(plan.backup.as_ref(), Some(&backup));

    let report = reset::run(&plan);
    assert_eq!(report.failed, None);
    assert_eq!(hashes(&roots), before, "a photo changed");
    for gone in [
        ".config/helios",
        ".local/share/helios",
        ".local/state/helios",
        ".cache/helios",
    ] {
        assert!(!home.has(gone), "{gone} is still there");
    }
    // A reset can be undone by hand: settings, library and logs are in
    // the backup, the thumbnails and caches are not.
    assert!(backup.join("config/helios.toml").exists());
    assert!(backup.join("config/workspaces/main.json").exists());
    assert!(backup.join("data/library.db").exists());
    assert!(backup.join("data/library.db-wal").exists());
    assert!(backup.join("data/library.db.snapshots").exists());
    assert!(backup.join("state/logs/helios.log").exists());
    assert!(!backup.join("data/.helios/thumbs").exists());
}

#[test]
fn a_library_among_the_photos_takes_only_its_own_files() {
    let home = Home::new("among");
    home.helios();
    let roots = home.photos();
    home.write("Pictures/library.db", b"SQLite format 3\0");
    home.write("Pictures/library.db-shm", b"shm");
    home.write("Pictures/.helios/thumbs/cd/2.1.256.jpg", b"thumb");
    home.write("Pictures/.helios/map.json", b"{}");
    let photos: BTreeMap<_, _> = hashes(&roots)
        .into_iter()
        .filter(|(p, _)| {
            !p.to_string_lossy().contains("library.db") && !p.to_string_lossy().contains(".helios")
        })
        .collect();
    assert_eq!(photos.len(), 8);
    let places = home.places(Some("Pictures/library.db"));

    let plan = reset::plan(&places, Scope::Everything, &roots, "s").unwrap();
    let report = reset::run(&plan);
    assert_eq!(report.failed, None);

    let after = hashes(&roots);
    assert_eq!(after, photos, "only the library's own files may go");
    let backup = plan.backup.unwrap();
    assert!(backup.join("library/library.db").exists());
    assert!(backup.join("library/library.db-shm").exists());
    assert!(backup.join("library/.helios/map.json").exists());
    assert!(!home.has("Pictures/.helios"));
}

#[test]
fn the_caches_only_and_the_settings_only_keep_the_library() {
    let home = Home::new("finer");
    home.helios();
    let roots = home.photos();
    let before = hashes(&roots);
    let places = home.places(Some(".local/share/helios/library.db"));

    let caches = reset::plan(&places, Scope::Caches, &roots, "c").unwrap();
    assert_eq!(caches.backup, None);
    assert!(caches.targets.iter().all(|t| t.fate == Fate::Remove));
    assert_eq!(reset::run(&caches).failed, None);
    assert!(!home.has(".cache/helios"));
    assert!(!home.has(".local/share/helios/.helios/thumbs"));
    assert!(home.has(".local/share/helios/library.db"));
    assert!(home.has(".config/helios/helios.toml"));

    let settings = reset::plan(&places, Scope::Settings, &roots, "s").unwrap();
    assert_eq!(reset::run(&settings).failed, None);
    assert!(!home.has(".config/helios"));
    assert!(settings.backup.unwrap().join("config/helios.toml").exists());
    assert!(home.has(".local/share/helios/library.db"));
    assert!(home.has(".local/state/helios/logs/helios.log"));
    assert_eq!(hashes(&roots), before);
}

#[test]
fn a_photo_folder_inside_a_helios_folder_refuses_the_reset_untouched() {
    let home = Home::new("refused");
    home.helios();
    home.write(".local/share/helios/imports/IMG_0009.CR3", b"raw");
    let roots = vec![home.0.join(".local/share/helios/imports")];
    let places = home.places(None);
    let err = reset::plan(&places, Scope::Everything, &roots, "r").unwrap_err();
    assert!(err.0.contains("holds photos"), "{err}");
    assert!(home.has(".local/share/helios/imports/IMG_0009.CR3"));
    assert!(home.has(".config/helios/helios.toml"));
}

#[test]
fn a_place_that_is_not_helioss_own_is_refused() {
    let home = Home::new("foreign");
    let mut places = home.places(None);
    places.cache = home.0.join(".cache");
    let err = reset::plan(&places, Scope::Caches, &[], "f").unwrap_err();
    assert!(err.0.contains("not Helios's own"), "{err}");
    places.cache = PathBuf::from("relative/helios");
    assert!(reset::plan(&places, Scope::Caches, &[], "f").is_err());
}

#[test]
fn a_cache_folder_that_is_the_data_folder_is_never_removed() {
    // As on Windows without HOME: both are %LOCALAPPDATA%\helios.
    let home = Home::new("shared");
    let mut places = home.places(None);
    places.cache = places.data.clone();
    home.write(".local/share/helios/library.db", b"db");
    // Clearing the caches would take the library: refused.
    assert!(reset::plan(&places, Scope::Caches, &[], "w").is_err());
    // The full reset moves it, once.
    let plan = reset::plan(&places, Scope::Everything, &[], "w").unwrap();
    let data: Vec<_> = plan
        .targets
        .iter()
        .filter(|t| t.path == places.data)
        .collect();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0].fate, Fate::Backup);
    assert_eq!(reset::run(&plan).failed, None);
    assert!(plan.backup.unwrap().join("data/library.db").exists());
}

#[test]
fn a_missing_folder_is_skipped() {
    let home = Home::new("fresh");
    let plan = reset::plan(&home.places(None), Scope::Everything, &[], "m").unwrap();
    let report = reset::run(&plan);
    assert_eq!(report.failed, None);
    assert!(report.moved.is_empty() && report.removed.is_empty());
}

/// An "everything" reset records the photo folders in its backup folder:
/// the next first run reads them back once; a folder gone is left out,
/// and a reset of the caches records nothing.
#[test]
fn an_everything_reset_records_the_photo_folders_for_one_reimport() {
    let home = Home::new("reimport");
    home.helios();
    let roots = home.photos();
    let gone = home.0.join("Old card");
    let mut checked = roots.clone();
    checked.push(gone.clone());
    let before = hashes(&roots);
    let places = home.places(Some(".local/share/helios/library.db"));

    let caches = reset::plan(&places, Scope::Caches, &checked, "1").unwrap();
    assert_eq!(reset::run(&caches).failed, None);
    assert_eq!(reset::pending_reimport(&places.data), None, "caches only");

    let plan = reset::plan(&places, Scope::Everything, &checked, "2").unwrap();
    assert_eq!(plan.photos, checked);
    assert_eq!(reset::run(&plan).failed, None);
    let backup = plan.backup.clone().unwrap();
    assert!(backup.join(reset::REIMPORT_FILE).is_file());
    let offer = reset::pending_reimport(&places.data).expect("a re-import to offer");
    assert_eq!(offer.folders, roots, "the folders still there");
    assert_eq!(offer.record, backup.join(reset::REIMPORT_FILE));

    reset::reimport_offered(&offer.record).unwrap();
    assert_eq!(reset::pending_reimport(&places.data), None, "offered once");
    assert!(backup.join(reset::REIMPORT_OFFERED).is_file());
    assert_eq!(hashes(&roots), before, "a photo changed");
}
