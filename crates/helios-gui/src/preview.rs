//! A small render of what the viewport shows: its node's output, the
//! whole image, its longest side at most [`SIDE`] pixels, in the working
//! space. The scopes measure it (the histogram reads the pipeline's
//! output, not the screen) and the navigator draws it.
//!
//! It is a state of the image-state router
//! ([`helios_node_core::image_states`]): asked from a thread of its own
//! each time the graph shown changes (an edit, another image), rendered
//! from the viewport's engine and cache on the router's pool, shared with
//! any other asker of the same state: the interface never waits on it. A
//! newer graph asked while one renders replaces the older one, whose ask
//! is withdrawn (its computation stops unless another asker wants it).
//!
//! The follower also asks for the shown photo's full-size step
//! ([`StateKind::FullSize`], [`hold`]) and pins it while the photo is
//! shown: under memory pressure the fit, a zoom and the next edit read it
//! instead of demosaicing again. Once another photo is shown it stays
//! kept as long as the router's budget allows, unpinned.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helios_core::{Engine, Graph, Image, NodeId, Priority, Region};
use helios_node_core::image_states::{
    Compute, Handle, ImageStates, Ready, StateError, StateKey, StateKind, StatePin, Value,
};

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
    /// A preview asked of `states`, rendered with `engine` (the
    /// viewport's: its cache is shared), followed on a thread that ends
    /// with it.
    pub fn new(engine: Arc<Engine>, states: ImageStates) -> Preview {
        let state: Arc<(Mutex<State>, Condvar)> = Arc::default();
        let (jobs, rx) = channel();
        let s = state.clone();
        std::thread::Builder::new()
            .name("helios-preview".into())
            .spawn(move || work(&engine, &states, &rx, &s))
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

/// Asks `states` for the preview of `node`'s output (`port`) in `graph`,
/// whose extent is `extent`, rendered with `engine` at `priority`.
///
/// # Errors
/// The engine's, when it cannot plan the render.
pub fn ask(
    states: &ImageStates,
    engine: &Arc<Engine>,
    graph: &Arc<Graph>,
    node: NodeId,
    port: &str,
    extent: Region,
    priority: Priority,
) -> Result<Handle, helios_core::Error> {
    let (scale, region) = plan(extent);
    let key = StateKey {
        photo: format!("graph-{}", graph.id()),
        development: engine.key_at(graph, node, region, scale)?,
        state: StateKind::Preview,
        size: SIDE,
    };
    let (engine, graph, port) = (engine.clone(), graph.clone(), port.to_string());
    let compute: Compute = Arc::new(move || {
        let _span = helios_core::trace::span("preview.render", || {
            format!("{}x{} at {scale:.4}", region.width, region.height)
        });
        let image = engine.render_at(&graph, node, &port, region, scale)?;
        let bytes = image.bytes() as u64;
        Ok(Ready::new(image, bytes))
    });
    Ok(states.ask(key, priority, compute))
}

/// Asks `states` to keep the full-size steps the preview of `node`'s
/// output reads (the demosaic's output) in `engine`'s cache: a
/// [`StateKind::FullSize`] state holding them ([`Engine::hold_at`]),
/// computed with the preview's own render, so it shares the preview's
/// steps. `None` when the render reads no full-size step (a source
/// shown as is).
///
/// # Errors
/// The engine's, when it cannot plan the render.
pub fn hold(
    states: &ImageStates,
    engine: &Arc<Engine>,
    graph: &Arc<Graph>,
    node: NodeId,
    port: &str,
    extent: Region,
    priority: Priority,
) -> Result<Option<Handle>, helios_core::Error> {
    let (scale, region) = plan(extent);
    let keys = engine.full_size_keys(graph, node, region, scale)?;
    if keys.is_empty() {
        return Ok(None);
    }
    let mut development = Vec::with_capacity(keys.len() * 32);
    for k in &keys {
        development.extend_from_slice(&k.0);
    }
    let key = StateKey {
        photo: format!("graph-{}", graph.id()),
        development: helios_core::ContentHash::of(&development),
        state: StateKind::FullSize,
        size: 0,
    };
    let (engine, graph, port) = (engine.clone(), graph.clone(), port.to_string());
    let compute: Compute = Arc::new(move || {
        let held = engine.hold_at(&graph, node, &port, region, scale)?;
        let bytes = held.bytes();
        Ok(Ready::held(held, bytes))
    });
    Ok(Some(states.ask(key, priority, compute)))
}

/// The shown photo's full-size step: the ask (kept until it lands) and
/// the pin that keeps it from eviction while the photo is shown.
struct Kept {
    _ask: Handle,
    pin: StatePin,
}

/// Moves the shown photo's full-size hold to `shown`'s: the same steps
/// (an edit downstream) keep their ask and pin; others replace them, the
/// new pin taken before the old one goes.
fn keep_full_size(
    kept: &mut Option<Kept>,
    states: &ImageStates,
    engine: &Arc<Engine>,
    shown: &Shown,
) {
    match hold(
        states,
        engine,
        &shown.graph,
        shown.node,
        &shown.port,
        shown.extent,
        Priority::Interactive,
    ) {
        Ok(Some(ask)) => {
            if kept
                .as_ref()
                .is_some_and(|k| k.pin.key().development == ask.key().development)
            {
                return;
            }
            let pin = states.pin(ask.key());
            *kept = Some(Kept { _ask: ask, pin });
        }
        Ok(None) => *kept = None,
        Err(e) => log::warn!("preview: the full-size step: {e}"),
    }
}

/// A preview state's image.
pub fn image(value: Value) -> Option<Arc<Image>> {
    value.downcast::<Image>().ok()
}

/// How often the follower looks for a newer graph while a preview renders.
const LOOK: Duration = Duration::from_millis(20);

fn work(
    engine: &Arc<Engine>,
    states: &ImageStates,
    rx: &Receiver<(u64, Shown)>,
    state: &(Mutex<State>, Condvar),
) {
    let mut next = None;
    let mut full_size = None;
    while let Some(mut job) = next.take().or_else(|| rx.recv().ok()) {
        // Only the newest graph is worth rendering.
        while let Ok(newer) = rx.try_recv() {
            job = newer;
        }
        let (n, shown) = job;
        keep_full_size(&mut full_size, states, engine, &shown);
        let asked = ask(
            states,
            engine,
            &shown.graph,
            shown.node,
            &shown.port,
            shown.extent,
            Priority::Interactive,
        );
        let image = match asked {
            Err(e) => Err(e.to_string()),
            Ok(mut handle) => loop {
                match handle.wait(LOOK) {
                    Err(StateError::Timeout) => {
                        // A newer graph withdraws this ask (dropping it).
                        if let Ok(newer) = rx.try_recv() {
                            next = Some(newer);
                            break Err(StateError::Cancelled.to_string());
                        }
                    }
                    Ok(value) => {
                        break image(value).ok_or_else(|| "not an image".to_string());
                    }
                    Err(e) => break Err(e.to_string()),
                }
            },
        };
        if next.is_some() {
            // Superseded: the newer graph's render answers for it.
            continue;
        }
        if let Err(e) = &image {
            log::warn!("preview: {e}");
        }
        let (lock, cv) = state;
        let mut s = lock.lock().unwrap_or_else(PoisonError::into_inner);
        match image {
            Ok(image) => {
                s.image = Some((n, image));
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

    /// An open says the preview's state on the bus, transition by
    /// transition, and a second window asking the same preview shares it.
    #[test]
    fn an_open_says_the_preview_state_in_order_and_shares_it() {
        use helios_node_core::image_states::facts;
        let bus = Arc::new(helios_core::Bus::new());
        let states = ImageStates::new(bus.clone(), 2);
        let engine = Arc::new(Engine::new(bus.clone()));
        let mut graph = helios_core::Graph::new();
        let image = Image::from_fn(Region::sized(600, 400), 4, |x, y, _| {
            ((x * y) % 5) as f32 / 5.0
        });
        let node = graph.add_source("source", image);
        let shown = Shown::new(Arc::new(graph), node, helios_core::graph::SOURCE_PORT)
            .unwrap()
            .unwrap();
        let ctx = egui::Context::default();
        let one = Preview::new(engine.clone(), states.clone());
        let two = Preview::new(engine, states.clone());
        one.follow(Some(shown.clone()), &ctx);
        assert!(one.wait(Duration::from_secs(30)), "the preview renders");
        two.follow(Some(shown), &ctx);
        assert!(two.wait(Duration::from_secs(30)), "the second one too");
        assert!(Arc::ptr_eq(
            &one.latest().unwrap().1,
            &two.latest().unwrap().1
        ));
        let kinds: Vec<&str> = bus
            .since(0)
            .into_iter()
            .map(|e| e.kind)
            .filter(|k| k.starts_with("helios.image.state."))
            .collect();
        assert_eq!(kinds, [facts::REQUESTED, facts::COMPUTING, facts::READY]);
        let c = states.counters();
        assert_eq!((c.asks, c.computed, c.ready_hits), (2, 1, 1), "{c:?}");
    }

    /// The open path's tail leaves its trace spans in order: the preview
    /// rendered on the router's pool (the engine's steps inside it), then the
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
        let bus = Arc::new(helios_core::Bus::new());
        let states = ImageStates::new(bus.clone(), 1);
        let prefix = states.threads_prefix().to_string();
        let preview = Preview::new(Arc::new(Engine::new(bus)), states);
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
                r.thread.as_deref().is_some_and(|t| t.starts_with(&prefix))
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

    /// A demosaic stand-in: it needs full resolution.
    struct FullSize;

    impl helios_core::Module for FullSize {
        type Plan = ();

        fn manifest(&self) -> helios_core::Manifest {
            use helios_core::*;
            Manifest {
                name: "full_size".into(),
                version: 1,
                description: "a demosaic stand-in".into(),
                params: vec![],
                inputs: vec![PortSpec::image("input", 1)],
                outputs: vec![PortSpec::image("output", 1)],
                context: vec![],
                needs: Needs {
                    overlap: 1,
                    full_resolution: true,
                    ..Needs::default()
                },
                kind: ModuleKind::Process,
                family: Family::Detail,
                role: Role::General,
            }
        }

        fn prepare(&self, _: &helios_core::Params, _: &helios_core::Context) -> Result<(), String> {
            Ok(())
        }

        fn process(
            &self,
            (): &(),
            inputs: &[&Image],
            output: Region,
        ) -> Result<Vec<Image>, String> {
            let input = inputs[0];
            Ok(vec![Image::from_fn(output, 1, |x, y, _| {
                (input.pixel(x - 1, y)[0] + input.pixel(x + 1, y)[0]) * 0.5
            })])
        }
    }

    /// A photo whose full-size step is named `name` (the spans say it).
    fn photo(name: &str, seed: u32) -> Shown {
        let mut g = Graph::new();
        let src = g.add_source(
            "src",
            Image::from_fn(Region::sized(600, 400), 1, move |x, y, _| {
                ((x * 7 + y * 13 + i64::from(seed)) % 97) as f32
            }),
        );
        let node = g
            .add_module(
                name,
                helios_core::ModuleRef::new(FullSize),
                &helios_core::Params::new(),
            )
            .unwrap();
        g.connect(src, helios_core::graph::SOURCE_PORT, node, "input")
            .unwrap();
        Shown::new(Arc::new(g), node, "output").unwrap().unwrap()
    }

    /// The full-size steps of the node `name` computed, as the engine's
    /// spans say.
    fn demosaics(records: &Mutex<Vec<helios_core::trace::Record>>, name: &str) -> usize {
        let node = format!("node={name} ");
        records
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.name == "engine.step" && r.detail.contains(&node))
            .count()
    }

    /// Waits until `states` holds a ready full-size state of `shown`.
    fn held(states: &ImageStates, engine: &Arc<Engine>, shown: &Shown) {
        let (scale, region) = plan(shown.extent);
        let keys = engine
            .full_size_keys(&shown.graph, shown.node, region, scale)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !states.list().0.iter().any(|k| {
            k.state == StateKind::FullSize && k.photo == format!("graph-{}", shown.graph.id())
        }) {
            assert!(Instant::now() < deadline, "the full-size step never held");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(keys.len(), 1, "one full-size step");
    }

    fn fit(engine: &Engine, shown: &Shown) {
        engine
            .render_at(
                &shown.graph,
                shown.node,
                "output",
                Region::sized(300, 200),
                0.5,
            )
            .unwrap();
    }

    /// Under memory pressure, the shown photo's full-size step
    /// is kept, so fitting it again demosaics nothing; once another photo
    /// is shown and the router lets it go, the pressure evicts it.
    #[test]
    fn the_shown_photo_s_full_size_step_survives_memory_pressure() {
        let (listener, records) = helios_core::trace::collect();
        let bus = Arc::new(helios_core::Bus::new());
        let states = ImageStates::new(bus.clone(), 2);
        let engine = Arc::new(Engine::new(bus));
        let (a, b) = (photo("demosaic-r3-a", 1), photo("demosaic-r3-b", 2));
        let preview = Preview::new(engine.clone(), states.clone());
        let ctx = egui::Context::default();
        preview.follow(Some(a.clone()), &ctx);
        assert!(preview.wait(Duration::from_secs(30)), "A's preview renders");
        held(&states, &engine, &a);
        fit(&engine, &a);
        assert_eq!(
            demosaics(&records, "demosaic-r3-a"),
            1,
            "one demosaic per open"
        );
        // Room for one full-size step (600×400 floats: 960 kB) and a little.
        engine.set_budget(Some(1_200_000));
        // Another photo rendered (a compare head, an export): its own
        // full-size step needs the room.
        fit(&engine, &b);
        assert_eq!(demosaics(&records, "demosaic-r3-b"), 1);
        assert!(
            engine.counters().evictions > 0,
            "the budget forced evictions"
        );
        fit(&engine, &a);
        assert_eq!(
            demosaics(&records, "demosaic-r3-a"),
            1,
            "the shown photo's re-fit read its kept full-size step"
        );
        // B shown, the router keeping only what fits 1 MB: A's hold goes,
        // and the pressure evicts its step.
        states.set_budget(Some(1_000_000));
        preview.follow(Some(b.clone()), &ctx);
        assert!(preview.wait(Duration::from_secs(30)), "B's preview renders");
        held(&states, &engine, &b);
        // Evicted while A was shown, B's step was computed again.
        assert_eq!(demosaics(&records, "demosaic-r3-b"), 2);
        fit(&engine, &b);
        fit(&engine, &a);
        drop(listener);
        assert_eq!(
            demosaics(&records, "demosaic-r3-a"),
            2,
            "unpinned and let go, A's step was evicted"
        );
        assert_eq!(
            demosaics(&records, "demosaic-r3-b"),
            2,
            "B's kept while shown"
        );
        assert!(states.counters().evicted >= 1);
    }
}
