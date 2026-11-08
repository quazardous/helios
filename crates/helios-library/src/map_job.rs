//! The image map's background job (`docs/design/image-map.md`, section 4,
//! "Background scans"): one low-priority job of kind `map` in the jobs
//! centre that reads the files the map knows, in this order:
//!
//! 1. [`Stage::Screen`]: what is on screen ([`Feed::on_screen`], fed by
//!    the interface with the paths in view): their partial hash;
//! 2. [`Stage::Library`]: Library sightings never verified by their full
//!    hash;
//! 3. [`Stage::Plugged`]: volumes plugged in just now ([`Feed::plugged`]):
//!    the partial hash of their files;
//! 4. [`Stage::Confirm`]: duplicates to confirm (same size and partial hash
//!    as an image): their full hash links them to it;
//! 5. [`Stage::Rest`]: every other file without a partial hash.
//!
//! It never reads a place the blacklist excludes, nor a file changed since
//! it was listed (the next listing records it again). It pauses while an
//! import, an export, a batch, a backup or a render runs ([`Load`]), on
//! battery ([`Power`]) and when the user pauses it; it can be cancelled.
//! At most once an hour it prunes the map ([`Vault::prune_sightings`],
//! `system.map.limit` and `system.map.keepMissing`).
//!
//! [`Job::next_read`] and [`Job::read`] do one file at a time and are what the
//! tests drive; [`Job::run`] loops them on a thread of its own, starting a
//! jobs-centre entry when there is work and finishing it once there is
//! none.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helios_core::ContentHash;
use helios_core::bus::{Bus, Envelope};
use helios_core::jobs::{self, Jobs, Progress, Start};
use helios_storage::Volume;
use helios_storage::exclude::Checker;
use helios_vault::storage::VolumeId;
use helios_vault::{ImageId, Sighting, Store, ToRead, Vault};

/// The jobs-centre kind of the map's job.
pub const KIND: &str = "map";
/// The settings it reads.
pub const LIMIT: &str = "system.map.limit";
pub const KEEP_MISSING: &str = "system.map.keepMissing";
/// The shortest time between two prunings, in seconds.
pub const PRUNE_EVERY: u64 = 3600;
/// The kinds of jobs it waits for.
pub const YIELDS_TO: &[&str] = &["import", "export", "batch", "backup", "render"];
/// How many sightings a stage asks the vault for at once.
const BATCH: usize = 64;

/// Where a file read comes from, in the job's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Screen,
    Library,
    Plugged,
    Confirm,
    Rest,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Library => "library",
            Self::Plugged => "plugged",
            Self::Confirm => "confirm",
            Self::Rest => "rest",
        }
    }
}

/// How many paths shown before the thumbnail re-check remembers.
pub const RECENT: usize = 512;

#[derive(Debug, Default)]
struct Fed {
    screen: Vec<PathBuf>,
    /// What is on screen now, kept for the thumbnail re-check
    /// ([`crate::thumb_check`]), and how many times it changed.
    shown: Vec<PathBuf>,
    shown_changes: u64,
    /// What was on screen before, last shown first, at most [`RECENT`].
    recent: VecDeque<PathBuf>,
    plugged: Vec<VolumeId>,
    paused: bool,
    /// Something new to do: the idle wait ends.
    woken: bool,
}

/// What the rest of Helios tells the job: the paths on screen, the volumes
/// just plugged in, the user's pause. Cheap to clone; clones share.
#[derive(Clone, Debug, Default)]
pub struct Feed(Arc<Mutex<Fed>>);

impl Feed {
    fn fed(&self) -> std::sync::MutexGuard<'_, Fed> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The files in view now (the import window's cells, a dialog, the
    /// Library), replacing what was: read first.
    pub fn on_screen(&self, paths: Vec<PathBuf>) {
        let mut fed = self.fed();
        let gone: Vec<PathBuf> = fed
            .shown
            .iter()
            .filter(|p| !paths.contains(p))
            .cloned()
            .collect();
        for p in gone.into_iter().rev() {
            fed.recent.retain(|r| r != &p);
            fed.recent.push_front(p);
        }
        fed.recent.truncate(RECENT);
        for p in &paths {
            fed.recent.retain(|r| r != p);
        }
        fed.shown.clone_from(&paths);
        fed.shown_changes += 1;
        fed.screen = paths;
        fed.woken = true;
    }

