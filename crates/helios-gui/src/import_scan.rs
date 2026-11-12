//! What the import window's sources hold, read off the interface thread.
//!
//! - [`Connectors`]: the import connectors of the registry
//!   ([`helios_connect::registry`]), created once for the window: the card
//!   connector given the cards as last polled, the folder connector the
//!   folders the window keeps. The Sources column lists their sources.
//! - [`Scan`] lists one source through its connector (a card from its
//!   `DCIM`, a folder with its subfolders), page by page on a thread of
//!   its own, for the files Helios reads (`IMAGE_EXTENSIONS`), each read
//!   in place. It sends each file as it is listed, then each
//!   one's glance (its smallest embedded preview at the cell's size and its
//!   capture time, [`helios_load::glance_at`], never a raw decoded), read
//!   on a few threads ([`glancers`]), the files the window wants first
//!   (the cells in view, then the next screen), then the rest in order.
//! - [`GlanceCache`]: the glances kept in RAM by path, size and
//!   modification time (the cache service's `import.glance`, with a
//!   budget), so a window opened again or scrolled back reads nothing; a
//!   card's also on disk by the card's volume, the path on it, size and
//!   modification time, so a card plugged in again (after a restart, at
//!   another mount point) shows its glances without a read.
//! - [`Prefilter`]: whether a file is probably in the Library already, a
//!   lookup on the image map first (a Library image's file seen at that
//!   path, unchanged since: known without a read), then by size, then by
//!   partial hash for a file whose size the vault holds. An import
//!   confirms it with the full hash.
//! - [`Contents`]: the cells of the grid (a RAW+JPEG pair is one), their
//!   days, ticks and counts, and the files a selection imports.
//! - [`Recorded`]: what the image map recorded under a folder no volume
//!   holds now (a card or a disk offline), listed by [`Scan::recorded`]
//!   as the files were last seen, each said imported or not.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use chrono::{NaiveDate, NaiveDateTime, TimeZone};
use egui::ColorImage;
use helios_cache::{Counters, Disk, IMPORT_GLANCE, Scope, Service, Store, Tier, Usage};
use helios_connect::{
    Access, Cancel, Connector, ConnectorError, ConnectorServices, Cursor, Item, ItemRef, PAGE,
    Settings, SourceRef,
};
use helios_core::ContentHash;
use helios_storage::Card;
use image::ImageEncoder;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

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
    /// The thumbnail cache entries of the files seen: their images'.
    pub keys: HashMap<PathBuf, helios_thumbs::Key>,
}

impl From<Vec<(u64, ContentHash)>> for Holdings {
    fn from(partials: Vec<(u64, ContentHash)>) -> Self {
        Self {
            partials,
            ..Self::default()
        }
    }
}

/// The prefilter's lookup on a vault: the images in the Library (their
/// flag) and their sightings.
pub fn known_of<S: helios_vault::Store + 'static>(vault: Arc<helios_vault::Vault<S>>) -> Known {
    Arc::new(move || {
        let present: Vec<_> = vault
            .all_locations()
            .map(|all| all.into_iter().filter(|(_, l)| l.present()).collect())
            .unwrap_or_default();
        Holdings {
            partials: vault
                .images()
                .map(|images| images.into_iter().map(|i| (i.size, i.partial)).collect())
                .unwrap_or_default(),
            keys: present
                .iter()
                .map(|(id, l)| (l.path.clone(), helios_thumbs::Key::preview(id.hex())))
                .collect(),
            seen: present
                .into_iter()
                .map(|(_, l)| (l.path, l.size, l.modified))
                .collect(),
        }
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
    keys: HashMap<PathBuf, helios_thumbs::Key>,
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
            keys: HashMap::new(),
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
        filter.keys = holdings.keys;
        filter
    }

    /// The thumbnail cache entry of a Library file seen at `path` with
    /// this size and modification time, unchanged since.
    pub fn key_at(
        &self,
        path: &Path,
        size: u64,
        modified: Option<u64>,
    ) -> Option<helios_thumbs::Key> {
        modified
            .filter(|m| self.seen.get(path) == Some(&(size, *m)))
            .and_then(|_| self.keys.get(path).cloned())
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
    /// Listed from the image map's record, its volume offline: shown,
    /// never imported.
    pub offline: bool,
    /// Its thumbnail's entry in the library's thumbnail cache, when the
    /// image map links it to one.
    pub key: Option<helios_thumbs::Key>,
}

/// A file the image map recorded under a folder, as last seen.
#[derive(Clone, Debug)]
pub struct Seen {
    pub path: PathBuf,
    pub bytes: u64,
    /// Its modification time when listed, in seconds since 1970.
    pub modified: Option<i64>,
    /// Its capture time (the camera's local time), once glanced.
    pub taken: Option<NaiveDateTime>,
    pub camera: Option<String>,
    /// Its content is an image of the Library.
    pub imported: bool,
    pub key: Option<helios_thumbs::Key>,
}

/// What the image map recorded under a folder (`docs/design/image-map.md`):
/// Helios's record of every image it saw on every known volume, read when
/// the folder's volume is offline. Called on the scan's thread.
pub type Recorded = Arc<dyn Fn(&Path) -> Vec<Seen> + Send + Sync>;

/// The record of a vault: the sightings under `folder`, on the known
/// volume last mounted above it, else on no volume.
pub fn recorded_of<S: helios_vault::Store + 'static>(
    vault: Arc<helios_vault::Vault<S>>,
) -> Recorded {
    Arc::new(move |folder: &Path| {
        let volumes = helios_storage::registry::known(&vault).unwrap_or_default();
        let on = volumes
            .iter()
            .filter_map(|v| {
                let mount = v.last_seen.as_ref()?.mount.as_ref()?;
                let rel = folder.strip_prefix(mount).ok()?;
                Some((v, mount.clone(), rel.to_path_buf()))
            })
            .max_by_key(|(_, mount, _)| mount.components().count());
        let (volume, mount, rel) = match on {
            Some((v, mount, rel)) => (Some(v.id.clone()), Some(mount), rel),
            None => (None, None, folder.to_path_buf()),
        };
        vault
            .sightings_under(volume.as_ref(), &rel)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| {
                s.path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| {
                        helios_vault::IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())
                    })
            })
            .map(|s| {
                let path = match (&mount, &s.rel_path) {
                    (Some(m), Some(r)) => m.join(r),
                    _ => s.path.clone(),
                };
                Seen {
                    key: helios_library::thumb_check::key_of(&s),
                    imported: s.image.is_some(),
                    bytes: s.size,
                    modified: i64::try_from(s.modified).ok(),
                    taken: s
                        .taken
                        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                        .map(|t| t.naive_utc()),
                    camera: s.camera,
                    path,
                }
            })
            .collect()
    })
}

