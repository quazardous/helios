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

use std::path::PathBuf;

use helios_core::ContentHash;
use serde::Serialize;

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

/// Whether an image is in the Library, since when and by what.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
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
            .put_sighting(sighting)
    }

    /// Every sighting of an image, in the Library or not.
    pub fn sightings(&self, id: &ImageId) -> Result<Vec<Sighting>, String> {
        self.store.lock().expect("store lock").sightings_of(id)
    }

    /// The sightings of files of `size` bytes with this partial hash.
    pub fn sightings_like(
        &self,
        size: u64,
        partial: &ContentHash,
    ) -> Result<Vec<Sighting>, String> {
        self.store
            .lock()
            .expect("store lock")
            .sightings_like(size, partial)
    }

    /// An image's Library flag; `None` for an unknown image.
    pub fn library_flag(&self, id: &ImageId) -> Result<Option<LibraryFlag>, String> {
        self.store.lock().expect("store lock").library_flag(id)
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
            Ok(Changes { images })
        })?;
        if !changes.is_empty() {
            self.catalog.lock().expect("catalog lock").version += 1;
        }
        Ok(changes)
    }
}