    /// What is on screen now and how many times that changed (the job's
    /// reads do not consume it).
    pub fn shown(&self) -> (u64, Vec<PathBuf>) {
        let fed = self.fed();
        (fed.shown_changes, fed.shown.clone())
    }

    /// What was on screen before, last shown first.
    pub fn recent(&self) -> Vec<PathBuf> {
        self.fed().recent.iter().cloned().collect()
    }

    /// A volume just plugged in: its files are read before the rest.
    pub fn plugged(&self, volume: VolumeId) {
        let mut fed = self.fed();
        if !fed.plugged.contains(&volume) {
            fed.plugged.push(volume);
        }
        fed.woken = true;
    }

    /// The user paused (`true`) or resumed it.
    pub fn pause(&self, paused: bool) {
        let mut fed = self.fed();
        fed.paused = paused;
        fed.woken = true;
    }

    pub fn paused(&self) -> bool {
        self.fed().paused
    }

    fn take_woken(&self) -> bool {
        std::mem::take(&mut self.fed().woken)
    }
}

/// How files are read: the disk, or a test's fake.
pub trait Files: Send {
    /// Its size and modification time (seconds), if it is there.
    fn stat(&self, path: &Path) -> Option<(u64, u64)>;
    fn partial(&self, path: &Path, size: u64) -> Result<ContentHash, String>;
    fn full(&self, path: &Path) -> Result<ContentHash, String>;
}

/// The files on disk.
#[derive(Clone, Copy, Debug, Default)]
pub struct Disk;

impl Files for Disk {
    fn stat(&self, path: &Path) -> Option<(u64, u64)> {
        let meta = std::fs::metadata(path).ok()?;
        let modified = meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        meta.is_file().then_some((meta.len(), modified))
    }

    fn partial(&self, path: &Path, size: u64) -> Result<ContentHash, String> {
        helios_vault::partial_hash(path, size)
    }

    fn full(&self, path: &Path) -> Result<ContentHash, String> {
        let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut hasher = blake3::Hasher::new();
        std::io::copy(&mut file, &mut hasher).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(ContentHash(*hasher.finalize().as_bytes()))
    }
}

/// Whether other work runs that the job gives way to: the kind running.
pub trait Load: Send {
    fn busy(&mut self) -> Option<String>;
}

/// The jobs running on a bus, from their `job.*` events: busy while one of
/// [`YIELDS_TO`]'s kinds runs (started after this was made).
pub struct BusLoad {
    events: Receiver<Envelope>,
    running: BTreeMap<String, String>,
}

impl BusLoad {
    pub fn new(bus: &Bus) -> Self {
        Self {
            events: bus.subscribe(),
            running: BTreeMap::new(),
        }
    }
}

impl Load for BusLoad {
    fn busy(&mut self) -> Option<String> {
        while let Ok(e) = self.events.try_recv() {
            let id = e.payload["id"].as_str().unwrap_or_default().to_string();
            match e.kind {
                jobs::STARTED => {
                    let kind = e.payload["kind"].as_str().unwrap_or_default();
                    if YIELDS_TO.contains(&kind) {
                        self.running.insert(id, kind.into());
                    }
                }
                jobs::FINISHED | jobs::FAILED | jobs::CANCELLED => {
                    self.running.remove(&id);
                }
                _ => {}
            }
        }
        self.running.values().next().cloned()
    }
}

/// Whether the machine runs on battery.
pub trait Power: Send {
    fn on_battery(&mut self) -> bool;
}

/// The platform's power source. Linux reads `/sys/class/power_supply` (a
/// mains supply online, or no battery discharging, is not on battery).
// TODO: Windows (`GetSystemPowerStatus`) and macOS
// (`IOPSCopyPowerSourcesInfo`): on mains until then.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemPower;

impl Power for SystemPower {
    fn on_battery(&mut self) -> bool {
        if cfg!(target_os = "linux") {
            on_battery_in(Path::new("/sys/class/power_supply"))
        } else {
            false
        }
    }
}

