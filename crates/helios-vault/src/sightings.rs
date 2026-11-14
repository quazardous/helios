//! The image map (`docs/design/image-map.md`): every file Helios has seen,
//! and which images are in the Library.
//!
//! - A **sighting** is one file at one place, as last listed: its volume
//!   and path there (or its absolute path, on no volume yet), its size and
//!   modification time, and what reading it told (its partial hash, its
//!   image once fully hashed, its capture time, camera and lens). A place
//!   holds one sighting.
//! - A [`Location`] is the sighting of a file whose image is known: the
//!   `Store`'s location calls read and write sightings.
//! - An image is **in the Library** when its [`LibraryFlag`] says so, with
//!   when and by what it was set. Every Library read leaves the other
//!   images out; setting or clearing the flag is an undoable step
//!   ([`Vault::set_in_library`]).

use std::path::{Path, PathBuf};

use helios_core::ContentHash;
use serde::{Deserialize, Serialize};

use crate::storage::VolumeId;
use crate::undo::{Change, Changes, rows_of};
use crate::{ImageId, Location, Presence, Store, Vault};

/// One file at one place, as last listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Sighting {
    /// The volume it is on; `None` until a scan links it.
    pub volume: Option<VolumeId>,
    /// Its path inside that volume.
    pub rel_path: Option<PathBuf>,
    /// Its absolute path, as last resolved (a hint on a volume).
    pub path: PathBuf,
    pub size: u64,
    /// Modification time when last listed, in seconds since the epoch.
    pub modified: u64,
    /// Its partial hash ([`crate::partial_hash`]), once read.
    pub partial: Option<ContentHash>,
    /// Its content, once fully hashed.
    pub image: Option<ImageId>,
    /// Capture time, in seconds since the epoch, once glanced.
    pub taken: Option<i64>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub first_seen: u64,
    pub last_seen: u64,
    /// When its content was last read and matched.
    pub last_verified: Option<u64>,
    pub presence: Presence,
    /// What listed it first: `import`, `dialog`, `drop`, `scan`.
    pub seen_by: String,
}

impl Sighting {
    /// A file just listed at `path` by `seen_by`: present, on no volume
    /// yet, nothing read.
    pub fn seen(path: PathBuf, size: u64, modified: u64, now: u64, seen_by: &str) -> Self {
        Self {
            volume: None,
            rel_path: None,
            path,
            size,
            modified,
            partial: None,
            image: None,
            taken: None,
            camera: None,
            lens: None,
            first_seen: now,
            last_seen: now,
            last_verified: None,
            presence: Presence::Present,
            seen_by: seen_by.into(),
        }
    }

    /// Whether `other` is at the same place: the same volume and path
    /// there, or, both on no volume, the same absolute path.
    pub fn same_place(&self, other: &Self) -> bool {
        match (&self.volume, &other.volume) {
            (Some(a), Some(b)) => {
                a == b && self.rel_path.is_some() && self.rel_path == other.rel_path
            }
            (None, None) => self.path == other.path,
            _ => false,
        }
    }

    /// Its place is well formed: a sighting on a volume has its path
    /// there.
    pub fn check(&self) -> Result<(), String> {
        if self.volume.is_some() && self.rel_path.is_none() {
            return Err(format!(
                "{}: a sighting on a volume needs its path there",
                self.path.display()
            ));
        }
        Ok(())
    }

    /// As a location, when its image is known.
    pub fn location(&self) -> Option<(ImageId, Location)> {
        Some((
            self.image?,
            Location {
                path: self.path.clone(),
                size: self.size,
                modified: self.modified,
                last_verified: self.last_verified.unwrap_or(0),
                volume: self.volume.clone(),
                rel_path: self.rel_path.clone(),
                presence: self.presence,
            },
        ))
    }

    /// The sighting a location writes (for a store's location calls):
    /// `old`, what was seen there before,
    /// keeps its first sighting, what listed it and, for the same content,
    /// its glance.
    pub fn of_location(
        id: &ImageId,
        location: &Location,
        partial: ContentHash,
        old: Option<&Self>,
    ) -> Self {
        let same = old.filter(|o| o.image == Some(*id));
        let seen = location.last_verified;
        Self {
            volume: location.volume.clone(),
            rel_path: location.rel_path.clone(),
            path: location.path.clone(),
            size: location.size,
            modified: location.modified,
            partial: Some(partial),
            image: Some(*id),
            taken: same.and_then(|o| o.taken),
            camera: same.and_then(|o| o.camera.clone()),
            lens: same.and_then(|o| o.lens.clone()),
            first_seen: old.map_or(seen, |o| o.first_seen.min(seen)),
            last_seen: old.map_or(seen, |o| o.last_seen.max(seen)),
            last_verified: Some(seen),
            presence: location.presence,
            seen_by: old.map_or_else(|| "import".into(), |o| o.seen_by.clone()),
        }
    }
}

