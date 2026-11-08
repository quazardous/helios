//! Recording the image map (`docs/design/image-map.md`, section 4): what
//! a listing found becomes sightings, off the interface thread, in
//! batches.
//!
//! [`Recorder`] takes each file a listing gives (the import window's scan,
//! files dropped or pasted): its place (the volume holding it and the path
//! there, from the volumes known), its size and modification time, and the
//! partial hash when the listing read it anyway. It never reads a file to
//! record it. A place the blacklist excludes, or in a folder the user
//! forgot, is not recorded. The rows go to the vault a batch at a time,
//! one transaction each ([`helios_vault::Vault::record_listed`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::ContentHash;
use helios_storage::Volume;
use helios_storage::exclude::Checker;
use helios_vault::{Sighting, Store, Vault};

/// The setting that says what the map records.
pub const RECORD: &str = "system.map.record";

/// How many files a batch holds before it is written.
pub const BATCH: usize = 200;

/// What the map records (`system.map.record`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// Every listing, and background scans of every known volume.
    Everywhere,
    /// What Helios is shown, plus scans of volumes with a role.
    Listings,
    /// Only what is imported: listings keep nothing.
    Import,
}

impl Record {
    /// The setting's value; an unknown one is the default, `listings`.
    pub fn parse(text: &str) -> Self {
        match text {
            "everywhere" => Self::Everywhere,
            "import" => Self::Import,
            _ => Self::Listings,
        }
    }

    /// Whether a listing (a scan, a dialog, a drop) records what it found.
    pub fn listings(self) -> bool {
        self != Self::Import
    }
}

/// Where a listing's rows go: the vault, or a test's list.
pub trait Sink: Send {
    fn write(&self, batch: &[Sighting]) -> Result<(), String>;
    /// Whether a place lies in a folder the user forgot.
    fn forgotten(&self, volume: Option<&helios_vault::storage::VolumeId>, rel: &Path) -> bool;
}

impl<S: Store> Sink for Arc<Vault<S>> {
    fn write(&self, batch: &[Sighting]) -> Result<(), String> {
        self.record_listed(batch).map(|_| ())
    }

    fn forgotten(&self, volume: Option<&helios_vault::storage::VolumeId>, rel: &Path) -> bool {
        Vault::forgotten(self, volume, rel).unwrap_or(false)
    }
}

/// Records one listing: [`Recorder::listed`] per file, then
/// [`Recorder::finish`] (also run when dropped).
pub struct Recorder {
    sink: Box<dyn Sink>,
    checker: Checker,
    volumes: Vec<Volume>,
    seen_by: String,
    batch: Vec<Sighting>,
    /// Paths recorded, to add what a glance tells later.
    recorded: std::collections::HashMap<PathBuf, usize>,
    written: Vec<Sighting>,
    now: u64,
}

impl Recorder {
    /// A listing by `seen_by` (`import`, `drop`, `dialog`, `scan`) at
    /// `now`, over the `volumes` known (their mount points place each
    /// file).
    pub fn new(
        sink: Box<dyn Sink>,
        checker: Checker,
        volumes: Vec<Volume>,
        seen_by: &str,
        now: u64,
    ) -> Self {
        Self {
            sink,
            checker,
            volumes,
            seen_by: seen_by.into(),
            batch: Vec::new(),
            recorded: std::collections::HashMap::new(),
            written: Vec::new(),
            now,
        }
    }

    /// The sighting of a file listed, unless its place is excluded or
    /// forgotten.
    pub fn sighting(
        &mut self,
        path: &Path,
        size: u64,
        modified: u64,
        partial: Option<ContentHash>,
    ) -> Option<Sighting> {
        let volume = helios_storage::registry::holding(&self.volumes, path);
        let place = volume.and_then(|v| {
            let rel = path.strip_prefix(v.mount.as_ref()?).ok()?;
            Some((v.id.clone(), rel.to_path_buf()))
        });
        if let Some(reason) = self
            .checker
            .excluded(path, place.as_ref().map(|(id, rel)| (id, rel.as_path())))
        {
            log::debug!("map: {} not recorded: {reason:?}", path.display());
            return None;
        }
        let (volume, rel) = match place {
            Some((id, rel)) => (Some(id), rel),
            None => (None, path.to_path_buf()),
        };
        if self.sink.forgotten(volume.as_ref(), &rel) {
            return None;
        }
        let mut s = Sighting::seen(path.to_path_buf(), size, modified, self.now, &self.seen_by);
        if volume.is_some() {
            s.rel_path = Some(rel);
        }
        s.volume = volume;
        s.partial = partial;
        Some(s)
    }

