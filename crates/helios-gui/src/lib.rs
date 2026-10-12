//! Helios's desktop: an egui-pop application around Helios's panels (on
//! egui-pop's workbench), its commands run by Helios's kernel, and the
//! framework's own pictures and recordings of it.

pub mod dialogs;
pub mod engine_source;
pub mod host;
pub mod library;
pub mod masks;
pub mod params;
pub mod persist;
mod plugins;
pub mod scopes;
pub mod shell;
pub mod snapshot;
pub mod theme;
pub mod viewport;
mod windows;
mod workbench;

pub use shell::{Shell, headless, name_rect};
pub use workbench::{IMAGE_FIRST, area_named, image_first};

use egui_pop::model::ids::PanelId;
use egui_pop::model::panels::{Area, PanelSpec};

/// The panels Helios declares today, with the areas each may be moved to
/// (egui-pop's placement rules); each shows a placeholder until its
/// widget lands. Every panel may be detached into a window of its own.
pub fn helios_panels() -> Vec<PanelSpec> {
    use Area::{Bottom, Center, EndSide, StartSide};
    let sides = [StartSide, EndSide, Bottom];
    [
        ("helios.viewport", "Image", Center, &[Center][..]),
        ("helios.graph", "Graph", Center, &[Center][..]),
        ("helios.vault", "Vault", StartSide, &sides[..]),
        ("helios.modules", "Modules", StartSide, &sides[..]),
        ("helios.params", "Parameters", EndSide, &sides[..]),
        ("helios.scopes", "Scopes", EndSide, &sides[..]),
        ("helios.filmstrip", "Filmstrip", Bottom, &sides[..]),
        ("helios.queue", "Queue", Bottom, &[][..]),
        ("helios.log", "Log", Bottom, &[][..]),
    ]
    .into_iter()
    .map(|(id, title, area, areas)| {
        let id = PanelId::new(id).expect("Helios's panel ids are valid");
        let spec = PanelSpec::new(id, title, area).only_in(areas);
        // The sizes Helios's areas open at.
        match area {
            Center => spec,
            Bottom => spec.default_size(200.0),
            _ => spec.default_size(260.0),
        }
    })
    .collect()
}
