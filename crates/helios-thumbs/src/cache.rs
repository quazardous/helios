//! The thumbnail cache on disk: JPEG files under one folder, named by
//! their key, with a size limit kept by evicting what was used least
//! recently. Its entries are the cache service's kinds
//! [`THUMB_SMALL`](helios_cache::THUMB_SMALL) and
//! [`THUMB_LARGE`](helios_cache::THUMB_LARGE), in the library's scope (a
//! silo: a library's thumbnails stay beside its catalogue); attached to
//! the service ([`ThumbCache::attach`]), it is counted and cleared with
//! the other caches.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::SystemTime;

use helios_cache::{
    Counters, Kind, Lru, Scope, Service, Store, THUMB_LARGE, THUMB_SMALL, Tier, Usage,
};
use image::RgbImage;

use crate::Rgb8;

/// The cache's default size limit: 2 GiB.
pub const DEFAULT_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

/// The JPEG quality of the cache's files.
pub const QUALITY: u8 = 80;

/// The version of the reader whose thumbnails [`ThumbCache::put`] stores,
/// written in each entry as a JPEG comment: 2 since a file's preview is
/// turned by its EXIF orientation. An entry without it was read before
/// (see [`ThumbCache::get_marked`]).
pub const READER: u32 = 2;

/// The comment that says an entry was stored by [`READER`].
const MARK: &[u8] = b"helios-thumbs reader 2";

/// Whether a JPEG starts with [`MARK`], right after its start marker.
fn marked(jpeg: &[u8]) -> bool {
    jpeg.get(2..4) == Some(&[0xFF, 0xFE][..]) && jpeg.get(6..6 + MARK.len()) == Some(MARK)
}

/// A JPEG with [`MARK`] as a comment right after its start marker.
fn with_mark(jpeg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(jpeg.len() + MARK.len() + 4);
    out.extend_from_slice(&jpeg[..2.min(jpeg.len())]);
    out.extend_from_slice(&[0xFF, 0xFE]);
    out.extend_from_slice(&u16::try_from(MARK.len() + 2).unwrap_or(0).to_be_bytes());
    out.extend_from_slice(MARK);
    out.extend_from_slice(jpeg.get(2..).unwrap_or_default());
    out
}

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

/// What a cache folder holds to tell backup tools it is a cache
/// (<https://bford.info/cachedir/>); the image map skips such folders too.
pub const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n\
# This file is a cache directory tag created by Helios (its thumbnails).\n\
# For information about cache directory tags, see https://bford.info/cachedir/\n";

/// The kind of an entry, by its file name (`<image>.<version>.<size>.jpg`).
fn kind_of(rel: &Path) -> Kind {
    let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if name.ends_with(&format!(".{}.jpg", crate::LARGE)) {
        THUMB_LARGE
    } else {
        THUMB_SMALL
    }
}

/// Whether the folder has been listed into the index.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listing {
    NotYet,
    Running,
    Done,
}

