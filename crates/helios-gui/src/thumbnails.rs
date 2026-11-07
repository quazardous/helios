//! The library's thumbnails: the cache beside the catalogue and the pool
//! that fills the library from it (see `helios_thumbs`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_thumbs::{Job, Loader, Outcome, ThumbCache};

use crate::shell::Shell;

/// The setting that limits the cache's size, in MiB.
pub const CACHE_LIMIT: &str = "system.cache.thumbnails";

/// The cache's size limit, in bytes, from the user's settings layer (its
/// TOML text; none: the default).
///
/// # Errors
/// The layer does not read.
pub fn cache_limit(user_layer: Option<&str>) -> Result<u64, String> {
    let mut config = helios_config::Config::new(helios_config::Schema::core());
    if let Some(text) = user_layer {
        config
            .load_str(helios_config::Layer::User, text)
            .map_err(|problems| {
                problems
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            })?;
    }
    Ok((config.number(CACHE_LIMIT).max(1.0) as u64) * 1024 * 1024)
}

/// Opens the cache of the library whose catalogue is `vault_file`
/// (`.helios/thumbs` beside it).
///
/// # Errors
/// Its folder cannot be made.
pub fn open_cache(vault_file: &Path, limit: u64) -> Result<Arc<ThumbCache>, String> {
    let file = std::path::absolute(vault_file).unwrap_or_else(|_| vault_file.to_path_buf());
    ThumbCache::open(&helios_thumbs::cache_dir(&file), limit).map(Arc::new)
}

/// Starts filling the library's thumbnails: every image without one, the
/// cache first (an offline photo shows its cached thumbnail), its file
/// read on a miss (`files`: where each image's file is), the cells in
/// view first. The pool stops when the returned loader is dropped.
pub fn start(
    shell: &Shell,
    cache: Option<Arc<ThumbCache>>,
    files: &[(String, PathBuf)],
    warn: impl Fn(&str) + Send + Sync + 'static,
) -> Loader {
    let sink = shell.library_thumbnails();
    let loader = Loader::start(
        cache,
        helios_thumbs::workers(),
        move |id, outcome| match outcome {
            Outcome::Thumb(rgb) => sink.send_srgb(id.to_string(), rgb),
            Outcome::NoPreview | Outcome::Unavailable => sink.send_no_preview(id.to_string()),
            Outcome::Failed(e) => {
                warn(&format!("no thumbnail for {id}: {e}"));
                sink.send_no_preview(id.to_string())
            }
        },
    );
    let mut library = shell.library_mut();
    let paths: std::collections::HashMap<&str, &PathBuf> =
        files.iter().map(|(id, p)| (id.as_str(), p)).collect();
    let jobs: Vec<Job> = library
        .entries
        .iter()
        .filter(|e| !library.has_thumbnail(&e.id))
        .map(|e| Job {
            id: e.id.clone(),
            path: paths.get(e.id.as_str()).map(|p| (*p).clone()),
        })
        .collect();
    loader.load(jobs);
    // The cells already in view, then each change of them.
    loader.prefer(&library.in_view());
    library.set_on_view(loader.preferrer());
    loader
}
