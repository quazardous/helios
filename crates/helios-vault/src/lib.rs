//! The vault: the catalog as an API.
//!
//! This first part is identity and storage (ROADMAP section 2.3):
//!
//! - an image **is its content**: its id is the BLAKE3 hash of the file;
//! - where a file sits is **data**: an image has any number of locations
//!   (path, size, modification time, last verified, the volume it is on
//!   and its path there, and whether it is present, offline or missing);
//! - **import** walks folders and records what it finds; a file already
//!   known at the same path, size and modification time is not read again;
//! - **repair** marks vanished locations and relinks moved or renamed files
//!   by content, reading as little as it can: same size first, then a
//!   partial hash, then the full hash only for the remaining candidates.
//!   An offline location (its volume unplugged) is left alone;
//! - **stacks** group images shown as one, and each image's import
//!   **signals** are kept beside it ([`stacks`]).
//! - the **Library** is the images flagged in it, and every file seen is a
//!   **sighting**, a location being the sighting of a known image
//!   ([`sightings`]); every Library read leaves the other images out.
//!
//! Rows live in a [`Store`], an abstraction no storage engine leaks into:
//! [`MemoryStore`] here, drivers in their own crates (SQLite in
//! `helios-vault-sqlite`), every one checked by the same [`contract`]. Time is passed in, never
//! read, and every byte hashed is counted, so tests check budgets.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use helios_core::{ContentHash, Describe};
use serde::Serialize;
use serde_json::json;
use storage::VolumeId;

pub mod capture;
pub mod catalog;
pub mod changed;
pub mod contract;
pub mod edit;
pub mod fields;
pub mod journal;
pub mod settings;
pub mod sightings;
pub mod snapshots;
pub mod stacks;
pub mod storage;
pub mod undo;
pub mod versions;
pub mod workflow;
pub mod workshops;

pub use capture::{Backfill, Capture};
pub use catalog::{ApplyReport, CursorId, CursorInfo, Delta, LibraryRow, Row};
pub use edit::{EditSaved, EditSession};
pub use fields::{
    AuditEntry, BackupFilter, BackupState, BackupVolume, Backups, Command, Facets, Order, Query,
    Sort,
};
pub use sightings::{Folder, FolderState, LibraryFlag, Sighting, ToRead};
pub use stacks::Stack;
pub use undo::{Change, Changes, ImageRows, Replayed};
pub use versions::{Version, VersionId};

/// An image, identified by the hash of its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ImageId(pub ContentHash);

impl ImageId {
    pub fn hex(&self) -> String {
        self.0.to_string()
    }

    pub fn from_hex(hex: &str) -> Result<Self, String> {
        parse_hash(hex).map(ImageId)
    }
}

/// A content hash from its 64 hex digits, as drivers store it.
pub fn parse_hash(hex: &str) -> Result<ContentHash, String> {
    if hex.len() != 64 {
        return Err(format!("`{hex}` is not a 64-digit hash"));
    }
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
            .map_err(|_| format!("`{hex}` is not hexadecimal"))?;
    }
    Ok(ContentHash(bytes))
}

/// What the vault knows of an image itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Image {
    pub id: ImageId,
    pub size: u64,
    /// Hash of the file's beginning, end and size: a cheap first filter.
    pub partial: ContentHash,
}

/// The bus event a scan's location changes are announced as
/// ([`Vault::update_locations`]).
pub const LOCATIONS_CHANGED: &str = "vault.locations_changed";

/// Whether a location's file can be read now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    /// The file is there.
    #[default]
    Present,
    /// Its volume is not at hand (unplugged, or a fixed disk of another
    /// node): the file is not known to be gone.
    Offline,
    /// Its volume is online (or it has none) and the file is not there.
    Missing,
}

impl Presence {
    /// As stored: `present`, `offline` or `missing`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Offline => "offline",
            Self::Missing => "missing",
        }
    }

    /// The stored word read back; an unknown one is missing.
    pub fn parse(word: &str) -> Self {
        match word {
            "present" => Self::Present,
            "offline" => Self::Offline,
            _ => Self::Missing,
        }
    }

    /// An image's presence from its locations': present when one copy is,
    /// else offline when one is, else missing (no location at all too).
    pub fn of<'a>(locations: impl IntoIterator<Item = &'a Location>) -> Self {
        locations
            .into_iter()
            .map(|l| l.presence)
            .min()
            .unwrap_or(Self::Missing)
    }
}

/// Where a copy of an image was seen: the [`Sighting`] of a file whose
/// content (its image) is known, as the Library reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Location {
    /// Its absolute path, as last resolved: on a volume, the volume's
    /// mount point joined with [`Location::rel_path`].
    pub path: PathBuf,
    pub size: u64,
    /// Modification time when last seen, in seconds since the epoch.
    pub modified: u64,
    pub last_verified: u64,
    /// The volume it is on; `None` until a scan links it.
    pub volume: Option<storage::VolumeId>,
    /// Its path inside that volume (relative to the volume's root).
    pub rel_path: Option<PathBuf>,
    pub presence: Presence,
}

impl Location {
    /// A file just seen at `path`: present, on no volume yet.
    pub fn seen(path: PathBuf, size: u64, modified: u64, now: u64) -> Self {
        Self {
            path,
            size,
            modified,
            last_verified: now,
            volume: None,
            rel_path: None,
            presence: Presence::Present,
        }
    }

    /// Whether its file is there.
    pub fn present(&self) -> bool {
        self.presence == Presence::Present
    }
}

/// An image's fields and locations, as [`Store::library_rows`] gives them.
pub type LibraryRows = (BTreeMap<String, serde_json::Value>, Vec<Location>);

