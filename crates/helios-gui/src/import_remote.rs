//! The import window's glances of remote items: a Google Photos pick, any
//! source whose bytes a connector fetches ([`helios_connect::Access::Fetch`]).
//!
//! - A glance is the connector's own preview ([`Connector::thumbnail`],
//!   when its caps say `thumbnails`), never the file: kept in the
//!   [`GlanceCache`] as a local file's, in RAM and on disk, keyed by the
//!   connector, the account (the source's id), the item's id and its
//!   revision ([`disk_key`]). A pick opened again after a restart reads
//!   nothing from the network; an item whose revision changed is read
//!   again.
//! - Shown vs prefetched as for local sources: a glance read enters cold,
//!   one shown in a cell ([`RemoteGlances::show`]) goes after every
//!   prefetched one.
//! - [`RemoteGlances::read`] reads the items wanted first (the cells in
//!   view), then the rest in order while the connector's declared limits
//!   ([`helios_connect::Limits`], counted by a [`Throttle`]) and the glance
//!   cache's budget allow: it pauses at the first one met, and never
//!   evicts a glance shown.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use egui::ColorImage;
use helios_connect::{Cancel, Connector, ConnectorError, Item, ItemId, ItemRef, Limits, SourceRef};

use crate::import_scan::{GlanceCache, GlanceKey, Glanced};

const MINUTE: Duration = Duration::from_mins(1);
const DAY: Duration = Duration::from_hours(24);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The disk key of a remote item's glance: its connector, the account
/// (the source's id), the item's id in the source and its revision (the
/// connector's own version field, empty when it has none). The same pick
/// listed again, in another run, finds it; an item changed has another.
pub fn disk_key(source: &SourceRef, id: &ItemId, revision: Option<&str>) -> String {
    format!(
        "remote\n{}\n{}\n{}\n{}",
        source.connector,
        source.id,
        id.0,
        revision.unwrap_or_default()
    )
}

/// What the window asks the [`GlanceCache`] for a remote item's glance
/// ([`GlanceCache::cached`], [`GlanceCache::show`]): this path, size 0, no
/// modification time. The path is its disk key: never a file.
pub fn glance_path(source: &SourceRef, item: &Item) -> PathBuf {
    PathBuf::from(disk_key(source, &item.id, item.revision.as_deref()))
}

fn ram_key(source: &SourceRef, item: &Item) -> GlanceKey {
    (glance_path(source, item), 0, None)
}

/// The requests made to a service, held to its declared [`Limits`]: a
/// minute's, a day's, and a wait the service asked for.
#[derive(Debug)]
pub struct Throttle {
    limits: Limits,
    state: Mutex<Sent>,
}

#[derive(Debug, Default)]
struct Sent {
    /// When each request of the longest window counted went.
    at: VecDeque<Instant>,
    /// No request before this: the service asked to wait.
    hold: Option<Instant>,
}

impl Throttle {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            state: Mutex::default(),
        }
    }

    /// Takes one request at `now` when the limits leave room for it.
    pub fn take(&self, now: Instant) -> bool {
        let Limits {
            per_minute,
            per_day,
        } = self.limits;
        let mut sent = lock(&self.state);
        if sent.hold.is_some_and(|until| now < until) {
            return false;
        }
        if per_minute.is_none() && per_day.is_none() {
            return true;
        }
        let window = if per_day.is_some() { DAY } else { MINUTE };
        while sent
            .at
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= window)
        {
            sent.at.pop_front();
        }
        if per_day.is_some_and(|n| sent.at.len() >= n as usize) {
            return false;
        }
        if let Some(n) = per_minute {
            let recent = sent
                .at
                .iter()
                .rev()
                .take_while(|t| now.saturating_duration_since(**t) < MINUTE)
                .count();
            if recent >= n as usize {
                return false;
            }
        }
        sent.at.push_back(now);
        true
    }

    /// The service asked to wait (`retry_after`, else a minute): no
    /// request before.
    pub fn hold(&self, now: Instant, retry_after: Option<Duration>) {
        lock(&self.state).hold = Some(now + retry_after.unwrap_or(MINUTE));
    }
}

