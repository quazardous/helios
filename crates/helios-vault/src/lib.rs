//! The vault: the catalog as an API.
//!
//! This first part is identity and storage (ROADMAP section 2.3):
//!
//! - an image **is its content**: its id is the BLAKE3 hash of the file;
//! - where a file sits is **data**: an image has any number of locations
//!   (path, size, modification time, last verified, present or not);
//! - **import** walks folders and records what it finds; a file already
//!   known at the same path, size and modification time is not read again;
//! - **repair** marks vanished locations and relinks moved or renamed files
//!   by content, reading as little as it can: same size first, then a
//!   partial hash, then the full hash only for the remaining candidates.
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

pub mod catalog;
pub mod contract;
pub mod edit;
pub mod fields;
pub mod journal;
pub mod settings;
pub mod versions;

pub use catalog::{ApplyReport, CursorId, CursorInfo, Delta, Row};
pub use edit::{EditSaved, EditSession};
pub use fields::{AuditEntry, Command, Order, Query};
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

/// Where a copy of an image was seen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Location {
    pub path: PathBuf,
    pub size: u64,
    /// Modification time when last seen, in seconds since the epoch.
    pub modified: u64,
    pub last_verified: u64,
    /// False once the file is no longer found there.
    pub present: bool,
}

/// Where rows are kept. A store only stores: every rule is the vault's.
pub trait Store: Send {
    /// Adds an image; false if it was already known.
    fn add_image(&mut self, image: &Image) -> Result<bool, String>;
    fn image(&self, id: &ImageId) -> Result<Option<Image>, String>;
    fn images(&self) -> Result<Vec<Image>, String>;
    /// Records a location; a path belongs to one image at a time.
    fn put_location(&mut self, id: &ImageId, location: &Location) -> Result<(), String>;
    fn locations(&self, id: &ImageId) -> Result<Vec<Location>, String>;
    fn at_path(&self, path: &Path) -> Result<Option<(ImageId, Location)>, String>;
    fn all_locations(&self) -> Result<Vec<(ImageId, Location)>, String>;

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
    /// The images a query selects, in its order.
    fn select(&self, query: &Query) -> Result<Vec<ImageId>, String>;
    /// Records a change; returns its sequence number.
    fn add_audit(&mut self, entry: &AuditEntry) -> Result<u64, String>;
    fn audit(&self, id: &ImageId) -> Result<Vec<AuditEntry>, String>;

    /// Keeps a version; false if it was already kept (same id).
    fn put_version(&mut self, version: &Version) -> Result<bool, String>;
    fn version(&self, id: &VersionId) -> Result<Option<Version>, String>;
    /// An image's versions, in no particular order.
    fn versions(&self, image: &ImageId) -> Result<Vec<Version>, String>;

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
    images: BTreeMap<ImageId, Image>,
    locations: BTreeMap<PathBuf, (ImageId, Location)>,
    fields: BTreeMap<(ImageId, String), serde_json::Value>,
    audit: Vec<AuditEntry>,
}

impl Store for MemoryStore {
    fn add_image(&mut self, image: &Image) -> Result<bool, String> {
        Ok(self.images.insert(image.id, image.clone()).is_none())
    }

    fn image(&self, id: &ImageId) -> Result<Option<Image>, String> {
        Ok(self.images.get(id).cloned())
    }

    fn images(&self) -> Result<Vec<Image>, String> {
        Ok(self.images.values().cloned().collect())
    }

    fn put_location(&mut self, id: &ImageId, location: &Location) -> Result<(), String> {
        if !self.images.contains_key(id) {
            return Err(format!("no image {}", id.hex()));
        }
        self.locations
            .insert(location.path.clone(), (*id, location.clone()));
        Ok(())
    }

    fn locations(&self, id: &ImageId) -> Result<Vec<Location>, String> {
        Ok(self
            .locations
            .values()
            .filter(|(i, _)| i == id)
            .map(|(_, l)| l.clone())
            .collect())
    }

    fn at_path(&self, path: &Path) -> Result<Option<(ImageId, Location)>, String> {
        Ok(self.locations.get(path).cloned())
    }