/// Where rows are kept. A store only stores: every rule is the vault's.
///
/// **The Library is the images flagged in it** ([`LibraryFlag`]): every
/// read of the Library (`images`, `select`, `all_locations`) leaves the
/// other images out. An image off the Library is still known by its id
/// (`image`, `locations`, `fields`...) and through its sightings.
pub trait Store: Send {
    /// Records an image, off the Library (seen, not imported); false if
    /// it was already known (its flag is then left as it is). An import
    /// puts it in the Library with [`Store::set_library_flag`].
    fn add_image(&mut self, image: &Image) -> Result<bool, String>;
    /// Records an image if unknown and puts it in the Library (a flag
    /// already on keeps its provenance): what an import does in one go.
    /// Whether it was new to the Library.
    fn add_to_library(&mut self, image: &Image) -> Result<bool, String> {
        self.add_image(image)?;
        let on = self.library_flag(&image.id)?.is_some_and(|f| f.in_library);
        if !on {
            self.set_library_flag(&image.id, &LibraryFlag::added())?;
        }
        Ok(!on)
    }
    /// Any image the vault knows, in the Library or not.
    fn image(&self, id: &ImageId) -> Result<Option<Image>, String>;
    /// The images in the Library.
    fn images(&self) -> Result<Vec<Image>, String>;
    /// An image's Library flag; `None` for an unknown image.
    fn library_flag(&self, id: &ImageId) -> Result<Option<LibraryFlag>, String>;
    /// Sets an image's Library flag; the image must exist.
    fn set_library_flag(&mut self, id: &ImageId, flag: &LibraryFlag) -> Result<(), String>;
    /// Records a location: the sighting at its path (or, on a volume, at
    /// its place there) now holds this image. A path belongs to one image
    /// at a time.
    fn put_location(&mut self, id: &ImageId, location: &Location) -> Result<(), String>;
    /// Every location of an image, in the Library or not, by path.
    fn locations(&self, id: &ImageId) -> Result<Vec<Location>, String>;
    /// The image known at a path, with its location.
    fn at_path(&self, path: &Path) -> Result<Option<(ImageId, Location)>, String>;
    /// Every location of the images in the Library, by path.
    fn all_locations(&self) -> Result<Vec<(ImageId, Location)>, String>;
    /// Forgets the location at `path` (the file stays where it is); false
    /// if none was recorded there.
    fn remove_location(&mut self, path: &Path) -> Result<bool, String>;
    /// Forgets an image with its fields; false if it was not known.
    /// Refused while a location or a version still names it. Its audit
    /// rows stay: they are a record.
    fn remove_image(&mut self, id: &ImageId) -> Result<bool, String>;

    /// Records a sighting at its place (on a volume: the volume and the
    /// path there, which it must have; else its absolute path), replacing
    /// what was seen there and keeping the earliest `first_seen`. Its
    /// image, when given, must exist.
    fn put_sighting(&mut self, sighting: &Sighting) -> Result<(), String>;
    /// Every sighting of an image, by path.
    fn sightings_of(&self, id: &ImageId) -> Result<Vec<Sighting>, String>;
    /// The sightings of files of `size` bytes with this partial hash, by
    /// path: "probably the same content".
    fn sightings_like(&self, size: u64, partial: &ContentHash) -> Result<Vec<Sighting>, String>;
    /// The sighting at a place: on a volume, its volume and path there;
    /// else its absolute path.
    fn sighting_at(
        &self,
        volume: Option<&VolumeId>,
        rel_path: Option<&Path>,
        path: &Path,
    ) -> Result<Option<Sighting>, String>;
    /// The sightings in a folder and below ([`Sighting::under`]), by path.
    fn sightings_under(
        &self,
        volume: Option<&VolumeId>,
        rel_dir: &Path,
    ) -> Result<Vec<Sighting>, String>;
    /// What the `map` job reads next ([`ToRead`]), on `volume` only when
    /// given, at most `limit`.
    fn sightings_to_read(
        &self,
        read: ToRead,
        volume: Option<&VolumeId>,
        limit: usize,
    ) -> Result<Vec<Sighting>, String>;
    /// Drops the sightings not in the Library on a volume, or in a folder
    /// of it and below; returns how many.
    fn forget_sightings(
        &mut self,
        volume: Option<&VolumeId>,
        rel_dir: Option<&Path>,
    ) -> Result<usize, String>;
    /// Drops sightings not in the Library: the ones missing since before
    /// `missing_before`, then the ones seen longest ago past `limit`.
    fn prune_sightings(&mut self, limit: usize, missing_before: u64) -> Result<usize, String>;
    /// Records what is known of a folder, replacing what was.
    fn put_folder(&mut self, folder: &Folder) -> Result<(), String>;
    /// What is known of a folder.
    fn folder(&self, volume: Option<&VolumeId>, rel_dir: &Path) -> Result<Option<Folder>, String>;

    /// One field of an image.
    fn field(&self, id: &ImageId, name: &str) -> Result<Option<serde_json::Value>, String>;
    /// Sets (or with `None` clears) a field; the image must exist.
    fn set_field(
        &mut self,
        id: &ImageId,
        name: &str,
        value: Option<&serde_json::Value>,
    ) -> Result<(), String>;
    /// The asked fields of many images in one call, in the order of `ids`.
    fn fields_of(
        &self,
        ids: &[ImageId],
        names: &[&str],
    ) -> Result<Vec<BTreeMap<String, serde_json::Value>>, String>;
    /// The asked fields and the locations (by path, as
    /// [`Store::locations`]) of many images, in the order of `ids`: what a
    /// library view shows. A store backed by a database answers in one
    /// statement; this default asks for each image's locations in turn.
    fn library_rows(&self, ids: &[ImageId], names: &[&str]) -> Result<Vec<LibraryRows>, String> {
        let fields = self.fields_of(ids, names)?;
        ids.iter()
            .zip(fields)
            .map(|(id, fields)| Ok((fields, self.locations(id)?)))
            .collect()
    }
    /// The images a query selects, in its order.
    fn select(&self, query: &Query) -> Result<Vec<ImageId>, String>;
    /// How many of the images a query selects carry each value of each
    /// facet ([`Facets`]); the backup states only with `backups`. A store
    /// backed by a database answers in one statement.
    fn facets(&self, query: &Query, backups: Option<&Backups>) -> Result<Facets, String>;
    /// Records a change; returns its sequence number.
    fn add_audit(&mut self, entry: &AuditEntry) -> Result<u64, String>;
    fn audit(&self, id: &ImageId) -> Result<Vec<AuditEntry>, String>;

    /// Keeps a version; false if it was already kept (same id).
    fn put_version(&mut self, version: &Version) -> Result<bool, String>;
    fn version(&self, id: &VersionId) -> Result<Option<Version>, String>;
    /// An image's versions, in no particular order.
    fn versions(&self, image: &ImageId) -> Result<Vec<Version>, String>;
    /// Forgets a version; false if it was not kept. Refused while another
    /// version names it as a parent.
    fn remove_version(&mut self, id: &VersionId) -> Result<bool, String>;

