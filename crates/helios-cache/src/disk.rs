//! The disk tier: a kind's entries as files under one folder, kept within
//! a size budget, least recently used first out, and pruned once not used
//! for the kind's [`Kind::unused`].
//!
//! The folder is `<root>/<kind id>/v<version>`: a kind whose format
//! changes bumps its version and the older folders are deleted when it
//! opens. Each entry is one file named by the BLAKE3 hash of its key, so
//! a key may be anything (a content hash, a volume and a path) and never
//! leaks into a file name. Its last use is its file's modification time:
//! listing the folder rebuilds the index, no database to corrupt. Writes
//! go to a temporary name then a rename, so a crash leaves a whole entry
//! or none, and two processes may share the folder.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use crate::{Counters, Kind, Lru, Scope, Service, Store, Tier, Usage};

/// What a cache folder holds to tell backup tools it is a cache
/// (<https://bford.info/cachedir/>); Helios's image map skips such
/// folders too.
pub const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n\
# This file is a cache directory tag created by Helios.\n\
# For information about cache directory tags, see https://bford.info/cachedir/\n";

/// An entry's file extension; anything else in the folder is a write a
/// crash interrupted, removed when the folder is listed.
const EXTENSION: &str = "entry";

fn nanos(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// Helios's folder in the platform's cache folder (`$XDG_CACHE_HOME/helios`,
/// else `~/.cache/helios`; `%LOCALAPPDATA%\helios` on Windows): the user
/// cache scope's disk tier.
pub fn user_cache_dir() -> Option<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))?;
    Some(cache.join("helios"))
}

