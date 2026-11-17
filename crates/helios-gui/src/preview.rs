//! A small render of what the viewport shows: its node's output, the
//! whole image, its longest side at most [`SIDE`] pixels, in the working
//! space. The scopes measure it (the histogram reads the pipeline's
//! output, not the screen) and the navigator draws it.
//!
//! It renders on a thread of its own, from the viewport's engine and
//! cache, each time the graph shown changes (an edit, another image):
//! the interface never waits on it. A newer graph asked while one renders
//! replaces the older one, which is then never rendered.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helios_core::{Engine, Image, Region};

use crate::engine_source::Shown;

/// The longest side of the preview, in pixels.
pub const SIDE: u32 = 256;

#[derive(Default)]
struct State {
    /// Graphs asked so far, and renders done (failed ones included).
    asked: u64,
    done: u64,
    /// The last render and the request it answered.
    image: Option<(u64, Arc<Image>)>,
    /// Why the last render failed, when it did.
    failed: Option<String>,
    /// The window to wake when a render lands.
    ctx: Option<egui::Context>,
}

/// Where the preview stands, for the panels that follow it
/// ([`crate::photo_panels`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// A graph asked is still rendering.
    Busy,
    /// The last render failed: why.
    Failed(String),
    /// Every graph asked is rendered.
    Done,
}

/// The preview of the viewport's output.
pub struct Preview {
    jobs: Sender<(u64, Shown)>,
    state: Arc<(Mutex<State>, Condvar)>,
    /// The graph last asked for, to ask once per change.
    asked: Mutex<Option<Arc<helios_core::Graph>>>,
}

impl Preview {
    /// A preview rendering with `engine` (the viewport's: its cache is
    /// shared) on a thread that ends with it.
    pub fn new(engine: Arc<Engine>) -> Preview {
        let state: Arc<(Mutex<State>, Condvar)> = Arc::default();
        let (jobs, rx) = channel();
        let s = state.clone();
        std::thread::Builder::new()
            .name("helios-preview".into())
            .spawn(move || work(&engine, &rx, &s))
            .expect("a thread for the preview");
        Preview {
            jobs,
            state,
            asked: Mutex::default(),
        }
    }

    /// Follows what the viewport shows: a graph not rendered yet is asked
    /// for; the window `ctx` is woken when it lands.
    pub fn follow(&self, shown: Option<Shown>, ctx: &egui::Context) {
        let Some(shown) = shown else { return };
        let mut asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
        if asked.as_ref().is_some_and(|g| Arc::ptr_eq(g, &shown.graph)) {
            return;
        }
        *asked = Some(shown.graph.clone());
        let n = {
            let mut s = self.state.0.lock().unwrap_or_else(PoisonError::into_inner);
            s.asked += 1;
            s.ctx = Some(ctx.clone());
            s.asked
        };
        if self.jobs.send((n, shown)).is_err() {
            // The thread is gone: nothing will come.
            self.state
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .done = n;
        }
    }

    /// The last preview rendered, with a number that grows with each.
    pub fn latest(&self) -> Option<(u64, Arc<Image>)> {
        self.state
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .image
            .clone()
    }

    /// Where it stands: rendering, its last render failed, or done.
    pub fn status(&self) -> Status {
        let s = self.state.0.lock().unwrap_or_else(PoisonError::into_inner);
        if s.done < s.asked {
            Status::Busy
        } else if let Some(why) = &s.failed {
            Status::Failed(why.clone())
        } else {
            Status::Done
        }
    }