    fn all_locations(&self) -> Result<Vec<(ImageId, Location)>, String> {
        Ok(self.locations.values().cloned().collect())
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
        let rating = |id: &ImageId| {
            self.fields
                .get(&(*id, fields::RATING.to_string()))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0)
        };
        let mut ids: Vec<ImageId> = self
            .images
            .keys()
            .filter(|id| query.min_rating.is_none_or(|min| rating(id) >= min))
            .filter(|id| {
                query.label.as_ref().is_none_or(|label| {
                    self.fields
                        .get(&(**id, fields::LABELS.to_string()))
                        .and_then(|v| v.as_array())
                        .is_some_and(|labels| {
                            labels.iter().any(|l| l.as_str() == Some(label.as_str()))
                        })
                })
            })
            .copied()
            .collect();
        if query.order == Order::RatingDesc {
            ids.sort_by_key(|id| (std::cmp::Reverse(rating(id)), *id));
        }
        Ok(ids)
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
    /// Store queries for catalog reads (selections, field fetches).
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
    /// Images left with no present location.
    pub missing: u64,
}

const PARTIAL: u64 = 64 * 1024;

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

    pub fn locations(&self, id: &ImageId) -> Result<Vec<Location>, String> {
        self.store.lock().expect("store lock").locations(id)
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
        let mut reads = self.reads.lock().expect("reads lock");
        reads.partial_hashes += 1;
        reads.bytes += read;
        Ok(ContentHash(*hasher.finalize().as_bytes()))
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
                    present: true,
                    ..location.clone()
                };
                self.store
                    .lock()
                    .expect("store lock")
                    .put_location(id, &verified)?;
                report.unchanged += 1;
                continue;
            }
            let id = ImageId(self.full_hash(&path)?);
            let partial = self.partial_hash(&path, size)?;
            // The image and its location go in together, or not at all.
            let new = self.transaction(|store| {
                let new = store.add_image(&Image { id, size, partial })?;
                store.put_location(
                    &id,
                    &Location {
                        path,
                        size,
                        modified,
                        last_verified: now,
                        present: true,
                    },
                )?;
                Ok(new)
            })?;
            if new {
                report.added += 1;
                self.catalog.lock().expect("catalog lock").version += 1;
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
            && location.present
        {
            return Ok(id);
        }
        let id = ImageId(self.full_hash(&path)?);
        let partial = self.partial_hash(&path, size)?;
        let new = self.transaction(|store| {
            let new = store.add_image(&Image { id, size, partial })?;
            store.put_location(
                &id,
                &Location {
                    path,
                    size,
                    modified,
                    last_verified: now,
                    present: true,
                },
            )?;
            Ok(new)
        })?;
        if new {
            self.catalog.lock().expect("catalog lock").version += 1;
        }
        Ok(id)
    }

    /// Marks locations whose file is gone, then looks under `roots` for the
    /// images left with no present location, relinking them by content.
    pub fn repair(&self, roots: &[&Path], now: u64) -> Result<RepairReport, String> {
        let mut report = RepairReport::default();
        let locations = self.store.lock().expect("store lock").all_locations()?;
        for (id, location) in &locations {
            let still = location.present
                && stat(&location.path).is_ok_and(|(size, _)| size == location.size);
            if location.present && !still {
                let lost = Location {
                    present: false,
                    last_verified: now,
                    ..location.clone()
                };
                self.store
                    .lock()
                    .expect("store lock")
                    .put_location(id, &lost)?;
                report.lost += 1;
            }
        }
        let mut missing: Vec<Image> = {
            let store = self.store.lock().expect("store lock");
            let mut missing = Vec::new();
            for image in store.images()? {
                if !store.locations(&image.id)?.iter().any(|l| l.present) {
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
                if known.is_some_and(|(_, l)| l.present) {
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
                    let location = Location {
                        path,
                        size,
                        modified,
                        last_verified: now,
                        present: true,
                    };
                    self.store
                        .lock()
                        .expect("store lock")
                        .put_location(&id, &location)?;
                    missing.remove(i);
                    report.relinked += 1;
                }
            }
        }
        report.missing = missing.len() as u64;
        Ok(report)
    }
}

impl<S: Store> Describe for Vault<S> {
    fn describe(&self) -> serde_json::Value {
        let store = self.store.lock().expect("store lock");
        let images = store.images().map_or(0, |i| i.len());
        let locations = store.all_locations().unwrap_or_default();
        let present = locations.iter().filter(|(_, l)| l.present).count();
        json!({
            "images": images,
            "locations": { "all": locations.len(), "present": present },
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
