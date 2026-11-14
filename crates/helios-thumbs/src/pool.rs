//! The pool that fills the library's thumbnails: a few threads, the
//! cache first, a file read only on a miss, the images in view first.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, Weak};

use helios_load::Kind;

use crate::cache::{Key, ThumbCache};
use crate::{LARGE, Rgb8, SMALL, key_of, read};

/// An image whose thumbnail is wanted, and where its file is (none when
/// no copy is at hand: an offline or missing photo).
#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub path: Option<PathBuf>,
}

/// What became of a job.
#[derive(Debug)]
pub enum Outcome {
    /// The thumbnail, [`SMALL`] pixels on its long side, 8-bit sRGB.
    Thumb(Rgb8),
    /// The [`LARGE`] entry asked by [`Loader::large`]: Compare's and the
    /// loupe's first paint.
    Large(Rgb8),
    /// The file holds nothing readable as a picture.
    NoPreview,
    /// No copy of the file is at hand and the cache has no thumbnail.
    Unavailable,
    /// The file could not be read.
    Failed(String),
}

/// Where outcomes go, from the pool's threads; `false` once nobody
/// listens (the pool then stops).
pub type Sink = dyn Fn(&str, Outcome) -> bool + Send + Sync;

struct Queue {
    /// [`LARGE`] entries to show (Compare's first paint): before all.
    show_large: VecDeque<Job>,
    /// Thumbnails to show, in the order wanted.
    small: VecDeque<Job>,
    /// [`LARGE`] entries to fill once nothing is waiting to be shown.
    large: VecDeque<Job>,
    closed: bool,
    /// Every image loaded, and where its file is: what
    /// [`Loader::prefer`] queues again once its thumbnail was dropped.
    known: HashMap<String, Option<PathBuf>>,
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    cache: Option<Arc<ThumbCache>>,
    sink: Box<Sink>,
}

/// The pool: its threads stop when it is dropped (after the job each one
/// is on).
pub struct Loader {
    shared: Arc<Shared>,
}

/// How many threads read thumbnails: half the CPUs, two to four.
pub fn workers() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 4))
}

impl Loader {
    /// Starts `threads` threads reading through `cache` (none: every
    /// thumbnail is read from its file), sending to `sink`. The threads
    /// list the cache's folder first ([`ThumbCache::list`]), so the
    /// caller never waits for it.
    pub fn start(
        cache: Option<Arc<ThumbCache>>,
        threads: usize,
        sink: impl Fn(&str, Outcome) -> bool + Send + Sync + 'static,
    ) -> Loader {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                show_large: VecDeque::new(),
                small: VecDeque::new(),
                known: HashMap::new(),
                large: VecDeque::new(),
                closed: false,
            }),
            ready: Condvar::new(),
            cache,
            sink: Box::new(sink),
        });
        for n in 0..threads.max(1) {
            let shared = shared.clone();
            let _ = std::thread::Builder::new()
                .name(format!("helios-thumbs-{n}"))
                .spawn(move || work(&shared));
        }
        Loader { shared }
    }

    /// Queues jobs, after those already queued.
    pub fn load(&self, jobs: impl IntoIterator<Item = Job>) {
        let mut queue = self.shared.queue.lock().expect("the pool's queue");
        for job in jobs {
            queue.known.insert(job.id.clone(), job.path.clone());
            queue.small.push_back(job);
        }
        drop(queue);
        self.shared.ready.notify_all();
    }

    /// The images in view, to read before the others (in this order).
    /// One already read whose thumbnail was dropped (its texture freed) is
    /// queued again: read back from the cache.
    pub fn prefer(&self, ids: &[String]) {
        prefer(&self.shared, ids);
    }

    /// [`Loader::prefer`] for a hook that must not keep the pool alive
    /// (the grid's "in view" one).
    pub fn preferrer(&self) -> impl Fn(&[String]) + Send + Sync + 'static {
        let weak: Weak<Shared> = Arc::downgrade(&self.shared);
        move |ids| {
            if let Some(shared) = weak.upgrade() {
                prefer(&shared, ids);
            }
        }
    }

    /// Asks an image's [`LARGE`] thumbnail (Compare's and the loupe's
    /// first paint), before any other job: from the cache, else read from
    /// its file and stored. Sent as [`Outcome::Large`]; nothing is sent
    /// when there is none.
    pub fn large(&self, id: &str) {
        large(&self.shared, id);
    }

    /// [`Loader::large`] for a hook that must not keep the pool alive.
    pub fn larger(&self) -> impl Fn(&str) + Send + Sync + 'static {
        let weak: Weak<Shared> = Arc::downgrade(&self.shared);
        move |id| {
            if let Some(shared) = weak.upgrade() {
                large(&shared, id);
            }
        }
    }

    /// The jobs still queued (thumbnails to show, large entries to fill).
    pub fn queued(&self) -> (usize, usize) {
        let queue = self.shared.queue.lock().expect("the pool's queue");
        (queue.small.len(), queue.large.len())
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        close(&self.shared);
    }
}

fn close(shared: &Shared) {
    shared.queue.lock().expect("the pool's queue").closed = true;
    shared.ready.notify_all();
}

fn large(shared: &Shared, id: &str) {
    let mut queue = shared.queue.lock().expect("the pool's queue");
    let path = queue.known.get(id).cloned().flatten();
    queue.show_large.push_back(Job {
        id: id.to_string(),
        path,
    });
    drop(queue);
    shared.ready.notify_all();
}