/// Every item of a source, its pages listed one after the other.
fn list_all(
    connector: &dyn Connector,
    source: &SourceRef,
    cancel: &Cancel,
) -> Result<Vec<Item>, ConnectorError> {
    let mut items = Vec::new();
    let mut from: Option<Cursor> = None;
    loop {
        let page = connector.list(source, from.as_ref(), PAGE, cancel)?;
        items.extend(page.items);
        match page.next {
            Some(next) if !page.complete => from = Some(next),
            _ => return Ok(items),
        }
    }
}

/// The import connectors of the registry, created once with what the
/// window gives them: the cards as last polled (never a poll of its own
/// on the interface thread) and the folders it keeps.
pub struct Connectors {
    /// By id, in the registry's order: cards, then folders.
    by_id: Vec<(&'static str, Arc<dyn Connector>)>,
    cards: Arc<Mutex<Vec<Card>>>,
    places: Arc<Mutex<Vec<PathBuf>>>,
}

impl Default for Connectors {
    fn default() -> Self {
        let cards: Arc<Mutex<Vec<Card>>> = Arc::default();
        let places: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
        let (c, p) = (cards.clone(), places.clone());
        let services = ConnectorServices {
            cards: Some(Arc::new(move || lock(&c).clone())),
            places: Some(Arc::new(move || lock(&p).clone())),
        };
        let by_id = helios_connect::registry()
            .into_iter()
            .filter_map(
                |(id, provider)| match provider.create(&Settings::Null, &services) {
                    Ok(c) => Some((id, Arc::<dyn Connector>::from(c))),
                    Err(e) => {
                        log::warn!("import connector {id}: {e}");
                        None
                    }
                },
            )
            .collect();
        Self {
            by_id,
            cards,
            places,
        }
    }
}

impl Connectors {
    /// The cards mounted, as the window last polled them.
    pub fn set_cards(&self, cards: &[Card]) {
        *lock(&self.cards) = cards.to_vec();
    }

    /// The folders the window keeps (recent, favourites).
    pub fn set_places(&self, places: Vec<PathBuf>) {
        *lock(&self.places) = places;
    }

    /// Every connector's sources now, in the registry's order, each with
    /// whether its connector detects them (cards) or was given them.
    pub fn sources(&self) -> Vec<(SourceRef, bool)> {
        self.by_id
            .iter()
            .flat_map(|(_, c)| {
                let detect = c.describe().caps.detect;
                c.sources(None)
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |s| (s, detect))
            })
            .collect()
    }

    fn get(&self, id: &str) -> Option<Arc<dyn Connector>> {
        self.by_id
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, c)| c.clone())
    }

    /// The connector that lists the source at `path` and the source it
    /// is: a card's when a card is mounted there, else a folder's.
    pub fn for_path(&self, path: &Path) -> Option<(Arc<dyn Connector>, SourceRef)> {
        let card = lock(&self.cards).iter().find(|c| c.root == path).cloned();
        match card {
            Some(card) => Some((
                self.get(helios_connect::card::ID)?,
                helios_connect::card::CardConnector::source(&card),
            )),
            None => Some((
                self.get(helios_connect::folder::ID)?,
                helios_connect::folder::Folder::source(path),
            )),
        }
    }
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
        /// Its preview, an sRGB picture at most [`crate::library::THUMB`]
        /// pixels on its long side.
        thumb: Option<ColorImage>,
    },
    /// Every file listed and glanced at.
    Done,
}

/// One source read on a thread of its own; stopped when dropped.
pub struct Scan {
    receiver: Receiver<Scanned>,
    stop: Arc<AtomicBool>,
    stats: Arc<Stats>,
    /// What it reads now: the folder while it lists it, then the file
    /// last glanced at.
    reading: Arc<Mutex<PathBuf>>,
}