/// Whether the supplies under `dir` (`/sys/class/power_supply`) say the
/// machine is on battery: no mains online and a battery discharging.
pub fn on_battery_in(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let read = |p: PathBuf| std::fs::read_to_string(p).unwrap_or_default();
    let mut discharging = false;
    for e in entries.flatten() {
        let p = e.path();
        match read(p.join("type")).trim() {
            "Mains" if read(p.join("online")).trim() == "1" => return false,
            "Battery" if read(p.join("status")).trim() == "Discharging" => discharging = true,
            _ => {}
        }
    }
    discharging
}

/// Why background reads wait now, if they do: the user's pause, another
/// job, the battery.
pub fn waits(feed: &Feed, load: &mut dyn Load, power: &mut dyn Power) -> Option<String> {
    if feed.paused() {
        return Some("Paused".into());
    }
    if let Some(kind) = load.busy() {
        return Some(format!("Waiting for the {kind} job to end"));
    }
    if power.on_battery() {
        return Some("Waiting for mains power".into());
    }
    None
}

/// Where `path` is: its volume and its path inside it, if a volume holds it.
pub fn place_of(volumes: &[Volume], path: &Path) -> Option<(VolumeId, PathBuf)> {
    let v = helios_storage::registry::holding(volumes, path)?;
    let rel = path.strip_prefix(v.mount.as_ref()?).ok()?;
    Some((v.id.clone(), rel.to_path_buf()))
}

/// What the job needs of the vault.
pub trait Map: Send + Sync {
    fn to_read(
        &self,
        read: ToRead,
        volume: Option<&VolumeId>,
        limit: usize,
    ) -> Result<Vec<Sighting>, String>;
    fn sighting_at(
        &self,
        volume: Option<&VolumeId>,
        rel: Option<&Path>,
        path: &Path,
    ) -> Result<Option<Sighting>, String>;
    fn record(&self, sighting: &Sighting) -> Result<(), String>;
    fn image_known(&self, id: &ImageId) -> bool;
    /// Whether image `id` is in the Library.
    fn in_library(&self, id: &ImageId) -> bool;
    /// The content image `id`'s file holds now: its id, unless another
    /// application changed it ([`Vault::content_of`]).
    fn content_of(&self, id: &ImageId) -> ImageId;
    /// Records that the file of Library image `id` now holds `hash`, a
    /// change made outside Helios ([`Vault::changed_outside`]).
    ///
    /// # Errors
    /// The vault cannot be written.
    fn changed_outside(
        &self,
        id: &ImageId,
        hash: ImageId,
        size: u64,
        partial: &ContentHash,
        at: u64,
    ) -> Result<(), String>;
    fn prune(&self, limit: usize, missing_before: u64) -> Result<usize, String>;
}

impl<S: Store> Map for Vault<S> {
    fn to_read(
        &self,
        read: ToRead,
        volume: Option<&VolumeId>,
        limit: usize,
    ) -> Result<Vec<Sighting>, String> {
        self.sightings_to_read(read, volume, limit)
    }

    fn sighting_at(
        &self,
        volume: Option<&VolumeId>,
        rel: Option<&Path>,
        path: &Path,
    ) -> Result<Option<Sighting>, String> {
        Vault::sighting_at(self, volume, rel, path)
    }

    fn record(&self, sighting: &Sighting) -> Result<(), String> {
        self.record_sighting(sighting)
    }

    fn image_known(&self, id: &ImageId) -> bool {
        self.image(id).is_ok_and(|i| i.is_some())
    }

    fn in_library(&self, id: &ImageId) -> bool {
        Vault::in_library(self, id).unwrap_or(false)
    }

    fn content_of(&self, id: &ImageId) -> ImageId {
        Vault::content_of(self, id).unwrap_or(*id)
    }

    fn changed_outside(
        &self,
        id: &ImageId,
        hash: ImageId,
        size: u64,
        partial: &ContentHash,
        at: u64,
    ) -> Result<(), String> {
        Vault::changed_outside(self, id, hash, size, partial, at)
    }

    fn prune(&self, limit: usize, missing_before: u64) -> Result<usize, String> {
        self.prune_sightings(limit, missing_before)
    }
}

