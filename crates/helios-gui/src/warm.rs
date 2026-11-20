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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use helios_core::{ContentHash, Engine, Graph, Hold, Image, NodeId, Priority};
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
/// in the engine's cache, and the preview.
pub struct Developed {
    _hold: Hold,
    pub preview: Arc<Image>,
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

/// Where the photos around the shown one are and through which network
/// they are developed: kept by the darkroom as it says them near, read
/// by the warmer's recipe.
#[derive(Clone, Default)]
pub struct Book {
    photos: Arc<Mutex<BTreeMap<String, (PathBuf, Bundle)>>>,
    render: Arc<Mutex<Option<Render>>>,
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

    /// The photos now around, each with its file and network.
    pub fn set(&self, photos: impl IntoIterator<Item = (String, PathBuf, Bundle)>) {
        *self.photos.lock().unwrap_or_else(PoisonError::into_inner) = photos
            .into_iter()
            .map(|(photo, path, bundle)| (photo, (path, bundle)))
            .collect();
    }

    /// The warmer's recipe: a photo of the book warms its `developed`
    /// state (its `decoded` one on the way), or its `decoded` state alone
    /// with nothing to render with; another photo nothing.
    pub fn recipe(&self) -> Recipe {
        let (book, render) = (self.photos.clone(), self.render.clone());
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
                    vec![(key, developed(&r.states, decoded, r.engine, head))]
                }
                Err(e) => {
                    log::warn!("warm-up of {photo}: its edit: {e}");
                    vec![decoded]
                }
            }
        })
    }
}
