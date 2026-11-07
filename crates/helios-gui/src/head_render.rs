//! An image of the library rendered as it is edited: its file through
//! the workspace's network, its head version's edit applied, at the size
//! a view needs. The Compare panel draws a raw from it rather than from
//! the camera's embedded preview, which stays the first paint until the
//! render lands.
//!
//! A render runs on a thread of its own ([`spawn`]), through an engine
//! and its cache; the library keeps each one with the version it shows
//! and drops it when the edit changes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use helios_core::{Engine, Image, Region};
use helios_template::Bundle;
use helios_vault::edit::{self, EditSession};
use helios_vault::{ImageId, Store, Vault};

/// What a render of an image's head version gives back.
#[derive(Clone, Debug)]
pub struct HeadRender {
    /// The image's id (hex).
    pub id: String,
    /// The version rendered (hex); none for an image never edited.
    pub version: Option<String>,
    /// The longest side asked for.
    pub size: u32,
    /// The render, in the working space; or why there is none.
    pub image: Result<Image, String>,
}

/// The scale and region of a render of `extent` whose longest side is at
/// most `size` pixels, never above full resolution.
pub fn plan(extent: Region, size: u32) -> (f64, Region) {
    let long = extent.width.max(extent.height).max(1);
    let scale = (f64::from(size.max(1)) / f64::from(long)).min(1.0);
    let side = |n: u32| ((f64::from(n) * scale).ceil() as u32).max(1);
    let at = |v: i64| (v as f64 * scale).floor() as i64;
    (
        scale,
        Region::new(
            at(extent.x),
            at(extent.y),
            side(extent.width),
            side(extent.height),
        ),
    )
}

/// Renders `image` (its file at `path`) through `bundle`'s network with
/// its head version's edit, its longest side at most `size` pixels: the
/// version rendered, and the render. A raw goes through its sensor data,
/// never its embedded preview (see [`crate::templates::develop_file`]); a
/// part of the edit the network has no place for is left out, as the
/// darkroom does.
pub fn render<S: Store>(
    vault: &Vault<S>,
    engine: &Engine,
    bundle: &Bundle,
    path: &Path,
    image: ImageId,
    size: u32,
) -> Result<(Option<String>, Image), String> {
    let (mut graph, node) = crate::templates::develop_file(path, bundle)?;
    let (_, head) = EditSession::open(vault, image)?;
    if let Some(head) = &head {
        edit::apply_to_graph(&mut graph, &head.edit)?;
    }
    let extent = graph
        .extent(node)
        .map_err(|e| e.to_string())?
        .ok_or("the image has no extent")?;
    let (scale, region) = plan(extent, size);
    let out = engine
        .render_at(&graph, node, "output", region, scale)
        .map_err(|e| e.to_string())?;
    Ok((head.map(|h| h.id.hex()), out))
}

/// Renders on a thread of its own and hands the result to `send`; an
/// error when no thread could be started.
pub fn spawn<S: Store + 'static>(
    vault: Arc<Vault<S>>,
    engine: Arc<Engine>,
    bundle: Bundle,
    path: PathBuf,
    id: String,
    size: u32,
    send: impl FnOnce(HeadRender) + Send + 'static,
) -> Result<(), String>
where
    Vault<S>: Send + Sync,
{
    std::thread::Builder::new()
        .name("helios-head-render".into())
        .spawn(move || {
            let rendered = ImageId::from_hex(&id)
                .and_then(|image| render(&vault, &engine, &bundle, &path, image, size));
            let (version, image) = match rendered {
                Ok((version, image)) => (version, Ok(image)),
                Err(e) => (None, Err(e)),
            };
            send(HeadRender {
                id,
                version,
                size,
                image,
            });
        })
        .map(|_| ())
        .map_err(|e| format!("no thread to render the image: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_render_is_planned_at_the_size_asked_never_above_full_resolution() {
        let (scale, r) = plan(Region::sized(6000, 4000), 1500);
        assert!((scale - 0.25).abs() < 1e-12);
        assert_eq!((r.width, r.height), (1500, 1000));
        let (scale, r) = plan(Region::sized(100, 40), 1500);
        assert!((scale - 1.0).abs() < f64::EPSILON);
        assert_eq!((r.width, r.height), (100, 40));
    }
}
