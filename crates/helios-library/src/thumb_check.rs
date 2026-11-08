//! The shared thumbnail cache's re-check (`docs/design/image-map.md`,
//! section 7, "Re-check"): a thumbnail is trusted only while the file it
//! was made from still matches its sighting. In the background, by
//! [`Priority`]: the files shown now, then those shown recently, then the
//! Library's files (verified longest ago first), each once a pass
//! ([`PASS_EVERY`]). For each file with a cache entry it compares, in
//! steps that stop at the first answer:
//!
//! 1. size and modification time (a `stat`): unchanged, nothing read;
//! 2. the partial hash: a different one, or a different size, is a
//!    changed file;
//! 3. the full hash: the same content is a file only touched (its sighting
//!    takes the new time); a provisional entry ([`Key::provisional`]) is
//!    renamed to its full key when the hash is an image's.
//!
//! A changed file's sighting is updated (size, time, partial hash) and its
//! image cleared; its new thumbnail is made from the file under the
//! provisional key; the old entry is left to the cache's LRU (another
//! sighting may still use it). It waits, as the map's job does, while the
//! user pauses the map, while an import, an export, a backup or a render
//! runs, and on battery ([`map_job::waits`]).
//!
//! [`Check::next_check`] and [`Check::check`] do one file at a time and are
//! what the tests drive; [`Check::run`] loops them on a thread of its own.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use helios_storage::Volume;
use helios_storage::exclude::Checker;
use helios_thumbs::{Key, SMALL, ThumbCache};
use helios_vault::{ImageId, Presence, Sighting, ToRead};

use crate::map_job::{self, Feed, Files, Load, Map, Never, Power, place_of};

/// The shortest time between two passes over the Library, in seconds.
pub const PASS_EVERY: u64 = 3600;
/// How many Library sightings are asked for at once.
const BATCH: usize = 64;

/// Where a file to re-check comes from, in the re-check's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Shown,
    Recent,
    Library,
}

/// What a re-check found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Same size and time: nothing read.
    Unchanged,
    /// Its time changed, its content did not: the sighting takes the time.
    Touched,
    /// A provisional entry whose file the full hash links to an image:
    /// renamed to the image's key.
    Linked,
    /// Its content changed: its thumbnail was made again.
    Remade,
    /// Its content changed and it holds no readable preview: the sighting
    /// is updated, no thumbnail made.
    NoPreview,
    /// Not checked: no cache entry, an excluded place, or the file is not
    /// there (presence is the scans' business).
    Skipped,
}

/// The cache's entry of a sighting: its image's key once its full hash
/// links it to one, else its provisional key once its partial hash is
/// read; none before.
pub fn key_of(s: &Sighting) -> Option<Key> {
    match (&s.image, &s.partial) {
        (Some(id), _) => Some(Key::preview(id.hex())),
        (None, Some(partial)) => Some(Key::provisional(s.size, &partial.to_string())),
        (None, None) => None,
    }
}

/// What the re-check needs of the thumbnail cache.
pub trait Thumbs: Send {
    fn has(&self, key: &Key) -> bool;
    /// Makes the thumbnail of the file at `path` under `key`; `false` when
    /// the file holds no readable preview.
    ///
    /// # Errors
    /// The file cannot be read or the entry written.
    fn remake(&self, path: &Path, key: &Key) -> Result<bool, String>;
    fn rename(&self, from: &Key, to: &Key);
}

impl Thumbs for Arc<ThumbCache> {
    fn has(&self, key: &Key) -> bool {
        self.contains(key, SMALL)
    }

    fn remake(&self, path: &Path, key: &Key) -> Result<bool, String> {
        match helios_thumbs::read::read(path, SMALL)? {
            Some(thumb) => self.put(key, SMALL, &thumb).map(|()| true),
            None => Ok(false),
        }
    }

    fn rename(&self, from: &Key, to: &Key) {
        ThumbCache::rename(self, from, to);
    }
}

/// What [`Check::next_check`] found.
#[derive(Clone, Debug, PartialEq)]
pub enum Next {
    /// Waits, saying why.
    Paused(String),
    /// A file to re-check.
    Check(Priority, Box<Sighting>),
    /// Nothing to do until the next pass or something new on screen.
    Idle,
}