fn prefer(shared: &Shared, ids: &[String]) {
    let mut queue = shared.queue.lock().expect("the pool's queue");
    let place: HashMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let mut front: Vec<(usize, Job)> = Vec::new();
    let mut rest = VecDeque::with_capacity(queue.small.len());
    let mut queued = std::collections::HashSet::new();
    for job in std::mem::take(&mut queue.small) {
        match place.get(job.id.as_str()) {
            Some(&i) => {
                queued.insert(job.id.clone());
                front.push((i, job));
            }
            None => rest.push_back(job),
        }
    }
    // Those read already: again, from the cache.
    for (i, id) in ids.iter().enumerate() {
        if !queued.contains(id)
            && let Some(path) = queue.known.get(id)
        {
            front.push((
                i,
                Job {
                    id: id.clone(),
                    path: path.clone(),
                },
            ));
        }
    }
    if front.is_empty() {
        queue.small = rest;
        return;
    }
    front.sort_by_key(|(i, _)| *i);
    let mut small: VecDeque<Job> = front.into_iter().map(|(_, j)| j).collect();
    small.append(&mut rest);
    queue.small = small;
}

/// What a thread does next.
enum Task {
    ShowLarge,
    Small,
    FillLarge,
}

fn work(shared: &Shared) {
    if let Some(cache) = &shared.cache {
        cache.list();
    }
    loop {
        let (job, task) = {
            let mut queue = shared.queue.lock().expect("the pool's queue");
            loop {
                if queue.closed {
                    return;
                }
                if let Some(job) = queue.show_large.pop_front() {
                    break (job, Task::ShowLarge);
                }
                if let Some(job) = queue.small.pop_front() {
                    break (job, Task::Small);
                }
                if let Some(job) = queue.large.pop_front() {
                    break (job, Task::FillLarge);
                }
                queue = shared.ready.wait(queue).expect("the pool's queue");
            }
        };
        let sent = match task {
            Task::FillLarge => {
                fill_large(shared, &job);
                true
            }
            Task::ShowLarge => match show_large(shared, &job) {
                Some(rgb) => (shared.sink)(&job.id, Outcome::Large(rgb)),
                None => true,
            },
            Task::Small => (shared.sink)(&job.id, small(shared, &job)),
        };
        if !sent {
            close(shared);
            return;
        }
    }
}

/// A thumbnail to show: from the cache, else read from the file (and
/// stored, its large entry queued).
fn small(shared: &Shared, job: &Job) -> Outcome {
    let key = key_of(&job.id);
    if let Some(cache) = &shared.cache
        && let Some(thumb) = cached(cache, job, &key, SMALL)
    {
        if job.path.is_some() && !cache.contains(&key, LARGE) {
            queue_large(shared, job);
        }
        return Outcome::Thumb(thumb);
    }
    let Some(path) = &job.path else {
        return Outcome::Unavailable;
    };
    match read::read(path, SMALL) {
        Ok(Some(thumb)) => {
            if let Some(cache) = &shared.cache {
                let _ = cache.put(&key, SMALL, &thumb);
                queue_large(shared, job);
            }
            Outcome::Thumb(thumb)
        }
        Ok(None) => Outcome::NoPreview,
        Err(e) => Outcome::Failed(e),
    }
}

/// The cached thumbnail of `job` at `size`, when it is still right. An
/// entry stored before this reader ([`crate::cache::READER`]) of a
/// picture its EXIF orientation turns was read sideways, and one of a
/// picture holding a grey stream beside its colour one (a phone's gain
/// map) may show that grey stream: their entries are dropped, to be read
/// again. One of another picture is marked and kept, never read again. A
/// raw's is kept as it is: its tag would cost reading the whole raw, for
/// every raw.
fn cached(cache: &ThumbCache, job: &Job, key: &Key, size: u32) -> Option<Rgb8> {
    let (rgb, current) = cache.get_marked(key, size)?;
    if current {
        return Some(rgb);
    }
    let Some(path) = job.path.as_deref().filter(|p| Kind::of(p) == Kind::Picture) else {
        return Some(rgb);
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Some(rgb);
    };
    if helios_load::picture_orientation(&bytes) == 1 && !read::has_grey_beside_colour(&bytes) {
        cache.mark(key);
        return Some(rgb);
    }
    cache.remove(key, SMALL);
    cache.remove(key, LARGE);
    None
}

fn queue_large(shared: &Shared, job: &Job) {
    shared
        .queue
        .lock()
        .expect("the pool's queue")
        .large
        .push_back(job.clone());
}

fn fill_large(shared: &Shared, job: &Job) {
    let (Some(cache), Some(path)) = (&shared.cache, &job.path) else {
        return;
    };
    let key = key_of(&job.id);
    if cache.contains(&key, LARGE) {
        return;
    }
    if let Ok(Some(thumb)) = read::read(path, LARGE) {
        let _ = cache.put(&key, LARGE, &thumb);
    }
}

/// A [`LARGE`] thumbnail to show: from the cache, else read from the file
/// (and stored).
fn show_large(shared: &Shared, job: &Job) -> Option<Rgb8> {
    let key = key_of(&job.id);
    if let Some(cache) = &shared.cache
        && let Some(rgb) = cached(cache, job, &key, LARGE)
    {
        return Some(rgb);
    }
    let thumb = read::read(job.path.as_ref()?, LARGE).ok()??;
    if let Some(cache) = &shared.cache {
        let _ = cache.put(&key, LARGE, &thumb);
    }
    Some(thumb)
}