/// Why a read of glances stopped before the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pause {
    /// The connector's declared limits (or the service's own wait):
    /// read on later.
    Rate,
    /// The glance cache has no room for more without evicting a glance
    /// shown: read on once room comes back.
    Budget,
}

/// What a [`RemoteGlances::read`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Glancing {
    /// Previews asked of the connector.
    pub read: usize,
    /// Glances found in the cache, nothing asked.
    pub cached: usize,
    /// Of those, the ones from its disk tier.
    pub from_disk: usize,
    /// Why it stopped before the last item, if it did.
    pub paused: Option<Pause>,
}

/// Where a glance came from.
enum Got {
    Ram(Arc<Glanced>),
    /// Its RAM bytes.
    Disk(Arc<Glanced>, u64),
    /// Its RAM bytes and the bytes written on disk.
    Read(Arc<Glanced>, u64, u64),
    /// The connector has no preview for it (or failed, not kept).
    Nothing,
}

/// The glances of one connector's remote items, in a [`GlanceCache`].
pub struct RemoteGlances {
    cache: Arc<GlanceCache>,
    connector: Arc<dyn Connector>,
    thumbnails: bool,
    throttle: Throttle,
    /// What its read-ahead added to the cache, from one read to the next:
    /// what the budget rule counts.
    own: Mutex<Own>,
}

/// The bytes a read-ahead added: (all, the last one's) in RAM, then on
/// disk, counted since the cache's generation.
#[derive(Default)]
struct Own {
    generation: u64,
    ram: (u64, u64),
    disk: (u64, u64),
}

impl RemoteGlances {
    /// The glances `connector` gives, within its declared limits.
    pub fn new(cache: Arc<GlanceCache>, connector: Arc<dyn Connector>) -> Self {
        let caps = connector.describe().caps;
        Self {
            cache,
            connector,
            thumbnails: caps.thumbnails,
            throttle: Throttle::new(caps.limits),
            own: Mutex::default(),
        }
    }

    /// The glance of `item` shown in a cell now: it goes after every
    /// prefetched glance, on disk too. Writes to disk: never on the
    /// interface thread. `false` when it is not kept.
    pub fn show(&self, source: &SourceRef, item: &Item) -> bool {
        match self.cache.show(&glance_path(source, item), 0, None) {
            None => false,
            Some(on_disk) => {
                if let Some(k) = on_disk {
                    self.cache.warm(&[k]);
                }
                true
            }
        }
    }

    /// The glances of `items` of `source`: the `wanted` first (the cells
    /// in view), then the rest in order while the connector's limits and
    /// the cache's budget allow. Stops at the first limit met, said in
    /// [`Glancing::paused`]: called again later (the next minute, once
    /// room came back) it reads on, what it read before found in the
    /// cache. Each glance found is given to `got`. `now` is the clock the
    /// limits are counted on.
    pub fn read(
        &self,
        (source, items): (&SourceRef, &[Item]),
        wanted: &[ItemId],
        cancel: &Cancel,
        now: &dyn Fn() -> Instant,
        got: &mut dyn FnMut(&Item, &Arc<Glanced>),
    ) -> Glancing {
        let mut done = Glancing::default();
        let first: HashSet<&ItemId> = wanted.iter().collect();
        let in_view = wanted
            .iter()
            .filter_map(|id| items.iter().find(|i| i.id == *id));
        let rest = items.iter().filter(|i| !first.contains(&i.id));
        let in_view = in_view.map(|i| (i, true));
        for (item, wanted) in in_view.chain(rest.map(|i| (i, false))) {
            if cancel.is_cancelled() {
                break;
            }
            if !wanted {
                let room = {
                    let generation = self.cache.generation();
                    let mut own = lock(&self.own);
                    if own.generation != generation {
                        // Cleared since: what it added is gone.
                        *own = Own {
                            generation,
                            ..Own::default()
                        };
                    }
                    self.cache.room(own.ram, Some(own.disk))
                };
                let kept = self.cache.holds(&ram_key(source, item));
                if !kept && !room {
                    done.paused = Some(Pause::Budget);
                    break;
                }
            }
            match self.glance(source, item, now()) {
                Err(pause) => {
                    done.paused = Some(pause);
                    break;
                }
                Ok(Got::Nothing) => {}
                Ok(Got::Ram(g)) => {
                    done.cached += 1;
                    got(item, &g);
                }
                Ok(Got::Disk(g, bytes)) => {
                    done.cached += 1;
                    done.from_disk += 1;
                    if !wanted {
                        self.owns(bytes, 0);
                    }
                    got(item, &g);
                }
                Ok(Got::Read(g, bytes, written)) => {
                    done.read += 1;
                    if !wanted {
                        self.owns(bytes, written);
                    }
                    got(item, &g);
                }
            }
        }
        done
    }