/// The files the window wants glanced first, in order: the cells in view,
/// then the next screen. Shared by the window and its scans.
pub type Wanted = Arc<Mutex<Vec<PathBuf>>>;

/// How many threads glance at a source's files: half the cores, two to
/// four (the library's pool reads on as many).
pub fn glancers() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 4))
}

/// What a scan's glances did.
#[derive(Debug, Default)]
struct Stats {
    /// Files read (a preview decoded).
    read: AtomicUsize,
    /// Files answered from the [`GlanceCache`].
    cached: AtomicUsize,
    /// Of those, the ones from its disk tier.
    from_disk: AtomicUsize,
    busy: AtomicUsize,
    /// The most files read at once.
    peak: AtomicUsize,
}

/// What a scan's glances did, so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Glances {
    /// Files read (a preview decoded).
    pub read: usize,
    /// Files answered from the [`GlanceCache`], nothing read.
    pub cached: usize,
    /// Of those, the ones from its disk tier (a card seen before).
    pub from_disk: usize,
    /// The most files read at once.
    pub peak: usize,
}

impl std::ops::Add for Glances {
    type Output = Glances;
    fn add(self, o: Glances) -> Glances {
        Glances {
            read: self.read + o.read,
            cached: self.cached + o.cached,
            from_disk: self.from_disk + o.from_disk,
            peak: self.peak.max(o.peak),
        }
    }
}

/// The listing's files not yet glanced at: the next is the first the
/// window wants, else the first in order.
struct Picking {
    index: HashMap<PathBuf, usize>,
    taken: Vec<bool>,
    next: usize,
}

impl Picking {
    /// The files listed, by their paths in order.
    fn new<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> Self {
        let index: HashMap<PathBuf, usize> = paths
            .into_iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), i))
            .collect();
        Self {
            taken: vec![false; index.len()],
            index,
            next: 0,
        }
    }

    fn pick(&mut self, wanted: &[PathBuf]) -> Option<usize> {
        let first = wanted
            .iter()
            .filter_map(|p| self.index.get(p).copied())
            .find(|&i| !self.taken[i]);
        let at = first.or_else(|| {
            while self.next < self.taken.len() && self.taken[self.next] {
                self.next += 1;
            }
            (self.next < self.taken.len()).then_some(self.next)
        })?;
        self.taken[at] = true;
        Some(at)
    }
}

/// A file's glance, read or from the cache: its RAM tier, then, for a
/// card's file (`on_disk`, its key there), its disk tier.
fn glance_one(
    path: &Path,
    bytes: u64,
    modified: Option<i64>,
    on_disk: Option<&str>,
    stats: &Stats,
) -> Arc<Glanced> {
    let cache = GlanceCache::global();
    let key = (path.to_path_buf(), bytes, modified);
    if let Some(hit) = cache.get(&key) {
        stats.cached.fetch_add(1, Ordering::Relaxed);
        return hit;
    }
    if let Some(hit) = on_disk.and_then(|k| cache.disk_glance(k, key.clone())) {
        stats.cached.fetch_add(1, Ordering::Relaxed);
        stats.from_disk.fetch_add(1, Ordering::Relaxed);
        return hit;
    }
    let busy = stats.busy.fetch_add(1, Ordering::SeqCst) + 1;
    stats.peak.fetch_max(busy, Ordering::SeqCst);
    let started = std::time::Instant::now();
    let read = helios_load::glance_at(path, Some(crate::library::THUMB));
    // A file that could not be read (perhaps a card's read error) is not
    // kept on disk: read again next time.
    let on_disk = on_disk.filter(|_| read.is_ok());
    let glanced = match read {
        Ok(g) => Glanced {
            taken: g.taken.as_deref().and_then(exif_time),
            camera: g.camera,
            thumb: g.preview.as_ref().map(crate::library::thumbnail),
        },
        Err(e) => {
            log::debug!("import scan: {e}");
            Glanced::default()
        }
    };
    stats.busy.fetch_sub(1, Ordering::SeqCst);
    stats.read.fetch_add(1, Ordering::Relaxed);
    let glanced = Arc::new(glanced);
    cache.put(key, glanced.clone(), started.elapsed());
    if let Some(k) = on_disk {
        cache.keep_on_disk(k, &glanced);
    }
    glanced
}

/// The disk key of a card's file: the card's volume, the file's path on
/// it, its size and modification time. The same card mounted elsewhere,
/// or after a restart, finds it; a file changed has another.
pub fn disk_key(volume: &str, on_card: &Path, bytes: u64, modified: Option<i64>) -> String {
    let path = on_card.to_string_lossy();
    let modified = modified.map_or_else(String::new, |m| m.to_string());
    format!("{volume}\n{path}\n{bytes}\n{modified}")
}

/// The volume a scanned source is and its mount point, for a card: the
/// key its glances are kept under on disk.
fn card_volume(source: &SourceRef) -> Option<(String, PathBuf)> {
    if source.connector != helios_connect::card::ID {
        return None;
    }
    let root = source.root.clone()?;
    let card = Card {
        label: source.label.clone(),
        root: root.clone(),
        capacity: source.capacity,
    };
    Some((card.volume().id.0, root))
}

