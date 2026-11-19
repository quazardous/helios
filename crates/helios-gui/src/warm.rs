//! The desktop's side of the photos' warm-up
//! ([`helios_node_core::warm`]): the `decoded` state of a photo of the
//! library (its file through the workspace's network, before its edit),
//! which the darkroom's open asks of the image-state router
//! ([`open`]) rather than decoding by itself, and the [`Book`] of the
//! photos near the shown one, where the warmer's recipe finds their files
//! and network to compute the same state ahead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use helios_core::{ContentHash, Graph, NodeId, Priority};
use helios_node_core::image_states::{
    Compute, ImageStates, Ready, StateError, StateKey, StateKind,
};
use helios_node_core::warm::Recipe;
use helios_template::Bundle;

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
const OPEN_WAIT: Duration = Duration::from_secs(600);

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

/// Where the photos around the shown one are and through which network
/// they are developed: kept by the darkroom as it says them near, read
/// by the warmer's recipe.
#[derive(Clone, Default)]
pub struct Book(Arc<Mutex<BTreeMap<String, (PathBuf, Bundle)>>>);

impl Book {
    /// The photos now around, each with its file and network.
    pub fn set(&self, photos: impl IntoIterator<Item = (String, PathBuf, Bundle)>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = photos
            .into_iter()
            .map(|(photo, path, bundle)| (photo, (path, bundle)))
            .collect();
    }

    /// The warmer's recipe: a photo of the book warms its `decoded`
    /// state; another nothing.
    pub fn recipe(&self) -> Recipe {
        let book = self.0.clone();
        Arc::new(move |photo: &str| {
            let book = book.lock().unwrap_or_else(PoisonError::into_inner);
            book.get(photo)
                .map(|(path, bundle)| {
                    vec![(
                        key(photo, path, bundle),
                        compute(path.clone(), bundle.clone()),
                    )]
                })
                .unwrap_or_default()
        })
    }
}