    /// A JSON document by namespace and key: what the vault keeps besides
    /// images (a workflow journal's rows, configuration layers).
    fn document(&self, ns: &str, key: &str) -> Result<Option<serde_json::Value>, String>;
    /// Sets (or with `None` removes) a document.
    fn set_document(
        &mut self,
        ns: &str,
        key: &str,
        value: Option<&serde_json::Value>,
    ) -> Result<(), String>;
    /// Every document of a namespace, by key.
    fn documents(&self, ns: &str) -> Result<Vec<(String, serde_json::Value)>, String>;

    /// Starts a write transaction: the calls until [`Store::commit`] are
    /// kept whole or, after [`Store::rollback`] or a crash, not at all, and
    /// the reads in it see no other writer. Transactions do not nest.
    fn begin(&mut self) -> Result<(), String>;
    fn commit(&mut self) -> Result<(), String>;
    fn rollback(&mut self) -> Result<(), String>;
}

/// Rows in memory: the reference other stores are checked against.
#[derive(Clone, Default)]
pub struct MemoryStore {
    /// The rows as they were when the open transaction began.
    before: Option<Box<MemoryStore>>,
    documents: BTreeMap<(String, String), serde_json::Value>,
    versions: BTreeMap<VersionId, Version>,
    images: BTreeMap<ImageId, (Image, LibraryFlag)>,
    /// One per place.
    sightings: Vec<Sighting>,
    folders: Vec<Folder>,
    fields: BTreeMap<(ImageId, String), serde_json::Value>,
    audit: Vec<AuditEntry>,
}

impl MemoryStore {
    fn in_library(&self, id: &ImageId) -> bool {
        self.images.get(id).is_some_and(|(_, f)| f.in_library)
    }

    fn by_path(mut sightings: Vec<Sighting>) -> Vec<Sighting> {
        sightings.sort_by(|a, b| a.path.cmp(&b.path));
        sightings
    }

    fn locations_where(&self, keep: impl Fn(&ImageId) -> bool) -> Vec<(ImageId, Location)> {
        let mut all: Vec<(ImageId, Location)> = self
            .sightings
            .iter()
            .filter_map(Sighting::location)
            .filter(|(id, _)| keep(id))
            .collect();
        all.sort_by(|a, b| a.1.path.cmp(&b.1.path));
        all
    }
}

/// A photo's value for a sort, as the SQL driver compares it.
#[derive(PartialEq, PartialOrd)]
enum SortKey {
    Number(f64),
    Text(String),
}

impl MemoryStore {
    /// A photo's presence: the best of its copies' ([`Presence::of`]).
    fn presence_of(&self, id: &ImageId) -> Presence {
        self.sightings
            .iter()
            .filter(|s| s.image.as_ref() == Some(id))
            .map(|s| s.presence)
            .min()
            .unwrap_or(Presence::Missing)
    }

    /// What the Library calls a photo: its first present copy's file
    /// name, by path, else the start of its id.
    fn shown_name(&self, id: &ImageId) -> String {
        self.sightings
            .iter()
            .filter(|s| s.image.as_ref() == Some(id) && s.presence == Presence::Present)
            .map(|s| s.path.to_string_lossy())
            .min()
            .map_or_else(
                || id.hex()[..12].to_string(),
                |path| {
                    path.rsplit(std::path::MAIN_SEPARATOR)
                        .next()
                        .unwrap_or_default()
                        .to_string()
                },
            )
    }

    fn text_of(&self, id: &ImageId, field: &str) -> Option<&str> {
        self.fields
            .get(&(*id, field.to_string()))
            .and_then(serde_json::Value::as_str)
    }

    fn items_of(&self, id: &ImageId, field: &str) -> Vec<&str> {
        self.fields
            .get(&(*id, field.to_string()))
            .and_then(serde_json::Value::as_array)
            .map(|a| a.iter().filter_map(serde_json::Value::as_str).collect())
            .unwrap_or_default()
    }

    /// A photo's backup state against `rule`, as the SQL driver measures
    /// it.
    fn backup_state(&self, id: &ImageId, rule: &Backups) -> BackupState {
        use std::collections::BTreeSet;
        let verified = self
            .documents
            .get(&(storage::VERIFIED.to_string(), id.hex()))
            .and_then(serde_json::Value::as_object);
        let mut have = BTreeSet::new();
        let mut unverified = BTreeSet::new();
        let mut off_site = false;
        for s in self
            .sightings
            .iter()
            .filter(|s| s.image.as_ref() == Some(id) && s.presence != Presence::Missing)
        {
            let (Some(volume), Some(rel)) = (&s.volume, &s.rel_path) else {
                continue;
            };
            let Some(counted) = rule.volumes.iter().find(|v| &v.id == volume) else {
                continue;
            };
            let key = format!("{volume}\t{}", rel.display());
            if verified.is_some_and(|v| v.contains_key(&key)) {
                off_site |= counted.off_site;
                have.insert(counted.drive.as_str());
            } else {
                unverified.insert(counted.drive.as_str());
            }
        }
        let unverified = unverified.difference(&have).count() as u64;
        BackupState::of(have.len() as u64, unverified, off_site, rule)
    }

    /// Whether a photo of the Library passes every term of a query.
    fn passes(&self, id: &ImageId, query: &Query) -> bool {
        let field = |name: &str| self.fields.get(&(*id, name.to_string()));
        let yes = |name: &str| {
            field(name)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        };
        let rating = field(fields::RATING)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let labels = self.items_of(id, fields::LABELS);
        let tags = self.items_of(id, fields::TAGS);
        let flag = Query::flag_of(yes(fields::PICKED), yes(fields::REJECTED));
        let one_of = |wanted: &[String], name: &str| {
            wanted.is_empty()
                || self
                    .text_of(id, name)
                    .is_some_and(|v| wanted.iter().any(|w| w == v))
        };
        let captured = field(capture::CAPTURED).and_then(serde_json::Value::as_f64);
        let in_range = (query.captured_from.is_none() && query.captured_before.is_none())
            || captured.is_some_and(|t| {
                query.captured_from.is_none_or(|from| t >= from as f64)
                    && query.captured_before.is_none_or(|before| t < before as f64)
            });
        let words_ok = query.words.is_empty() || {
            let name = fields::fold_case(&self.shown_name(id));
            let tags: Vec<String> = tags.iter().map(|t| fields::fold_case(t)).collect();
            query.words.iter().all(|w| {
                let w = fields::fold_case(w);
                name.contains(&w) || tags.iter().any(|t| t.contains(&w))
            })
        };
        let folder_ok = query.folder.as_deref().is_none_or(|folder| {
            let prefix = folder_prefix(folder);
            self.sightings.iter().any(|s| {
                s.image.as_ref() == Some(id) && s.path.to_string_lossy().starts_with(&prefix)
            })
        });
        query.min_rating.is_none_or(|min| rating >= min)
            && query
                .label
                .as_ref()
                .is_none_or(|l| labels.contains(&l.as_str()))
            && (query.labels.is_empty()
                || query.labels.iter().any(|l| labels.contains(&l.as_str())))
            && query.tags.iter().all(|t| tags.contains(&t.as_str()))
            && (query.flags.is_empty() || query.flags.iter().any(|f| f == flag))
            && in_range
            && words_ok
            && one_of(&query.cameras, capture::CAMERA_MODEL)
            && one_of(&query.lenses, capture::LENS)
            && one_of(&query.file_types, capture::FILE_TYPE)
            && folder_ok
            && (query.presence.is_empty() || query.presence.contains(&self.presence_of(id)))
            && query
                .backup
                .as_ref()
                .is_none_or(|b| b.states.contains(&self.backup_state(id, &b.rule)))
    }
}

