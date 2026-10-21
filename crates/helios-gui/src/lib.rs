//! Helios's desktop: an egui-pop application around Helios's panels (on
//! egui-pop's workbench), its commands run by Helios's kernel, and the
//! framework's own pictures and recordings of it.

pub mod caption;
pub mod contexts;
pub mod decisions;
pub mod dialogs;
pub mod display;
pub mod duplicates;
pub mod engine_source;
pub mod graph;
pub mod host;
pub mod i18n;
pub mod jobs;
pub mod keymaps;
pub mod keys;
pub mod library;
pub mod mask_display;
pub mod mask_tree;
pub mod masks;
pub mod navigator;
pub mod params;
pub mod persist;
pub mod platform;
mod plugins;
pub mod pointer_choice;
pub mod preview;
pub mod quick_open;
pub mod quit;
pub mod relay;
pub mod scopes;
pub mod shell;
pub mod snapshot;
pub mod stack_review;
pub mod start;
pub mod status_items;
pub mod templates;
pub mod theme;
pub mod theme_editor;
pub mod viewport;
mod windows;
mod workbench;

pub use shell::{Shell, headless, name_rect};
pub use workbench::{IMAGE_FIRST, area_named, image_first};

use egui_pop::model::ids::PanelId;
use egui_pop::model::panels::{Area, PanelSpec};

/// The panels that may float over the image as palettes (egui-pop's
/// `PanelSpec::floatable`): docked by default, floated from their tab's
/// menu or `<panel>.float`, hidden and shown again with Tab.
pub const FLOATABLE: [&str; 3] = ["helios.masks", "helios.scopes", "helios.navigator"];

/// The panels Helios declares today, with the areas each may be moved to
/// (egui-pop's placement rules); each shows a placeholder until its
/// widget lands. Every panel may be detached into a window of its own;
/// the [`FLOATABLE`] ones may float too.
/// The culling panels (Compare, Stacks, Duplicates) and the Navigator
/// start closed: a template's layout, or the user, opens them.
pub fn helios_panels() -> Vec<PanelSpec> {
    use Area::{Bottom, Center, EndSide, StartSide};
    let sides = [StartSide, EndSide, Bottom];
    let anywhere = [Center, StartSide, EndSide, Bottom];
    [
        ("helios.viewport", "Image", Center, &[Center][..], true),
        ("helios.graph", "Graph", Center, &[Center][..], true),
        ("helios.vault", "Vault", StartSide, &anywhere[..], true),
        ("helios.modules", "Modules", StartSide, &sides[..], true),
        ("helios.masks", "Masks", StartSide, &sides[..], true),
        (
            "helios.navigator",
            "Navigator",
            StartSide,
            &sides[..],
            false,
        ),
        ("helios.params", "Parameters", EndSide, &sides[..], true),
        ("helios.scopes", "Scopes", EndSide, &sides[..], true),
        ("helios.filmstrip", "Filmstrip", Bottom, &sides[..], true),
        ("helios.queue", "Queue", Bottom, &[][..], true),
        ("helios.log", "Log", Bottom, &[][..], true),
        ("helios.compare", "Compare", Center, &anywhere[..], false),
        ("helios.stacks", "Stacks", EndSide, &sides[..], false),
        (
            "helios.duplicates",
            "Duplicates",
            EndSide,
            &sides[..],
            false,
        ),
        ("helios.decisions", "Decisions", EndSide, &sides[..], false),
    ]
    .into_iter()
    .map(|(id, title, area, areas, open)| {
        let id = PanelId::new(id).expect("Helios's panel ids are valid");
        let floats = FLOATABLE.contains(&id.as_str());
        let mut spec = PanelSpec::new(id, title, area)
            .only_in(areas)
            .floatable(floats);
        if !open {
            spec = spec.closed();
        }
        // The sizes Helios's areas open at.
        match area {
            Center => spec,
            Bottom => spec.default_size(200.0),
            _ => spec.default_size(260.0),
        }
    })
    .collect()
}
