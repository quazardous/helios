//! What the import window's sources hold, read off the interface thread.
//!
//! - [`Scan`] walks one source (a card from its `DCIM`, a folder with its
//!   subfolders) on a thread of its own, for the files Helios reads
//!   (`IMAGE_EXTENSIONS`). It sends each file as it is listed, then each
//!   one's glance (its smallest embedded preview and its capture time,
//!   [`helios_load::glance`], never a raw decoded), so the first
//!   thumbnails show while the rest is read.
//! - [`Prefilter`]: whether a file is probably in the Library already, a
//!   lookup on the image map first (a Library image's file seen at that
//!   path, unchanged since: known without a read), then by size, then by
//!   partial hash for a file whose size the vault holds. An import
//!   confirms it with the full hash.
//! - [`Contents`]: the cells of the grid (a RAW+JPEG pair is one), their
//!   days, ticks and counts, and the files a selection imports.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use chrono::{NaiveDate, NaiveDateTime, TimeZone};
use helios_core::{ContentHash, Image};

/// What the vault holds ([`Holdings`]): read once per scan, on its
/// thread.
pub type Known = Arc<dyn Fn() -> Holdings + Send + Sync>;

/// What the Library holds, for the prefilter: its images as (size,
/// partial hash), and where their files were seen (path, size,
/// modification time in seconds).
#[derive(Clone, Debug, Default)]
pub struct Holdings {
    pub partials: Vec<(u64, ContentHash)>,
    pub seen: Vec<(PathBuf, u64, u64)>,
}

impl From<Vec<(u64, ContentHash)>> for Holdings {
    fn from(partials: Vec<(u64, ContentHash)>) -> Self {
        Self {
            partials,
            seen: Vec::new(),
        }
    }
}