/// A folder as the start of the paths below it: its path with one
/// separator at its end.
pub fn folder_prefix(folder: &str) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    format!("{}{sep}", folder.trim_end_matches(sep))
}

impl MemoryStore {
    /// The file name a photo sorts by: its first present copy's, by path,
    /// else its first copy's (the last part of the path, as the SQL
    /// driver cuts it).
    fn sort_name(&self, id: &ImageId) -> Option<String> {
        self.sightings
            .iter()
            .filter(|s| s.image.as_ref() == Some(id))
            .map(|s| (s.presence != Presence::Present, s.path.to_string_lossy()))
            .min()
            .map(|(_, path)| {
                path.rsplit(std::path::MAIN_SEPARATOR)
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
    }

    fn sort_key(&self, id: &ImageId, by: Sort) -> Option<SortKey> {
        let number = |name: &str| {
            self.fields
                .get(&(*id, name.to_string()))
                .and_then(serde_json::Value::as_f64)
        };
        match by {
            Sort::Id => Some(SortKey::Text(id.hex())),
            Sort::Captured => number(capture::CAPTURED).map(SortKey::Number),
            Sort::Imported => self
                .images
                .get(id)
                .and_then(|(_, f)| f.at)
                .map(|at| SortKey::Number(at as f64)),
            Sort::Name => self.sort_name(id).map(SortKey::Text),
            Sort::Rating => Some(SortKey::Number(number(fields::RATING).unwrap_or(0.0))),
            Sort::Edited => self
                .versions
                .values()
                .filter(|v| v.image == *id)
                .map(|v| v.at)
                .max()
                .map(|at| SortKey::Number(at as f64)),
            Sort::Size => self
                .images
                .get(id)
                .map(|(image, _)| SortKey::Number(image.size as f64)),
        }
    }

    /// Sorts as [`Order`] says: the photos without a value last, ties by
    /// file name for the capture time, then by id.
    fn sort(&self, ids: &mut [ImageId], order: Order) {
        use std::cmp::Ordering;
        if order == Order::ID {
            ids.sort();
            return;
        }
        let last = |a: &Option<SortKey>, b: &Option<SortKey>, descending: bool| match (a, b) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => {
                let o = a.partial_cmp(b).unwrap_or(Ordering::Equal);
                if descending { o.reverse() } else { o }
            }
        };
        let mut keyed: Vec<(Option<SortKey>, Option<SortKey>, ImageId)> = ids
            .iter()
            .map(|id| {
                let tie = (order.by == Sort::Captured)
                    .then(|| self.sort_name(id).map(SortKey::Text))
                    .flatten();
                (self.sort_key(id, order.by), tie, *id)
            })
            .collect();
        keyed.sort_by(|a, b| {
            last(&a.0, &b.0, order.descending)
                .then_with(|| last(&a.1, &b.1, false))
                .then_with(|| a.2.cmp(&b.2))
        });
        for (slot, (_, _, id)) in ids.iter_mut().zip(keyed) {
            *slot = id;
        }
    }
}

impl Store for MemoryStore {
    fn add_image(&mut self, image: &Image) -> Result<bool, String> {
        if self.images.contains_key(&image.id) {
            return Ok(false);
        }
        self.images
            .insert(image.id, (image.clone(), LibraryFlag::default()));
        Ok(true)
    }

    fn image(&self, id: &ImageId) -> Result<Option<Image>, String> {
        Ok(self.images.get(id).map(|(i, _)| i.clone()))
    }

    fn images(&self) -> Result<Vec<Image>, String> {
        Ok(self
            .images
            .values()
            .filter(|(_, f)| f.in_library)
            .map(|(i, _)| i.clone())
            .collect())
    }

    fn library_flag(&self, id: &ImageId) -> Result<Option<LibraryFlag>, String> {
        Ok(self.images.get(id).map(|(_, f)| f.clone()))
    }

    fn set_library_flag(&mut self, id: &ImageId, flag: &LibraryFlag) -> Result<(), String> {
        let (_, f) = self
            .images
            .get_mut(id)
            .ok_or_else(|| format!("no image {}", id.hex()))?;
        *f = flag.clone();
        Ok(())
    }

    fn put_location(&mut self, id: &ImageId, location: &Location) -> Result<(), String> {
        let Some((image, _)) = self.images.get(id) else {
            return Err(format!("no image {}", id.hex()));
        };
        let place = |s: &Sighting| {
            s.path == location.path
                || (location.volume.is_some()
                    && location.rel_path.is_some()
                    && s.volume == location.volume
                    && s.rel_path == location.rel_path)
        };
        let old = self
            .sightings
            .iter()
            .find(|s| s.path == location.path)
            .or_else(|| self.sightings.iter().find(|s| place(s)));
        let new = Sighting::of_location(id, location, image.partial, old);
        self.sightings.retain(|s| !place(s));
        self.sightings.push(new);
        Ok(())
    }

    fn locations(&self, id: &ImageId) -> Result<Vec<Location>, String> {
        Ok(self
            .locations_where(|i| i == id)
            .into_iter()
            .map(|(_, l)| l)
            .collect())
    }

