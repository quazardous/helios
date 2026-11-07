//! The pool that fills the library's thumbnails: a few threads, the
//! cache first, a file read only on a miss, the images in view first.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, Weak};

use crate::cache::{Key, ThumbCache};
use crate::{LARGE, Rgb8, SMALL, read};

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
    /// Thumbnails to show, in the order wanted.
    small: VecDeque<Job>,
    /// [`LARGE`] entries to fill once nothing is waiting to be shown.
    large: VecDeque<Job>,
    closed: bool,
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
    /// thumbnail is read from its file), sending to `sink`.
    pub fn start(
        cache: Option<Arc<ThumbCache>>,
        threads: usize,
        sink: impl Fn(&str, Outcome) -> bool + Send + Sync + 'static,
    ) -> Loader {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                small: VecDeque::new(),
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
        queue.small.extend(jobs);
        drop(queue);
        self.shared.ready.notify_all();
    }

    /// The images in view, to read before the others (in this order).
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

    /// An image's [`LARGE`] thumbnail, when the cache holds it (Compare's
    /// and the loupe's first paint).
    pub fn large(&self, id: &str) -> Option<Rgb8> {
        self.shared.cache.as_ref()?.get(&Key::preview(id), LARGE)
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

fn prefer(shared: &Shared, ids: &[String]) {
    let mut queue = shared.queue.lock().expect("the pool's queue");
    let place: HashMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let mut front: Vec<(usize, Job)> = Vec::new();
    let mut rest = VecDeque::with_capacity(queue.small.len());
    for job in queue.small.drain(..) {
        match place.get(job.id.as_str()) {
            Some(&i) => front.push((i, job)),
            None => rest.push_back(job),
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

fn work(shared: &Shared) {
    loop {
        let (job, large) = {
            let mut queue = shared.queue.lock().expect("the pool's queue");
            loop {
                if queue.closed {
                    return;
                }
                if let Some(job) = queue.small.pop_front() {
                    break (job, false);
                }
                if let Some(job) = queue.large.pop_front() {
                    break (job, true);
                }
                queue = shared.ready.wait(queue).expect("the pool's queue");
            }
        };
        if large {
            fill_large(shared, &job);
        } else if !(shared.sink)(&job.id, small(shared, &job)) {
            close(shared);
            return;
        }
    }
}

/// A thumbnail to show: from the cache, else read from the file (and
/// stored, its large entry queued).
fn small(shared: &Shared, job: &Job) -> Outcome {
    let key = Key::preview(&job.id);
    if let Some(cache) = &shared.cache
        && let Some(thumb) = cache.get(&key, SMALL)
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
    let key = Key::preview(&job.id);
    if cache.contains(&key, LARGE) {
        return;
    }
    if let Ok(Some(thumb)) = read::read(path, LARGE) {
        let _ = cache.put(&key, LARGE, &thumb);
    }
}