/// A glance as the bytes of a disk entry: a magic, flags, the capture
/// time, the camera, the preview's size and the preview as a JPEG.
fn encode_glance(g: &Glanced) -> Vec<u8> {
    let mut out = GLANCE_MAGIC.to_vec();
    let flags = u8::from(g.taken.is_some())
        | u8::from(g.camera.is_some()) << 1
        | u8::from(g.thumb.is_some()) << 2;
    out.push(flags);
    if let Some(t) = g.taken {
        out.extend_from_slice(&t.and_utc().timestamp().to_le_bytes());
    }
    if let Some(c) = &g.camera {
        let c = &c.as_bytes()[..c.len().min(usize::from(u16::MAX))];
        out.extend_from_slice(&(c.len() as u16).to_le_bytes());
        out.extend_from_slice(c);
    }
    if let Some(t) = &g.thumb {
        let rgb: Vec<u8> = t
            .pixels
            .iter()
            .flat_map(|p| [p.r(), p.g(), p.b()])
            .collect();
        let (w, h) = (t.size[0] as u32, t.size[1] as u32);
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88);
        if encoder
            .write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
            .is_err()
        {
            // The flags say a preview: an entry that cannot be read back
            // is a miss, read again.
            return Vec::new();
        }
    }
    out
}

/// What a disk entry says before its preview: its flags, capture time,
/// camera, the preview's size and where its JPEG starts.
struct GlanceHeader {
    taken: Option<NaiveDateTime>,
    camera: Option<String>,
    size: Option<[usize; 2]>,
    jpeg: usize,
}

fn glance_header(bytes: &[u8]) -> Option<GlanceHeader> {
    let rest = bytes.strip_prefix(GLANCE_MAGIC)?;
    let (&flags, mut rest) = rest.split_first()?;
    let mut take = |n: usize| -> Option<&[u8]> {
        let (head, tail) = rest.split_at_checked(n)?;
        rest = tail;
        Some(head)
    };
    let taken = if flags & 1 != 0 {
        let secs = i64::from_le_bytes(take(8)?.try_into().ok()?);
        Some(chrono::DateTime::from_timestamp(secs, 0)?.naive_utc())
    } else {
        None
    };
    let camera = if flags & 2 != 0 {
        let len = u16::from_le_bytes(take(2)?.try_into().ok()?);
        Some(String::from_utf8_lossy(take(usize::from(len))?).into_owned())
    } else {
        None
    };
    let size = if flags & 4 != 0 {
        let w = u32::from_le_bytes(take(4)?.try_into().ok()?);
        let h = u32::from_le_bytes(take(4)?.try_into().ok()?);
        Some([w as usize, h as usize])
    } else {
        None
    };
    let jpeg = bytes.len() - rest.len();
    Some(GlanceHeader {
        taken,
        camera,
        size,
        jpeg,
    })
}