    fn at_path(&self, path: &Path) -> Result<Option<(ImageId, Location)>, String> {
        Ok(self
            .sightings
            .iter()
            .filter(|s| s.path == path)
            .find_map(Sighting::location))
    }

    fn all_locations(&self) -> Result<Vec<(ImageId, Location)>, String> {
        Ok(self.locations_where(|id| self.in_library(id)))
    }

    fn remove_location(&mut self, path: &Path) -> Result<bool, String> {
        let before = self.sightings.len();
        self.sightings
            .retain(|s| s.path != path || s.image.is_none());
        Ok(self.sightings.len() < before)
    }

    fn remove_image(&mut self, id: &ImageId) -> Result<bool, String> {
        if !self.images.contains_key(id) {
            return Ok(false);
        }
        if self.sightings.iter().any(|s| s.image == Some(*id))
            || self.versions.values().any(|v| v.image == *id)
        {
            return Err(format!(
                "image {} still has locations or versions",
                id.hex()
            ));
        }
        self.fields.retain(|(i, _), _| i != id);
        self.images.remove(id);
        Ok(true)
    }

    fn put_sighting(&mut self, sighting: &Sighting) -> Result<(), String> {
        sighting.check()?;
        if let Some(id) = &sighting.image
            && !self.images.contains_key(id)
        {
            return Err(format!("no image {}", id.hex()));
        }
        let mut new = sighting.clone();
        if let Some(old) = self.sightings.iter().find(|s| s.same_place(sighting)) {
            new.first_seen = new.first_seen.min(old.first_seen);
        }
        self.sightings.retain(|s| !s.same_place(sighting));
        self.sightings.push(new);
        Ok(())
    }

    fn sightings_of(&self, id: &ImageId) -> Result<Vec<Sighting>, String> {
        Ok(Self::by_path(
            self.sightings
                .iter()
                .filter(|s| s.image == Some(*id))
                .cloned()
                .collect(),
        ))
    }

    fn sightings_like(&self, size: u64, partial: &ContentHash) -> Result<Vec<Sighting>, String> {
        Ok(Self::by_path(
            self.sightings
                .iter()
                .filter(|s| s.size == size && s.partial.as_ref() == Some(partial))
                .cloned()
                .collect(),
        ))
    }

    fn sighting_at(
        &self,
        volume: Option<&VolumeId>,
        rel_path: Option<&Path>,
        path: &Path,
    ) -> Result<Option<Sighting>, String> {
        let place = Sighting {
            volume: volume.cloned(),
            rel_path: rel_path.map(Path::to_path_buf),
            ..Sighting::seen(path.to_path_buf(), 0, 0, 0, "")
        };
        Ok(self
            .sightings
            .iter()
            .find(|s| s.same_place(&place))
            .cloned())
    }

    fn sightings_under(
        &self,
        volume: Option<&VolumeId>,
        rel_dir: &Path,
    ) -> Result<Vec<Sighting>, String> {
        Ok(Self::by_path(
            self.sightings
                .iter()
                .filter(|s| s.under(volume, rel_dir))
                .cloned()
                .collect(),
        ))
    }

    fn sightings_to_read(
        &self,
        read: ToRead,
        volume: Option<&VolumeId>,
        limit: usize,
    ) -> Result<Vec<Sighting>, String> {
        let partials: std::collections::HashSet<(u64, ContentHash)> = self
            .images
            .values()
            .map(|(i, _)| (i.size, i.partial))
            .collect();
        let mut found: Vec<Sighting> = self
            .sightings
            .iter()
            .filter(|s| s.presence == Presence::Present)
            .filter(|s| volume.is_none() || s.volume.as_ref() == volume)
            .filter(|s| match read {
                ToRead::Partial => s.partial.is_none(),
                ToRead::Library => s.image.is_some_and(|i| self.in_library(&i)),
                ToRead::Confirm => {
                    s.image.is_none() && s.partial.is_some_and(|p| partials.contains(&(s.size, p)))
                }
            })
            .cloned()
            .collect();
        match read {
            ToRead::Partial | ToRead::Confirm => {
                found.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(a.path.cmp(&b.path)));
            }
            ToRead::Library => {
                found.sort_by(|a, b| {
                    a.last_verified
                        .cmp(&b.last_verified)
                        .then(a.path.cmp(&b.path))
                });
            }
        }
        found.truncate(limit);
        Ok(found)
    }

    fn forget_sightings(
        &mut self,
        volume: Option<&VolumeId>,
        rel_dir: Option<&Path>,
    ) -> Result<usize, String> {
        let before = self.sightings.len();
        let library: Vec<ImageId> = self
            .images
            .iter()
            .filter(|(_, (_, f))| f.in_library)
            .map(|(id, _)| *id)
            .collect();
        self.sightings.retain(|s| {
            s.image.is_some_and(|i| library.contains(&i))
                || !s.under(volume, rel_dir.unwrap_or(Path::new("")))
        });
        Ok(before - self.sightings.len())
    }

    fn prune_sightings(&mut self, limit: usize, missing_before: u64) -> Result<usize, String> {
        let before = self.sightings.len();
        let library: Vec<ImageId> = self
            .images
            .iter()
            .filter(|(_, (_, f))| f.in_library)
            .map(|(id, _)| *id)
            .collect();
        let kept = |s: &Sighting| s.image.is_some_and(|i| library.contains(&i));
        self.sightings.retain(|s| {
            kept(s) || !(s.presence == Presence::Missing && s.last_seen < missing_before)
        });
        let mut outside: Vec<(u64, PathBuf)> = self
            .sightings
            .iter()
            .filter(|s| !kept(s))
            .map(|s| (s.last_seen, s.path.clone()))
            .collect();
        if outside.len() > limit {
            outside.sort();
            let drop: std::collections::HashSet<PathBuf> = outside[..outside.len() - limit]
                .iter()
                .map(|(_, p)| p.clone())
                .collect();
            self.sightings
                .retain(|s| kept(s) || !drop.contains(&s.path));
        }
        Ok(before - self.sightings.len())
    }

    fn put_folder(&mut self, folder: &Folder) -> Result<(), String> {
        self.folders
            .retain(|f| !(f.volume == folder.volume && f.rel_dir == folder.rel_dir));
        self.folders.push(folder.clone());
        Ok(())
    }