    /// Records a file the listing found (its size and modification time
    /// from a `stat`; its partial hash when it was read anyway).
    pub fn listed(&mut self, path: &Path, size: u64, modified: u64, partial: Option<ContentHash>) {
        if let Some(s) = self.sighting(path, size, modified, partial) {
            self.recorded.insert(path.to_path_buf(), self.written.len());
            self.written.push(s.clone());
            self.batch.push(s);
            if self.batch.len() >= BATCH {
                self.flush();
            }
        }
    }

    /// Adds what a glance told of a file recorded (its capture time, in
    /// seconds, and camera): written again with the next batch.
    pub fn glanced(&mut self, path: &Path, taken: Option<i64>, camera: Option<String>) {
        let Some(&at) = self.recorded.get(path) else {
            return;
        };
        let s = &mut self.written[at];
        if taken.is_none() && camera.is_none() {
            return;
        }
        s.taken = taken.or(s.taken);
        if camera.is_some() {
            s.camera = camera;
        }
        self.batch.push(s.clone());
        if self.batch.len() >= BATCH {
            self.flush();
        }
    }

    /// Writes the batch waiting.
    pub fn flush(&mut self) {
        if self.batch.is_empty() {
            return;
        }
        if let Err(e) = self.sink.write(&self.batch) {
            log::warn!("map: {} files not recorded: {e}", self.batch.len());
        }
        self.batch.clear();
    }

