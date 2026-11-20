//! The desktop's side of the photos' warm-up
//! ([`helios_node_core::warm`]): the `decoded` state of a photo of the
//! library (its file through the workspace's network, before its edit),
//! which the darkroom's open asks of the image-state router
//! ([`open()`]) rather than decoding by itself, and the [`Book`] of the
//! photos near the shown one, where the warmer's recipe finds their files
//! and network to compute the same state ahead.
//!
//! With an engine and the library's reads ([`Book::set_render`]), a photo
//! near is warmed further: its `developed` state ([`developed`]) applies
//! its head version's edit to the decoded graph, as the darkroom's open
//! does, and holds the full-size steps of its preview in the viewport's
//! engine, the preview rendered there. Opened, the photo's frame and
//! preview read them: no demosaic.
//!
//! Once the darkroom's viewport size is known ([`Book::follow_view`]),
//! a photo near is framed too: its `framed` state ([`framed`]) renders
//! the developed output whole at the scales the viewport asks for its
//! proxy and its fit, kept in the engine's cache, so the open's first
//! frame crops them instead of rendering. A new viewport size, once it
//! settles ([`VIEW_SETTLE`]), lets those frames go and frames the photos
//! near again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helios_core::{ContentHash, Engine, Graph, Hold, Image, NodeId, Priority, Region};
use helios_node_core::image_states::{
    Compute, ImageStates, Ready, StateError, StateKey, StateKind,
};
use helios_node_core::warm::Recipe;
use helios_template::Bundle;
use helios_vault::Version;
use serde_json::{Value as Json, json};

use crate::persist::Reader;

/// A photo decoded through a network: a graph to clone (its images are
/// shared) and the node it shows.
pub struct Decoded {
    pub graph: Graph,
    pub node: NodeId,
}

/// The `decoded` state of `photo` (a vault image's id), its file at
/// `path` through `bundle`'s network.
pub fn key(photo: &str, path: &Path, bundle: &Bundle) -> StateKey {
    let network = format!("{:?}{:?}", bundle.template, bundle.composites);
    let mut bytes = path.as_os_str().as_encoded_bytes().to_vec();
    bytes.push(0);
    bytes.extend_from_slice(network.as_bytes());
    StateKey {
        photo: photo.to_string(),
        development: ContentHash::of(&bytes),
        state: StateKind::Decoded,
        size: 0,
    }
}

/// How the `decoded` state is computed: the file opened through the
/// network ([`crate::templates::open_file`]), weighing its sources.
pub fn compute(path: PathBuf, bundle: Bundle) -> Compute {
    Arc::new(move || {
        let (graph, node) =
            crate::templates::open_file(&path, &bundle).map_err(StateError::Failed)?;
        let bytes = graph
            .ids()
            .filter_map(|id| graph.source_image(id).ok().flatten())
            .map(|image| image.bytes() as u64)
            .sum();
        Ok(Ready::new(Decoded { graph, node }, bytes))
    })
}

/// How long an open waits for a decode a warm-up started.
const OPEN_WAIT: Duration = Duration::from_mins(10);

/// `photo`'s file at `path` through `bundle`'s network, for the darkroom:
/// the router's `decoded` state, ready (warmed), joined (a warm-up in
/// flight, raised to interactive) or computed on the calling thread.
///
/// # Errors
/// Why the file cannot be opened through the network.
pub fn open(
    states: &ImageStates,
    photo: &str,
    path: &Path,
    bundle: &Bundle,
) -> Result<(Graph, NodeId), String> {
    let value = states
        .get_here(
            key(photo, path, bundle),
            Priority::Interactive,
            compute(path.to_path_buf(), bundle.clone()),
            OPEN_WAIT,
        )
        .map_err(|e| e.to_string())?;
    let decoded = value
        .downcast_ref::<Decoded>()
        .ok_or("the decoded state holds no graph")?;
    Ok((decoded.graph.clone(), decoded.node))
}