    fn folder(&self, volume: Option<&VolumeId>, rel_dir: &Path) -> Result<Option<Folder>, String> {
        Ok(self
            .folders
            .iter()
            .find(|f| f.volume.as_ref() == volume && f.rel_dir == rel_dir)
            .cloned())
    }

    fn field(&self, id: &ImageId, name: &str) -> Result<Option<serde_json::Value>, String> {
        Ok(self.fields.get(&(*id, name.to_string())).cloned())
    }

    fn set_field(
        &mut self,
        id: &ImageId,
        name: &str,
        value: Option<&serde_json::Value>,
    ) -> Result<(), String> {
        if !self.images.contains_key(id) {
            return Err(format!("no image {}", id.hex()));
        }
        let key = (*id, name.to_string());
        match value {
            Some(v) => self.fields.insert(key, v.clone()),
            None => self.fields.remove(&key),
        };
        Ok(())
    }

    fn fields_of(
        &self,
        ids: &[ImageId],
        names: &[&str],
    ) -> Result<Vec<BTreeMap<String, serde_json::Value>>, String> {
        Ok(ids
            .iter()
            .map(|id| {
                names
                    .iter()
                    .filter_map(|n| {
                        self.fields
                            .get(&(*id, n.to_string()))
                            .map(|v| (n.to_string(), v.clone()))
                    })
                    .collect()
            })
            .collect())
    }

    fn select(&self, query: &Query) -> Result<Vec<ImageId>, String> {
        query.check()?;
        let mut ids: Vec<ImageId> = self
            .images
            .keys()
            .filter(|id| self.in_library(id) && self.passes(id, query))
            .copied()
            .collect();
        self.sort(&mut ids, query.order);
        Ok(ids)
    }

    fn facets(&self, query: &Query, backups: Option<&Backups>) -> Result<Facets, String> {
        query.check()?;
        let mut facets = Facets::default();
        for id in self
            .images
            .keys()
            .filter(|id| self.in_library(id) && self.passes(id, query))
        {
            let field = |name: &str| self.fields.get(&(*id, name.to_string()));
            let yes = |name: &str| {
                field(name)
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            };
            let rating = field(fields::RATING)
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            facets.add("rating", &rating.to_string(), 1);
            for label in self.items_of(id, fields::LABELS) {
                facets.add("label", label, 1);
            }
            let flag = Query::flag_of(yes(fields::PICKED), yes(fields::REJECTED));
            facets.add("flag", flag, 1);
            for (facet, name) in [
                ("camera", capture::CAMERA_MODEL),
                ("lens", capture::LENS),
                ("file_type", capture::FILE_TYPE),
            ] {
                if let Some(value) = self.text_of(id, name) {
                    facets.add(facet, value, 1);
                }
            }
            facets.add("presence", self.presence_of(id).as_str(), 1);
            if let Some(rule) = backups {
                facets.add("backup", self.backup_state(id, rule).name(), 1);
            }
        }
        Ok(facets)
    }

    fn add_audit(&mut self, entry: &AuditEntry) -> Result<u64, String> {
        let seq = self.audit.len() as u64 + 1;
        self.audit.push(AuditEntry {
            seq,
            ..entry.clone()
        });
        Ok(seq)
    }

    fn audit(&self, id: &ImageId) -> Result<Vec<AuditEntry>, String> {
        Ok(self
            .audit
            .iter()
            .filter(|e| e.image == *id)
            .cloned()
            .collect())
    }

    fn put_version(&mut self, version: &Version) -> Result<bool, String> {
        if self.versions.contains_key(&version.id) {
            return Ok(false);
        }
        self.versions.insert(version.id, version.clone());
        Ok(true)
    }

    fn version(&self, id: &VersionId) -> Result<Option<Version>, String> {
        Ok(self.versions.get(id).cloned())
    }

    fn versions(&self, image: &ImageId) -> Result<Vec<Version>, String> {
        Ok(self
            .versions
            .values()
            .filter(|v| v.image == *image)
            .cloned()
            .collect())
    }

    fn remove_version(&mut self, id: &VersionId) -> Result<bool, String> {
        if self.versions.values().any(|v| v.parents.contains(id)) {
            return Err(format!("version {} is a parent", id.hex()));
        }
        Ok(self.versions.remove(id).is_some())
    }

    fn document(&self, ns: &str, key: &str) -> Result<Option<serde_json::Value>, String> {
        Ok(self.documents.get(&(ns.into(), key.into())).cloned())
    }

    fn set_document(
        &mut self,
        ns: &str,
        key: &str,
        value: Option<&serde_json::Value>,
    ) -> Result<(), String> {
        let k = (ns.to_string(), key.to_string());
        match value {
            Some(v) => self.documents.insert(k, v.clone()),
            None => self.documents.remove(&k),
        };
        Ok(())
    }

    fn documents(&self, ns: &str) -> Result<Vec<(String, serde_json::Value)>, String> {
        Ok(self
            .documents
            .iter()
            .filter(|((n, _), _)| n == ns)
            .map(|((_, k), v)| (k.clone(), v.clone()))
            .collect())
    }

    fn begin(&mut self) -> Result<(), String> {
        if self.before.is_some() {
            return Err("a transaction is already open".into());
        }
        self.before = Some(Box::new(self.clone()));
        Ok(())
    }

    fn commit(&mut self) -> Result<(), String> {
        self.before
            .take()
            .map(|_| ())
            .ok_or_else(|| "no transaction to commit".into())
    }

    fn rollback(&mut self) -> Result<(), String> {
        let before = self
            .before
            .take()
            .ok_or_else(|| "no transaction to roll back".to_string())?;
        *self = *before;
        Ok(())
    }
}

/// Files the vault treats as images, by extension (raw formats and common
/// image formats).
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "3fr", "arw", "cr2", "cr3", "crw", "dng", "erf", "iiq", "kdc", "mef", "mos", "mrw", "nef",
    "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f", "jpg", "jpeg",
    "png", "tif", "tiff", "exr", "heic", "avif", "jxl", "webp",
];

/// What the vault has read, to check that it reads no more than needed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Reads {
    /// Files fully hashed.
    pub full_hashes: u64,
    /// Files partially hashed (beginning and end).
    pub partial_hashes: u64,
    pub bytes: u64,
    /// Store queries for catalog reads (selections, field fetches, an
    /// image's locations, library windows).
    pub queries: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    /// New images.
    pub added: u64,
    /// Known images found at a new path.
    pub located: u64,
    /// Files already known at that path, unchanged: not read.
    pub unchanged: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RepairReport {
    /// Locations found missing in this pass.
    pub lost: u64,
    /// Images found again elsewhere, by content.
    pub relinked: u64,
    /// Images left with every location missing (an offline copy is not
    /// missing).
    pub missing: u64,
}