/// A disk entry's glance back; `None` for one that cannot be read.
fn decode_glance(bytes: &[u8]) -> Option<Glanced> {
    let header = glance_header(bytes)?;
    let thumb = match header.size {
        None => None,
        Some(size) => {
            let img = image::load_from_memory_with_format(
                &bytes[header.jpeg..],
                image::ImageFormat::Jpeg,
            )
            .ok()?
            .to_rgba8();
            if [img.width() as usize, img.height() as usize] != size {
                return None;
            }
            Some(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
        }
    };
    Some(Glanced {
        taken: header.taken,
        camera: header.camera,
        thumb,
    })
}

/// How good a glance is, to keep the better of two: its preview's pixels,
/// then whether it has a capture time and a camera.
fn glance_rank(size: Option<[usize; 2]>, taken: bool, camera: bool) -> (usize, bool, bool) {
    (size.map_or(0, |[w, h]| w * h), taken, camera)
}

/// The start of every glance entry on disk.
const GLANCE_MAGIC: &[u8; 4] = b"HGL2";

/// The disk budget of the glances: 256 MiB, about ten thousand previews.
pub const GLANCE_DISK_BUDGET: u64 = 256 << 20;

/// What a glance found: when a file was taken, by which camera, and its
/// preview at the cell's size.
#[derive(Clone, Debug, Default)]
pub struct Glanced {
    pub taken: Option<NaiveDateTime>,
    pub camera: Option<String>,
    pub thumb: Option<ColorImage>,
}

impl Glanced {
    fn bytes(&self) -> u64 {
        let pixels = self.thumb.as_ref().map_or(0, |t| t.pixels.len());
        (pixels * 4 + 128) as u64
    }
}

/// A glance's key: the file's path, size and modification time.
type GlanceKey = (PathBuf, u64, Option<i64>);

/// The RAM budget of the glances: 192 MiB, about a thousand previews.
pub const GLANCE_BUDGET: u64 = 192 << 20;

#[derive(Default)]
struct GlanceIndex {
    entries: HashMap<GlanceKey, (Arc<Glanced>, u64)>,
    clock: u64,
    bytes: u64,
}

/// The import window's glances, kept in RAM within a budget (the cache
/// service's kind `import.glance`), least recently used first out. Keyed
/// by path, size and modification time: a file changed is read again.
pub struct GlanceCache {
    index: Mutex<GlanceIndex>,
    budget: u64,
    counters: Arc<Counters>,
    /// Its disk tier, once opened ([`GlanceCache::open_disk`]): the
    /// cards' glances.
    disk: Mutex<Option<Arc<Disk>>>,
}

impl GlanceCache {
    pub fn new(budget: u64) -> Self {
        Self {
            index: Mutex::default(),
            budget,
            counters: Service::global().counters(&Scope::Shared, &IMPORT_GLANCE),
            disk: Mutex::default(),
        }
    }

    /// Opens its disk tier under `root` (Helios's user cache folder),
    /// attached to the cache service: what was not used for the kind's
    /// age limit is pruned. Lists a folder: never on the interface thread.
    /// Until it is opened, glances are kept in RAM only.
    ///
    /// # Errors
    /// The folder cannot be made.
    pub fn open_disk(&self, root: &Path) -> std::io::Result<()> {
        let disk = Arc::new(Disk::open(
            root,
            IMPORT_GLANCE,
            Scope::Shared,
            GLANCE_DISK_BUDGET,
            std::time::SystemTime::now(),
        )?);
        disk.attach();
        *lock(&self.disk) = Some(disk);
        Ok(())
    }

    /// Its disk tier, once opened.
    pub fn disk(&self) -> Option<Arc<Disk>> {
        lock(&self.disk).clone()
    }

    /// Forgets its RAM tier, the disk tier kept: what a restart leaves.
    pub fn forget_ram(&self) {
        let mut index = lock(&self.index);
        index.entries.clear();
        index.bytes = 0;
    }

    /// The glance kept on disk under `on_disk`, put in RAM under `key`.
    fn disk_glance(&self, on_disk: &str, key: GlanceKey) -> Option<Arc<Glanced>> {
        let disk = self.disk()?;
        let bytes = disk.get(on_disk.as_bytes())?;
        let Some(glanced) = decode_glance(&bytes) else {
            disk.remove(on_disk.as_bytes());
            return None;
        };
        let glanced = Arc::new(glanced);
        self.put(key, glanced.clone(), std::time::Duration::ZERO);
        Some(glanced)
    }

    /// Keeps `glanced` on disk under `on_disk`; an entry already there is
    /// replaced only by a better one (a larger preview, a capture time or
    /// a camera it lacks).
    fn keep_on_disk(&self, on_disk: &str, glanced: &Glanced) {
        let Some(disk) = self.disk() else { return };
        let key = on_disk.as_bytes();
        if let Some(old) = disk.peek(key).as_deref().and_then(glance_header) {
            let rank_old = glance_rank(old.size, old.taken.is_some(), old.camera.is_some());
            let rank_new = glance_rank(
                glanced.thumb.as_ref().map(|t| t.size),
                glanced.taken.is_some(),
                glanced.camera.is_some(),
            );
            if rank_new <= rank_old {
                return;
            }
        }
        let bytes = encode_glance(glanced);
        if bytes.is_empty() {
            return;
        }
        if let Err(e) = disk.put(key, &bytes) {
            log::debug!("import glances on disk: {e}");
        }
    }

    /// The process's, attached to the cache service.
    pub fn global() -> &'static Arc<GlanceCache> {
        static GLOBAL: OnceLock<Arc<GlanceCache>> = OnceLock::new();
        GLOBAL.get_or_init(|| {
            let cache = Arc::new(GlanceCache::new(GLANCE_BUDGET));
            let weak = Arc::downgrade(&cache) as std::sync::Weak<dyn Store>;
            Service::global().attach(weak);
            cache
        })
    }

    /// The glance of the file at `path` of `bytes`, changed at `modified`,
    /// when kept: what the window shows with the file's cell, on the frame
    /// that draws it (a miss is not counted: the scan asks again).
    pub fn cached(&self, path: &Path, bytes: u64, modified: Option<i64>) -> Option<Arc<Glanced>> {
        let key = (path.to_path_buf(), bytes, modified);
        let mut index = lock(&self.index);
        index.clock += 1;
        let now = index.clock;
        let (glanced, used) = index.entries.get_mut(&key)?;
        *used = now;
        let hit = glanced.clone();
        drop(index);
        Counters::add(&self.counters.hits, 1);
        Some(hit)
    }

    fn get(&self, key: &GlanceKey) -> Option<Arc<Glanced>> {
        let mut index = lock(&self.index);
        index.clock += 1;
        let now = index.clock;
        let hit = index.entries.get_mut(key).map(|(g, used)| {
            *used = now;
            g.clone()
        });
        let counter = if hit.is_some() {
            &self.counters.hits
        } else {
            &self.counters.misses
        };
        Counters::add(counter, 1);
        hit
    }

    fn put(&self, key: GlanceKey, glanced: Arc<Glanced>, took: std::time::Duration) {
        let bytes = glanced.bytes();
        Counters::add(&self.counters.bytes_written, bytes);
        Counters::add(
            &self.counters.produce_ns,
            u64::try_from(took.as_nanos()).unwrap_or(u64::MAX),
        );
        let mut index = lock(&self.index);
        index.clock += 1;
        let now = index.clock;
        if let Some((old, _)) = index.entries.insert(key, (glanced, now)) {
            index.bytes -= old.bytes();
        }
        index.bytes += bytes;
        if index.bytes > self.budget {
            // Down to 90 % at once, so a full cache does not evict at
            // every write.
            let target = helios_cache::eviction_target(self.budget);
            let mut by_age: Vec<(u64, GlanceKey)> = index
                .entries
                .iter()
                .map(|(k, (_, used))| (*used, k.clone()))
                .collect();
            by_age.sort_unstable_by_key(|(used, _)| *used);
            for (_, k) in by_age {
                if index.bytes <= target {
                    break;
                }
                if let Some((g, _)) = index.entries.remove(&k) {
                    index.bytes -= g.bytes();
                    Counters::add(&self.counters.evictions, 1);
                }
            }
        }
    }
}

