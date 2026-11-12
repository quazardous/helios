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
//!
//! Two kinds of use: an entry put or got with [`Disk::put`] and
//! [`Disk::get`] is *used* (a glance shown); one put with
//! [`Disk::put_cold`] is only *prefetched* (`.cold` instead of `.entry`)
//! and goes before every used one, oldest first, until
//! [`Disk::warm`] (it was shown) makes it used. [`Disk::read`] reads an
//! entry without using it.

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

/// A prefetched entry's file extension: never used yet.
const COLD: &str = "cold";

/// How much later an entry used goes than a prefetched one put at the same
/// time, in nanoseconds of last use: about 146 years, so every prefetched
/// entry goes before every used one, and the used ones by last use.
const USED: u64 = 1 << 62;

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
    /// The entries by file (relative to `dir`, without its extension),
    /// each last used at a time in nanoseconds since the Unix epoch, the
    /// used ones [`USED`] later: prefetched first, then least recently
    /// used first.
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
                let bonus = match path.extension().and_then(|e| e.to_str()) {
                    Some(EXTENSION) => USED,
                    Some(COLD) => 0,
                    _ => {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                };
                let Ok(meta) = file.metadata() else { continue };
                let used = meta.modified().map_or(0, nanos);
                let rel = path.strip_prefix(&dir).unwrap_or(&path).with_extension("");
                if entries.bonus(&rel).is_some_and(|b| b >= bonus) {
                    // Used and prefetched both (a crash between the two):
                    // the used one is kept.
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                if entries.contains(&rel) {
                    let _ = std::fs::remove_file(dir.join(&rel).with_extension(COLD));
                }
                entries.insert(rel, meta.len(), used, bonus);
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

    /// The bytes and count of the entries used (shown), then of those
    /// only prefetched.
    pub fn split(&self) -> ((u64, usize), (u64, usize)) {
        let index = lock(&self.index);
        let used = index
            .iter()
            .filter(|(rel, _)| index.bonus(rel) == Some(USED))
            .count();
        let bytes = index.bytes_of_bonus(USED);
        ((bytes, used), (index.bytes() - bytes, index.len() - used))
    }

    /// The bytes of the entries used (shown): what a prefetch must never
    /// evict.
    pub fn used_bytes(&self) -> u64 {
        lock(&self.index).bytes_of_bonus(USED)
    }

    /// The entry of `key`, relative to its folder, without its extension.
    fn rel(key: &[u8]) -> PathBuf {
        let hex = blake3::hash(key).to_hex();
        PathBuf::from(&hex[..2]).join(&hex[2..32])
    }

    /// The file of an entry: `.entry` when used, `.cold` when only
    /// prefetched.
    fn file(&self, rel: &Path, used: bool) -> PathBuf {
        self.dir
            .join(rel)
            .with_extension(if used { EXTENSION } else { COLD })
    }

    /// The file of the entry of `key` (there or not: then as used).
    pub fn file_of(&self, key: &[u8]) -> PathBuf {
        let rel = Disk::rel(key);
        let used = lock(&self.index).bonus(&rel).is_none_or(|b| b == USED);
        self.file(&rel, used)
    }

    /// Whether the entry of `key` is there and was used (not only
    /// prefetched).
    pub fn is_used(&self, key: &[u8]) -> bool {
        lock(&self.index).bonus(&Disk::rel(key)) == Some(USED)
    }

    /// Whether an entry of `key` is there; not a use.
    pub fn contains(&self, key: &[u8]) -> bool {
        lock(&self.index).contains(&Disk::rel(key))
    }

    /// The bytes of the entry of `key`, when there: marked used now, so
    /// it goes last, and its use survives a restart.
    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let bytes = self.read(key)?;
        self.warm(key);
        Some(bytes)
    }

    /// The bytes of the entry of `key`, when there, its use unchanged (a
    /// prefetch reading it back): counted as read.
    pub fn read(&self, key: &[u8]) -> Option<Vec<u8>> {
        let rel = Disk::rel(key);
        let used = lock(&self.index).bonus(&rel)? == USED;
        let Ok(bytes) = std::fs::read(self.file(&rel, used)) else {
            // Gone (another process, a person): forgotten.
            lock(&self.index).remove(&rel);
            return None;
        };
        Counters::add(&self.counters.bytes_read, bytes.len() as u64);
        Some(bytes)
    }

    /// Marks the entry of `key` used now (a prefetched one shown becomes
    /// used): it goes after every prefetched entry, and its use survives
    /// a restart. False when it is not there.
    pub fn warm(&self, key: &[u8]) -> bool {
        let rel = Disk::rel(key);
        let now = SystemTime::now();
        let was = {
            let mut index = lock(&self.index);
            let Some(bonus) = index.bonus(&rel) else {
                return false;
            };
            index.touch_with(&rel, nanos(now), USED);
            bonus == USED
        };
        let path = self.file(&rel, true);
        if !was && std::fs::rename(self.file(&rel, false), &path).is_err() {
            lock(&self.index).remove(&rel);
            return false;
        }
        if let Ok(f) = std::fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(now);
        }
        true
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
        self.write(key, bytes, true)
    }

    /// Stores `bytes` as the entry of `key` (replacing it), only
    /// prefetched: it goes before every used entry until
    /// [`Disk::warm`]. An entry used before stays used.
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn put_cold(&self, key: &[u8], bytes: &[u8]) -> std::io::Result<()> {
        self.write(key, bytes, self.is_used(key))
    }

    fn write(&self, key: &[u8], bytes: &[u8], used: bool) -> std::io::Result<()> {
        let rel = Disk::rel(key);
        let path = self.file(&rel, used);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("{}.part", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
        let bonus = if used { USED } else { 0 };
        let old = lock(&self.index).bonus(&rel);
        if old.is_some_and(|b| b != bonus) {
            // The other kind's file of the same entry.
            let _ = std::fs::remove_file(self.file(&rel, !used));
        }
        lock(&self.index).insert(
            rel.clone(),
            bytes.len() as u64,
            nanos(SystemTime::now()),
            bonus,
        );
        Counters::add(&self.counters.bytes_written, bytes.len() as u64);
        self.evict(Some(&rel));
        Ok(())
    }

    /// Removes the entry of `key`, when there.
    pub fn remove(&self, key: &[u8]) {
        let rel = Disk::rel(key);
        let bonus = lock(&self.index).bonus(&rel);
        self.forget(&rel, bonus);
    }

    /// Takes `rel` out of the index and deletes its file, given its bonus
    /// as it was (`None`: it was not there).
    fn forget(&self, rel: &Path, bonus: Option<u64>) {
        let Some(bonus) = bonus else { return };
        if lock(&self.index).remove(&rel.to_path_buf()).is_some() {
            let _ = std::fs::remove_file(self.file(rel, bonus == USED));
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
        let gone = {
            let mut index = lock(&self.index);
            let gone = index.older_than(nanos(before));
            self.delete(&gone);
            gone
        };
        for _ in &gone {
            Counters::add(&self.counters.evictions, 1);
        }
        gone.len()
    }

    /// Deletes the files of entries just taken out of the index (either
    /// kind: the one there is removed).
    fn delete(&self, gone: &[(PathBuf, u64)]) {
        for (rel, _) in gone {
            if std::fs::remove_file(self.file(rel, true)).is_err() {
                let _ = std::fs::remove_file(self.file(rel, false));
            }
        }
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
        self.delete(&gone);
        for _ in gone {
            Counters::add(&self.counters.evictions, 1);
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
            shown: Some(self.used_bytes()),
        }]
    }

    fn clear(&self, kind: Option<&str>) -> u64 {
        if kind.is_some_and(|k| k != self.kind.id) {
            return 0;
        }
        let gone = lock(&self.index).clear(|_| false);
        self.delete(&gone);
        let mut freed = 0;
        for (_, bytes) in gone {
            Counters::add(&self.counters.cleared, 1);
            freed += bytes;
        }
        freed
    }
}
