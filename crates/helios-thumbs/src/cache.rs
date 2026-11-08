//! The thumbnail cache on disk: JPEG files under one folder, named by
//! their key, with a size limit kept by evicting what was used least
//! recently.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::SystemTime;

use image::RgbImage;

use crate::Rgb8;

/// The cache's default size limit: 2 GiB.
pub const DEFAULT_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

/// The JPEG quality of the cache's files.
pub const QUALITY: u8 = 80;

/// What a cache entry is the thumbnail of.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    /// The image's id: the BLAKE3 of its file, so a file imported again
    /// finds its entry.
    pub image: String,
    /// The version drawn: [`Key::PREVIEW`] for the file's embedded
    /// preview; a version's id once thumbnails are rendered from edits.
    pub version: String,
    /// The version of the chain that rendered it (0 for the embedded
    /// preview).
    pub chain: u64,
}

impl Key {
    /// The version of a thumbnail read from the file's embedded preview.
    pub const PREVIEW: &'static str = "preview";

    /// The key of an image's embedded preview.
    pub fn preview(image: impl Into<String>) -> Key {
        Key {
            image: image.into(),
            version: Key::PREVIEW.to_string(),
            chain: 0,
        }
    }

    /// The provisional key of a file seen but not fully hashed yet:
    /// `p:<size>:<partial hash>` (`docs/design/image-map.md`, section 7).
    /// Its entry is renamed to the full key ([`ThumbCache::rename`]) once
    /// the full hash links the file to an image.
    pub fn provisional(size: u64, partial: &str) -> Key {
        Key::preview(format!("p:{size}:{partial}"))
    }

    /// Whether it is a provisional key ([`Key::provisional`]).
    pub fn is_provisional(&self) -> bool {
        self.image.starts_with("p:")
    }

    /// The entry's file name, relative to the cache's folder.
    fn file(&self, size: u32) -> PathBuf {
        let image: String = self
            .image
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        let shard = image.get(..2).unwrap_or("00").to_string();
        let version = if self.version == Key::PREVIEW && self.chain == 0 {
            Key::PREVIEW.to_string()
        } else {
            let mut h = blake3::Hasher::new();
            h.update(self.version.as_bytes());
            h.update(&self.chain.to_le_bytes());
            h.finalize().to_hex()[..16].to_string()
        };
        PathBuf::from(shard).join(format!("{image}.{version}.{size}.jpg"))
    }
}

struct Entry {
    bytes: u64,
    /// The last use, as a tick of [`ThumbCache::clock`] (the file's
    /// modification time when the cache was opened).
    used: u64,
}

/// Whether the folder has been listed into the index.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listing {
    NotYet,
    Running,
    Done,
}

struct Index {
    entries: HashMap<PathBuf, Entry>,
    total: u64,
    listing: Listing,
}

/// The cache: a folder of JPEG thumbnails.
pub struct ThumbCache {
    dir: PathBuf,
    limit: u64,
    index: Mutex<Index>,
    /// Signalled once the folder is listed.
    listed: Condvar,
    clock: AtomicU64,
}