impl Store for GlanceCache {
    fn usage(&self) -> Vec<Usage> {
        let index = lock(&self.index);
        vec![Usage {
            scope: Scope::Shared,
            kind: IMPORT_GLANCE,
            tier: Tier::Ram,
            entries: index.entries.len() as u64,
            bytes: index.bytes,
            pinned: 0,
            budget: Some(self.budget),
        }]
    }

    fn clear(&self, kind: Option<&str>) -> u64 {
        if kind.is_some_and(|k| k != IMPORT_GLANCE.id) {
            return 0;
        }
        let mut index = lock(&self.index);
        let freed = index.bytes;
        Counters::add(&self.counters.cleared, index.entries.len() as u64);
        index.entries.clear();
        index.bytes = 0;
        freed
    }
}

impl Scan {
    /// Starts reading `source`; `wake` is called (from the threads) when
    /// something was sent. With `map`, what is listed and glanced is
    /// recorded in the image map as `seen_by` (no file is read for it).
    /// The files in `wanted` are glanced at first.
    pub fn start(
        (connector, listed_source): (Arc<dyn Connector>, SourceRef),
        source: PathBuf,
        known: Option<Known>,
        map: Option<(MapRecorder, &'static str)>,
        wanted: Wanted,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (sender, receiver) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let stats = Arc::new(Stats::default());
        let counted = stats.clone();
        let reading = Arc::new(Mutex::new(source.clone()));
        let now_reading = reading.clone();
        let spawned = std::thread::Builder::new()
            .name("helios-import-scan".into())
            .spawn(move || {
                let cancel = Cancel::from_flag(stopped.clone());
                let items = match list_all(connector.as_ref(), &listed_source, &cancel) {
                    Ok(items) => items,
                    Err(ConnectorError::Cancelled) => return,
                    Err(e) => {
                        log::debug!("import scan: {}: {e}", source.display());
                        Vec::new()
                    }
                };
                let prefilter = known.map(|k| Prefilter::of(k())).unwrap_or_default();
                // A card's glances are kept on disk too, by its volume.
                let volume = card_volume(&listed_source);
                let mut recorder = map.and_then(|(make, seen_by)| make(seen_by));
                let mut listed = Vec::with_capacity(items.len());
                for item in items {
                    if stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    let at = ItemRef {
                        source: listed_source.clone(),
                        id: item.id,
                    };
                    // Read in place; a source fetched comes with fetching.
                    let Access::InPlace(path) = connector.access(&at) else {
                        continue;
                    };
                    let bytes = item.size.unwrap_or(0);
                    let modified = item.modified;
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
                        offline: false,
                        // A Library file: its thumbnail is in the cache.
                        key: prefilter.key_at(&path, bytes, at),
                    };
                    if sender.send(Scanned::Found(found)).is_err() {
                        return;
                    }
                    let on_disk = volume.as_ref().and_then(|(id, root)| {
                        let on_card = path.strip_prefix(root).ok()?;
                        Some(disk_key(id, on_card, bytes, modified))
                    });
                    listed.push((path, bytes, modified, on_disk));
                }
                wake();
                if let Some(r) = recorder.as_mut() {
                    r.flush();
                }
                // The glances, on a few threads, the files wanted first.
                let picking = Mutex::new(Picking::new(listed.iter().map(|f| &f.0)));
                let recorder = Mutex::new(recorder);
                let (listed, picking, recorder, wanted, wake, stopped, stats, now_reading) = (
                    &listed,
                    &picking,
                    &recorder,
                    &wanted,
                    &wake,
                    &stopped,
                    &counted,
                    &now_reading,
                );
                std::thread::scope(|threads| {
                    for _ in 0..glancers().min(listed.len()) {
                        let sender = sender.clone();
                        threads.spawn(move || {
                            loop {
                                if stopped.load(Ordering::Relaxed) {
                                    return;
                                }
                                let next = {
                                    let wanted = lock(wanted);
                                    lock(picking).pick(&wanted)
                                };
                                let Some(i) = next else { return };
                                let (path, bytes, modified, on_disk) = &listed[i];
                                lock(now_reading).clone_from(path);
                                let g =
                                    glance_one(path, *bytes, *modified, on_disk.as_deref(), stats);
                                if let Some(r) = lock(recorder).as_mut() {
                                    // The camera's local time, kept as if it
                                    // were UTC.
                                    let at = g.taken.map(|t| t.and_utc().timestamp());
                                    r.glanced(path, at, g.camera.clone());
                                }
                                let glance = Scanned::Glance {
                                    path: path.clone(),
                                    taken: g.taken,
                                    camera: g.camera.clone(),
                                    thumb: g.thumb.clone(),
                                };
                                if sender.send(glance).is_err() {
                                    return;
                                }
                                wake();
                            }
                        });
                    }
                });
                if stopped.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(r) = lock(recorder).take() {
                    r.finish();
                }
                let _ = sender.send(Scanned::Done);
                wake();
            });
        if let Err(e) = spawned {
            log::warn!("import scan: no thread: {e}");
        }
        Self {
            receiver,
            stop,
            stats,
            reading,
        }
    }