struct Index {
    /// The entries, by file relative to the folder, in eviction order:
    /// each last used at a tick of [`ThumbCache::clock`] (the file's
    /// modification time when the folder was listed).
    entries: Lru<PathBuf>,
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
    /// The scope's statistics, per kind (small, large).
    counters: [Arc<Counters>; 2],
    scope: Scope,
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
        let tag = dir.join("CACHEDIR.TAG");
        if !tag.exists() {
            // Best effort: a cache without its tag still works.
            let _ = std::fs::write(&tag, CACHEDIR_TAG);
        }
        let scope = Scope::silo(format!("library:{}", dir.display()));
        let service = Service::global();
        Ok(ThumbCache {
            dir: dir.to_path_buf(),
            limit,
            index: Mutex::new(Index {
                entries: Lru::new(),
                listing: Listing::NotYet,
            }),
            listed: Condvar::new(),
            clock: AtomicU64::new(nanos(SystemTime::now())),
            counters: [
                service.counters(&scope, &THUMB_SMALL),
                service.counters(&scope, &THUMB_LARGE),
            ],
            scope,
        })
    }

    /// Attaches the cache to the process's cache service: counted on the
    /// Caches page and by `cache.stats`, cleared by "Clear caches".
    pub fn attach(self: &Arc<Self>) {
        let weak: std::sync::Weak<dyn Store> = Arc::downgrade(self) as std::sync::Weak<dyn Store>;
        Service::global().attach(weak);
    }

    /// The statistics of an entry's kind.
    fn counters_of(&self, rel: &Path) -> &Counters {
        &self.counters[usize::from(kind_of(rel).id == THUMB_LARGE.id)]
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
        let mut entries = Lru::new();
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
                let rel = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
                entries.insert(rel, meta.len(), used, 0);
            }
        }
        self.clock.fetch_max(latest, Ordering::Relaxed);
        {
            let mut index = self.index.lock().expect("the cache's index");
            index.entries = entries;
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
        (index.entries.bytes(), index.entries.len())
    }

    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Whether an entry is there.
    pub fn contains(&self, key: &Key, size: u32) -> bool {
        let index = self.ready();
        index.entries.contains(&key.file(size))
    }

    /// The thumbnail of `key` at `size` pixels on its long side, when the
    /// cache holds it: marked used, so evicted last.
    pub fn get(&self, key: &Key, size: u32) -> Option<Rgb8> {
        self.get_marked(key, size).map(|(rgb, _)| rgb)
    }

    /// [`ThumbCache::get`] for the interface's frame: never waits (none
    /// while the folder is being listed), and only an entry stored by this
    /// reader ([`READER`]); an older one is left to the pool, which checks
    /// its picture's orientation.
    pub fn try_get(&self, key: &Key, size: u32) -> Option<Rgb8> {
        {
            let index = self.index.lock().expect("the cache's index");
            if index.listing != Listing::Done || !index.entries.contains(&key.file(size)) {
                return None;
            }
        }
        let (rgb, current) = self.get_marked(key, size)?;
        current.then_some(rgb)
    }

    /// [`ThumbCache::get`], and whether the entry was stored by this
    /// reader ([`READER`]): one stored before was read unturned (a
    /// portrait picture's sideways).
    pub fn get_marked(&self, key: &Key, size: u32) -> Option<(Rgb8, bool)> {
        let rel = key.file(size);
        let tick = self.tick();
        {
            let mut index = self.ready();
            if !index.entries.touch(&rel, tick) {
                drop(index);
                Counters::add(&self.counters_of(&rel).misses, 1);
                return None;
            }
        }
        let path = self.dir.join(&rel);
        let bytes = std::fs::read(&path).ok();
        let counters = self.counters_of(&rel);
        match &bytes {
            Some(b) => {
                Counters::add(&counters.hits, 1);
                Counters::add(&counters.bytes_read, b.len() as u64);
            }
            None => Counters::add(&counters.misses, 1),
        }
        let current = bytes.as_deref().is_some_and(marked);
        let decoded = bytes.and_then(|bytes| image::load_from_memory(&bytes).ok());
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
        Some((
            Rgb8 {
                width: rgb.width(),
                height: rgb.height(),
                pixels: rgb.into_raw(),
            },
            current,
        ))
    }

    /// Marks the entries of `key` (both sizes) as stored by this reader
    /// ([`READER`]), their pixels kept: what an entry stored before gets
    /// once it is known to be right (a file with nothing to turn).
    pub fn mark(&self, key: &Key) {
        for size in [crate::SMALL, crate::LARGE] {
            if !self.contains(key, size) {
                continue;
            }
            let Ok(jpeg) = std::fs::read(self.dir.join(key.file(size))) else {
                continue;
            };
            if !marked(&jpeg) {
                let _ = self.put_jpeg(key, size, &with_mark(&jpeg));
            }
        }
    }

    /// Removes the entry of `key` at `size`, when there is one (its
    /// picture is out of date).
    pub fn remove(&self, key: &Key, size: u32) {
        if self.contains(key, size) {
            self.forget(&key.file(size));
        }
    }

    fn forget(&self, rel: &Path) {
        let mut index = self.index.lock().expect("the cache's index");
        index.entries.remove(&rel.to_path_buf());
        let _ = std::fs::remove_file(self.dir.join(rel));
    }

    /// Stores the thumbnail of `key` at `size`, as a JPEG marked as this
    /// reader's ([`READER`]); then evicts
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
        self.put_jpeg(key, size, &with_mark(&jpeg))
    }

    /// The file of an entry, relative to the cache's folder.
    pub fn file_of(&self, key: &Key, size: u32) -> PathBuf {
        key.file(size)
    }

    /// Stores a thumbnail already encoded as a JPEG (a render's, a copy of
    /// another entry), as it is; then evicts as [`ThumbCache::put`] does.
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
            index
                .entries
                .insert(rel.clone(), jpeg.len() as u64, tick, 0);
        }
        Counters::add(&self.counters_of(&rel).bytes_written, jpeg.len() as u64);
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
            let Some(bytes) = index.entries.remove(&old) else {
                continue;
            };
            if index.entries.contains(&new) {
                drop(index);
                let _ = std::fs::remove_file(self.dir.join(&old));
                continue;
            }
            let path = self.dir.join(&new);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::rename(self.dir.join(&old), &path).is_ok() {
                // Its last use kept: a rename is not a use.
                let tick = self.clock.load(Ordering::Relaxed);
                index.entries.insert(new, bytes, tick, 0);
                moved += 1;
            }
        }
        moved
    }

    /// Removes the entries used least recently until the cache is under
    /// its limit, down to a tenth below it so a full cache does not evict
    /// at every write; `keep` stays (the entry just written). The index is
    /// kept in eviction order: the cost is the victims', not the entries'.
    fn evict(&self, keep: Option<&Path>) {
        let gone = {
            let mut index = self.index.lock().expect("the cache's index");
            if index.entries.bytes() <= self.limit {
                return;
            }
            let target = helios_cache::eviction_target(self.limit);
            index
                .entries
                .evict(target, |rel| Some(rel.as_path()) == keep)
        };
        for (rel, _) in gone {
            Counters::add(&self.counters_of(&rel).evictions, 1);
            let _ = std::fs::remove_file(self.dir.join(rel));
        }
    }
}