/// A photo developed ahead: the full-size steps its preview reads, held
/// in the engine's cache, the preview, and the developed graph and node
/// its frame is rendered from ([`framed`]).
pub struct Developed {
    _hold: Hold,
    pub preview: Arc<Image>,
    pub graph: Arc<Graph>,
    pub node: NodeId,
}

/// The `developed` state of `photo`: its `decoded` state's development
/// with its head version (`head`, none for a photo never edited).
pub fn developed_key(decoded: &StateKey, head: Option<&Version>) -> StateKey {
    let mut bytes = decoded.development.0.to_vec();
    if let Some(head) = head {
        bytes.extend_from_slice(head.id.hex().as_bytes());
    }
    StateKey {
        photo: decoded.photo.clone(),
        development: ContentHash::of(&bytes),
        state: StateKind::Developed,
        size: crate::preview::SIDE,
    }
}

/// How the `developed` state is computed: the `decoded` state taken from
/// `states` (ready, joined or computed here), `head`'s edit applied as
/// the darkroom's open applies it, then the preview rendered through
/// `engine` while holding the full-size steps it reads.
pub fn developed(
    states: &ImageStates,
    decoded: (StateKey, Compute),
    engine: Arc<Engine>,
    head: Option<Version>,
) -> Compute {
    let states = states.clone();
    Arc::new(move || {
        let value = states.get_here(
            decoded.0.clone(),
            Priority::Prefetch,
            decoded.1.clone(),
            OPEN_WAIT,
        )?;
        let source = value
            .downcast_ref::<Decoded>()
            .ok_or_else(|| StateError::Failed("the decoded state holds no graph".into()))?;
        let mut graph = source.graph.clone();
        if let Some(head) = &head {
            helios_vault::edit::apply_to_graph(&mut graph, &head.edit)
                .map_err(StateError::Failed)?;
        }
        let node = source.node;
        let extent = graph
            .extent(node)?
            .ok_or_else(|| StateError::Failed("the image has no extent".into()))?;
        let (scale, region) = crate::preview::plan(extent);
        let hold = engine.hold_at(&graph, node, "output", region, scale)?;
        let preview = engine.render_at(&graph, node, "output", region, scale)?;
        let bytes = hold.bytes() + preview.bytes() as u64;
        Ok(Ready::held(
            Developed {
                _hold: hold,
                preview: Arc::new(preview),
                graph: Arc::new(graph),
                node,
            },
            bytes,
        ))
    })
}

/// The darkroom's viewport as the frame's scales depend on it: its
/// rectangle in points and the physical pixels per point of its window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewSize {
    pub rect: egui::Rect,
    pub ppp: f32,
}

impl ViewSize {
    /// The viewport in physical pixels, rounded as egui-pop's mapping
    /// rounds it: `[width, height]`.
    pub fn pixels(&self) -> [u32; 2] {
        let p = f64::from(self.ppp);
        let px = |v: f32| (f64::from(v) * p).round();
        [
            (px(self.rect.max.x) - px(self.rect.min.x)).max(0.0) as u32,
            (px(self.rect.max.y) - px(self.rect.min.y)).max(0.0) as u32,
        ]
    }

    /// The scales the viewport asks an image of `extent` at, fitted: its
    /// proxy's and its exact tiles' (egui-pop's `proxy_scale` and
    /// `exact_scale`), the same numbers to the bit, so the tiles find
    /// what was rendered ahead.
    pub fn scales(&self, extent: Region) -> (f64, f64) {
        use egui_pop::viewport::{tiles, view};
        let image = [f64::from(extent.width), f64::from(extent.height)];
        let m = view::Mapping::new(&view::View::fit(), image, self.rect, self.ppp);
        (tiles::proxy_scale(&m), tiles::exact_scale(&m))
    }
}

/// The whole of `extent` at `scale`, placed as the viewport places its
/// tiles (`engine_source::region_of`): a tile at any scale is inside it.
pub fn whole_at(extent: Region, scale: f64) -> Region {
    let at = |v: i64| (v as f64 * scale).floor() as i64;
    let size = |n: u32| ((f64::from(n) * scale).ceil() as u32).max(1);
    Region::new(
        at(extent.x),
        at(extent.y),
        size(extent.width),
        size(extent.height),
    )
}

