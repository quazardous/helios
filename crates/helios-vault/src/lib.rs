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
//! Rows live in a [`Store`]: [`MemoryStore`] and [`sqlite::SqliteStore`]
//! here, both checked by the same [`contract`]. Time is passed in, never
//! read, and every byte hashed is counted, so tests check budgets.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use helios_core::{ContentHash, Describe};
use serde::Serialize;
use serde_json::json;

pub mod contract;
pub mod sqlite;

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

fn parse_hash(hex: &str) -> Result<ContentHash, String> {
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
}

/// Rows in memory: the reference other stores are checked against.
#[derive(Default)]
pub struct MemoryStore {
    images: BTreeMap<ImageId, Image>,
    locations: BTreeMap<PathBuf, (ImageId, Location)>,
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
}

impl<S: Store> Vault<S> {
    pub fn new(store: S) -> Self {
        Self {
            store: Mutex::new(store),
            reads: Mutex::new(Reads::default()),
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
            let mut store = self.store.lock().expect("store lock");
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
            if new {
                report.added += 1;
            } else {
                report.located += 1;
            }
        }
        Ok(report)
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
        let images = store.images().map(|i| i.len()).unwrap_or(0);
        let locations = store.all_locations().unwrap_or_default();
        let present = locations.iter().filter(|(_, l)| l.present).count();
        json!({
            "images": images,
            "locations": { "all": locations.len(), "present": present },
            "reads": self.reads(),
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