    /// Writes what is left; returns every sighting this listing recorded.
    pub fn finish(mut self) -> Vec<Sighting> {
        self.flush();
        std::mem::take(&mut self.written)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Where a path lies: the volume of `volumes` holding it and its path
/// there, or on no volume, the path itself.
pub fn place(
    volumes: &[Volume],
    path: &Path,
) -> (Option<helios_vault::storage::VolumeId>, PathBuf) {
    helios_storage::registry::holding(volumes, path)
        .and_then(|v| {
            let rel = path.strip_prefix(v.mount.as_ref()?).ok()?;
            Some((Some(v.id.clone()), rel.to_path_buf()))
        })
        .unwrap_or_else(|| (None, path.to_path_buf()))
}

/// Forgets the folder at `path` (`library.forgetFolder`, the import
/// window's sources): its sightings not in the Library go, and listings
/// skip it until it is listed again on purpose. Returns how many went.
pub fn forget_folder<S: Store>(
    vault: &Vault<S>,
    volumes: &[Volume],
    path: &Path,
    now: u64,
) -> Result<usize, String> {
    let (volume, rel) = place(volumes, path);
    vault.forget(volume.as_ref(), Some(&rel), now)
}

/// Forgets what the blacklist `checker` now excludes (a rule just added
/// to `system.map.exclude`): each folder of a sighting on `volumes` or on
/// no volume that is itself excluded is forgotten. A file excluded alone
/// (a glob on file names) keeps its folder. Returns how many sightings
/// went.
pub fn forget_excluded<S: Store>(
    vault: &Vault<S>,
    volumes: &[Volume],
    checker: &mut Checker,
    now: u64,
) -> Result<usize, String> {
    let mut sightings = Vec::new();
    for v in volumes {
        sightings.extend(vault.sightings_under(Some(&v.id), Path::new(""))?);
    }
    sightings.extend(vault.sightings_under(None, Path::new("/"))?);
    let mut folders = std::collections::BTreeSet::new();
    for s in &sightings {
        let Some(dir) = s.path.parent() else {
            continue;
        };
        let rel = s.rel_path.as_deref().and_then(Path::parent);
        let place = s.volume.as_ref().zip(rel);
        if checker.excluded(dir, place).is_some() {
            let key = match (&s.volume, rel) {
                (Some(v), Some(r)) => (Some(v.clone()), r.to_path_buf()),
                _ => (None, dir.to_path_buf()),
            };
            folders.insert(key);
        }
    }
    let mut dropped = 0;
    for (volume, rel) in &folders {
        // A folder below one forgotten already went with it.
        if folders
            .iter()
            .any(|(v, r)| v == volume && r != rel && rel.starts_with(r))
        {
            continue;
        }
        dropped += vault.forget(volume.as_ref(), Some(rel), now)?;
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use helios_storage::exclude::{Blacklist, Os, Places, Rule};
    use helios_vault::storage::VolumeId;

    use super::*;

    #[derive(Clone, Default)]
    struct List(Arc<Mutex<Vec<Vec<Sighting>>>>);

    impl Sink for List {
        fn write(&self, batch: &[Sighting]) -> Result<(), String> {
            self.0.lock().unwrap().push(batch.to_vec());
            Ok(())
        }
        fn forgotten(&self, _: Option<&VolumeId>, rel: &Path) -> bool {
            rel.starts_with("forgot")
        }
    }

    fn volume(id: &str, mount: &str) -> Volume {
        let mut v: Volume = serde_json::from_value(serde_json::json!({
            "id": id, "identified_by": "uuid", "name": id, "kind": "external",
        }))
        .unwrap();
        v.mount = Some(mount.into());
        v
    }

    #[test]
    fn a_fake_scan_records_each_file_not_excluded() {
        let list = List::default();
        let rules = vec![Rule {
            glob: "**/Private/**".into(),
            volume: None,
        }];
        let checker = Checker::new(Blacklist::new(Os::MacOs, &Places::default(), true, rules));
        let mut r = Recorder::new(
            Box::new(list.clone()),
            checker,
            vec![volume("uuid:disk", "/media/disk")],
            "import",
            100,
        );
        let p = ContentHash([7; 32]);
        r.listed(Path::new("/media/disk/shoot/a.cr3"), 10, 5, Some(p));
        r.listed(Path::new("/media/disk/shoot/.hidden/b.cr3"), 10, 5, None);
        r.listed(Path::new("/media/disk/Private/c.cr3"), 10, 5, None);
        r.listed(Path::new("/media/disk/forgot/d.cr3"), 10, 5, None);
        r.listed(Path::new("/elsewhere/e.jpg"), 3, 4, None);
        r.glanced(
            Path::new("/media/disk/shoot/a.cr3"),
            Some(1_700_000_000),
            Some("Z 8".into()),
        );
        r.glanced(Path::new("/media/disk/Private/c.cr3"), Some(1), None);
        let all = r.finish();
        assert_eq!(all.len(), 2);
        let a = &all[0];
        assert_eq!(a.volume, Some(VolumeId("uuid:disk".into())));
        assert_eq!(a.rel_path.as_deref(), Some(Path::new("shoot/a.cr3")));
        assert_eq!((a.size, a.modified, a.partial), (10, 5, Some(p)));
        assert_eq!(
            (a.taken, a.camera.as_deref()),
            (Some(1_700_000_000), Some("Z 8"))
        );
        assert_eq!(a.seen_by, "import");
        assert_eq!(
            (all[1].volume.clone(), all[1].rel_path.clone()),
            (None, None)
        );
        // One batch: the two files, then the glance's update.
        let written = list.0.lock().unwrap();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].len(), 3);
    }

    #[test]
    fn batches_are_written_as_they_fill() {
        let list = List::default();
        let checker = Checker::new(Blacklist::new(
            Os::MacOs,
            &Places::default(),
            false,
            Vec::new(),
        ));
        let mut r = Recorder::new(Box::new(list.clone()), checker, Vec::new(), "drop", 1);
        for i in 0..(BATCH + 3) {
            r.listed(&PathBuf::from(format!("/p/{i}.jpg")), 1, 1, None);
        }
        assert_eq!(list.0.lock().unwrap().len(), 1);
        drop(r);
        let written = list.0.lock().unwrap();
        assert_eq!(written.len(), 2);
        assert_eq!(written[1].len(), 3);
    }

    #[test]
    fn forget_a_folder_and_what_a_new_rule_excludes() {
        let vault = Vault::new(helios_vault::MemoryStore::default());
        let volumes = vec![volume("uuid:disk", "/media/disk")];
        let id = VolumeId("uuid:disk".into());
        let record = |rel: &str| {
            let mut s = Sighting::seen(Path::new("/media/disk").join(rel), 1, 1, 1, "import");
            s.volume = Some(id.clone());
            s.rel_path = Some(rel.into());
            vault.record_sighting(&s).unwrap();
        };
        for rel in [
            "a/1.jpg",
            "a/b/2.jpg",
            "c/3.jpg",
            "Private/4.jpg",
            "Private/x/5.jpg",
            "d/6.png",
        ] {
            record(rel);
        }
        let all = |v: &Vault<helios_vault::MemoryStore>| {
            v.sightings_under(Some(&id), Path::new("")).unwrap().len()
        };
        assert_eq!(
            forget_folder(&vault, &volumes, Path::new("/media/disk/a"), 2).unwrap(),
            2
        );
        assert!(vault.forgotten(Some(&id), Path::new("a/new.jpg")).unwrap());
        assert_eq!(all(&vault), 4);
        // A rule just added: the folders it excludes go, a file rule
        // leaves its folder.
        let rules = vec![
            Rule {
                glob: "**/Private/**".into(),
                volume: None,
            },
            Rule {
                glob: "**/*.png".into(),
                volume: None,
            },
        ];
        let mut checker = Checker::new(Blacklist::new(Os::Linux, &Places::default(), false, rules));
        assert_eq!(
            forget_excluded(&vault, &volumes, &mut checker, 3).unwrap(),
            2
        );
        let left: Vec<_> = vault
            .sightings_under(Some(&id), Path::new(""))
            .unwrap()
            .into_iter()
            .map(|s| s.rel_path.unwrap())
            .collect();
        assert_eq!(left, [PathBuf::from("c/3.jpg"), PathBuf::from("d/6.png")]);
    }

    #[test]
    fn the_setting_reads() {
        assert!(Record::parse("listings").listings());
        assert!(Record::parse("everywhere").listings());
        assert!(!Record::parse("import").listings());
        assert_eq!(Record::parse("?"), Record::Listings);
    }

    #[test]
    fn a_listing_reaches_the_vault() {
        let vault = Arc::new(Vault::new(helios_vault::MemoryStore::default()));
        let checker = Checker::new(Blacklist::new(
            Os::MacOs,
            &Places::default(),
            true,
            Vec::new(),
        ));
        let mut r = Recorder::new(
            Box::new(vault.clone()),
            checker,
            vec![volume("uuid:card", "/media/card")],
            "import",
            7,
        );
        r.listed(Path::new("/media/card/DCIM/a.cr3"), 10, 5, None);
        r.finish();
        let id = VolumeId("uuid:card".into());
        let got = vault.sightings_under(Some(&id), Path::new("DCIM")).unwrap();
        assert_eq!(got.len(), 1);
        // A folder the user forgot is not recorded again.
        vault.forget(Some(&id), Some(Path::new("DCIM")), 8).unwrap();
        let checker = Checker::new(Blacklist::new(
            Os::MacOs,
            &Places::default(),
            true,
            Vec::new(),
        ));
        let mut r = Recorder::new(
            Box::new(vault.clone()),
            checker,
            vec![volume("uuid:card", "/media/card")],
            "import",
            9,
        );
        r.listed(Path::new("/media/card/DCIM/b.cr3"), 10, 5, None);
        assert!(r.finish().is_empty());
        assert!(
            vault
                .sightings_under(Some(&id), Path::new(""))
                .unwrap()
                .is_empty()
        );
    }
}