/// One kind's disk tier in one scope.
pub struct Disk {
    dir: PathBuf,
    kind: Kind,
    scope: Scope,
    limit: u64,
    /// The entries by file (relative to `dir`), each last used at a time
    /// in nanoseconds since the Unix epoch, no bonus: least recently used
    /// first.
    index: Mutex<Lru<PathBuf>>,
    counters: Arc<Counters>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Disk {
    /// Opens (creating it) the disk tier of `kind` under `root`, at most
    /// `limit` bytes, counted in `scope`. Lists its folder (each entry
    /// last used when its file last changed), deletes the folders of the
    /// kind's older versions, and prunes what was not used since `now`
    /// less the kind's [`Kind::unused`]. It reads a folder: never on the
    /// interface thread.
    ///
    /// # Errors
    /// The folder cannot be made.
    pub fn open(
        root: &Path,
        kind: Kind,
        scope: Scope,
        limit: u64,
        now: SystemTime,
    ) -> std::io::Result<Disk> {
        let base = root.join(kind.id);
        let dir = base.join(format!("v{}", kind.version));
        std::fs::create_dir_all(&dir)?;
        let tag = root.join("CACHEDIR.TAG");
        if !tag.exists() {
            // Best effort: a cache without its tag still works.
            let _ = std::fs::write(&tag, CACHEDIR_TAG);
        }
        for old in std::fs::read_dir(&base).into_iter().flatten().flatten() {
            if old.path() != dir && old.path().is_dir() {
                let _ = std::fs::remove_dir_all(old.path());
            }
        }
        let mut entries = Lru::new();
        for shard in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(files) = std::fs::read_dir(shard.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_none_or(|e| e != EXTENSION) {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                let Ok(meta) = file.metadata() else { continue };
                let used = meta.modified().map_or(0, nanos);
                let rel = path.strip_prefix(&dir).unwrap_or(&path).to_path_buf();
                entries.insert(rel, meta.len(), used, 0);
            }
        }
        let disk = Disk {
            counters: Service::global().counters(&scope, &kind),
            dir,
            kind,
            scope,
            limit,
            index: Mutex::new(entries),
        };
        disk.prune(now);
        disk.evict(None);
        Ok(disk)
    }

    /// Attaches it to the process's cache service: counted on the Caches
    /// page and by `cache.stats`, cleared by "Clear caches".
    pub fn attach(self: &Arc<Self>) {
        let weak = Arc::downgrade(self) as std::sync::Weak<dyn Store>;
        Service::global().attach(weak);
    }

    /// Its folder: `<root>/<kind id>/v<version>`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// The bytes its entries take, and how many there are.
    pub fn usage(&self) -> (u64, usize) {
        let index = lock(&self.index);
        (index.bytes(), index.len())
    }

    /// The file of the entry of `key`, relative to its folder.
    fn rel(key: &[u8]) -> PathBuf {
        let hex = blake3::hash(key).to_hex();
        PathBuf::from(&hex[..2]).join(format!("{}.{EXTENSION}", &hex[2..32]))
    }

    /// The file of the entry of `key` (there or not).
    pub fn file_of(&self, key: &[u8]) -> PathBuf {
        self.dir.join(Disk::rel(key))
    }

    /// Whether an entry of `key` is there; not a use.
    pub fn contains(&self, key: &[u8]) -> bool {
        lock(&self.index).contains(&Disk::rel(key))
    }

    /// The bytes of the entry of `key`, when there: marked used now, so
    /// it goes last, and its use survives a restart.
    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let rel = Disk::rel(key);
        let now = SystemTime::now();
        if !lock(&self.index).touch(&rel, nanos(now)) {
            return None;
        }
        let path = self.dir.join(&rel);
        let Ok(bytes) = std::fs::read(&path) else {
            // Gone (another process, a person): forgotten.
            lock(&self.index).remove(&rel);
            return None;
        };
        Counters::add(&self.counters.bytes_read, bytes.len() as u64);
        if let Ok(f) = std::fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(now);
        }
        Some(bytes)
    }

    /// The bytes of the entry of `key`, when there, without marking it
    /// used: to compare it with one that may replace it.
    pub fn peek(&self, key: &[u8]) -> Option<Vec<u8>> {
        if !self.contains(key) {
            return None;
        }
        std::fs::read(self.file_of(key)).ok()
    }

    /// Stores `bytes` as the entry of `key` (replacing it), used now;
    /// then evicts the least recently used until the tier is under its
    /// limit, down to 90 % of it so a full tier does not evict at every
    /// write.
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn put(&self, key: &[u8], bytes: &[u8]) -> std::io::Result<()> {
        let rel = Disk::rel(key);
        let path = self.dir.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("{}.part", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
        lock(&self.index).insert(rel.clone(), bytes.len() as u64, nanos(SystemTime::now()), 0);
        Counters::add(&self.counters.bytes_written, bytes.len() as u64);
        self.evict(Some(&rel));
        Ok(())
    }

    /// Removes the entry of `key`, when there.
    pub fn remove(&self, key: &[u8]) {
        let rel = Disk::rel(key);
        if lock(&self.index).remove(&rel).is_some() {
            let _ = std::fs::remove_file(self.dir.join(rel));
        }
    }

    /// Removes the entries not used since `now` less the kind's
    /// [`Kind::unused`] (none for a kind without one): how many went.
    pub fn prune(&self, now: SystemTime) -> usize {
        let Some(unused) = self.kind.unused else {
            return 0;
        };
        let before = now.checked_sub(unused).unwrap_or(SystemTime::UNIX_EPOCH);
        self.prune_before(before)
    }

    /// Removes the entries not used since `before`: how many went.
    pub fn prune_before(&self, before: SystemTime) -> usize {
        let gone = lock(&self.index).older_than(nanos(before));
        for (rel, _) in &gone {
            Counters::add(&self.counters.evictions, 1);
            let _ = std::fs::remove_file(self.dir.join(rel));
        }
        gone.len()
    }

    /// The kind's age limit, if it has one.
    pub fn unused(&self) -> Option<Duration> {
        self.kind.unused
    }

    fn evict(&self, keep: Option<&Path>) {
        let gone = {
            let mut index = lock(&self.index);
            if index.bytes() <= self.limit {
                return;
            }
            index.evict(crate::eviction_target(self.limit), |rel| {
                Some(rel.as_path()) == keep
            })
        };
        for (rel, _) in gone {
            Counters::add(&self.counters.evictions, 1);
            let _ = std::fs::remove_file(self.dir.join(rel));
        }
    }
}

impl Store for Disk {
    fn usage(&self) -> Vec<Usage> {
        let (bytes, entries) = Disk::usage(self);
        vec![Usage {
            scope: self.scope.clone(),
            kind: self.kind,
            tier: Tier::Disk,
            entries: entries as u64,
            bytes,
            pinned: 0,
            budget: Some(self.limit),
        }]
    }

    fn clear(&self, kind: Option<&str>) -> u64 {
        if kind.is_some_and(|k| k != self.kind.id) {
            return 0;
        }
        let gone = lock(&self.index).clear(|_| false);
        let mut freed = 0;
        for (rel, bytes) in gone {
            Counters::add(&self.counters.cleared, 1);
            let _ = std::fs::remove_file(self.dir.join(rel));
            freed += bytes;
        }
        freed
    }
}