/// A photo's frame rendered ahead for one viewport size: the proxy and
/// the fit kept in the engine's cache.
pub struct Framed {
    _holds: Vec<Hold>,
    /// The scales they were rendered at: the proxy's, the fit's.
    pub scales: (f64, f64),
}

/// The `framed` state of a photo developed as `developed` says, for the
/// viewport `view`.
pub fn framed_key(developed: &StateKey, view: ViewSize) -> StateKey {
    let [w, h] = view.pixels();
    let mut bytes = developed.development.0.to_vec();
    bytes.extend_from_slice(&w.to_le_bytes());
    bytes.extend_from_slice(&h.to_le_bytes());
    // The rectangle's position counts too: the pixels round per edge.
    for v in [view.rect.min.x, view.rect.min.y, view.ppp] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    StateKey {
        photo: developed.photo.clone(),
        development: ContentHash::of(&bytes),
        state: StateKind::Framed,
        size: w.max(h),
    }
}

/// How the `framed` state is computed: the `developed` state taken from
/// `states` (ready, joined or computed here), then its output rendered
/// whole at the viewport's proxy scale and at its fit, each kept in
/// `engine`'s cache ([`Engine::keep_at`]): the open's first tiles crop
/// them.
pub fn framed(
    states: &ImageStates,
    developed: (StateKey, Compute),
    engine: Arc<Engine>,
    view: ViewSize,
) -> Compute {
    let states = states.clone();
    Arc::new(move || {
        let value = states.get_here(
            developed.0.clone(),
            Priority::Prefetch,
            developed.1.clone(),
            OPEN_WAIT,
        )?;
        let d = value
            .downcast_ref::<Developed>()
            .ok_or_else(|| StateError::Failed("the developed state holds no graph".into()))?;
        let extent = d
            .graph
            .extent(d.node)?
            .ok_or_else(|| StateError::Failed("the image has no extent".into()))?;
        let (proxy, exact) = view.scales(extent);
        let keep =
            |scale: f64| engine.keep_at(&d.graph, d.node, "output", whole_at(extent, scale), scale);
        let mut holds = vec![keep(proxy)?];
        if exact != proxy {
            holds.push(keep(exact)?);
        }
        let bytes = holds.iter().map(Hold::bytes).sum();
        Ok(Ready::held(
            Framed {
                _holds: holds,
                scales: (proxy, exact),
            },
            bytes,
        ))
    })
}

/// A photo's head version, read through the node (`library.edit.head`).
///
/// # Errors
/// The read's.
pub fn head(reader: &Reader, photo: &str) -> Result<Option<Version>, String> {
    let read = reader.read("library.edit.head", json!({ "image": photo }))?;
    match &read["head"] {
        Json::Null => Ok(None),
        row => serde_json::from_value(row.clone())
            .map(Some)
            .map_err(|e| e.to_string()),
    }
}

/// What the book develops the photos near with.
#[derive(Clone)]
struct Render {
    states: ImageStates,
    engine: Arc<Engine>,
    reader: Reader,
}

/// How long the darkroom's viewport keeps one size before the photos
/// near are framed again at it: a window being resized changes it every
/// frame, and only the size it stops at is worth rendering.
pub const VIEW_SETTLE: Duration = Duration::from_millis(300);

/// The viewport sizes the book follows.
#[derive(Default)]
struct Views {
    /// The size the photos near are framed at (none: not known yet, they
    /// are developed only).
    framed: Option<ViewSize>,
    /// The last size seen, and since when, while it differs.
    seen: Option<(ViewSize, Instant)>,
}

/// Where the photos around the shown one are and through which network
/// they are developed: kept by the darkroom as it says them near, read
/// by the warmer's recipe.
#[derive(Clone, Default)]
pub struct Book {
    photos: Arc<Mutex<BTreeMap<String, (PathBuf, Bundle)>>>,
    render: Arc<Mutex<Option<Render>>>,
    views: Arc<Mutex<Views>>,
}