    /// What it reads now: the folder while it lists it, then the file last
    /// glanced at.
    pub fn reading(&self) -> PathBuf {
        lock(&self.reading).clone()
    }

    /// What the glances did so far: files read, from the cache, and the
    /// most read at once.
    pub fn glances(&self) -> Glances {
        Glances {
            read: self.stats.read.load(Ordering::Relaxed),
            cached: self.stats.cached.load(Ordering::Relaxed),
            from_disk: self.stats.from_disk.load(Ordering::Relaxed),
            peak: self.stats.peak.load(Ordering::Relaxed),
        }
    }

    /// Lists what the image map recorded under `source`, a folder no
    /// volume holds now: each file as last seen, offline, said imported
    /// when its content is a Library image; nothing is read from it.
    pub fn recorded(source: PathBuf, recorded: Recorded, wake: impl Fn() + Send + 'static) -> Self {
        let reading = Arc::new(Mutex::new(source.clone()));
        let (sender, receiver) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("helios-import-recorded".into())
            .spawn(move || {
                for seen in recorded(&source) {
                    if stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    let found = Found {
                        source: source.clone(),
                        path: seen.path.clone(),
                        bytes: seen.bytes,
                        modified: seen.modified,
                        known: seen.imported,
                        offline: true,
                        key: seen.key,
                    };
                    let glance = Scanned::Glance {
                        path: seen.path,
                        taken: seen.taken,
                        camera: seen.camera,
                        thumb: None,
                    };
                    if sender.send(Scanned::Found(found)).is_err() || sender.send(glance).is_err() {
                        return;
                    }
                }
                let _ = sender.send(Scanned::Done);
                wake();
            });
        if let Err(e) = spawned {
            log::warn!("import scan: no thread: {e}");
        }
        Self {
            receiver,
            stop,
            stats: Arc::default(),
            reading,
        }
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
    /// Listed from the image map's record: its volume is offline.
    pub offline: bool,
    /// Its thumbnail's entry in the library's cache, when known.
    pub key: Option<helios_thumbs::Key>,
    pub ticked: bool,
    /// The preview read, until it goes to the thumbnails.
    pub thumb: Option<ColorImage>,
    /// Its preview went to the thumbnails.
    pub previewed: bool,
}

impl Cell {
    pub fn is_pair(&self) -> bool {
        self.raw.is_some() && self.picture.is_some()
    }

    /// Probably in the Library already: every file of it.
    pub fn known(&self) -> bool {
        (self.raw.is_none() || self.known[0]) && (self.picture.is_none() || self.known[1])
    }

    /// The file it is shown by: the raw, else the picture.
    pub fn main(&self) -> Option<&PathBuf> {
        self.raw.as_ref().or(self.picture.as_ref())
    }

    pub fn name(&self) -> String {
        let main = self.main();
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
    /// Bumped whenever a cell, its facts or the cells shown change.
    revision: u64,
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
        if found.offline {
            cell.offline = true;
        }
        if found.key.is_some() {
            cell.key = found.key;
        }
        self.by_path.insert(found.path, at);
        self.order = None;
        self.revision += 1;
    }

    /// Changes whenever the cells, their facts or the type filter do.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// A file's glance: its time, its camera and, the raw's first, its
    /// preview.
    pub fn glance(
        &mut self,
        path: &Path,
        taken: Option<NaiveDateTime>,
        camera: Option<String>,
        thumb: Option<ColorImage>,
    ) {
        let Some(&at) = self.by_path.get(path) else {
            return;
        };
        self.revision += 1;
        let cell = &mut self.cells[at];
        let raw = is_raw(path);
        if camera.is_some() && (raw || cell.camera.is_none()) {
            cell.camera = camera;
        }
        if taken.is_some() && (raw || cell.taken.is_none()) {
            cell.taken = taken;
            self.order = None;
        }
        if thumb.is_some() && (cell.thumb.is_none() && !cell.previewed || raw) {
            cell.thumb = thumb;
            cell.previewed = false;
        }
    }

    /// The cell holding the file at `path`.
    pub fn cell_of(&self, path: &Path) -> Option<usize> {
        self.by_path.get(path).copied()
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
        self.revision += 1;
    }

    pub fn set_types(&mut self, types: Types) {
        if self.types != types {
            self.types = types;
            self.order = None;
            self.revision += 1;
        }
    }

