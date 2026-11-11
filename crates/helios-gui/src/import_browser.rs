//! The import window's Contents on egui-pop's file browser
//! (`egui_pop::api::files::browser`): a [`FileSource`] over what the
//! import's sources hold, and a [`ThumbnailProvider`] over the previews
//! the scans read and the library's thumbnail cache.
//!
//! - [`ImportSource`] lists the photos of the sources ticked, as the
//!   window's scans find them ([`crate::import_scan`]): an online folder or
//!   card as it is read, an offline one as the image map recorded it. The
//!   window publishes each change ([`ImportSource::publish`]); the
//!   listings under way get the new entries, the facts learnt later (a
//!   capture time) and a fresh start when entries went (a source
//!   unticked, a type filter). A RAW+JPEG pair is one entry, by its raw.
//! - [`ImportThumbnails`] answers from the preview a scan glanced at (the
//!   raw's embedded one first), else from the library's thumbnail cache
//!   (an offline photo, a photo of the Library), read on two threads: the
//!   requests of each frame in priority order (visible, near,
//!   background), those not renewed in a frame cancelled. The files still
//!   waiting for a picture, in that order, are what the scans glance at
//!   first ([`ImportThumbnails::waiting`]).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use egui::{ColorImage, TextureHandle};
use egui_pop::api::files::browser::{
    FileSource, ListOptions, Listing, ListingSender, Page, Priority, Renewals, SourceEntry,
    ThumbRequest, ThumbState, ThumbnailProvider,
};
use helios_thumbs::{Key, SMALL, ThumbCache};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The place the browser shows while no source is ticked.
pub const NOWHERE: &str = "helios-import:";

/// What the listings are fed.
#[derive(Debug)]
struct Feed {
    place: PathBuf,
    entries: Vec<SourceEntry>,
    done: bool,
    listeners: Vec<ListingSender>,
}

/// The photos of the import's sources, as a file browser's source: the
/// window publishes them, every listing of the place follows.
#[derive(Clone, Debug)]
pub struct ImportSource {
    feed: Arc<Mutex<Feed>>,
}

impl Default for ImportSource {
    fn default() -> Self {
        Self {
            feed: Arc::new(Mutex::new(Feed {
                place: PathBuf::from(NOWHERE),
                entries: Vec::new(),
                done: true,
                listeners: Vec::new(),
            })),
        }
    }
}

impl ImportSource {
    pub const ID: &'static str = "helios.import";

    /// What the sources hold now: `place` the folder named (the first
    /// source ticked), `entries` the photos shown, `done` once every
    /// source is read through. The listings under way get what changed.
    pub fn publish(&self, place: &Path, entries: Vec<SourceEntry>, done: bool) {
        let mut feed = lock(&self.feed);
        let start_again = feed.place != place || {
            let now: HashSet<&Path> = entries.iter().map(|e| e.path.as_path()).collect();
            feed.entries.iter().any(|e| !now.contains(e.path.as_path()))
        };
        if start_again {
            // Entries went: each listing starts again (the browser keeps
            // its selection and focus by path).
            feed.place = place.to_path_buf();
            feed.entries = entries;
            feed.done = done;
            for l in feed.listeners.drain(..) {
                l.send(Page::Changed);
            }
            return;
        }
        let before: HashMap<&Path, &SourceEntry> =
            feed.entries.iter().map(|e| (e.path.as_path(), e)).collect();
        let mut added = Vec::new();
        let mut updated = Vec::new();
        for e in &entries {
            match before.get(e.path.as_path()) {
                None => added.push(e.clone()),
                Some(old) if *old != e => updated.push(e.clone()),
                Some(_) => {}
            }
        }
        let finished = done && !feed.done;
        feed.listeners.retain(|l| {
            (added.is_empty() || l.send(Page::Entries(added.clone())))
                && (updated.is_empty() || l.send(Page::Updated(updated.clone())))
        });
        if finished {
            for l in feed.listeners.drain(..) {
                l.send(Page::Done);
            }
        }
        feed.entries = entries;
        feed.done = done;
    }

    /// The photos as last published.
    pub fn entries(&self) -> Vec<SourceEntry> {
        lock(&self.feed).entries.clone()
    }
}

impl FileSource for ImportSource {
    fn id(&self) -> &str {
        Self::ID
    }

    fn list(&self, place: &Path, _options: &ListOptions) -> Listing {
        let (tx, listing) = Listing::channel();
        let mut feed = lock(&self.feed);
        if place != feed.place {
            // Only the import's place is listed here.
            tx.send(Page::Done);
            return listing;
        }
        tx.send(Page::Entries(feed.entries.clone()));
        if feed.done {
            tx.send(Page::Done);
        } else {
            feed.listeners.push(tx);
        }
        listing
    }
}