const PARTIAL: u64 = 64 * 1024;

/// The partial hash of a file of `size` bytes, as [`Image::partial`] holds
/// it (its beginning, its end and its size): a cheap first filter, which a
/// full hash confirms.
///
/// # Errors
/// The file cannot be read.
pub fn partial_hash(path: &Path, size: u64) -> Result<ContentHash, String> {
    partial_read(path, size).map(|(hash, _)| hash)
}

/// The partial hash of a file, and how many bytes it read.
fn partial_read(path: &Path, size: u64) -> Result<(ContentHash, u64), String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut file = File::open(path).map_err(err)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    let mut buffer = vec![0u8; PARTIAL.min(size) as usize];
    file.read_exact(&mut buffer).map_err(err)?;
    hasher.update(&buffer);
    let mut read = buffer.len() as u64;
    if size > PARTIAL {
        let tail = PARTIAL.min(size - PARTIAL);
        file.seek(SeekFrom::End(-(tail as i64))).map_err(err)?;
        let mut end = vec![0u8; tail as usize];
        file.read_exact(&mut end).map_err(err)?;
        hasher.update(&end);
        read += tail;
    }
    Ok((ContentHash(*hasher.finalize().as_bytes()), read))
}

pub struct Vault<S: Store> {
    store: Mutex<S>,
    reads: Mutex<Reads>,
    catalog: Mutex<catalog::State>,
    bus: Option<std::sync::Arc<helios_core::Bus>>,
}

impl<S: Store> Vault<S> {
    pub fn new(store: S) -> Self {
        Self {
            store: Mutex::new(store),
            reads: Mutex::new(Reads::default()),
            catalog: Mutex::new(catalog::State::default()),
            bus: None,
        }
    }