impl Store for ThumbCache {
    /// Never waits for the folder's listing: until it is done, what is
    /// known so far.
    fn usage(&self) -> Vec<Usage> {
        let index = self.index.lock().expect("the cache's index");
        let mut usage = [THUMB_SMALL, THUMB_LARGE].map(|kind| Usage {
            scope: self.scope.clone(),
            kind,
            tier: Tier::Disk,
            entries: 0,
            bytes: 0,
            pinned: 0,
            budget: Some(self.limit),
            shown: None,
        });
        for (rel, bytes) in index.entries.iter() {
            let u = &mut usage[usize::from(kind_of(rel).id == THUMB_LARGE.id)];
            u.entries += 1;
            u.bytes += bytes;
        }
        usage.into()
    }

    /// Lists the folder first when it was not yet: what is on disk goes.
    fn clear(&self, kind: Option<&str>) -> u64 {
        self.list();
        let gone = {
            let mut index = self.index.lock().expect("the cache's index");
            index
                .entries
                .clear(|rel| kind.is_some_and(|k| k != kind_of(rel).id))
        };
        let mut freed = 0;
        for (rel, bytes) in gone {
            Counters::add(&self.counters_of(&rel).cleared, 1);
            let _ = std::fs::remove_file(self.dir.join(rel));
            freed += bytes;
        }
        freed
    }
}
