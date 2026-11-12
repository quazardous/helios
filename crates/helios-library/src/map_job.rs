//! The image map's background job (`docs/design/image-map.md`, section 4,
//! "Background scans"): one low-priority job of kind `map` in the jobs
//! centre that reads the files the map knows, in this order:
//!
//! 1. [`Stage::Screen`]: what is on screen ([`Feed::on_screen`], fed by
//!    the interface with the paths in view): their partial hash;
//!
//!    then the **scans** of volumes plugged in just now ([`Feed::plugged`])
//!    or asked for ([`Feed::scan_now`]), a folder at a time
//!    ([`Next::List`]): the folders the map knows there first, then the
//!    rest of the volume. A scan costs a `stat` per file and reads none;
//!    it records what it finds as sightings (`seen_by: scan`), never in a
//!    place the blacklist excludes or a folder the user forgot. Which
//!    volumes are scanned follows `system.map.record` ([`Record`]): every
//!    one, those with a role, or none;
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
//! At most once an hour, and after each scan, it prunes the map
//! ([`Vault::prune_sightings`], `system.map.limit` and
//! `system.map.keepMissing`): past the cap, the files outside the Library
//! seen longest ago go first.
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
use helios_vault::{IMAGE_EXTENSIONS, ImageId, Sighting, Store, ToRead, Vault};

use crate::map::Record;

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
    /// Volumes to scan (listed whole), in the order asked.
    scans: Vec<Scan>,
    paused: bool,
    /// Something new to do: the idle wait ends.
    woken: bool,
}

/// A scan asked for: a volume, or every volume the setting allows.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Scan {
    /// Plugged in just now: scanned when the setting allows it.
    Plugged(VolumeId),
    /// Asked for on purpose: a volume forgotten whole is listed again.
    Now(VolumeId),
    /// Every volume mounted that the setting allows.
    All,
    /// A folder and those below it (an absolute path), asked for on
    /// purpose: listed on the volume mounted above it.
    Folder(PathBuf),
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
    /// It is scanned (listed) first, once, when `system.map.record` allows.
    pub fn plugged(&self, volume: VolumeId) {
        let mut fed = self.fed();
        if !fed.plugged.contains(&volume) {
            fed.plugged.push(volume.clone());
        }
        let scan = Scan::Plugged(volume);
        if !fed.scans.contains(&scan) {
            fed.scans.push(scan);
        }
        fed.woken = true;
    }

    /// Scans a volume now (`library.map.scanNow`), or with `None` every
    /// volume mounted that `system.map.record` allows. A volume asked for
    /// by its id is listed again even if it was forgotten whole.
    pub fn scan_now(&self, volume: Option<VolumeId>) {
        let mut fed = self.fed();
        let scan = volume.map_or(Scan::All, Scan::Now);
        if !fed.scans.contains(&scan) {
            fed.scans.push(scan);
        }
        fed.woken = true;
    }

    /// Scans a folder and those below it now (`library.scanFolder`, the
    /// Library's folders tree): listed on the volume mounted above it, as
    /// a volume asked for on purpose is.
    pub fn scan_folder(&self, folder: PathBuf) {
        let mut fed = self.fed();
        let scan = Scan::Folder(folder);
        if !fed.scans.contains(&scan) {
            fed.scans.push(scan);
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
    /// The entries of a folder: each name and whether it is a folder
    /// (links are not followed: a link is neither).
    fn list(&self, dir: &Path) -> Result<Vec<(std::ffi::OsString, Entry)>, String>;
}

/// What a folder's entry is, as its listing says (no `stat`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    File,
    Folder,
    /// A link or anything else: not followed, not recorded.
    Other,
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

    fn list(&self, dir: &Path) -> Result<Vec<(std::ffi::OsString, Entry)>, String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(entries
            .flatten()
            .map(|e| {
                let kind = match e.file_type() {
                    Ok(t) if t.is_dir() => Entry::Folder,
                    Ok(t) if t.is_file() => Entry::File,
                    _ => Entry::Other,
                };
                (e.file_name(), kind)
            })
            .collect())
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
    /// Records what a scan listed in a folder, in one transaction
    /// ([`Vault::record_listed`]).
    ///
    /// # Errors
    /// The vault cannot be written.
    fn listed(&self, batch: &[Sighting]) -> Result<(), String>;
    /// The folders of `volume` the map holds files of.
    fn known_folders(&self, volume: &VolumeId) -> Vec<PathBuf>;
    /// Whether the folder `rel_dir` of `volume`, or one above it, was
    /// forgotten.
    fn forgotten(&self, volume: &VolumeId, rel_dir: &Path) -> bool;
    /// Notes that a scan listed a folder (a forgotten one is listed again).
    fn folder_listed(&self, volume: &VolumeId, rel_dir: &Path, files: u64, now: u64);
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

    fn listed(&self, batch: &[Sighting]) -> Result<(), String> {
        self.record_listed(batch).map(|_| ())
    }

    fn known_folders(&self, volume: &VolumeId) -> Vec<PathBuf> {
        let mut folders: Vec<PathBuf> = self
            .sightings_under(Some(volume), Path::new(""))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| s.rel_path?.parent().map(Path::to_path_buf))
            .collect();
        folders.sort();
        folders.dedup();
        folders
    }

    fn forgotten(&self, volume: &VolumeId, rel_dir: &Path) -> bool {
        // `Vault::forgotten` asks about a file: the folders above it.
        Vault::forgotten(self, Some(volume), &rel_dir.join("-")).unwrap_or(false)
    }

    fn folder_listed(&self, volume: &VolumeId, rel_dir: &Path, files: u64, now: u64) {
        if let Err(e) = Vault::folder_listed(self, Some(volume), rel_dir, files, now) {
            log::warn!("map: {}: {e}", rel_dir.display());
        }
    }
}