/// The prefilter's lookup on a vault: the images in the Library (their
/// flag) and their sightings.
pub fn known_of<S: helios_vault::Store + 'static>(vault: Arc<helios_vault::Vault<S>>) -> Known {
    Arc::new(move || Holdings {
        partials: vault
            .images()
            .map(|images| images.into_iter().map(|i| (i.size, i.partial)).collect())
            .unwrap_or_default(),
        seen: vault
            .all_locations()
            .map(|all| {
                all.into_iter()
                    .filter(|(_, l)| l.present())
                    .map(|(_, l)| (l.path, l.size, l.modified))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// The image map's recorder of a listing by what (`import`, `drop`), made
/// on the scan's thread; `None` when the map keeps no listing
/// (`system.map.record` is `import`) or there is no vault.
pub type MapRecorder = Arc<dyn Fn(&str) -> Option<helios_library::map::Recorder> + Send + Sync>;

/// Whether a file is probably in the Library already: a file of a size the
/// vault holds is read in part (its beginning and end), any other is not
/// read at all.
#[derive(Default)]
pub struct Prefilter {
    by_size: HashMap<u64, HashSet<ContentHash>>,
    /// Library files by path: (size, modification time).
    seen: HashMap<PathBuf, (u64, u64)>,
}

impl Prefilter {
    pub fn new(known: impl IntoIterator<Item = (u64, ContentHash)>) -> Self {
        let mut by_size: HashMap<u64, HashSet<ContentHash>> = HashMap::new();
        for (size, partial) in known {
            by_size.entry(size).or_default().insert(partial);
        }
        Self {
            by_size,
            seen: HashMap::new(),
        }
    }

    /// From what the Library holds: its partial hashes and its sightings.
    pub fn of(holdings: Holdings) -> Self {
        let mut filter = Self::new(holdings.partials);
        filter.seen = holdings
            .seen
            .into_iter()
            .map(|(path, size, modified)| (path, (size, modified)))
            .collect();
        filter
    }

    /// [`Prefilter::read`], a file of the Library seen at `path` with this
    /// size and modification time known by a lookup, without a read.
    pub fn read_at(
        &self,
        path: &Path,
        size: u64,
        modified: Option<u64>,
    ) -> (bool, Option<ContentHash>) {
        if modified.is_some_and(|m| self.seen.get(path) == Some(&(size, m))) {
            return (true, None);
        }
        self.read(path, size)
    }

    /// Whether the file at `path`, of `size` bytes, is probably known.
    pub fn holds(&self, path: &Path, size: u64) -> bool {
        self.read(path, size).0
    }

    /// Whether the file is probably known, with its partial hash when it
    /// was read to tell (a size the vault holds).
    pub fn read(&self, path: &Path, size: u64) -> (bool, Option<ContentHash>) {
        let Some(partials) = self.by_size.get(&size) else {
            return (false, None);
        };
        match helios_vault::partial_hash(path, size) {
            Ok(p) => (partials.contains(&p), Some(p)),
            Err(_) => (false, None),
        }
    }
}

/// A file a scan listed.
#[derive(Clone, Debug)]
pub struct Found {
    /// The source ticked it was found in.
    pub source: PathBuf,
    pub path: PathBuf,
    pub bytes: u64,
    /// Its modification time, in seconds since 1970.
    pub modified: Option<i64>,
    /// Probably in the Library already ([`Prefilter`]).
    pub known: bool,
}

/// What a scan sends.
pub enum Scanned {
    Found(Found),
    /// A file's glance: when it was taken, by which camera, and a small
    /// preview.
    Glance {
        path: PathBuf,
        taken: Option<NaiveDateTime>,
        camera: Option<String>,
        thumb: Option<Image>,
    },
    /// Every file listed and glanced at.
    Done,
}

/// One source read on a thread of its own; stopped when dropped.
pub struct Scan {
    receiver: Receiver<Scanned>,
    stop: Arc<AtomicBool>,
}

impl Scan {
    /// Starts reading `source`; `wake` is called (from the thread) when
    /// something was sent. With `map`, what is listed and glanced is
    /// recorded in the image map as `seen_by` (no file is read for it).
    pub fn start(
        source: PathBuf,
        known: Option<Known>,
        map: Option<(MapRecorder, &'static str)>,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (sender, receiver) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("helios-import-scan".into())
            .spawn(move || {
                let files = helios_library::import::files_of(&source).unwrap_or_default();
                let prefilter = known.map(|k| Prefilter::of(k())).unwrap_or_default();
                let mut recorder = map.and_then(|(make, seen_by)| make(seen_by));
                let mut listed = Vec::with_capacity(files.len());
                for path in files {
                    if stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(meta) = std::fs::metadata(&path) else {
                        continue;
                    };
                    let bytes = meta.len();
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .and_then(|d| i64::try_from(d.as_secs()).ok());
                    let at = modified.and_then(|m| u64::try_from(m).ok());
                    let (known, partial) = prefilter.read_at(&path, bytes, at);
                    if let Some(r) = recorder.as_mut() {
                        let at = modified.and_then(|m| u64::try_from(m).ok()).unwrap_or(0);
                        r.listed(&path, bytes, at, partial);
                    }
                    let found = Found {
                        source: source.clone(),
                        path: path.clone(),
                        bytes,
                        modified,
                        known,
                    };
                    if sender.send(Scanned::Found(found)).is_err() {
                        return;
                    }
                    listed.push(path);
                }
                wake();
                if let Some(r) = recorder.as_mut() {
                    r.flush();
                }
                for path in listed {
                    if stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    let (taken, camera, thumb) = match helios_load::glance(&path) {
                        Ok(g) => (
                            g.taken.as_deref().and_then(exif_time),
                            g.camera,
                            g.preview.as_ref().map(crate::library::small),
                        ),
                        Err(e) => {
                            log::debug!("import scan: {e}");
                            (None, None, None)
                        }
                    };
                    if let Some(r) = recorder.as_mut() {
                        // The camera's local time, kept as if it were UTC.
                        let at = taken.map(|t| t.and_utc().timestamp());
                        r.glanced(&path, at, camera.clone());
                    }
                    let glance = Scanned::Glance {
                        path,
                        taken,
                        camera,
                        thumb,
                    };
                    if sender.send(glance).is_err() {
                        return;
                    }
                    wake();
                }
                if let Some(r) = recorder.take() {
                    r.finish();
                }
                let _ = sender.send(Scanned::Done);
                wake();
            });
        if let Err(e) = spawned {
            log::warn!("import scan: no thread: {e}");
        }
        Self { receiver, stop }
    }

    /// What was sent since the last call.
    pub fn take(&self) -> Vec<Scanned> {
        self.receiver.try_iter().collect()
    }
}

impl Drop for Scan {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// An Exif time (`YYYY:MM:DD HH:MM:SS`, the camera's local time).
pub fn exif_time(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text.trim(), "%Y:%m:%d %H:%M:%S").ok()
}

/// Which files the grid shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Types {
    #[default]
    All,
    Raw,
    Jpeg,
}

fn is_jpeg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg"))
}

fn is_raw(path: &Path) -> bool {
    helios_load::Kind::of(path) == helios_load::Kind::Raw
}

/// A cell of the grid: a file, or a RAW+JPEG pair (same stem, same folder).
#[derive(Default)]
pub struct Cell {
    pub source: PathBuf,
    pub raw: Option<PathBuf>,
    /// The picture: a JPEG (a pair's), or a PNG, TIFF…
    pub picture: Option<PathBuf>,
    pub bytes: u64,
    pub modified: Option<i64>,
    pub taken: Option<NaiveDateTime>,
    /// The camera's model, when a glance found it.
    pub camera: Option<String>,
    known: [bool; 2],
    pub ticked: bool,
    /// The preview read, until it becomes a texture.
    pub thumb: Option<Image>,
    pub texture: Option<egui::TextureHandle>,
}

impl Cell {
    pub fn is_pair(&self) -> bool {
        self.raw.is_some() && self.picture.is_some()
    }

    /// Probably in the Library already: every file of it.
    pub fn known(&self) -> bool {
        (self.raw.is_none() || self.known[0]) && (self.picture.is_none() || self.known[1])
    }

    pub fn name(&self) -> String {
        let main = self.raw.as_ref().or(self.picture.as_ref());
        let name = |p: &PathBuf| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        match (&self.raw, &self.picture) {
            (Some(r), Some(p)) => format!("{} + {}", name(r), name(p)),
            _ => main.map(name).unwrap_or_default(),
        }
    }

    /// When it was taken, else when its file was last changed (local time).
    pub fn time(&self) -> Option<NaiveDateTime> {
        self.taken.or_else(|| {
            let s = self.modified?;
            chrono::Local
                .timestamp_opt(s, 0)
                .single()
                .map(|t| t.naive_local())
        })
    }

    pub fn day(&self) -> Option<NaiveDate> {
        self.time().map(|t| t.date())
    }

    /// Whether the type filter shows it.
    pub fn passes(&self, types: Types) -> bool {
        match types {
            Types::All => true,
            Types::Raw => self.raw.is_some(),
            Types::Jpeg => self.picture.as_deref().is_some_and(is_jpeg),
        }
    }

    /// The files it imports under `types`.
    fn files(&self, types: Types) -> Vec<PathBuf> {
        match types {
            Types::All => self.raw.iter().chain(&self.picture).cloned().collect(),
            Types::Raw => self.raw.iter().cloned().collect(),
            Types::Jpeg => self.picture.iter().cloned().collect(),
        }
    }
}

/// The grid's cells, as the scans fill them.
#[derive(Default)]
pub struct Contents {
    pub cells: Vec<Cell>,
    /// A cell by (folder, stem) for pairs, or by path.
    by_key: HashMap<PathBuf, usize>,
    by_path: HashMap<PathBuf, usize>,
    pub types: Types,
    /// Images already in the Library are greyed and left out.
    pub new_only: bool,
    /// The cells shown, by day, in time order; rebuilt when stale.
    order: Option<Vec<(Option<NaiveDate>, Vec<usize>)>>,
}

/// The counts the line above the grid says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub photos: usize,
    pub new: usize,
    pub bytes: u64,
    /// The cells an Import takes, and their size.
    pub selected: usize,
    pub selected_bytes: u64,
}

impl Contents {
    pub fn new() -> Self {
        Self {
            new_only: true,
            ..Self::default()
        }
    }

    fn key(path: &Path) -> PathBuf {
        if is_raw(path) || is_jpeg(path) {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            path.with_file_name(format!("{stem}\u{0}pair"))
        } else {
            path.to_path_buf()
        }
    }

    /// Takes in a file found: the cell it went to.
    pub fn add(&mut self, found: Found) -> usize {
        let key = Self::key(&found.path);
        let raw = is_raw(&found.path);
        // A cell of the same stem waiting for this kind of file, else a
        // new one (two files of one kind under one stem, `a.jpg` and
        // `a.jpeg`, are a cell each).
        let free = self.by_key.get(&key).copied().filter(|&i| {
            let c = &self.cells[i];
            if raw {
                c.raw.is_none()
            } else {
                c.picture.is_none()
            }
        });
        let at = free.unwrap_or_else(|| {
            self.cells.push(Cell {
                source: found.source.clone(),
                ticked: true,
                ..Cell::default()
            });
            self.by_key.insert(key, self.cells.len() - 1);
            self.cells.len() - 1
        });
        self.fill(at, found);
        at
    }

    fn fill(&mut self, at: usize, found: Found) {
        let cell = &mut self.cells[at];
        let slot = usize::from(!is_raw(&found.path));
        if slot == 0 {
            cell.raw = Some(found.path.clone());
        } else {
            cell.picture = Some(found.path.clone());
        }
        cell.known[slot] = found.known;
        cell.bytes += found.bytes;
        cell.modified = match (cell.modified, found.modified) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.by_path.insert(found.path, at);
        self.order = None;
    }

    /// A file's glance: its time, its camera and, the raw's first, its
    /// preview.
    pub fn glance(
        &mut self,
        path: &Path,
        taken: Option<NaiveDateTime>,
        camera: Option<String>,
        thumb: Option<Image>,
    ) {
        let Some(&at) = self.by_path.get(path) else {
            return;
        };
        let cell = &mut self.cells[at];
        let raw = is_raw(path);
        if camera.is_some() && (raw || cell.camera.is_none()) {
            cell.camera = camera;
        }
        if taken.is_some() && (raw || cell.taken.is_none()) {
            cell.taken = taken;
            self.order = None;
        }
        if thumb.is_some() && (cell.thumb.is_none() && cell.texture.is_none() || raw) {
            cell.thumb = thumb;
            cell.texture = None;
        }
    }

    /// Removes the cells of the sources not in `sources`.
    pub fn keep_sources(&mut self, sources: &[PathBuf]) {
        if self.cells.iter().all(|c| sources.contains(&c.source)) {
            return;
        }
        let cells = std::mem::take(&mut self.cells);
        self.by_key.clear();
        self.by_path.clear();
        for cell in cells.into_iter().filter(|c| sources.contains(&c.source)) {
            let i = self.cells.len();
            for p in cell.raw.iter().chain(&cell.picture) {
                self.by_key.insert(Self::key(p), i);
                self.by_path.insert(p.clone(), i);
            }
            self.cells.push(cell);
        }
        self.order = None;
    }

    pub fn set_types(&mut self, types: Types) {
        if self.types != types {
            self.types = types;
            self.order = None;
        }
    }

    /// Whether an Import takes the cell `i` when it is ticked.
    pub fn importable(&self, i: usize) -> bool {
        let c = &self.cells[i];
        c.passes(self.types) && !(self.new_only && c.known())
    }

    /// The cells shown, by day (oldest first), each day in time order.
    pub fn days(&mut self) -> &[(Option<NaiveDate>, Vec<usize>)] {
        if self.order.is_none() {
            let mut days: BTreeMap<Option<NaiveDate>, Vec<usize>> = BTreeMap::new();
            for (i, c) in self.cells.iter().enumerate() {
                if c.passes(self.types) {
                    days.entry(c.day()).or_default().push(i);
                }
            }
            let cells = &self.cells;
            let order = days
                .into_iter()
                .map(|(day, mut v)| {
                    v.sort_by_key(|&i| (cells[i].time(), cells[i].name()));
                    (day, v)
                })
                .collect();
            self.order = Some(order);
        }
        self.order.as_deref().unwrap_or_default()
    }

    /// The cells shown, in the grid's order.
    pub fn shown(&mut self) -> Vec<usize> {
        self.days()
            .iter()
            .flat_map(|(_, v)| v.iter().copied())
            .collect()
    }

    pub fn counts(&self) -> Counts {
        let mut n = Counts::default();
        for (i, c) in self.cells.iter().enumerate() {
            if !c.passes(self.types) {
                continue;
            }
            n.photos += 1;
            n.bytes += c.bytes;
            if !c.known() {
                n.new += 1;
            }
            if c.ticked && self.importable(i) {
                n.selected += 1;
                n.selected_bytes += c.bytes;
            }
        }
        n
    }

    /// Ticks or unticks every cell shown.
    pub fn tick_all(&mut self, on: bool) {
        let types = self.types;
        for c in self.cells.iter_mut().filter(|c| c.passes(types)) {
            c.ticked = on;
        }
    }

    /// The files an Import takes from `source`, in the grid's order.
    pub fn selected_files(&mut self, source: &Path) -> Vec<PathBuf> {
        let types = self.types;
        self.shown()
            .into_iter()
            .filter(|&i| {
                self.cells[i].source == source && self.cells[i].ticked && self.importable(i)
            })
            .flat_map(|i| self.cells[i].files(types))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("helios-scan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_prefilter_knows_a_file_by_size_and_partial_hash() {
        let dir = temp("prefilter");
        // Larger than the 64 KiB read at each end: the middle is not read.
        let mut content = vec![7u8; 200_000];
        let known = dir.join("known.raw");
        std::fs::write(&known, &content).unwrap();
        let size = content.len() as u64;
        let filter = Prefilter::new([(size, helios_vault::partial_hash(&known, size).unwrap())]);
        // The same bytes elsewhere: known.
        let copy = dir.join("copy.raw");
        std::fs::write(&copy, &content).unwrap();
        assert!(filter.holds(&copy, size));
        // The same size, another end: not known.
        content[size as usize - 1] = 8;
        let other = dir.join("other.raw");
        std::fs::write(&other, &content).unwrap();
        assert!(!filter.holds(&other, size));
        // Another size is never read: a file that is not there is not known.
        assert!(!filter.holds(&dir.join("missing.raw"), size + 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_prefilter_knows_a_library_file_by_lookup_without_reading_it() {
        let dir = temp("prefilter-lookup");
        // Not on disk: a read would fail; the map knows it.
        let path = dir.join("seen.raw");
        let filter = Prefilter::of(Holdings {
            partials: Vec::new(),
            seen: vec![(path.clone(), 1_000, 42)],
        });
        assert_eq!(filter.read_at(&path, 1_000, Some(42)), (true, None));
        // Changed since (another time or size): not known by the lookup.
        assert!(!filter.read_at(&path, 1_000, Some(43)).0);
        assert!(!filter.read_at(&path, 999, Some(42)).0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn found(path: &str, known: bool, modified: i64) -> Found {
        Found {
            source: PathBuf::from("/card"),
            path: PathBuf::from(path),
            bytes: 10,
            modified: Some(modified),
            known,
        }
    }

    #[test]
    fn a_raw_and_its_jpeg_are_one_cell_and_import_both() {
        let mut c = Contents::new();
        c.add(found("/card/DCIM/IMG_1.CR2", false, 100));
        c.add(found("/card/DCIM/IMG_1.JPG", false, 100));
        c.add(found("/card/DCIM/IMG_2.JPG", true, 100));
        assert_eq!(c.cells.len(), 2);
        assert!(c.cells[0].is_pair());
        let n = c.counts();
        assert_eq!((n.photos, n.new, n.selected), (2, 1, 1));
        assert_eq!(
            c.selected_files(Path::new("/card")),
            [
                PathBuf::from("/card/DCIM/IMG_1.CR2"),
                PathBuf::from("/card/DCIM/IMG_1.JPG")
            ]
        );
        c.set_types(Types::Raw);
        assert_eq!(
            c.selected_files(Path::new("/card")),
            [PathBuf::from("/card/DCIM/IMG_1.CR2")]
        );
        c.set_types(Types::All);
        c.new_only = false;
        assert_eq!(c.counts().selected, 2);
    }
}