/// A read of the thumbnail cache asked for: the file and its entry.
type Wanted = (PathBuf, Key);

/// The cache reader's queue: what the last frame wanted, in order.
#[derive(Default)]
struct Queue {
    wanted: VecDeque<Wanted>,
    closed: bool,
}

/// The provider's state, on the interface thread.
#[derive(Default)]
struct Inner {
    renewals: Renewals,
    /// Previews read but not yet textures: a scan's glance, a cache entry.
    pictures: HashMap<PathBuf, ColorImage>,
    textures: HashMap<PathBuf, TextureHandle>,
    /// Files with no picture to show (glanced without a preview, no cache
    /// entry).
    none: HashSet<PathBuf>,
    /// Files glanced at by their scan: no other picture is coming.
    glanced: HashSet<PathBuf>,
    /// The cache entries of the files the image map links to one.
    keys: HashMap<PathBuf, Key>,
    /// The files asked as visible this frame, in order.
    visible: Vec<PathBuf>,
    /// What the last frame asked as visible.
    on_screen: Vec<PathBuf>,
    /// Textures made this frame: a few per frame.
    made: usize,
    /// The files asked this frame with no picture yet: priority, order.
    asking: Vec<(u8, usize, PathBuf)>,
    /// What the last frame asked with no picture yet, in priority order.
    waiting: Vec<PathBuf>,
}

/// The import window's thumbnails: the scans' previews, then the
/// library's thumbnail cache, visible first, what scrolled away
/// cancelled.
pub struct ImportThumbnails {
    inner: Mutex<Inner>,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    answers: Mutex<Receiver<(PathBuf, Option<ColorImage>)>>,
    ctx: Arc<Mutex<Option<egui::Context>>>,
}

impl std::fmt::Debug for ImportThumbnails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportThumbnails").finish_non_exhaustive()
    }
}

impl Drop for ImportThumbnails {
    fn drop(&mut self) {
        let (queue, ready) = &*self.queue;
        lock(queue).closed = true;
        ready.notify_all();
    }
}

/// How many textures a frame makes: the rest the next frames.
const TEXTURES_PER_FRAME: usize = 48;

/// How many threads read the library's thumbnail cache.
const CACHE_READERS: usize = 2;

impl ImportThumbnails {
    /// A provider reading `cache` (the library's) for the files the image
    /// map links to an entry; none without a library.
    pub fn new(cache: Option<Arc<ThumbCache>>) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (tx, rx) = channel();
        let ctx: Arc<Mutex<Option<egui::Context>>> = Arc::default();
        if let Some(cache) = cache {
            for _ in 0..CACHE_READERS {
                let (cache, queue, ctx, tx) =
                    (cache.clone(), queue.clone(), ctx.clone(), tx.clone());
                let spawned = std::thread::Builder::new()
                    .name("helios-import-thumbs".into())
                    .spawn(move || read_cache(&cache, &queue, &tx, &ctx));
                if let Err(e) = spawned {
                    log::warn!("import thumbnails: no thread: {e}");
                }
            }
        }
        Self {
            inner: Mutex::default(),
            queue,
            answers: Mutex::new(rx),
            ctx,
        }
    }

    /// A scan's glance at `path`: its preview (none when it has none).
    pub fn glanced(&self, path: &Path, picture: Option<ColorImage>) {
        let mut inner = lock(&self.inner);
        inner.glanced.insert(path.to_path_buf());
        match picture {
            Some(p) => {
                inner.textures.remove(path);
                inner.none.remove(path);
                inner.pictures.insert(path.to_path_buf(), p);
            }
            None if !inner.textures.contains_key(path) && !inner.keys.contains_key(path) => {
                inner.none.insert(path.to_path_buf());
            }
            None => {}
        }
    }

    /// The cache entry of a file the image map links to one.
    pub fn key(&self, path: &Path, key: Key) {
        let mut inner = lock(&self.inner);
        inner.none.remove(path);
        inner.keys.insert(path.to_path_buf(), key);
    }

    /// Forgets every file: a new import.
    pub fn clear(&self) {
        *lock(&self.inner) = Inner::default();
        lock(&self.queue.0).wanted.clear();
    }

    /// Whether the thumbnail of the file at `path` is made.
    pub fn ready(&self, path: &Path) -> bool {
        lock(&self.inner).textures.contains_key(path)
    }

    /// The files the last frame drew, in order: the image map reads them
    /// first.
    pub fn on_screen(&self) -> Vec<PathBuf> {
        lock(&self.inner).on_screen.clone()
    }

    /// The files the last frame asked that have no picture yet: visible,
    /// then near, then the rest, each in the order asked.
    pub fn waiting(&self) -> Vec<PathBuf> {
        lock(&self.inner).waiting.clone()
    }

    /// Takes in what the cache reader answered.
    fn receive(&self, inner: &mut Inner) {
        for (path, picture) in lock(&self.answers).try_iter() {
            match picture {
                Some(p) => {
                    inner.pictures.insert(path, p);
                }
                None => {
                    inner.none.insert(path);
                }
            }
        }
    }
}