impl Book {
    /// Develops the photos near ahead, not only decodes them: their head
    /// edit read through `reader`, their preview's full-size steps held
    /// in `engine` (the viewport's) under `states`.
    pub fn set_render(&self, states: ImageStates, engine: Arc<Engine>, reader: Reader) {
        *self.render.lock().unwrap_or_else(PoisonError::into_inner) = Some(Render {
            states,
            engine,
            reader,
        });
    }

    /// The darkroom's viewport is `view` now (`now` being when): true
    /// once a size other than the one the photos near are framed at has
    /// stayed [`VIEW_SETTLE`] (at once for the first size known), the
    /// book then framing at it. The caller lets the frames rendered at the
    /// old size go and has the photos near warmed again
    /// ([`helios_node_core::warm::Warmer::rewarm`]). False while nothing
    /// changes or the size has not settled: [`Book::view_pending`] says
    /// when to look again.
    pub fn follow_view(&self, view: ViewSize, now: Instant) -> bool {
        let mut views = self.views.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(framed) = views.framed else {
            views.framed = Some(view);
            views.seen = None;
            return true;
        };
        if framed == view {
            views.seen = None;
            return false;
        }
        match views.seen {
            Some((seen, since)) if seen == view => {
                if now.duration_since(since) >= VIEW_SETTLE {
                    views.framed = Some(view);
                    views.seen = None;
                    true
                } else {
                    false
                }
            }
            _ => {
                views.seen = Some((view, now));
                false
            }
        }
    }

    /// How long until a size seen settles, while one waits.
    pub fn view_pending(&self, now: Instant) -> Option<Duration> {
        let views = self.views.lock().unwrap_or_else(PoisonError::into_inner);
        views
            .seen
            .map(|(_, since)| (since + VIEW_SETTLE).saturating_duration_since(now))
    }

    /// The viewport size the photos near are framed at.
    pub fn view(&self) -> Option<ViewSize> {
        self.views
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .framed
    }

    /// The photos now around, each with its file and network.
    pub fn set(&self, photos: impl IntoIterator<Item = (String, PathBuf, Bundle)>) {
        *self.photos.lock().unwrap_or_else(PoisonError::into_inner) = photos
            .into_iter()
            .map(|(photo, path, bundle)| (photo, (path, bundle)))
            .collect();
    }

    /// The warmer's recipe: a photo of the book warms its `framed` state
    /// at the viewport's size (its `developed` and `decoded` ones on the
    /// way), its `developed` state while the size is not known, or its
    /// `decoded` state alone with nothing to render with; another photo
    /// nothing.
    pub fn recipe(&self) -> Recipe {
        let (book, render, views) = (self.photos.clone(), self.render.clone(), self.views.clone());
        Arc::new(move |photo: &str| {
            let found = book
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(photo)
                .cloned();
            let Some((path, bundle)) = found else {
                return Vec::new();
            };
            let decoded = (key(photo, &path, &bundle), compute(path, bundle));
            let render = render
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let Some(r) = render else {
                return vec![decoded];
            };
            match head(&r.reader, photo) {
                Ok(head) => {
                    let key = developed_key(&decoded.0, head.as_ref());
                    let developed = (
                        key.clone(),
                        developed(&r.states, decoded, r.engine.clone(), head),
                    );
                    let view = views.lock().unwrap_or_else(PoisonError::into_inner).framed;
                    match view {
                        // One ask: the frame's computation takes the
                        // developed state on its own thread, never
                        // queued behind it on the router's pool.
                        Some(view) => vec![(
                            framed_key(&key, view),
                            framed(&r.states, developed, r.engine, view),
                        )],
                        None => vec![developed],
                    }
                }
                Err(e) => {
                    log::warn!("warm-up of {photo}: its edit: {e}");
                    vec![decoded]
                }
            }
        })
    }
}