    /// Waits until every graph asked so far is rendered (or failed);
    /// false when `timeout` passes first.
    pub fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let (lock, cv) = &*self.state;
        let mut s = lock.lock().unwrap_or_else(PoisonError::into_inner);
        while s.done < s.asked {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            s = cv
                .wait_timeout(s, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

/// The scale and region of a preview of `extent`: its longest side at
/// most [`SIDE`] pixels, never above full resolution.
pub fn plan(extent: Region) -> (f64, Region) {
    let long = extent.width.max(extent.height).max(1);
    let scale = (f64::from(SIDE) / f64::from(long)).min(1.0);
    let size = |n: u32| ((f64::from(n) * scale).ceil() as u32).max(1);
    let at = |v: i64| (v as f64 * scale).floor() as i64;
    (
        scale,
        Region::new(
            at(extent.x),
            at(extent.y),
            size(extent.width),
            size(extent.height),
        ),
    )
}

fn work(engine: &Engine, rx: &Receiver<(u64, Shown)>, state: &(Mutex<State>, Condvar)) {
    while let Ok(mut job) = rx.recv() {
        // Only the newest graph is worth rendering.
        while let Ok(newer) = rx.try_recv() {
            job = newer;
        }
        let (n, shown) = job;
        let (scale, region) = plan(shown.extent);
        let span = helios_core::trace::span("preview.render", || {
            format!("{}x{} at {scale:.4}", region.width, region.height)
        });
        let image = engine
            .render_at(&shown.graph, shown.node, &shown.port, region, scale)
            .map_err(|e| {
                log::warn!("preview: {e}");
                e.to_string()
            });
        drop(span);
        let (lock, cv) = state;
        let mut s = lock.lock().unwrap_or_else(PoisonError::into_inner);
        match image {
            Ok(image) => {
                s.image = Some((n, Arc::new(image)));
                s.failed = None;
            }
            Err(why) => s.failed = Some(why),
        }
        s.done = n;
        if let Some(ctx) = &s.ctx {
            ctx.request_repaint();
        }
        cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_keeps_the_image_s_shape_within_its_side() {
        let (scale, r) = plan(Region::sized(6000, 4000));
        assert!((scale - 256.0 / 6000.0).abs() < 1e-12);
        assert_eq!((r.width, r.height), (256, 171));
        // A small image stays at full resolution.
        let (scale, r) = plan(Region::sized(100, 40));
        assert!((scale - 1.0).abs() < f64::EPSILON);
        assert_eq!((r.width, r.height), (100, 40));
    }

    /// The open path's tail leaves its trace spans in order: the preview
    /// rendered on its own thread (the engine's steps inside it), then the
    /// histogram computed on it.
    #[test]
    fn the_preview_then_its_histogram_leave_trace_spans_in_order() {
        let (listener, records) = helios_core::trace::collect();
        let mut graph = helios_core::Graph::new();
        let image = Image::from_fn(Region::sized(600, 400), 4, |x, y, _| {
            ((x + y) % 7) as f32 / 7.0
        });
        let node = graph.add_source("source", image);
        let shown = Shown::new(Arc::new(graph), node, helios_core::graph::SOURCE_PORT)
            .unwrap()
            .unwrap();
        let preview = Preview::new(Arc::new(Engine::new(Arc::new(helios_core::Bus::new()))));
        preview.follow(Some(shown), &egui::Context::default());
        assert!(preview.wait(Duration::from_secs(30)), "the preview renders");
        let (_, image) = preview.latest().unwrap();
        crate::scopes::picture_sized(&image, crate::scopes::Mode::Histogram, false, [64, 32]);
        drop(listener);
        let records = records.lock().unwrap();
        let thread = std::thread::current().name().map(String::from);
        let mine: Vec<_> = records
            .iter()
            .filter(|r| {
                r.thread.as_deref() == Some("helios-preview")
                    || (r.thread == thread && r.name.starts_with("scopes."))
            })
            .collect();
        let names: Vec<&str> = mine.iter().map(|r| r.name).collect();
        assert_eq!(
            names,
            ["engine.source", "preview.render", "scopes.histogram"],
            "{records:?}"
        );
        assert_eq!(mine[1].detail, "256x171 at 0.4267");
        assert_eq!(mine[2].detail, "256x171");
        assert!(mine[0].end() <= mine[1].end());
        assert!(mine[1].end() <= mine[2].start);
    }
}