    /// Counts a glance the read-ahead put in the cache: `ram` bytes in
    /// RAM, `disk` written on disk.
    fn owns(&self, ram: u64, disk: u64) {
        let mut own = lock(&self.own);
        own.ram = (own.ram.0 + ram, ram);
        if disk > 0 {
            own.disk = (own.disk.0 + disk, disk);
        }
    }

    /// One item's glance: the cache's RAM tier, its disk tier, else the
    /// connector's preview, when the limits allow it.
    fn glance(&self, source: &SourceRef, item: &Item, now: Instant) -> Result<Got, Pause> {
        let key = ram_key(source, item);
        if let Some(hit) = self.cache.get(&key) {
            return Ok(Got::Ram(hit));
        }
        let on_disk = disk_key(source, &item.id, item.revision.as_deref());
        if let Some(hit) = self.cache.disk_glance(&on_disk, key.clone()) {
            let bytes = hit.bytes();
            return Ok(Got::Disk(hit, bytes));
        }
        if !self.thumbnails {
            return Ok(Got::Nothing);
        }
        if !self.throttle.take(now) {
            return Err(Pause::Rate);
        }
        let at = ItemRef {
            source: source.clone(),
            id: item.id.clone(),
        };
        let started = Instant::now();
        let bytes = match self.connector.thumbnail(&at, crate::library::THUMB) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(Got::Nothing),
            Err(ConnectorError::RateLimited { retry_after }) => {
                self.throttle.hold(now, retry_after);
                return Err(Pause::Rate);
            }
            Err(e) => {
                log::debug!("import glance of a {} item: {e}", source.connector);
                return Ok(Got::Nothing);
            }
        };
        let Some(thumb) = picture(&bytes) else {
            log::debug!(
                "import glance of a {} item: not a picture",
                source.connector
            );
            return Ok(Got::Nothing);
        };
        let glanced = Arc::new(Glanced {
            // As a camera writes it: the local time kept as if it were UTC.
            taken: item
                .taken
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|t| t.naive_utc()),
            camera: item.metadata.get("tiff:Model").cloned(),
            thumb: Some(thumb),
        });
        self.cache.put_as(
            key,
            glanced.clone(),
            started.elapsed(),
            Some(on_disk.clone()),
        );
        let written = self.cache.keep_on_disk(&on_disk, &glanced);
        let bytes = glanced.bytes();
        Ok(Got::Read(glanced, bytes, written))
    }
}