/// The pruning settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// `system.map.limit`: sightings not in the Library kept.
    pub limit: usize,
    /// `system.map.keepMissing`, in days.
    pub keep_missing_days: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            limit: 1_000_000,
            keep_missing_days: 30,
        }
    }
}

/// What [`Job::next_read`] found.
#[derive(Clone, Debug, PartialEq)]
pub enum Next {
    /// Waits, saying why.
    Paused(String),
    /// A file to read.
    Read(Stage, Box<Sighting>),
    /// Nothing to do.
    Idle,
}

/// The map's job: [`Job::next_read`] then [`Job::read`], or [`Job::run`].
pub struct Job<M: Map> {
    map: Arc<M>,
    files: Box<dyn Files>,
    checker: Checker,
    volumes: Vec<Volume>,
    load: Box<dyn Load>,
    power: Box<dyn Power>,
    feed: Feed,
    settings: Settings,
    clock: Box<dyn Fn() -> u64 + Send>,
    last_prune: Option<u64>,
    /// Places read this hour (or found unreadable), not asked again.
    done: HashSet<PathBuf>,
    queue: VecDeque<(Stage, Sighting)>,
}

impl<M: Map> Job<M> {
    /// A job over `map`, reading through `files`, never where `checker`
    /// excludes; `volumes` places the paths on screen.
    pub fn new(
        map: Arc<M>,
        files: Box<dyn Files>,
        checker: Checker,
        volumes: Vec<Volume>,
        feed: Feed,
        settings: Settings,
    ) -> Self {
        Self {
            map,
            files,
            checker,
            volumes,
            load: Box::new(Never),
            power: Box::new(Never),
            feed,
            settings,
            clock: Box::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs())
            }),
            last_prune: None,
            done: HashSet::new(),
            queue: VecDeque::new(),
        }
    }

    /// Gives way to the jobs `load` says run.
    #[must_use]
    pub fn load(mut self, load: Box<dyn Load>) -> Self {
        self.load = load;
        self
    }

    /// Pauses on battery, as `power` says.
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

    /// Why it waits now, if it does: the user's pause, another job, the
    /// battery.
    pub fn waits(&mut self) -> Option<String> {
        waits(&self.feed, self.load.as_mut(), self.power.as_mut())
    }

    /// Prunes the map if the last pruning is an hour old or more; returns
    /// how many sightings went, if it pruned.
    pub fn prune_if_due(&mut self) -> Option<usize> {
        let now = (self.clock)();
        if self
            .last_prune
            .is_some_and(|at| now.saturating_sub(at) < PRUNE_EVERY)
        {
            return None;
        }
        self.last_prune = Some(now);
        // A new hour: what could not be read is tried again.
        self.done.clear();
        let before = now.saturating_sub(self.settings.keep_missing_days * 86_400);
        match self.map.prune(self.settings.limit, before) {
            Ok(n) => Some(n),
            Err(e) => {
                log::warn!("map: pruning: {e}");
                Some(0)
            }
        }
    }

    /// The next file to read, in the job's order.
    pub fn next_read(&mut self) -> Next {
        if let Some(why) = self.waits() {
            return Next::Paused(why);
        }
        self.prune_if_due();
        if let Some(s) = self.on_screen() {
            return Next::Read(Stage::Screen, Box::new(s));
        }
        if self.queue.is_empty() {
            self.fill();
        }
        match self.queue.pop_front() {
            Some((stage, s)) => {
                self.done.insert(s.path.clone());
                Next::Read(stage, Box::new(s))
            }
            None => Next::Idle,
        }
    }

    /// The first path on screen with a sighting to read.
    fn on_screen(&mut self) -> Option<Sighting> {
        let paths = std::mem::take(&mut self.feed.fed().screen);
        let mut found = None;
        let mut left = Vec::new();
        for path in paths {
            if found.is_some() {
                left.push(path);
                continue;
            }
            if self.done.contains(&path) {
                continue;
            }
            let place = place_of(&self.volumes, &path);
            let s = self
                .map
                .sighting_at(
                    place.as_ref().map(|(v, _)| v),
                    place.as_ref().map(|(_, r)| r.as_path()),
                    &path,
                )
                .ok()
                .flatten();
            self.done.insert(path);
            if let Some(s) = s.filter(|s| s.partial.is_none()) {
                found = Some(s);
            }
        }
        if !left.is_empty() {
            let mut fed = self.feed.fed();
            if fed.screen.is_empty() {
                fed.screen = left;
            }
        }
        found
    }

    /// Fills the queue from the first stage with work.
    fn fill(&mut self) {
        let limit = self.done.len() + BATCH;
        let fresh = |found: Result<Vec<Sighting>, String>, done: &HashSet<PathBuf>| {
            found
                .unwrap_or_else(|e| {
                    log::warn!("map: {e}");
                    Vec::new()
                })
                .into_iter()
                .filter(|s| !done.contains(&s.path))
                .take(BATCH)
                .collect::<Vec<_>>()
        };
        let library: Vec<Sighting> =
            fresh(self.map.to_read(ToRead::Library, None, limit), &self.done)
                .into_iter()
                .filter(|s| s.last_verified.is_none())
                .collect();
        if self.push(Stage::Library, library) {
            return;
        }
        let plugged = self.feed.fed().plugged.clone();
        for volume in plugged {
            let found = fresh(
                self.map.to_read(ToRead::Partial, Some(&volume), limit),
                &self.done,
            );
            if self.push(Stage::Plugged, found) {
                return;
            }
            self.feed.fed().plugged.retain(|v| v != &volume);
        }
        let confirm = fresh(self.map.to_read(ToRead::Confirm, None, limit), &self.done);
        if self.push(Stage::Confirm, confirm) {
            return;
        }
        let rest = fresh(self.map.to_read(ToRead::Partial, None, limit), &self.done);
        self.push(Stage::Rest, rest);
    }

    fn push(&mut self, stage: Stage, found: Vec<Sighting>) -> bool {
        let any = !found.is_empty();
        self.queue.extend(found.into_iter().map(|s| (stage, s)));
        any
    }

    /// Reads a file: its partial hash, or its full hash for a Library file
    /// (verified) and a duplicate (confirmed when an image has it). Returns
    /// whether it was read: an excluded place, a file gone or changed
    /// since listed, is not.
    pub fn read(&mut self, stage: Stage, mut s: Sighting) -> Result<bool, String> {
        let place = s.volume.as_ref().zip(s.rel_path.as_deref());
        if let Some(reason) = self.checker.excluded(&s.path, place) {
            log::debug!("map: {} not read: {reason:?}", s.path.display());
            return Ok(false);
        }
        match self.files.stat(&s.path) {
            Some((size, modified)) if size == s.size && modified == s.modified => {}
            _ => return Ok(false),
        }
        let now = (self.clock)();
        match stage {
            Stage::Library => {
                let hash = self.files.full(&s.path)?;
                // A Library file changed outside Helios is checked against
                // the content recorded then (the thumbnail re-check's).
                if s.image.map(|id| self.map.content_of(&id)) != Some(ImageId(hash)) {
                    return Err(format!("{}: its content changed", s.path.display()));
                }
                s.last_verified = Some(now);
            }
            Stage::Confirm => {
                let id = ImageId(self.files.full(&s.path)?);
                if !self.map.image_known(&id) {
                    return Ok(true);
                }
                s.image = Some(id);
                s.last_verified = Some(now);
            }
            Stage::Screen | Stage::Plugged | Stage::Rest => {
                s.partial = Some(self.files.partial(&s.path, s.size)?);
            }
        }
        self.map.record(&s)?;
        Ok(true)
    }

    /// Runs until `stop`: a jobs-centre entry (`map-<n>`, pausable and
    /// cancellable) while there is work, none while idle. Cancelled, the
    /// job waits for something new (a feed) or the next hour.
    pub fn run(mut self, jobs: &Jobs, stop: &AtomicBool) {
        let mut runs = 0u32;
        let mut report: Option<jobs::Report> = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let mut paused: Option<String> = None;
        let mut read = 0u64;
        let mut shown = Instant::now();
        let mut idle_since: Option<Instant> = None;
        while !stop.load(Ordering::SeqCst) {
            if cancel.swap(false, Ordering::SeqCst) {
                if let Some(r) = report.take() {
                    r.cancelled();
                }
                self.queue.clear();
                self.wait_for_news(stop, Duration::from_secs(PRUNE_EVERY));
                continue;
            }
            match self.next_read() {
                Next::Paused(why) => {
                    if let Some(r) = &report
                        && paused.as_ref() != Some(&why)
                    {
                        r.paused(&why);
                    }
                    paused = Some(why);
                    self.wait_for_news(stop, Duration::from_secs(5));
                }
                Next::Idle => {
                    if let Some(r) = report.take() {
                        r.log(format!("{read} files read"));
                        r.finished();
                    }
                    paused = None;
                    let since = *idle_since.get_or_insert_with(Instant::now);
                    let wait = Duration::from_secs(PRUNE_EVERY).saturating_sub(since.elapsed());
                    self.wait_for_news(stop, wait.max(Duration::from_secs(1)));
                }
                Next::Read(stage, s) => {
                    idle_since = None;
                    let r = report.get_or_insert_with(|| {
                        runs += 1;
                        read = 0;
                        let r = jobs.start(
                            Start::new(&format!("map-{runs}"), "Image map", KIND).pausable(),
                        );
                        let c = cancel.clone();
                        r.on_cancel(move || c.store(true, Ordering::SeqCst));
                        let feed = self.feed.clone();
                        r.on_pause(move |p| feed.pause(p));
                        r
                    });
                    if paused.take().is_some() {
                        r.resumed();
                    }
                    let name = s.path.display().to_string();
                    match self.read(stage, *s) {
                        Ok(true) => read += 1,
                        Ok(false) => {}
                        Err(e) => r.item_failed(&name, &e),
                    }
                    if shown.elapsed() > Duration::from_millis(500) {
                        shown = Instant::now();
                        r.progress(Progress::new().done(read).item(&name).step(stage.as_str()));
                    }
                }
            }
        }
        if let Some(r) = report {
            r.cancelled();
        }
    }

    /// Sleeps up to `most`, ending early on a feed or `stop`.
    fn wait_for_news(&self, stop: &AtomicBool, most: Duration) {
        let start = Instant::now();
        while start.elapsed() < most && !stop.load(Ordering::SeqCst) {
            if self.feed.take_woken() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Never busy, never on battery.
pub(crate) struct Never;

impl Load for Never {
    fn busy(&mut self) -> Option<String> {
        None
    }
}

impl Power for Never {
    fn on_battery(&mut self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use helios_storage::exclude::{Blacklist, Os, Places};
    use helios_vault::{Image, MemoryStore, Presence};

    use super::*;

    /// Files that are what their sightings say: each read is logged.
    #[derive(Clone, Default)]
    struct Fake {
        files: HashMap<PathBuf, (u64, u64, ContentHash)>,
        reads: Arc<Mutex<Vec<String>>>,
    }

    impl Files for Fake {
        fn stat(&self, path: &Path) -> Option<(u64, u64)> {
            self.files.get(path).map(|&(s, m, _)| (s, m))
        }
        fn partial(&self, path: &Path, _: u64) -> Result<ContentHash, String> {
            self.reads
                .lock()
                .unwrap()
                .push(format!("partial {}", path.display()));
            Ok(ContentHash([9; 32]))
        }
        fn full(&self, path: &Path) -> Result<ContentHash, String> {
            self.reads
                .lock()
                .unwrap()
                .push(format!("full {}", path.display()));
            Ok(self.files[path].2)
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

    fn volume(id: &str, mount: &str) -> Volume {
        let mut v: Volume = serde_json::from_value(serde_json::json!({
            "id": id, "identified_by": "uuid", "name": id, "kind": "external",
        }))
        .unwrap();
        v.mount = Some(mount.into());
        v
    }

    fn on(volume: &str, mount: &str, rel: &str, size: u64, seen: u64) -> Sighting {
        let mut s = Sighting::seen(Path::new(mount).join(rel), size, 5, seen, "import");
        s.volume = Some(VolumeId(volume.into()));
        s.rel_path = Some(rel.into());
        s
    }

    fn image(n: u8) -> Image {
        Image {
            id: ImageId(ContentHash([n; 32])),
            size: 100 + u64::from(n),
            partial: ContentHash([n + 1; 32]),
        }
    }

    struct World {
        vault: Arc<Vault<MemoryStore>>,
        fake: Fake,
        feed: Feed,
        sightings: BTreeMap<&'static str, Sighting>,
    }

    /// A vault with a sighting of each kind, on a disk and a card.
    fn world() -> World {
        let mut store = MemoryStore::default();
        let a = image(1);
        store.add_to_library(&a).unwrap();
        let mut sightings = BTreeMap::new();
        let mut lib = on("uuid:disk", "/media/disk", "lib/a.cr3", a.size, 1);
        lib.partial = Some(a.partial);
        lib.image = Some(a.id);
        let mut dup = on("uuid:disk", "/media/disk", "dup/a.cr3", a.size, 2);
        dup.partial = Some(a.partial);
        sightings.insert("library", lib);
        sightings.insert("confirm", dup);
        sightings.insert(
            "plugged",
            on("uuid:card", "/media/card", "DCIM/b.cr3", 7, 3),
        );
        sightings.insert("rest", on("uuid:disk", "/media/disk", "old/c.jpg", 8, 4));
        sightings.insert(
            "screen",
            on("uuid:disk", "/media/disk", "shown/d.jpg", 9, 5),
        );
        sightings.insert(
            "excluded",
            on("uuid:disk", "/media/disk", "shown/.hidden/e.jpg", 9, 6),
        );
        let mut gone = on("uuid:disk", "/media/disk", "gone.jpg", 9, 7);
        gone.presence = Presence::Missing;
        sightings.insert("gone", gone);
        let mut fake = Fake::default();
        for s in sightings.values() {
            store.put_sighting(s).unwrap();
            let full = s.image.map_or(ContentHash([1; 32]), |i| i.0);
            fake.files
                .insert(s.path.clone(), (s.size, s.modified, full));
        }
        World {
            vault: Arc::new(Vault::new(store)),
            fake,
            feed: Feed::default(),
            sightings,
        }
    }

    fn job(w: &World) -> Job<Vault<MemoryStore>> {
        let checker = Checker::new(Blacklist::new(
            Os::Linux,
            &Places::default(),
            true,
            Vec::new(),
        ));
        Job::new(
            w.vault.clone(),
            Box::new(w.fake.clone()),
            checker,
            vec![
                volume("uuid:disk", "/media/disk"),
                volume("uuid:card", "/media/card"),
            ],
            w.feed.clone(),
            Settings::default(),
        )
        .clock(|| 1_000_000)
    }

    /// Drives the job until it is idle; returns the stages read.
    fn drain(job: &mut Job<Vault<MemoryStore>>) -> Vec<(Stage, String)> {
        let mut read = Vec::new();
        loop {
            match job.next_read() {
                Next::Read(stage, s) => {
                    let name = s.path.display().to_string();
                    if job.read(stage, *s).unwrap() {
                        read.push((stage, name));
                    }
                }
                Next::Idle => return read,
                Next::Paused(why) => panic!("paused: {why}"),
            }
        }
    }

    #[test]
    fn it_reads_in_the_notes_order() {
        let w = world();
        let mut job = job(&w);
        w.feed.on_screen(vec![w.sightings["screen"].path.clone()]);
        w.feed.plugged(VolumeId("uuid:card".into()));
        let read = drain(&mut job);
        let stages: Vec<Stage> = read.iter().map(|(s, _)| *s).collect();
        assert_eq!(
            stages,
            [
                Stage::Screen,
                Stage::Library,
                Stage::Plugged,
                Stage::Confirm,
                Stage::Rest
            ],
            "{read:?}"
        );
        assert_eq!(
            *w.fake.reads.lock().unwrap(),
            [
                "partial /media/disk/shown/d.jpg",
                "full /media/disk/lib/a.cr3",
                "partial /media/card/DCIM/b.cr3",
                "full /media/disk/dup/a.cr3",
                "partial /media/disk/old/c.jpg",
            ],
            "the excluded and missing files are never read"
        );
        let at = |k: &str| {
            let s = &w.sightings[k];
            w.vault
                .sighting_at(s.volume.as_ref(), s.rel_path.as_deref(), &s.path)
                .unwrap()
                .unwrap()
        };
        assert_eq!(at("confirm").image, Some(image(1).id), "confirmed");
        assert_eq!(at("library").last_verified, Some(1_000_000));
        assert_eq!(at("rest").partial, Some(ContentHash([9; 32])));
        assert_eq!(at("excluded").partial, None);
        // Nothing is read twice in the hour.
        assert_eq!(drain(&mut job), []);
    }

    #[test]
    fn it_gives_way_to_other_jobs_and_to_the_battery() {
        let w = world();
        let busy = Arc::new(Mutex::new(Some("import".to_string())));
        let battery = Arc::new(AtomicBool::new(false));
        let mut job = job(&w)
            .load(Box::new(Busy(busy.clone())))
            .power(Box::new(Battery(battery.clone())));
        assert_eq!(
            job.next_read(),
            Next::Paused("Waiting for the import job to end".into())
        );
        *busy.lock().unwrap() = None;
        battery.store(true, Ordering::SeqCst);
        assert_eq!(
            job.next_read(),
            Next::Paused("Waiting for mains power".into())
        );
        battery.store(false, Ordering::SeqCst);
        w.feed.pause(true);
        assert_eq!(job.next_read(), Next::Paused("Paused".into()));
        w.feed.pause(false);
        assert!(matches!(job.next_read(), Next::Read(Stage::Library, _)));
        assert!(
            w.fake.reads.lock().unwrap().is_empty(),
            "nothing read while paused"
        );
    }

    #[test]
    fn a_bus_says_which_jobs_run() {
        let bus = Arc::new(Bus::new());
        let jobs = Jobs::new(bus.clone());
        let mut load = BusLoad::new(&bus);
        let health = jobs.start(Start::new("h", "Health", "health"));
        assert_eq!(load.busy(), None, "a job it does not yield to");
        let import = jobs.start(Start::new("i", "Import", "import"));
        assert_eq!(load.busy().as_deref(), Some("import"));
        import.finished();
        health.finished();
        assert_eq!(load.busy(), None);
    }

    #[test]
    fn the_power_supply_says_battery() {
        let root = std::env::temp_dir().join(format!("helios-power-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let supply = |name: &str, files: &[(&str, &str)]| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            for (f, v) in files {
                std::fs::write(d.join(f), v).unwrap();
            }
        };
        assert!(!on_battery_in(&root));
        supply(
            "BAT0",
            &[("type", "Battery\n"), ("status", "Discharging\n")],
        );
        assert!(on_battery_in(&root));
        supply("AC", &[("type", "Mains\n"), ("online", "1\n")]);
        assert!(!on_battery_in(&root));
        assert!(!on_battery_in(&root.join("none")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn it_prunes_at_most_once_an_hour() {
        let w = world();
        let now = Arc::new(Mutex::new(10_000_000u64));
        let clock = now.clone();
        let mut job = job(&w).clock(move || *clock.lock().unwrap());
        // The missing sighting is older than keepMissing: pruned.
        assert_eq!(job.prune_if_due(), Some(1));
        assert_eq!(job.prune_if_due(), None);
        *now.lock().unwrap() += PRUNE_EVERY - 1;
        assert_eq!(job.prune_if_due(), None);
        *now.lock().unwrap() += 1;
        assert_eq!(job.prune_if_due(), Some(0));
    }

    #[test]
    fn a_run_shows_a_job_and_is_cancelled() {
        let w = world();
        let bus = Arc::new(Bus::new());
        let jobs = Jobs::new(bus.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let job = job(&w);
        let thread = {
            let (jobs, stop) = (jobs.clone(), stop.clone());
            std::thread::spawn(move || job.run(&jobs, &stop))
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !bus.since(0).iter().any(|e| e.kind == jobs::FINISHED) {
            assert!(Instant::now() < deadline, "the job never ended");
            std::thread::sleep(Duration::from_millis(20));
        }
        stop.store(true, Ordering::SeqCst);
        thread.join().unwrap();
        let events = bus.since(0);
        let started = events.iter().find(|e| e.kind == jobs::STARTED).unwrap();
        assert_eq!(started.payload["kind"], KIND);
        assert_eq!(started.payload["pausable"], true);
        assert_eq!(started.payload["cancellable"], true);
    }
}