    /// Whether an Import takes the cell `i` when it is ticked.
    pub fn importable(&self, i: usize) -> bool {
        let c = &self.cells[i];
        c.passes(self.types) && !c.offline && !(self.new_only && c.known())
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
    fn glances_take_the_files_wanted_first_then_the_rest_in_order_each_once() {
        let files: Vec<PathBuf> = (0..6)
            .map(|i| PathBuf::from(format!("/f/{i}.jpg")))
            .collect();
        let mut picking = Picking::new(&files);
        let wanted = vec![PathBuf::from("/f/4.jpg"), PathBuf::from("/elsewhere.jpg")];
        assert_eq!(picking.pick(&wanted), Some(4));
        // Wanted and taken: the rest in order, 4 skipped.
        assert_eq!(picking.pick(&wanted), Some(0));
        let wanted = vec![PathBuf::from("/f/3.jpg")];
        assert_eq!(picking.pick(&wanted), Some(3));
        let picked: Vec<_> = std::iter::from_fn(|| picking.pick(&wanted)).collect();
        assert_eq!(picked, [1, 2, 5]);
    }

    #[test]
    fn the_glance_cache_keeps_to_its_budget_least_recently_used_out() {
        let glanced = || {
            Arc::new(Glanced {
                thumb: Some(ColorImage::filled([16, 16], egui::Color32::GRAY)),
                ..Glanced::default()
            })
        };
        let each = glanced().bytes();
        let cache = GlanceCache::new(each * 3);
        let key = |i: u64| (PathBuf::from(format!("/g/{i}.jpg")), i, Some(1));
        let took = std::time::Duration::ZERO;
        for i in 0..3 {
            cache.put(key(i), glanced(), took);
        }
        // 0 used again: 1 is the least recently used.
        assert!(cache.get(&key(0)).is_some());
        cache.put(key(3), glanced(), took);
        assert!(cache.get(&key(1)).is_none(), "the oldest evicted");
        assert!(cache.get(&key(0)).is_some() && cache.get(&key(3)).is_some());
        // Another size or time: another file.
        assert!(cache.get(&(key(0).0, 0, Some(2))).is_none());
        let usage = cache.usage();
        assert!(usage[0].bytes <= each * 3);
        assert_eq!(usage[0].kind.id, "import.glance");
        assert!(cache.clear(Some("import.glance")) > 0);
        assert!(cache.get(&key(0)).is_none());
    }

    #[test]
    fn a_glance_on_disk_reads_back_and_is_replaced_only_by_a_better_one() {
        let taken = exif_time("2026:11:02 10:20:30");
        let glanced = Glanced {
            taken,
            camera: Some("Canon EOS R6".into()),
            thumb: Some(ColorImage::filled(
                [24, 16],
                egui::Color32::from_rgb(90, 120, 200),
            )),
        };
        let bytes = encode_glance(&glanced);
        let back = decode_glance(&bytes).unwrap();
        assert_eq!(back.taken, taken);
        assert_eq!(back.camera.as_deref(), Some("Canon EOS R6"));
        let thumb = back.thumb.unwrap();
        assert_eq!(thumb.size, [24, 16]);
        let p = thumb.pixels[0];
        assert!(p.r().abs_diff(90) < 8 && p.b().abs_diff(200) < 8, "{p:?}");
        // Nothing but a magic: not an entry.
        assert!(decode_glance(b"HGL2").is_none());
        assert!(decode_glance(&bytes[..8]).is_none());
        let bare = encode_glance(&Glanced::default());
        assert!(decode_glance(&bare).is_some_and(|g| g.thumb.is_none()));
        // Replaced only by a larger preview, or one with what it lacked.
        let header = glance_header(&bytes).unwrap();
        let old = glance_rank(header.size, true, true);
        assert!(glance_rank(Some([24, 16]), true, true) <= old);
        assert!(glance_rank(Some([12, 8]), true, true) < old);
        assert!(glance_rank(Some([48, 32]), false, false) > old);
    }

    #[test]
    fn the_prefilter_knows_a_library_file_by_lookup_without_reading_it() {
        let dir = temp("prefilter-lookup");
        // Not on disk: a read would fail; the map knows it.
        let path = dir.join("seen.raw");
        let filter = Prefilter::of(Holdings {
            partials: Vec::new(),
            seen: vec![(path.clone(), 1_000, 42)],
            keys: HashMap::from([(path.clone(), helios_thumbs::Key::preview("x"))]),
        });
        assert_eq!(
            filter.key_at(&path, 1_000, Some(42)),
            Some(helios_thumbs::Key::preview("x"))
        );
        assert_eq!(filter.key_at(&path, 1_000, Some(43)), None);
        assert_eq!(filter.read_at(&path, 1_000, Some(42)), (true, None));
        // Changed since (another time or size): not known by the lookup.
        assert!(!filter.read_at(&path, 1_000, Some(43)).0);
        assert!(!filter.read_at(&path, 999, Some(42)).0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_window_lists_cards_then_folders_from_the_registry_each_through_its_connector() {
        let connectors = Connectors::default();
        let card = Card {
            label: "EOS_DIGITAL".into(),
            root: PathBuf::from("/media/EOS_DIGITAL"),
            capacity: Some(32_000_000_000),
        };
        connectors.set_cards(std::slice::from_ref(&card));
        connectors.set_places(vec![PathBuf::from("/photos/trip")]);
        let sources: Vec<_> = connectors
            .sources()
            .into_iter()
            .map(|(s, detected)| (s.connector, s.label, detected))
            .collect();
        assert_eq!(
            sources,
            [
                ("card".to_owned(), "EOS_DIGITAL".to_owned(), true),
                ("folder".to_owned(), "trip".to_owned(), false),
            ]
        );
        let (by, source) = connectors.for_path(&card.root).unwrap();
        assert_eq!(
            (by.describe().id.as_str(), source.capacity),
            ("card", card.capacity)
        );
        let (by, _) = connectors.for_path(Path::new("/photos/trip")).unwrap();
        assert_eq!(by.describe().id, "folder");
    }

    fn found(path: &str, known: bool, modified: i64) -> Found {
        Found {
            source: PathBuf::from("/card"),
            path: PathBuf::from(path),
            bytes: 10,
            modified: Some(modified),
            known,
            offline: false,
            key: None,
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