/// The pruning settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// `system.map.limit`: sightings not in the Library kept.
    pub limit: usize,
    /// `system.map.keepMissing`, in days.
    pub keep_missing_days: u64,
    /// `system.map.record`: which volumes are scanned.
    pub record: Record,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            limit: 1_000_000,
            keep_missing_days: 30,
            record: Record::Listings,
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
    /// A folder of a volume to list ([`Job::list`]): a scan.
    List(VolumeId, PathBuf),
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
    /// The volumes known now, asked again when a scan starts.
    volumes_from: Option<Box<dyn Fn() -> Vec<Volume> + Send>>,
    /// The scans running, the first listed first.
    scanning: VecDeque<Scanning>,
}

/// A volume being scanned: the folders left to list.
struct Scanning {
    volume: VolumeId,
    mount: PathBuf,
    folders: VecDeque<PathBuf>,
    /// Folders listed or queued, never listed twice.
    seen: HashSet<PathBuf>,
    files: u64,
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
            volumes_from: None,
            scanning: VecDeque::new(),
        }
    }

    /// Where the volumes known come from (the system's, merged with the
    /// vault's registry): asked again when a scan starts, so a volume
    /// plugged in after the job started is placed.
    #[must_use]
    pub fn volumes_from(mut self, volumes: impl Fn() -> Vec<Volume> + Send + 'static) -> Self {
        self.volumes_from = Some(Box::new(volumes));
        self
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
        Some(self.prune_now())
    }

    /// Prunes the map now; returns how many sightings went.
    fn prune_now(&mut self) -> usize {
        let now = (self.clock)();
        let before = now.saturating_sub(self.settings.keep_missing_days * 86_400);
        self.map
            .prune(self.settings.limit, before)
            .unwrap_or_else(|e| {
                log::warn!("map: pruning: {e}");
                0
            })
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
        self.take_scans();
        if let Some((volume, folder)) = self.next_folder() {
            return Next::List(volume, folder);
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

    /// Whether `system.map.record` lets `volume` be scanned: everywhere,
    /// every volume; listings, those with a role or asked for on purpose;
    /// import, none.
    fn scans(&self, volume: &Volume, on_purpose: bool) -> bool {
        match self.settings.record {
            Record::Everywhere => true,
            Record::Listings => on_purpose || !volume.roles.is_empty(),
            Record::Import => false,
        }
    }

    /// Starts the scans the feed asks for.
    fn take_scans(&mut self) {
        let asked = std::mem::take(&mut self.feed.fed().scans);
        if asked.is_empty() {
            return;
        }
        if let Some(from) = &self.volumes_from {
            self.volumes = from();
        }
        let now = (self.clock)();
        for scan in asked {
            if let Scan::Folder(folder) = &scan {
                self.start_folder_scan(folder, now);
                continue;
            }
            let (wanted, on_purpose): (Vec<Volume>, bool) = match &scan {
                Scan::Plugged(id) | Scan::Now(id) => (
                    self.volumes
                        .iter()
                        .filter(|v| &v.id == id)
                        .cloned()
                        .collect(),
                    matches!(scan, Scan::Now(_)),
                ),
                Scan::All => (self.volumes.clone(), false),
                Scan::Folder(_) => continue,
            };
            for v in wanted {
                let Some(mount) = v.mount.clone() else {
                    continue;
                };
                if !self.scans(&v, on_purpose) || self.scanning.iter().any(|s| s.volume == v.id) {
                    continue;
                }
                if on_purpose && self.map.forgotten(&v.id, Path::new("")) {
                    // Asked for by name: a volume forgotten whole is listed
                    // again (its folders forgotten one by one stay so).
                    self.map.folder_listed(&v.id, Path::new(""), 0, now);
                }
                // The folders known first, then the whole volume.
                let mut folders: VecDeque<PathBuf> = self.map.known_folders(&v.id).into();
                if !folders.iter().any(|f| f.as_os_str().is_empty()) {
                    folders.push_back(PathBuf::new());
                }
                self.scanning.push_back(Scanning {
                    volume: v.id.clone(),
                    mount,
                    seen: folders.iter().cloned().collect(),
                    folders,
                    files: 0,
                });
            }
        }
    }

    /// Starts listing `folder` and the folders below it, on the volume
    /// mounted deepest above it; a folder on no volume mounted, or on one
    /// being scanned already, is left.
    fn start_folder_scan(&mut self, folder: &Path, now: u64) {
        let Some((v, mount)) = self
            .volumes
            .iter()
            .filter_map(|v| {
                let mount = v.mount.clone()?;
                folder.starts_with(&mount).then(|| (v.clone(), mount))
            })
            .max_by_key(|(_, m)| m.components().count())
        else {
            log::debug!("map: {} is on no volume mounted", folder.display());
            return;
        };
        if !self.scans(&v, true) || self.scanning.iter().any(|s| s.volume == v.id) {
            return;
        }
        let rel = folder.strip_prefix(&mount).unwrap_or(folder).to_path_buf();
        if self.map.forgotten(&v.id, &rel) {
            // Asked for by name: listed again.
            self.map.folder_listed(&v.id, &rel, 0, now);
        }
        self.scanning.push_back(Scanning {
            volume: v.id.clone(),
            mount,
            seen: std::iter::once(rel.clone()).collect(),
            folders: VecDeque::from([rel]),
            files: 0,
        });
    }

    /// The next folder a scan lists; a scan with none left ends (and the
    /// map is pruned: the cap holds).
    fn next_folder(&mut self) -> Option<(VolumeId, PathBuf)> {
        while let Some(scan) = self.scanning.front_mut() {
            if let Some(folder) = scan.folders.pop_front() {
                return Some((scan.volume.clone(), folder));
            }
            let done = self.scanning.pop_front().expect("a scan");
            log::info!("map: {} scanned, {} files", done.volume.0, done.files);
            self.prune_now();
        }
        None
    }

    /// Lists a folder of a volume being scanned: a `stat` per image file,
    /// recorded in one transaction; its folders are queued. An excluded
    /// or forgotten folder is skipped with what is below it. Returns how
    /// many files were recorded.
    ///
    /// # Errors
    /// The folder cannot be listed (a volume gone ends its scan), or the
    /// vault cannot be written.
    pub fn list(&mut self, volume: &VolumeId, rel_dir: &Path) -> Result<usize, String> {
        let Some(at) = self.scanning.iter().position(|s| &s.volume == volume) else {
            return Ok(0);
        };
        let mount = self.scanning[at].mount.clone();
        let root = rel_dir.as_os_str().is_empty();
        let dir = if root {
            mount.clone()
        } else {
            mount.join(rel_dir)
        };
        if !root && let Some(reason) = self.checker.excluded(&dir, Some((volume, rel_dir))) {
            log::debug!("map: {} not scanned: {reason:?}", dir.display());
            return Ok(0);
        }
        if self.map.forgotten(volume, rel_dir) {
            return Ok(0);
        }
        let mut entries = match self.files.list(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                if self.files.list(&mount).is_err() {
                    // The volume went away: its scan stops at once.
                    self.scanning.remove(at);
                }
                return Err(e);
            }
        };
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let now = (self.clock)();
        let mut batch = Vec::new();
        for (name, entry) in entries {
            let rel = rel_dir.join(&name);
            match entry {
                Entry::Folder => {
                    if self
                        .checker
                        .excluded(&dir.join(&name), Some((volume, &rel)))
                        .is_some()
                    {
                        continue;
                    }
                    let scan = &mut self.scanning[at];
                    if scan.seen.insert(rel.clone()) {
                        scan.folders.push_back(rel);
                    }
                }
                Entry::File => {
                    let path = dir.join(&name);
                    if !is_image(&path)
                        || self.checker.excluded(&path, Some((volume, &rel))).is_some()
                    {
                        continue;
                    }
                    let Some((size, modified)) = self.files.stat(&path) else {
                        continue;
                    };
                    let mut s = Sighting::seen(path, size, modified, now, "scan");
                    s.volume = Some(volume.clone());
                    s.rel_path = Some(rel);
                    batch.push(s);
                }
                Entry::Other => {}
            }
        }
        if !batch.is_empty() {
            self.map.listed(&batch)?;
        }
        let n = batch.len();
        self.map.folder_listed(volume, rel_dir, n as u64, now);
        self.scanning[at].files += n as u64;
        Ok(n)
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
        let mut listed = 0u64;
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
                        r.log(format!("{read} files read, {listed} listed"));
                        r.finished();
                    }
                    paused = None;
                    let since = *idle_since.get_or_insert_with(Instant::now);
                    let wait = Duration::from_secs(PRUNE_EVERY).saturating_sub(since.elapsed());
                    self.wait_for_news(stop, wait.max(Duration::from_secs(1)));
                }
                next @ (Next::Read(..) | Next::List(..)) => {
                    idle_since = None;
                    let r = report.get_or_insert_with(|| {
                        runs += 1;
                        read = 0;
                        listed = 0;
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
                    let (name, step) = match next {
                        Next::Read(stage, s) => {
                            let name = s.path.display().to_string();
                            match self.read(stage, *s) {
                                Ok(true) => read += 1,
                                Ok(false) => {}
                                Err(e) => r.item_failed(&name, &e),
                            }
                            (name, stage.as_str())
                        }
                        Next::List(volume, folder) => {
                            let name = format!("{}: {}", volume.0, folder.display());
                            match self.list(&volume, &folder) {
                                Ok(n) => listed += n as u64,
                                Err(e) => r.item_failed(&name, &e),
                            }
                            (name, "scan")
                        }
                        Next::Paused(_) | Next::Idle => unreachable!("matched above"),
                    };
                    if shown.elapsed() > Duration::from_millis(500) {
                        shown = Instant::now();
                        r.progress(Progress::new().done(read + listed).item(&name).step(step));
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

/// Whether `path` names an image file Helios imports.
fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
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
        stats: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Files for Fake {
        fn stat(&self, path: &Path) -> Option<(u64, u64)> {
            self.stats.fetch_add(1, Ordering::SeqCst);
            self.files.get(path).map(|&(s, m, _)| (s, m))
        }
        fn list(&self, dir: &Path) -> Result<Vec<(std::ffi::OsString, Entry)>, String> {
            let mut found = BTreeMap::new();
            for p in self.files.keys() {
                let Ok(rest) = p.strip_prefix(dir) else {
                    continue;
                };
                let mut parts = rest.components();
                if let Some(first) = parts.next() {
                    let kind = if parts.next().is_some() {
                        Entry::Folder
                    } else {
                        Entry::File
                    };
                    found.insert(first.as_os_str().to_owned(), kind);
                }
            }
            if found.is_empty() {
                return Err(format!("{}: gone", dir.display()));
            }
            Ok(found.into_iter().collect())
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
        job_with(w, Settings::default())
    }

    /// A job whose card is an ingest volume (a role: scanned when
    /// plugged under the default setting).
    fn job_with(w: &World, settings: Settings) -> Job<Vault<MemoryStore>> {
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
            vec![volume("uuid:disk", "/media/disk"), {
                let mut card = volume("uuid:card", "/media/card");
                card.roles = [helios_storage::Role::Ingest].into();
                card
            }],
            w.feed.clone(),
            settings,
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
                Next::List(volume, folder) => {
                    job.list(&volume, &folder).unwrap();
                    read.push((Stage::Plugged, format!("list {}", folder.display())));
                }
                Next::Idle => return read,
                Next::Paused(why) => panic!("paused: {why}"),
            }
        }
    }

    /// The folders a drain listed.
    fn listed(read: &[(Stage, String)]) -> Vec<&str> {
        read.iter()
            .filter_map(|(_, r)| r.strip_prefix("list "))
            .collect()
    }

    /// The world with more on the card: a new photo below `DCIM`, one in
    /// a hidden folder and a file that is no image.
    fn card_world() -> World {
        let mut w = world();
        for rel in ["DCIM/100/x.cr3", "DCIM/.thumbs/y.jpg", "notes.txt"] {
            w.fake.files.insert(
                Path::new("/media/card").join(rel),
                (11, 5, ContentHash([3; 32])),
            );
        }
        w
    }

    fn card_at(w: &World, rel: &str) -> Option<Sighting> {
        w.vault
            .sighting_at(
                Some(&VolumeId("uuid:card".into())),
                Some(Path::new(rel)),
                &Path::new("/media/card").join(rel),
            )
            .unwrap()
    }

    #[test]
    fn plugging_a_volume_scans_it_once() {
        let w = card_world();
        let mut job = job(&w);
        w.feed.plugged(VolumeId("uuid:card".into()));
        let read = drain(&mut job);
        // The folder known first, then the volume from its root; the
        // hidden folder is never listed.
        assert_eq!(listed(&read), ["DCIM", "", "DCIM/100"], "{read:?}");
        let x = card_at(&w, "DCIM/100/x.cr3").expect("recorded");
        assert_eq!((x.seen_by.as_str(), x.size), ("scan", 11));
        assert_eq!(
            card_at(&w, "DCIM/b.cr3").unwrap().seen_by,
            "import",
            "what listed it first stays"
        );
        assert!(card_at(&w, "DCIM/.thumbs/y.jpg").is_none(), "excluded");
        assert!(card_at(&w, "notes.txt").is_none(), "not an image");
        let folder = w
            .vault
            .folder(Some(&VolumeId("uuid:card".into())), Path::new("DCIM/100"))
            .unwrap()
            .unwrap();
        assert_eq!(folder.files, Some(1));
        // Once: nothing more to list until it is plugged again.
        assert!(listed(&drain(&mut job)).is_empty());
        // A volume with no role is not scanned under the default setting.
        w.feed.plugged(VolumeId("uuid:disk".into()));
        assert!(listed(&drain(&mut job)).is_empty());
    }

    #[test]
    fn a_pause_stops_the_scan() {
        let w = card_world();
        let mut job = job(&w);
        w.feed.plugged(VolumeId("uuid:card".into()));
        let Next::List(volume, folder) = job.next_read() else {
            panic!("a scan first");
        };
        assert_eq!(folder, Path::new("DCIM"));
        job.list(&volume, &folder).unwrap();
        w.feed.pause(true);
        assert_eq!(job.next_read(), Next::Paused("Paused".into()));
        assert!(card_at(&w, "DCIM/100/x.cr3").is_none(), "not listed yet");
        w.feed.pause(false);
        assert_eq!(listed(&drain(&mut job)), ["", "DCIM/100"]);
        assert!(card_at(&w, "DCIM/100/x.cr3").is_some());
    }

    #[test]
    fn a_scan_ends_with_the_cap_dropping_the_oldest_outside_the_library() {
        let w = card_world();
        let mut job = job_with(
            &w,
            Settings {
                limit: 2,
                ..Settings::default()
            },
        );
        w.feed.plugged(VolumeId("uuid:card".into()));
        drain(&mut job);
        let mut left: Vec<String> = Vec::new();
        for v in ["uuid:disk", "uuid:card"] {
            left.extend(
                w.vault
                    .sightings_under(Some(&VolumeId(v.into())), Path::new(""))
                    .unwrap()
                    .into_iter()
                    .map(|s| s.rel_path.unwrap().display().to_string()),
            );
        }
        left.sort();
        // The Library's file is never counted; of the rest, the two
        // just scanned are the most recently seen.
        assert_eq!(left, ["DCIM/100/x.cr3", "DCIM/b.cr3", "lib/a.cr3"]);
    }

    #[test]
    fn recording_only_on_import_scans_nothing() {
        let w = card_world();
        let mut job = job_with(
            &w,
            Settings {
                record: Record::Import,
                ..Settings::default()
            },
        );
        w.feed.plugged(VolumeId("uuid:card".into()));
        w.feed.scan_now(None);
        w.feed.scan_now(Some(VolumeId("uuid:disk".into())));
        assert!(listed(&drain(&mut job)).is_empty());
        assert!(card_at(&w, "DCIM/100/x.cr3").is_none());
        assert!(!Record::parse("import").listings());
    }

    #[test]
    fn scan_now_lists_a_volume_forgotten_whole_again() {
        let w = card_world();
        let mut job = job_with(
            &w,
            Settings {
                record: Record::Everywhere,
                ..Settings::default()
            },
        );
        let disk = VolumeId("uuid:disk".into());
        let old = |w: &World| {
            w.vault
                .sighting_at(
                    Some(&disk),
                    Some(Path::new("old/c.jpg")),
                    Path::new("/media/disk/old/c.jpg"),
                )
                .unwrap()
        };
        w.vault.forget(Some(&disk), None, 10).unwrap();
        assert!(old(&w).is_none());
        // Every volume is scanned, but the one forgotten records nothing.
        w.feed.scan_now(None);
        drain(&mut job);
        assert!(old(&w).is_none(), "forgotten");
        assert!(card_at(&w, "DCIM/100/x.cr3").is_some());
        // Asked for by name: listed again.
        w.feed.scan_now(Some(disk.clone()));
        drain(&mut job);
        assert!(old(&w).is_some());
        assert!(!w.vault.forgotten(Some(&disk), Path::new("x.jpg")).unwrap());
    }

    #[test]
    fn scan_folder_lists_that_folder_and_below_only() {
        let w = card_world();
        let mut job = job(&w);
        w.feed.scan_folder(PathBuf::from("/media/card/DCIM/100"));
        // Neither the volume's root nor `DCIM` above it.
        assert_eq!(listed(&drain(&mut job)), ["DCIM/100"]);
        assert!(card_at(&w, "DCIM/100/x.cr3").is_some());
        // A folder on no volume mounted lists nothing.
        w.feed.scan_folder(PathBuf::from("/nowhere/photos"));
        assert!(listed(&drain(&mut job)).is_empty());
    }

    #[test]
    fn an_unchanged_file_costs_one_stat() {
        let w = card_world();
        let mut job = job(&w);
        w.feed.plugged(VolumeId("uuid:card".into()));
        drain(&mut job);
        w.fake.stats.store(0, Ordering::SeqCst);
        w.fake.reads.lock().unwrap().clear();
        let before = card_at(&w, "DCIM/100/x.cr3").unwrap();
        w.feed.plugged(VolumeId("uuid:card".into()));
        assert_eq!(listed(&drain(&mut job)), ["DCIM", "DCIM/100", ""]);
        assert_eq!(
            w.fake.stats.load(Ordering::SeqCst),
            2,
            "one stat for each of the card's two photos"
        );
        assert!(w.fake.reads.lock().unwrap().is_empty(), "nothing read");
        let after = card_at(&w, "DCIM/100/x.cr3").unwrap();
        assert_eq!(after.partial, before.partial, "what was read stands");
        assert!(after.partial.is_some());
    }

    #[test]
    fn it_reads_in_the_notes_order() {
        let w = world();
        let mut job = job(&w);
        w.feed.on_screen(vec![w.sightings["screen"].path.clone()]);
        w.feed.plugged(VolumeId("uuid:card".into()));
        // The card's scan (its listings) comes right after the screen; the
        // reads are what this checks.
        let read: Vec<(Stage, String)> = drain(&mut job)
            .into_iter()
            .filter(|(_, r)| !r.starts_with("list "))
            .collect();
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