fn nanos(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

impl ThumbCache {
    /// Opens (creating it) the cache in `dir`, at most `limit` bytes; its
    /// entries are listed, each last used when its file last changed.
    ///
    /// # Errors
    /// The folder cannot be made.
    pub fn open(dir: &Path, limit: u64) -> Result<ThumbCache, String> {
        let cache = ThumbCache::open_unlisted(dir, limit)?;
        cache.list();
        Ok(cache)
    }

    /// Opens (creating it) the cache in `dir` without listing its folder:
    /// what the interface thread does at startup. The folder is listed by
    /// [`ThumbCache::list`] (the pool's threads call it first); until then
    /// every lookup waits for it, except [`ThumbCache::listed`].
    ///
    /// # Errors
    /// The folder cannot be made.
    pub fn open_unlisted(dir: &Path, limit: u64) -> Result<ThumbCache, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot make the thumbnail cache {}: {e}", dir.display()))?;
        Ok(ThumbCache {
            dir: dir.to_path_buf(),
            limit,
            index: Mutex::new(Index {
                entries: HashMap::new(),
                total: 0,
                listing: Listing::NotYet,
            }),
            listed: Condvar::new(),
            clock: AtomicU64::new(nanos(SystemTime::now())),
        })
    }

    /// Whether the folder has been listed (never waits).
    pub fn listed(&self) -> bool {
        self.index.lock().expect("the cache's index").listing == Listing::Done
    }

    /// Lists the folder into the index, once: each entry last used when
    /// its file last changed. A second caller waits for the first.
    pub fn list(&self) {
        {
            let mut index = self.index.lock().expect("the cache's index");
            match index.listing {
                Listing::Done => return,
                Listing::Running => {
                    drop(self.ready_from(index));
                    return;
                }
                Listing::NotYet => index.listing = Listing::Running,
            }
        }
        let dir = &self.dir;
        let mut entries = HashMap::new();
        let mut total = 0;
        let mut latest = 0;
        for shard in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(files) = std::fs::read_dir(shard.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_none_or(|e| e != "jpg") {
                    // An interrupted write.
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                let Ok(meta) = file.metadata() else { continue };
                let used = meta.modified().map_or(0, nanos);
                latest = latest.max(used);
                total += meta.len();
                let rel = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
                entries.insert(
                    rel,
                    Entry {
                        bytes: meta.len(),
                        used,
                    },
                );
            }
        }
        self.clock.fetch_max(latest, Ordering::Relaxed);
        {
            let mut index = self.index.lock().expect("the cache's index");
            index.entries = entries;
            index.total = total;
            index.listing = Listing::Done;
        }
        self.listed.notify_all();
        self.evict(None);
    }

    /// The index, once the folder is listed.
    fn ready(&self) -> MutexGuard<'_, Index> {
        self.ready_from(self.index.lock().expect("the cache's index"))
    }

    fn ready_from<'a>(&self, mut index: MutexGuard<'a, Index>) -> MutexGuard<'a, Index> {
        while index.listing != Listing::Done {
            index = self.listed.wait(index).expect("the cache's index");
        }
        index
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// The bytes the entries take, and how many there are.
    pub fn usage(&self) -> (u64, usize) {
        let index = self.ready();
        (index.total, index.entries.len())
    }

    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Whether an entry is there.
    pub fn contains(&self, key: &Key, size: u32) -> bool {
        let index = self.ready();
        index.entries.contains_key(&key.file(size))
    }

    /// The thumbnail of `key` at `size` pixels on its long side, when the
    /// cache holds it: marked used, so evicted last.
    pub fn get(&self, key: &Key, size: u32) -> Option<Rgb8> {
        let rel = key.file(size);
        let tick = self.tick();
        {
            let mut index = self.ready();
            index.entries.get_mut(&rel)?.used = tick;
        }
        let path = self.dir.join(&rel);
        let decoded = std::fs::read(&path)
            .ok()
            .and_then(|bytes| image::load_from_memory(&bytes).ok());
        let Some(img) = decoded else {
            // Gone or damaged: forgotten.
            self.forget(&rel);
            return None;
        };
        // Its use survives a restart (best effort).
        if let Ok(f) = std::fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
        let rgb = img.to_rgb8();
        Some(Rgb8 {
            width: rgb.width(),
            height: rgb.height(),
            pixels: rgb.into_raw(),
        })
    }

    fn forget(&self, rel: &Path) {
        let mut index = self.index.lock().expect("the cache's index");
        if let Some(e) = index.entries.remove(rel) {
            index.total -= e.bytes;
        }
        let _ = std::fs::remove_file(self.dir.join(rel));
    }

    /// Stores the thumbnail of `key` at `size`, as a JPEG; then evicts
    /// what was used least recently until the cache is under its limit.
    ///
    /// # Errors
    /// The thumbnail cannot be encoded or written.
    pub fn put(&self, key: &Key, size: u32, thumb: &Rgb8) -> Result<(), String> {
        let rgb = RgbImage::from_raw(thumb.width, thumb.height, thumb.pixels.clone())
            .ok_or("a thumbnail whose pixels do not fill it")?;
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), QUALITY)
            .encode_image(&rgb)
            .map_err(|e| e.to_string())?;
        self.put_jpeg(key, size, &jpeg)
    }

    /// The file of an entry, relative to the cache's folder.
    pub fn file_of(&self, key: &Key, size: u32) -> PathBuf {
        key.file(size)
    }

    /// Stores a thumbnail already encoded as a JPEG (a render's, a copy of
    /// another entry); then evicts as [`ThumbCache::put`] does.
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn put_jpeg(&self, key: &Key, size: u32, jpeg: &[u8]) -> Result<(), String> {
        // Not before the folder is listed: the listing removes what it
        // takes for an interrupted write.
        drop(self.ready());
        let rel = key.file(size);
        let path = self.dir.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // Written aside, then renamed: a reader never sees half a file.
        let tmp = path.with_extension(format!("{}.part", std::process::id()));
        std::fs::write(&tmp, jpeg).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        let tick = self.tick();
        {
            let mut index = self.index.lock().expect("the cache's index");
            let bytes = jpeg.len() as u64;
            if let Some(old) = index
                .entries
                .insert(rel.clone(), Entry { bytes, used: tick })
            {
                index.total -= old.bytes;
            }
            index.total += bytes;
        }
        self.evict(Some(&rel));
        Ok(())
    }

    /// Renames the entries of `from` (every size) to `to`: a cheap file
    /// rename, for a provisional key once the full hash is known. An entry
    /// `to` already has is kept and `from`'s is dropped. Returns how many
    /// entries moved.
    pub fn rename(&self, from: &Key, to: &Key) -> usize {
        let mut moved = 0;
        for size in [crate::SMALL, crate::LARGE] {
            let (old, new) = (from.file(size), to.file(size));
            let mut index = self.ready();
            let Some(entry) = index.entries.remove(&old) else {
                continue;
            };
            if index.entries.contains_key(&new) {
                index.total -= entry.bytes;
                drop(index);
                let _ = std::fs::remove_file(self.dir.join(&old));
                continue;
            }
            let path = self.dir.join(&new);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::rename(self.dir.join(&old), &path).is_ok() {
                index.entries.insert(new, entry);
                moved += 1;
            } else {
                index.total -= entry.bytes;
            }
        }
        moved
    }

    /// Removes the entries used least recently until the cache is under
    /// its limit; `keep` stays (the entry just written).
    fn evict(&self, keep: Option<&Path>) {
        let gone = {
            let mut index = self.index.lock().expect("the cache's index");
            if index.total <= self.limit {
                return;
            }
            let mut order: Vec<(u64, PathBuf, u64)> = index
                .entries
                .iter()
                .filter(|(rel, _)| Some(rel.as_path()) != keep)
                .map(|(rel, e)| (e.used, rel.clone(), e.bytes))
                .collect();
            order.sort_unstable();
            // A tenth below the limit, so a full cache does not evict at
            // every write.
            let target = self.limit - self.limit / 10;
            let mut gone = Vec::new();
            for (_, rel, bytes) in order {
                if index.total <= target {
                    break;
                }
                index.entries.remove(&rel);
                index.total -= bytes;
                gone.push(rel);
            }
            gone
        };
        for rel in gone {
            let _ = std::fs::remove_file(self.dir.join(rel));
        }
    }
}