/// The re-check: [`Check::next_check`] then [`Check::check`], or
/// [`Check::run`].
pub struct Check<M: Map> {
    map: Arc<M>,
    files: Box<dyn Files>,
    thumbs: Box<dyn Thumbs>,
    checker: Checker,
    volumes: Vec<Volume>,
    load: Box<dyn Load>,
    power: Box<dyn Power>,
    feed: Feed,
    clock: Box<dyn Fn() -> u64 + Send>,
    /// When this pass began.
    pass: Option<u64>,
    /// The feed's screen changes seen.
    shown_changes: u64,
    /// The paths on screen still to re-check, in their order.
    shown: VecDeque<PathBuf>,
    /// The paths re-checked this pass.
    done: HashSet<PathBuf>,
    library: VecDeque<Sighting>,
}

impl<M: Map> Check<M> {
    /// A re-check of `thumbs` against `map`, reading through `files`,
    /// never where `checker` excludes; `volumes` places the paths `feed`
    /// says are on screen.
    pub fn new(
        map: Arc<M>,
        files: Box<dyn Files>,
        thumbs: Box<dyn Thumbs>,
        checker: Checker,
        volumes: Vec<Volume>,
        feed: Feed,
    ) -> Self {
        Self {
            map,
            files,
            thumbs,
            checker,
            volumes,
            load: Box::new(Never),
            power: Box::new(Never),
            feed,
            clock: Box::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs())
            }),
            pass: None,
            shown_changes: 0,
            shown: VecDeque::new(),
            done: HashSet::new(),
            library: VecDeque::new(),
        }
    }

    /// Gives way to the jobs `load` says run.
    #[must_use]
    pub fn load(mut self, load: Box<dyn Load>) -> Self {
        self.load = load;
        self
    }

    /// Waits on battery, as `power` says.
    #[must_use]
    pub fn power(mut self, power: Box<dyn Power>) -> Self {
        self.power = power;
        self
    }

    /// The clock, in seconds since the epoch.
    #[must_use]
    pub fn clock(mut self, clock: impl Fn() -> u64 + Send + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// The next file to re-check, in the re-check's order.
    pub fn next_check(&mut self) -> Next {
        if let Some(why) = map_job::waits(&self.feed, self.load.as_mut(), self.power.as_mut()) {
            return Next::Paused(why);
        }
        let now = (self.clock)();
        if self
            .pass
            .is_none_or(|at| now.saturating_sub(at) >= PASS_EVERY)
        {
            self.pass = Some(now);
            self.done.clear();
            self.library.clear();
        }
        // What came on screen is re-checked again, even if it was this
        // pass: a stat is all it costs when unchanged.
        let (changes, shown) = self.feed.shown();
        if changes != self.shown_changes {
            self.shown_changes = changes;
            self.shown = shown.into();
            for p in &self.shown {
                self.done.remove(p);
            }
        }
        while let Some(path) = self.shown.pop_front() {
            if let Some(s) = self.sighting(path) {
                return Next::Check(Priority::Shown, Box::new(s));
            }
        }
        for path in self.feed.recent() {
            if let Some(s) = self.sighting(path) {
                return Next::Check(Priority::Recent, Box::new(s));
            }
        }
        if self.library.is_empty() {
            let limit = self.done.len() + BATCH;
            match self.map.to_read(ToRead::Library, None, limit) {
                Ok(found) => self.library.extend(
                    found
                        .into_iter()
                        .filter(|s| !self.done.contains(&s.path))
                        .take(BATCH),
                ),
                Err(e) => log::warn!("thumbnail re-check: {e}"),
            }
        }
        while let Some(s) = self.library.pop_front() {
            if self.done.insert(s.path.clone()) {
                return Next::Check(Priority::Library, Box::new(s));
            }
        }
        Next::Idle
    }

    /// The sighting at `path`, if not re-checked this pass (marked so).
    fn sighting(&mut self, path: PathBuf) -> Option<Sighting> {
        if !self.done.insert(path.clone()) {
            return None;
        }
        let place = place_of(&self.volumes, &path);
        self.map
            .sighting_at(
                place.as_ref().map(|(v, _)| v),
                place.as_ref().map(|(_, r)| r.as_path()),
                &path,
            )
            .ok()
            .flatten()
            .filter(|s| s.presence == Presence::Present)
    }

    /// Re-checks a file against its sighting, in the module's steps.
    ///
    /// # Errors
    /// The file cannot be read, the sighting recorded or the thumbnail
    /// made.
    pub fn check(&mut self, mut s: Sighting) -> Result<Verdict, String> {
        let Some(key) = key_of(&s).filter(|k| self.thumbs.has(k)) else {
            return Ok(Verdict::Skipped);
        };
        let place = s.volume.as_ref().zip(s.rel_path.as_deref());
        if self.checker.excluded(&s.path, place).is_some() {
            return Ok(Verdict::Skipped);
        }
        let Some((size, modified)) = self.files.stat(&s.path) else {
            return Ok(Verdict::Skipped);
        };
        if size == s.size && modified == s.modified {
            return Ok(Verdict::Unchanged);
        }
        let now = (self.clock)();
        let partial = self.files.partial(&s.path, size)?;
        if size == s.size && s.partial == Some(partial) {
            let full = ImageId(self.files.full(&s.path)?);
            let same = match s.image {
                Some(id) => id == full,
                None => true,
            };
            if same {
                let mut verdict = Verdict::Touched;
                if s.image.is_none() && self.map.image_known(&full) {
                    s.image = Some(full);
                    self.thumbs.rename(&key, &Key::preview(full.hex()));
                    verdict = Verdict::Linked;
                }
                s.modified = modified;
                s.last_seen = now;
                if s.image.is_some() {
                    s.last_verified = Some(now);
                }
                self.map.record(&s)?;
                return Ok(verdict);
            }
        }
        // Changed: the old entry stays for whoever else has that content.
        s.size = size;
        s.modified = modified;
        s.partial = Some(partial);
        s.image = None;
        s.last_verified = None;
        s.last_seen = now;
        self.map.record(&s)?;
        let new = key_of(&s).expect("a partial hash makes a key");
        Ok(if self.thumbs.remake(&s.path, &new)? {
            Verdict::Remade
        } else {
            Verdict::NoPreview
        })
    }

    /// Runs until `stop`, quietly (no jobs-centre entry: a pass over
    /// unchanged files is stats only); what it remade goes to the log.
    pub fn run(mut self, stop: &AtomicBool) {
        let mut remade = 0u64;
        while !stop.load(Ordering::SeqCst) {
            match self.next_check() {
                Next::Paused(_) => self.wait(stop, Duration::from_secs(5)),
                Next::Idle => {
                    if remade > 0 {
                        log::info!("thumbnail re-check: {remade} thumbnails made again");
                        remade = 0;
                    }
                    self.wait(stop, Duration::from_mins(1));
                }
                Next::Check(_, s) => {
                    let name = s.path.display().to_string();
                    match self.check(*s) {
                        Ok(Verdict::Remade) => remade += 1,
                        Ok(_) => {}
                        Err(e) => log::warn!("thumbnail re-check: {name}: {e}"),
                    }
                }
            }
        }
    }

    /// Sleeps up to `most`, ending early when the screen changes or on
    /// `stop`.
    fn wait(&self, stop: &AtomicBool, most: Duration) {
        let start = Instant::now();
        while start.elapsed() < most && !stop.load(Ordering::SeqCst) {
            if self.feed.shown().0 != self.shown_changes {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;

    use helios_core::ContentHash;
    use helios_storage::exclude::{Blacklist, Os, Places};
    use helios_vault::storage::VolumeId;
    use helios_vault::{Image, MemoryStore, Store, Vault};

    use super::*;

    /// A fake file: size, time, partial hash, full hash.
    type File = (u64, u64, ContentHash, ContentHash);

    /// Files on a fake disk; each read logged.
    #[derive(Clone, Default)]
    struct Fake {
        files: Arc<Mutex<HashMap<PathBuf, File>>>,
        reads: Arc<Mutex<Vec<String>>>,
    }

    impl Fake {
        fn log(&self, what: &str, path: &Path) {
            self.reads
                .lock()
                .unwrap()
                .push(format!("{what} {}", path.display()));
        }
        fn reads(&self) -> Vec<String> {
            std::mem::take(&mut *self.reads.lock().unwrap())
        }
    }

    impl Files for Fake {
        fn stat(&self, path: &Path) -> Option<(u64, u64)> {
            self.log("stat", path);
            self.files.lock().unwrap().get(path).map(|f| (f.0, f.1))
        }
        fn partial(&self, path: &Path, _: u64) -> Result<ContentHash, String> {
            self.log("partial", path);
            Ok(self.files.lock().unwrap()[path].2)
        }
        fn full(&self, path: &Path) -> Result<ContentHash, String> {
            self.log("full", path);
            Ok(self.files.lock().unwrap()[path].3)
        }
    }

    /// A cache of keys; what is made and renamed is logged.
    #[derive(Clone, Default)]
    struct Cache {
        keys: Arc<Mutex<HashSet<Key>>>,
        made: Arc<Mutex<Vec<String>>>,
    }

    impl Thumbs for Cache {
        fn has(&self, key: &Key) -> bool {
            self.keys.lock().unwrap().contains(key)
        }
        fn remake(&self, path: &Path, key: &Key) -> Result<bool, String> {
            self.keys.lock().unwrap().insert(key.clone());
            self.made
                .lock()
                .unwrap()
                .push(format!("make {} {}", path.display(), key.image));
            Ok(true)
        }
        fn rename(&self, from: &Key, to: &Key) {
            let mut keys = self.keys.lock().unwrap();
            if keys.remove(from) {
                keys.insert(to.clone());
            }
            self.made
                .lock()
                .unwrap()
                .push(format!("rename {} {}", from.image, to.image));
        }
    }

    struct Busy(Arc<Mutex<Option<String>>>);
    impl Load for Busy {
        fn busy(&mut self) -> Option<String> {
            self.0.lock().unwrap().clone()
        }
    }

    struct Battery(Arc<AtomicBool>);
    impl Power for Battery {
        fn on_battery(&mut self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }

    const MOUNT: &str = "/media/disk";

    fn h(n: u8) -> ContentHash {
        ContentHash([n; 32])
    }

    struct World {
        vault: Arc<Vault<MemoryStore>>,
        fake: Fake,
        cache: Cache,
        feed: Feed,
        paths: BTreeMap<&'static str, PathBuf>,
    }

    /// Library files `a`, `b`, `c` (images 1, 2, 3) and a file `p` seen
    /// in the import window (partial read, no image), each with a cache
    /// entry.
    fn world() -> World {
        let mut store = MemoryStore::default();
        let fake = Fake::default();
        let cache = Cache::default();
        let mut paths = BTreeMap::new();
        for (name, n) in [("a", 1u8), ("b", 2), ("c", 3), ("p", 4)] {
            let rel = format!("{name}.jpg");
            let path = Path::new(MOUNT).join(&rel);
            let mut s = Sighting::seen(path.clone(), 100 + u64::from(n), 5, 1, "import");
            s.volume = Some(VolumeId("uuid:disk".into()));
            s.rel_path = Some(rel.into());
            s.partial = Some(h(n + 10));
            if name != "p" {
                let image = Image {
                    id: ImageId(h(n)),
                    size: s.size,
                    partial: h(n + 10),
                };
                store.add_to_library(&image).unwrap();
                s.image = Some(image.id);
            }
            store.put_sighting(&s).unwrap();
            fake.files
                .lock()
                .unwrap()
                .insert(path.clone(), (s.size, 5, h(n + 10), h(n)));
            cache.keys.lock().unwrap().insert(key_of(&s).unwrap());
            paths.insert(name, path);
        }
        World {
            vault: Arc::new(Vault::new(store)),
            fake,
            cache,
            feed: Feed::default(),
            paths,
        }
    }

    fn checker() -> Checker {
        Checker::new(Blacklist::new(
            Os::Linux,
            &Places::default(),
            true,
            Vec::new(),
        ))
    }

    fn check(w: &World) -> Check<Vault<MemoryStore>> {
        let mut v: Volume = serde_json::from_value(serde_json::json!({
            "id": "uuid:disk", "identified_by": "uuid", "name": "disk", "kind": "external",
        }))
        .unwrap();
        v.mount = Some(MOUNT.into());
        Check::new(
            w.vault.clone(),
            Box::new(w.fake.clone()),
            Box::new(w.cache.clone()),
            checker(),
            vec![v],
            w.feed.clone(),
        )
        .clock(|| 1_000_000)
    }

    /// Drives the re-check until idle; returns what it checked.
    fn drain(c: &mut Check<Vault<MemoryStore>>) -> Vec<(Priority, String, Verdict)> {
        let mut out = Vec::new();
        loop {
            match c.next_check() {
                Next::Check(p, s) => {
                    let name = s.path.file_name().unwrap().to_string_lossy().to_string();
                    let v = c.check(*s).unwrap();
                    out.push((p, name, v));
                }
                Next::Idle => return out,
                Next::Paused(why) => panic!("paused: {why}"),
            }
        }
    }

    fn sighting(w: &World, name: &str) -> Sighting {
        let path = &w.paths[name];
        let rel = PathBuf::from(format!("{name}.jpg"));
        w.vault
            .sighting_at(Some(&VolumeId("uuid:disk".into())), Some(&rel), path)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn unchanged_files_cost_a_stat_and_no_read() {
        let w = world();
        let mut c = check(&w);
        let out = drain(&mut c);
        assert_eq!(out.len(), 3, "the Library's three files: {out:?}");
        assert!(out.iter().all(|(_, _, v)| *v == Verdict::Unchanged));
        let reads = w.fake.reads();
        assert_eq!(reads.len(), 3, "{reads:?}");
        assert!(reads.iter().all(|r| r.starts_with("stat ")), "{reads:?}");
        assert!(w.cache.made.lock().unwrap().is_empty());
        // Nothing again this pass.
        assert_eq!(drain(&mut c), []);
        assert!(w.fake.reads().is_empty());
    }

    #[test]
    fn a_changed_original_gets_a_new_thumbnail() {
        let w = world();
        w.fake
            .files
            .lock()
            .unwrap()
            .insert(w.paths["b"].clone(), (500, 9, h(50), h(51)));
        let mut c = check(&w);
        let out = drain(&mut c);
        let b: Vec<_> = out.iter().filter(|(_, n, _)| n == "b.jpg").collect();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].2, Verdict::Remade);
        let reads = w.fake.reads();
        assert!(reads.contains(&"partial /media/disk/b.jpg".to_string()));
        assert!(
            !reads.iter().any(|r| r.starts_with("full")),
            "a different partial hash is enough: {reads:?}"
        );
        let s = sighting(&w, "b");
        assert_eq!((s.size, s.modified, s.partial), (500, 9, Some(h(50))));
        assert_eq!(s.image, None, "the sighting no longer shows image 2");
        let new = Key::provisional(500, &h(50).to_string());
        assert_eq!(
            *w.cache.made.lock().unwrap(),
            [format!("make /media/disk/b.jpg {}", new.image)]
        );
        assert!(
            w.cache.has(&Key::preview(h(2).to_string())),
            "left to the LRU"
        );
    }

    #[test]
    fn a_touched_file_is_read_whole_and_kept() {
        let w = world();
        let mut files = w.fake.files.lock().unwrap();
        files.get_mut(&w.paths["a"]).unwrap().1 = 77;
        // `p`'s content is image 3's: its full hash links it.
        let p = files.get_mut(&w.paths["p"]).unwrap();
        p.1 = 78;
        p.3 = h(3);
        drop(files);
        let mut c = check(&w);
        w.feed.on_screen(vec![w.paths["p"].clone()]);
        let out = drain(&mut c);
        assert!(out.contains(&(Priority::Shown, "p.jpg".into(), Verdict::Linked)));
        assert!(out.contains(&(Priority::Library, "a.jpg".into(), Verdict::Touched)));
        let a = sighting(&w, "a");
        assert_eq!((a.modified, a.image), (77, Some(ImageId(h(1)))));
        assert_eq!(a.last_verified, Some(1_000_000));
        let p = sighting(&w, "p");
        assert_eq!(p.image, Some(ImageId(h(3))));
        let provisional = Key::provisional(104, &h(14).to_string());
        assert_eq!(
            *w.cache.made.lock().unwrap(),
            [format!("rename {} {}", provisional.image, h(3))],
            "no thumbnail made"
        );
    }

    #[test]
    fn shown_first_then_recent_then_the_library() {
        let w = world();
        let mut c = check(&w);
        w.feed.on_screen(vec![w.paths["c"].clone()]);
        w.feed.on_screen(vec![w.paths["p"].clone()]);
        let order: Vec<(Priority, String)> =
            drain(&mut c).into_iter().map(|(p, n, _)| (p, n)).collect();
        assert_eq!(
            order[..2],
            [
                (Priority::Shown, "p.jpg".into()),
                (Priority::Recent, "c.jpg".into()),
            ]
        );
        let mut library: Vec<_> = order[2..].to_vec();
        library.sort();
        assert_eq!(
            library,
            [
                (Priority::Library, "a.jpg".into()),
                (Priority::Library, "b.jpg".into()),
            ],
            "then the Library's other files, once"
        );
        // What comes on screen again is checked again, this pass.
        w.feed.on_screen(vec![w.paths["a"].clone()]);
        let again: Vec<_> = drain(&mut c).into_iter().map(|(p, n, _)| (p, n)).collect();
        assert_eq!(again, [(Priority::Shown, "a.jpg".into())]);
        // A new pass an hour later goes over the Library again.
        let mut c = c.clock(|| 1_000_000 + PASS_EVERY);
        assert_eq!(drain(&mut c).len(), 4, "the recent p and c, then a and b");
    }

    #[test]
    fn it_backs_off_under_load_and_on_battery() {
        let w = world();
        let busy = Arc::new(Mutex::new(Some("render".to_string())));
        let battery = Arc::new(AtomicBool::new(false));
        let mut c = check(&w)
            .load(Box::new(Busy(busy.clone())))
            .power(Box::new(Battery(battery.clone())));
        w.feed.on_screen(vec![w.paths["a"].clone()]);
        assert_eq!(
            c.next_check(),
            Next::Paused("Waiting for the render job to end".into())
        );
        *busy.lock().unwrap() = None;
        battery.store(true, Ordering::SeqCst);
        assert_eq!(
            c.next_check(),
            Next::Paused("Waiting for mains power".into())
        );
        battery.store(false, Ordering::SeqCst);
        w.feed.pause(true);
        assert_eq!(c.next_check(), Next::Paused("Paused".into()));
        w.feed.pause(false);
        assert!(matches!(c.next_check(), Next::Check(Priority::Shown, _)));
        assert!(w.fake.reads().is_empty(), "nothing read while waiting");
    }

    #[test]
    fn files_without_an_entry_are_not_checked() {
        let w = world();
        w.cache.keys.lock().unwrap().clear();
        let mut c = check(&w);
        let out = drain(&mut c);
        assert!(
            out.iter().all(|(_, _, v)| *v == Verdict::Skipped),
            "{out:?}"
        );
        assert!(w.fake.reads().is_empty(), "not even a stat");
    }

    /// The real cache and disk: a file rewritten gets a new entry.
    #[test]
    fn on_disk_a_rewritten_file_is_made_again() {
        let root = std::env::temp_dir().join(format!("helios-thumb-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let files = root.join("files");
        std::fs::create_dir_all(&files).unwrap();
        let cache = Arc::new(ThumbCache::open(&root.join("cache"), 1 << 30).unwrap());
        let jpeg = |shade: u8, w: u32, h: u32| -> Vec<u8> {
            let thumb = helios_thumbs::Rgb8 {
                width: w,
                height: h,
                pixels: vec![shade; (w * h * 3) as usize],
            };
            let scratch =
                ThumbCache::open(&root.join(format!("scratch-{shade}")), 1 << 30).unwrap();
            let k = Key::preview("ff".repeat(32));
            scratch.put(&k, SMALL, &thumb).unwrap();
            std::fs::read(scratch.dir().join(scratch.file_of(&k, SMALL))).unwrap()
        };
        let path = files.join("x.jpg");
        std::fs::write(&path, jpeg(40, 300, 200)).unwrap();
        let (size, modified) = map_job::Disk.stat(&path).unwrap();
        let mut s = Sighting::seen(path.clone(), size, modified, 1, "import");
        s.partial = Some(map_job::Disk.partial(&path, size).unwrap());
        let mut store = MemoryStore::default();
        store.put_sighting(&s).unwrap();
        let vault = Arc::new(Vault::new(store));
        let old = key_of(&s).unwrap();
        cache
            .put(
                &old,
                SMALL,
                &helios_thumbs::read::read(&path, SMALL).unwrap().unwrap(),
            )
            .unwrap();
        let feed = Feed::default();
        // The default blacklist excludes the temporary folder: off here.
        let mut c = Check::new(
            vault.clone(),
            Box::new(map_job::Disk),
            Box::new(cache.clone()),
            Checker::new(Blacklist::new(
                Os::Linux,
                &Places::default(),
                false,
                Vec::new(),
            )),
            Vec::new(),
            feed.clone(),
        );
        feed.on_screen(vec![path.clone()]);
        let Next::Check(_, found) = c.next_check() else {
            panic!("nothing to check")
        };
        assert_eq!(c.check(*found.clone()).unwrap(), Verdict::Unchanged);
        // Rewritten, larger: the size alone says it changed.
        std::fs::write(&path, jpeg(220, 640, 480)).unwrap();
        assert_eq!(c.check(*found).unwrap(), Verdict::Remade);
        let now = vault.sighting_at(None, None, &path).unwrap().unwrap();
        let new = key_of(&now).unwrap();
        assert_ne!(new, old);
        let thumb = cache.get(&new, SMALL).unwrap();
        assert!(
            thumb.pixels[0] > 150,
            "the new picture: {}",
            thumb.pixels[0]
        );
        assert!(cache.contains(&old, SMALL), "the old one left to the LRU");
        let _ = std::fs::remove_dir_all(&root);
    }
}