/// A preview's bytes as an sRGB picture no larger than the cells'.
fn picture(bytes: &[u8]) -> Option<ColorImage> {
    let side = crate::library::THUMB;
    let image = image::load_from_memory(bytes).ok()?;
    let image = if image.width().max(image.height()) > side {
        image.thumbnail(side, side)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use helios_connect::{
        Access, Caps, ConnectorInfo, Cursor, FetchSink, Fetched, ItemKind, Page, Text,
    };

    use super::*;
    use crate::import_scan::{GLANCE_BUDGET, GLANCE_DISK_BUDGET};

    /// A remote service: its previews, each call counted.
    struct FakeRemote {
        limits: Limits,
        calls: Arc<AtomicUsize>,
    }

    impl Connector for FakeRemote {
        fn describe(&self) -> ConnectorInfo {
            ConnectorInfo {
                id: "fake-remote".into(),
                label: Text::new("fake", "Fake"),
                caps: Caps {
                    accounts: true,
                    thumbnails: true,
                    limits: self.limits,
                    ..Caps::default()
                },
            }
        }

        fn sources(
            &self,
            _: Option<&helios_connect::AccountRef>,
        ) -> Result<Vec<SourceRef>, ConnectorError> {
            Ok(Vec::new())
        }

        fn list(
            &self,
            _: &SourceRef,
            _: Option<&Cursor>,
            _: usize,
            _: &Cancel,
        ) -> Result<Page, ConnectorError> {
            Ok(Page::default())
        }

        fn access(&self, _: &ItemRef) -> Access {
            Access::Fetch
        }

        fn fetch(
            &self,
            _: &ItemRef,
            _: u64,
            _: &mut dyn FetchSink,
            _: &Cancel,
        ) -> Result<Fetched, ConnectorError> {
            Err(ConnectorError::Unsupported)
        }

        fn thumbnail(&self, item: &ItemRef, _: u32) -> Result<Option<Vec<u8>>, ConnectorError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let shade = item.id.0.len() as u8 * 20;
            let picture = image::RgbImage::from_pixel(64, 48, image::Rgb([shade, 90, 160]));
            let mut out = std::io::Cursor::new(Vec::new());
            picture
                .write_to(&mut out, image::ImageFormat::Png)
                .map_err(|e| ConnectorError::Fatal(e.to_string()))?;
            Ok(Some(out.into_inner()))
        }
    }

    fn remote(limits: Limits) -> (Arc<dyn Connector>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let fake = FakeRemote {
            limits,
            calls: calls.clone(),
        };
        (Arc::new(fake), calls)
    }

    fn source() -> SourceRef {
        SourceRef {
            connector: "fake-remote".into(),
            id: "account-1".into(),
            label: "Fake".into(),
            root: None,
            capacity: None,
        }
    }

    fn items(n: usize) -> Vec<Item> {
        (0..n)
            .map(|i| Item {
                id: ItemId(format!("m{i}")),
                path: format!("IMG_{i}.jpg"),
                size: None,
                modified: None,
                taken: Some(1_714_558_830),
                kind: ItemKind::Photo,
                mime: Some("image/jpeg".into()),
                metadata: [("tiff:Model".to_owned(), "Pixel 8".to_owned())].into(),
                claim: None,
                revision: Some("r1".into()),
                group: None,
            })
            .collect()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("helios-remote-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read_all(glances: &RemoteGlances, items: &[Item], now: Instant) -> Glancing {
        glances.read(
            (&source(), items),
            &[],
            &Cancel::new(),
            &|| now,
            &mut |_, _| {},
        )
    }

    #[test]
    fn a_pick_opened_again_after_a_restart_reads_nothing_and_a_new_revision_is_read() {
        let root = temp("restart");
        let cache = Arc::new(GlanceCache::new(GLANCE_BUDGET));
        cache.open_disk_within(&root, GLANCE_DISK_BUDGET).unwrap();
        let (connector, calls) = remote(Limits::default());
        let glances = RemoteGlances::new(cache.clone(), connector.clone());
        let mut listed = items(5);
        let mut seen = Vec::new();
        let first = glances.read(
            (&source(), &listed),
            &[ItemId("m3".into())],
            &Cancel::new(),
            &Instant::now,
            &mut |item, g| seen.push((item.id.0.clone(), g.clone())),
        );
        assert_eq!((first.read, first.cached, first.paused), (5, 0, None));
        assert_eq!(calls.load(Ordering::SeqCst), 5);
        // The one wanted first; each with its preview, capture time and
        // camera as the service gave them.
        assert_eq!(seen[0].0, "m3");
        let g = &seen[0].1;
        assert_eq!(g.thumb.as_ref().unwrap().size, [64, 48]);
        assert_eq!(g.camera.as_deref(), Some("Pixel 8"));
        assert!(g.taken.is_some());
        assert!(glances.show(&source(), &listed[3]));
        // Opened again: from RAM.
        let again = read_all(&glances, &listed, Instant::now());
        assert_eq!((again.read, again.cached, again.from_disk), (0, 5, 0));
        // A restart leaves the disk tier: nothing asked of the service.
        cache.forget_ram();
        let glances = RemoteGlances::new(cache.clone(), connector);
        let after = read_all(&glances, &listed, Instant::now());
        assert_eq!((after.read, after.cached, after.from_disk), (0, 5, 5));
        assert_eq!(calls.load(Ordering::SeqCst), 5);
        // The item changed at the service: its new revision is read.
        listed[2].revision = Some("r2".into());
        let changed = read_all(&glances, &listed, Instant::now());
        assert_eq!((changed.read, changed.cached), (1, 4));
        assert_eq!(calls.load(Ordering::SeqCst), 6);
        // Another account's item of the same id is another glance.
        assert_ne!(
            disk_key(&source(), &listed[0].id, Some("r1")),
            disk_key(
                &SourceRef {
                    id: "account-2".into(),
                    ..source()
                },
                &listed[0].id,
                Some("r1")
            )
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_ahead_pauses_at_the_declared_rate_and_goes_on_the_next_minute() {
        let cache = Arc::new(GlanceCache::new(GLANCE_BUDGET));
        let limits = Limits {
            per_minute: Some(3),
            per_day: Some(7),
        };
        let (connector, calls) = remote(limits);
        let glances = RemoteGlances::new(cache, connector);
        let listed = items(10);
        let t0 = Instant::now();
        let first = read_all(&glances, &listed, t0);
        assert_eq!((first.read, first.paused), (3, Some(Pause::Rate)));
        // Still within the minute: nothing more asked.
        let held = read_all(&glances, &listed, t0 + Duration::from_secs(30));
        assert_eq!((held.read, held.cached), (0, 3));
        assert_eq!(held.paused, Some(Pause::Rate));
        // The next minute: three more.
        let next = read_all(&glances, &listed, t0 + Duration::from_secs(61));
        assert_eq!((next.read, next.paused), (3, Some(Pause::Rate)));
        // The day's hint: one more, then nothing until the day is past.
        let later = read_all(&glances, &listed, t0 + Duration::from_secs(130));
        assert_eq!(later.read, 1);
        let day = read_all(&glances, &listed, t0 + Duration::from_secs(200));
        assert_eq!((day.read, day.paused), (0, Some(Pause::Rate)));
        assert_eq!(calls.load(Ordering::SeqCst), 7);
        // A wait the service asks for holds every request.
        let throttle = Throttle::new(Limits::default());
        assert!(throttle.take(t0));
        throttle.hold(t0, Some(Duration::from_secs(5)));
        assert!(!throttle.take(t0 + Duration::from_secs(4)));
        assert!(throttle.take(t0 + Duration::from_secs(5)));
    }

    #[test]
    fn read_ahead_stops_at_the_budget_without_evicting_a_shown_glance() {
        const N: usize = 24;
        let listed = items(N);
        // One glance's size, as read.
        let (connector, calls) = remote(Limits::default());
        let probe = RemoteGlances::new(Arc::new(GlanceCache::new(GLANCE_BUDGET)), connector);
        read_all(&probe, &listed[..1], Instant::now());
        let each = probe
            .cache
            .get(&ram_key(&source(), &listed[0]))
            .unwrap()
            .bytes();
        calls.store(0, Ordering::SeqCst);
        // Room for about eight glances on top of the one shown.
        let cache = Arc::new(GlanceCache::new(each * 11));
        let glances = RemoteGlances::new(cache.clone(), probe.connector.clone());
        let shown = &listed[N - 1];
        let first = glances.read(
            (&source(), &listed),
            std::slice::from_ref(&shown.id),
            &Cancel::new(),
            &Instant::now,
            &mut |_, _| {},
        );
        assert_eq!(first.paused, Some(Pause::Budget), "{first:?}");
        assert!(first.read < N && first.read >= 4, "{first:?}");
        assert!(glances.show(&source(), shown));
        assert_eq!(cache.split().shown.1, 1);
        // Asked again, still no room: the shown glance stays.
        let again = read_all(&glances, &listed, Instant::now());
        assert_eq!(again.paused, Some(Pause::Budget));
        assert!(
            cache
                .cached(&glance_path(&source(), shown), 0, None)
                .is_some()
        );
        assert_eq!(cache.split().shown.1, 1);
        assert!(cache.split().shown.0 + cache.split().prefetched.0 <= each * 11);
        assert_eq!(calls.load(Ordering::SeqCst), first.read);
    }
}