impl ThumbnailProvider for ImportThumbnails {
    fn begin_frame(&self, ctx: &egui::Context) {
        lock(&self.ctx).get_or_insert_with(|| ctx.clone());
        let mut inner = lock(&self.inner);
        self.receive(&mut inner);
        inner.renewals.begin();
        inner.visible.clear();
        inner.asking.clear();
        inner.made = 0;
    }

    fn request(&self, ctx: &egui::Context, request: &ThumbRequest<'_>) -> ThumbState {
        let path = &request.entry.path;
        let mut inner = lock(&self.inner);
        if request.priority == Priority::Visible && !inner.visible.contains(path) {
            inner.visible.push(path.clone());
        }
        if let Some(t) = inner.textures.get(path) {
            return ThumbState::Ready(t.clone());
        }
        if inner.pictures.contains_key(path) {
            if inner.made >= TEXTURES_PER_FRAME {
                ctx.request_repaint();
                return ThumbState::Loading;
            }
            inner.made += 1;
            let picture = inner.pictures.remove(path).unwrap_or_default();
            let texture = ctx.load_texture(
                format!("helios.import.thumb.{}", path.display()),
                picture,
                egui::TextureOptions::LINEAR,
            );
            inner.textures.insert(path.clone(), texture.clone());
            return ThumbState::Ready(texture);
        }
        if inner.none.contains(path) {
            return ThumbState::None;
        }
        let rank = match request.priority {
            Priority::Visible => 0,
            Priority::Near => 1,
            Priority::Background => 2,
        };
        let seq = inner.asking.len();
        if inner.keys.contains_key(path) {
            // Asked of the cache: renewed each frame it is wanted.
            inner.renewals.ask(request.entry, request.priority);
            if !inner.glanced.contains(path) {
                inner.asking.push((rank, seq, path.clone()));
            }
            return ThumbState::Loading;
        }
        if inner.glanced.contains(path) {
            ThumbState::None
        } else {
            // The scan has not glanced at it yet: it is told to.
            inner.asking.push((rank, seq, path.clone()));
            ThumbState::Loading
        }
    }

    fn end_frame(&self, _ctx: &egui::Context) {
        let mut inner = lock(&self.inner);
        let wanted: VecDeque<Wanted> = inner
            .renewals
            .wanted()
            .into_iter()
            .filter_map(|(p, _)| Some((p.clone(), inner.keys.get(&p)?.clone())))
            .collect();
        // What is not wanted any more leaves the queue: cancelled.
        inner.on_screen = std::mem::take(&mut inner.visible);
        let mut asking = std::mem::take(&mut inner.asking);
        asking.sort_unstable_by_key(|(rank, seq, _)| (*rank, *seq));
        inner.waiting = asking.into_iter().map(|(_, _, p)| p).collect();
        drop(inner);
        let (queue, ready) = &*self.queue;
        lock(queue).wanted = wanted;
        ready.notify_all();
    }

    fn cancel(&self, path: &Path) {
        lock(&self.queue.0).wanted.retain(|(p, _)| p != path);
    }
}

/// The cache reader: the wanted entries, first first, each answered.
fn read_cache(
    cache: &ThumbCache,
    queue: &(Mutex<Queue>, Condvar),
    answers: &Sender<(PathBuf, Option<ColorImage>)>,
    ctx: &Mutex<Option<egui::Context>>,
) {
    let (lock_queue, ready) = queue;
    loop {
        let next = {
            let mut q = lock(lock_queue);
            loop {
                if q.closed {
                    return;
                }
                if let Some(w) = q.wanted.pop_front() {
                    break w;
                }
                q = ready.wait(q).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let (path, key) = next;
        let picture = cache
            .get(&key, SMALL)
            .map(|rgb| crate::library::rgb_image(&rgb));
        if answers.send((path, picture)).is_err() {
            return;
        }
        if let Some(ctx) = lock(ctx).as_ref() {
            ctx.request_repaint();
        }
    }
}