    /// Announces every change on `bus`.
    pub fn with_bus(mut self, bus: std::sync::Arc<helios_core::Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Runs `f` on the store in one transaction: committed when it
    /// succeeds, rolled back when it fails.
    pub(crate) fn transaction<T>(
        &self,
        f: impl FnOnce(&mut S) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut store = self.store.lock().expect("store lock");
        store.begin()?;
        match f(&mut store) {
            Ok(value) => {
                store.commit()?;
                Ok(value)
            }
            Err(e) => {
                // The first error is the one worth reporting.
                let _ = store.rollback();
                Err(e)
            }
        }
    }

    pub fn reads(&self) -> Reads {
        *self.reads.lock().expect("reads lock")
    }

    pub fn image(&self, id: &ImageId) -> Result<Option<Image>, String> {
        self.store.lock().expect("store lock").image(id)
    }

    pub fn images(&self) -> Result<Vec<Image>, String> {
        self.store.lock().expect("store lock").images()
    }

    /// An image's locations: one store query, counted in [`Vault::reads`].
    pub fn locations(&self, id: &ImageId) -> Result<Vec<Location>, String> {
        let locations = self.store.lock().expect("store lock").locations(id)?;
        self.reads.lock().expect("reads lock").queries += 1;
        Ok(locations)
    }

    fn full_hash(&self, path: &Path) -> Result<ContentHash, String> {
        let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut hasher = blake3::Hasher::new();
        let read = std::io::copy(&mut file, &mut hasher)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut reads = self.reads.lock().expect("reads lock");
        reads.full_hashes += 1;
        reads.bytes += read;
        Ok(ContentHash(*hasher.finalize().as_bytes()))
    }

    fn partial_hash(&self, path: &Path, size: u64) -> Result<ContentHash, String> {
        let (hash, read) = partial_read(path, size)?;
        let mut reads = self.reads.lock().expect("reads lock");
        reads.partial_hashes += 1;
        reads.bytes += read;
        Ok(hash)
    }

    /// Imports every image file under `root`.
    pub fn import(&self, root: &Path, now: u64) -> Result<ImportReport, String> {
        let mut report = ImportReport::default();
        for path in image_files(root)? {
            let (size, modified) = stat(&path)?;
            let known = self.store.lock().expect("store lock").at_path(&path)?;
            if let Some((id, location)) = &known
                && location.size == size
                && location.modified == modified
            {
                let verified = Location {
                    last_verified: now,
                    presence: Presence::Present,
                    ..location.clone()
                };
                // Seen before, imported now: the flag goes on, nothing is
                // read.
                let flagged = self.transaction(|store| {
                    store.put_location(id, &verified)?;
                    let image = store.image(id)?.ok_or("no image")?;
                    store.add_to_library(&image)
                })?;
                if flagged || !location.present() {
                    self.placed([*id]);
                }
                if flagged {
                    report.added += 1;
                } else {
                    report.unchanged += 1;
                }
                continue;
            }
            let id = ImageId(self.full_hash(&path)?);
            let partial = self.partial_hash(&path, size)?;
            // The image and its location go in together, or not at all:
            // recorded, then flagged in the Library.
            let new = self.transaction(|store| {
                store.add_image(&Image { id, size, partial })?;
                store.put_location(&id, &Location::seen(path, size, modified, now))?;
                store.add_to_library(&Image { id, size, partial })
            })?;
            // A new image, or a known one at a new path.
            self.placed([id]);
            if new {
                report.added += 1;
            } else {
                report.located += 1;
            }
        }
        Ok(report)
    }

    /// Imports one image file and returns its image: a file already known
    /// at that path, size and modification time is not read again.
    pub fn import_file(&self, path: &Path, now: u64) -> Result<ImageId, String> {
        let path = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (size, modified) = stat(&path)?;
        let known = self.store.lock().expect("store lock").at_path(&path)?;
        if let Some((id, location)) = known
            && location.size == size
            && location.modified == modified
            && location.present()
        {
            // Seen before: flagged without a read.
            let flagged = self.transaction(|store| {
                let image = store.image(&id)?.ok_or("no image")?;
                store.add_to_library(&image)
            })?;
            if flagged {
                self.placed([id]);
            }
            return Ok(id);
        }
        let id = ImageId(self.full_hash(&path)?);
        let partial = self.partial_hash(&path, size)?;
        self.transaction(|store| {
            store.add_image(&Image { id, size, partial })?;
            store.put_location(&id, &Location::seen(path, size, modified, now))?;
            store.add_to_library(&Image { id, size, partial })
        })?;
        // Even a known image has a new location.
        self.placed([id]);
        Ok(id)
    }

    /// The image a path is known as, with what was seen there.
    pub fn at_path(&self, path: &Path) -> Result<Option<(ImageId, Location)>, String> {
        self.store.lock().expect("store lock").at_path(path)
    }

    /// Records a file whose content hash the caller has just computed
    /// (while copying it, say), so it is not read in full again: only its
    /// partial hash is, unless the image is known (the same content has
    /// its partial hash). The image is put in the Library. Returns whether
    /// it is new to the Library.
    pub fn import_hashed(&self, path: &Path, id: ImageId, now: u64) -> Result<bool, String> {
        self.import_known(path, id, now, None)
    }

    /// [`Vault::import_hashed`], the Library flag set by `by` at `now`
    /// (an import's job): the file's location is recorded (a sighting at
    /// its place), its image recorded if unknown, then flagged.
    pub fn import_known(
        &self,
        path: &Path,
        id: ImageId,
        now: u64,
        by: Option<&str>,
    ) -> Result<bool, String> {
        let path = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (size, modified) = stat(&path)?;
        let known = self.image(&id)?.filter(|i| i.size == size);
        let partial = match known {
            Some(image) => image.partial,
            None => self.partial_hash(&path, size)?,
        };
        let new = self.transaction(|store| {
            let image = Image { id, size, partial };
            store.add_image(&image)?;
            store.put_location(&id, &Location::seen(path, size, modified, now))?;
            let on = store.library_flag(&id)?.is_some_and(|f| f.in_library);
            if !on {
                store.set_library_flag(
                    &id,
                    &LibraryFlag {
                        in_library: true,
                        at: by.map(|_| now),
                        by: by.map(Into::into),
                    },
                )?;
            }
            Ok(!on)
        })?;
        self.placed([id]);
        Ok(new)
    }

    /// Every location, with its image.
    pub fn all_locations(&self) -> Result<Vec<(ImageId, Location)>, String> {
        self.store.lock().expect("store lock").all_locations()
    }

    /// Writes locations a scan changed (their volume, path inside it,
    /// presence, or the absolute path a moved mount point gives), in one
    /// transaction. Each is `(image, the path it was stored at, the
    /// location now)`: a new path replaces the old row. Announced on the
    /// bus as [`LOCATIONS_CHANGED`] (`count`: how many changed), so a
    /// library shown reads its photos' presence again.
    pub fn update_locations(&self, updates: &[(ImageId, PathBuf, Location)]) -> Result<(), String> {
        if updates.is_empty() {
            return Ok(());
        }
        self.transaction(|store| {
            for (id, old, location) in updates {
                if *old != location.path {
                    store.remove_location(old)?;
                }
                store.put_location(id, location)?;
            }
            Ok(())
        })?;
        self.placed(updates.iter().map(|(id, _, _)| *id));
        if let Some(bus) = &self.bus {
            bus.publish_value(
                "vault",
                None,
                LOCATIONS_CHANGED,
                serde_json::json!({ "count": updates.len() }),
            );
        }
        Ok(())
    }

    /// Marks locations whose file is gone, then looks under `roots` for the
    /// images left with no present location, relinking them by content.
    /// An offline location (its volume unplugged, see `helios-storage`'s
    /// scan) is never marked missing, and an image with one is not looked
    /// for.
    pub fn repair(&self, roots: &[&Path], now: u64) -> Result<RepairReport, String> {
        let mut report = RepairReport::default();
        // The images whose copies this pass changed.
        let mut placed = Vec::new();
        let locations = self.store.lock().expect("store lock").all_locations()?;
        for (id, location) in &locations {
            if !location.present() {
                continue;
            }
            let still = stat(&location.path).is_ok_and(|(size, _)| size == location.size);
            if !still {
                let lost = Location {
                    presence: Presence::Missing,
                    last_verified: now,
                    ..location.clone()
                };
                self.store
                    .lock()
                    .expect("store lock")
                    .put_location(id, &lost)?;
                placed.push(*id);
                report.lost += 1;
            }
        }
        let mut missing: Vec<Image> = {
            let store = self.store.lock().expect("store lock");
            let mut missing = Vec::new();
            for image in store.images()? {
                if store
                    .locations(&image.id)?
                    .iter()
                    .all(|l| l.presence == Presence::Missing)
                {
                    missing.push(image);
                }
            }
            missing
        };
        for root in roots {
            for path in image_files(root)? {
                if missing.is_empty() {
                    break;
                }
                let known = self.store.lock().expect("store lock").at_path(&path)?;
                if known.is_some_and(|(_, l)| l.presence != Presence::Missing) {
                    continue;
                }
                let (size, modified) = stat(&path)?;
                if !missing.iter().any(|m| m.size == size) {
                    continue;
                }
                let partial = self.partial_hash(&path, size)?;
                if !missing
                    .iter()
                    .any(|m| m.size == size && m.partial == partial)
                {
                    continue;
                }
                let id = ImageId(self.full_hash(&path)?);
                if let Some(i) = missing.iter().position(|m| m.id == id) {
                    let location = Location::seen(path, size, modified, now);
                    self.store
                        .lock()
                        .expect("store lock")
                        .put_location(&id, &location)?;
                    missing.remove(i);
                    placed.push(id);
                    report.relinked += 1;
                }
            }
        }
        report.missing = missing.len() as u64;
        if !placed.is_empty() {
            self.placed(placed);
        }
        Ok(report)
    }
}

impl<S: Store> Describe for Vault<S> {
    fn describe(&self) -> serde_json::Value {
        let store = self.store.lock().expect("store lock");
        let images = store.images().map_or(0, |i| i.len());
        let locations = store.all_locations().unwrap_or_default();
        let count = |p: Presence| locations.iter().filter(|(_, l)| l.presence == p).count();
        json!({
            "images": images,
            "locations": {
                "all": locations.len(),
                "present": count(Presence::Present),
                "offline": count(Presence::Offline),
            },
            "reads": self.reads(),
            "catalog": self.catalog.lock().expect("catalog lock").describe(),
        })
    }
}

fn stat(path: &Path) -> Result<(u64, u64), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Ok((meta.len(), modified))
}

/// Image files under `root`, recursively, in a stable order.
fn image_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}