impl Sighting {
    /// What a new listing of the same place keeps from `old`, the sighting
    /// recorded there: the same file (same size and modification time)
    /// keeps what reading it told; a changed file keeps only when it was
    /// first seen and what listed it first, to be read again. A changed
    /// file whose image is known keeps its sighting as it is, for the
    /// `map` job to read again: a listing never drops a location.
    pub fn listed_again(mut self, old: Option<&Self>) -> Option<Self> {
        let Some(old) = old else {
            return Some(self);
        };
        self.first_seen = self.first_seen.min(old.first_seen);
        self.seen_by.clone_from(&old.seen_by);
        if old.size == self.size && old.modified == self.modified {
            self.partial = self.partial.or(old.partial);
            self.image = old.image;
            self.taken = self.taken.or(old.taken);
            if self.camera.is_none() {
                self.camera.clone_from(&old.camera);
            }
            if self.lens.is_none() {
                self.lens.clone_from(&old.lens);
            }
            self.last_verified = old.last_verified;
        } else if old.image.is_some() {
            return None;
        }
        Some(self)
    }

    /// Whether it lies in the folder `rel_dir` of `volume` (on no volume,
    /// `rel_dir` is an absolute folder), or in a folder below it.
    pub fn under(&self, volume: Option<&VolumeId>, rel_dir: &Path) -> bool {
        match (volume, &self.volume, &self.rel_path) {
            (Some(v), Some(own), Some(rel)) => v == own && rel.starts_with(rel_dir),
            (None, None, _) => self.path.starts_with(rel_dir),
            _ => false,
        }
    }
}

/// What the `map` job reads next, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToRead {
    /// Present sightings with no partial hash yet, seen last first.
    Partial,
    /// Present sightings of Library images, verified longest ago first.
    Library,
    /// Present sightings with no image whose size and partial hash match
    /// an image's: a duplicate to confirm by the full hash.
    Confirm,
}

/// What is known of a folder: when it was last listed, how many files it
/// held, and whether the user asked to forget it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub volume: Option<VolumeId>,
    /// Its path inside the volume (on no volume, its absolute path).
    pub rel_dir: PathBuf,
    pub last_listed: Option<u64>,
    pub files: Option<u64>,
    pub state: FolderState,
}

/// Whether scans visit a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderState {
    Listed,
    /// Forgotten by the user: skipped until listed again on purpose.
    Forgotten,
}

impl FolderState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Listed => "listed",
            Self::Forgotten => "forgotten",
        }
    }

    pub fn parse(text: &str) -> Self {
        if text == "forgotten" {
            Self::Forgotten
        } else {
            Self::Listed
        }
    }
}

/// Whether an image is in the Library, since when and by what.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct LibraryFlag {
    pub in_library: bool,
    /// When the flag was last set or cleared.
    pub at: Option<u64>,
    /// What set it: an import's job (`import-12`), `user`, `agent:<name>`,
    /// `importer:<plugin>`, `migrated`.
    pub by: Option<String>,
}

impl LibraryFlag {
    /// The flag of an image added by [`Store::add_image`]: in the Library,
    /// with no provenance.
    pub fn added() -> Self {
        Self {
            in_library: true,
            at: None,
            by: None,
        }
    }
}

impl<S: Store> Vault<S> {
    /// Records a sighting at its place ([`Store::put_sighting`]).
    pub fn record_sighting(&self, sighting: &Sighting) -> Result<(), String> {
        sighting.check()?;
        self.store
            .lock()
            .expect("store lock")
            .put_sighting(sighting)?;
        // A copy of an image: its library row changes.
        if let Some(id) = sighting.image {
            self.placed([id]);
        }
        Ok(())
    }

    /// Records what a listing found, in one transaction: each file at its
    /// place, keeping what was known of the same file there
    /// ([`Sighting::listed_again`]). Returns how many were written.
    pub fn record_listed(&self, listed: &[Sighting]) -> Result<usize, String> {
        for s in listed {
            s.check()?;
        }
        let (written, images) = self.transaction(|store| {
            let mut written = 0;
            let mut images = Vec::new();
            for s in listed {
                let old = store.sighting_at(s.volume.as_ref(), s.rel_path.as_deref(), &s.path)?;
                if let Some(new) = s.clone().listed_again(old.as_ref()) {
                    store.put_sighting(&new)?;
                    images.extend(new.image);
                    written += 1;
                }
            }
            Ok((written, images))
        })?;
        if !images.is_empty() {
            self.placed(images);
        }
        Ok(written)
    }

    /// The sighting at a place, if any.
    pub fn sighting_at(
        &self,
        volume: Option<&VolumeId>,
        rel_path: Option<&Path>,
        path: &Path,
    ) -> Result<Option<Sighting>, String> {
        self.read(|s| s.sighting_at(volume, rel_path, path))
    }

    /// The sightings in a folder and below ([`Sighting::under`]).
    pub fn sightings_under(
        &self,
        volume: Option<&VolumeId>,
        rel_dir: &Path,
    ) -> Result<Vec<Sighting>, String> {
        self.read(|s| s.sightings_under(volume, rel_dir))
    }

    /// What the `map` job reads next ([`ToRead`]), on `volume` only when
    /// given, at most `limit`.
    pub fn sightings_to_read(
        &self,
        read: ToRead,
        volume: Option<&VolumeId>,
        limit: usize,
    ) -> Result<Vec<Sighting>, String> {
        self.read(|s| s.sightings_to_read(read, volume, limit))
    }

    /// Forgets a volume (`rel_dir` `None`) or a folder of it: drops its
    /// sightings not in the Library, and marks the folder forgotten (a
    /// whole volume, its root folder) so scans skip it until it is listed
    /// again on purpose. Returns how many sightings were dropped.
    pub fn forget(
        &self,
        volume: Option<&VolumeId>,
        rel_dir: Option<&Path>,
        now: u64,
    ) -> Result<usize, String> {
        self.transaction(|store| {
            let dropped = store.forget_sightings(volume, rel_dir)?;
            store.put_folder(&Folder {
                volume: volume.cloned(),
                rel_dir: rel_dir.map(Path::to_path_buf).unwrap_or_default(),
                last_listed: Some(now),
                files: Some(0),
                state: FolderState::Forgotten,
            })?;
            Ok(dropped)
        })
    }

    /// Notes that a folder was listed (on purpose: a forgotten folder is
    /// listed again).
    pub fn folder_listed(
        &self,
        volume: Option<&VolumeId>,
        rel_dir: &Path,
        files: u64,
        now: u64,
    ) -> Result<(), String> {
        self.store.lock().expect("store lock").put_folder(&Folder {
            volume: volume.cloned(),
            rel_dir: rel_dir.to_path_buf(),
            last_listed: Some(now),
            files: Some(files),
            state: FolderState::Listed,
        })
    }

    /// What is known of a folder.
    pub fn folder(
        &self,
        volume: Option<&VolumeId>,
        rel_dir: &Path,
    ) -> Result<Option<Folder>, String> {
        self.read(|s| s.folder(volume, rel_dir))
    }

    /// Whether `path` (inside `volume` at `rel_path`) lies in a folder the
    /// user forgot: a listing does not record it.
    pub fn forgotten(&self, volume: Option<&VolumeId>, rel_path: &Path) -> Result<bool, String> {
        self.read(|store| {
            for dir in rel_path.ancestors().skip(1) {
                if store
                    .folder(volume, dir)?
                    .is_some_and(|f| f.state == FolderState::Forgotten)
                {
                    return Ok(true);
                }
            }
            Ok(false)
        })
    }

    /// The map's upkeep: drops sightings not in the Library missing since
    /// before `missing_before`, then the ones seen longest ago past
    /// `limit`. Library sightings are never dropped. Returns how many were.
    pub fn prune_sightings(&self, limit: usize, missing_before: u64) -> Result<usize, String> {
        self.transaction(|store| store.prune_sightings(limit, missing_before))
    }

    /// Every sighting of an image, in the Library or not.
    pub fn sightings(&self, id: &ImageId) -> Result<Vec<Sighting>, String> {
        self.read(|s| s.sightings_of(id))
    }

    /// The sightings of files of `size` bytes with this partial hash.
    pub fn sightings_like(
        &self,
        size: u64,
        partial: &ContentHash,
    ) -> Result<Vec<Sighting>, String> {
        self.read(|s| s.sightings_like(size, partial))
    }

    /// An image's Library flag; `None` for an unknown image.
    pub fn library_flag(&self, id: &ImageId) -> Result<Option<LibraryFlag>, String> {
        self.read(|s| s.library_flag(id))
    }

    /// Whether an image is in the Library.
    pub fn in_library(&self, id: &ImageId) -> Result<bool, String> {
        Ok(self.library_flag(id)?.is_some_and(|f| f.in_library))
    }

    /// Puts images in the Library (`on`) or takes them out, by `by` at
    /// `now`, in one transaction. Returns what changed, one library undo
    /// step: [`Vault::revert`] puts the flags back. Images already so are
    /// left as they are; an unknown image fails the whole step. Nothing
    /// else changes: fields, versions, sightings and files stay.
    pub fn set_in_library(
        &self,
        ids: &[ImageId],
        on: bool,
        by: &str,
        now: u64,
    ) -> Result<Changes, String> {
        let changes = self.transaction(|store| {
            let mut images = Vec::new();
            for id in ids {
                let before =
                    rows_of(&*store, id)?.ok_or_else(|| format!("no image {}", id.hex()))?;
                if before.library.in_library == on {
                    continue;
                }
                let flag = LibraryFlag {
                    in_library: on,
                    at: Some(now),
                    by: Some(by.into()),
                };
                store.set_library_flag(id, &flag)?;
                images.push(Change {
                    image: *id,
                    before: Some(before),
                    after: rows_of(&*store, id)?,
                });
            }
            Ok(Changes {
                images,
                ..Changes::default()
            })
        })?;
        if !changes.is_empty() {
            self.catalog.lock().expect("catalog lock").version += 1;
        }
        Ok(changes)
    }
}
