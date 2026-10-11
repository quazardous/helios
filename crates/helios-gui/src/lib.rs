//! Helios's desktop: the shell of `helios-shell` drawn with egui, and the
//! framework's own pictures and recordings of it.

pub mod dialogs;
pub mod dock;
pub mod frame;
pub mod keys;
pub mod library;
pub mod scopes;
pub mod shell;
pub mod snapshot;
pub mod theme;
pub mod viewport;

pub use shell::{Shell, headless, name_rect};

use helios_shell::panels::{Area, PanelSpec, Panels};

/// The panels Helios declares today, with the areas each may be moved to;
/// each shows a placeholder until its widget lands.
pub fn helios_panels() -> Panels {
    let mut panels = Panels::new();
    use Area::*;
    let sides = vec![PrimarySideBar, SecondarySideBar, BottomPanel];
    for (id, title, area, areas) in [
        ("helios.viewport", "Image", Editors, vec![Editors]),
        ("helios.graph", "Graph", Editors, vec![Editors]),
        ("helios.vault", "Vault", PrimarySideBar, sides.clone()),
        ("helios.modules", "Modules", PrimarySideBar, sides.clone()),
        (
            "helios.params",
            "Parameters",
            SecondarySideBar,
            sides.clone(),
        ),
        ("helios.scopes", "Scopes", SecondarySideBar, sides.clone()),
        ("helios.filmstrip", "Filmstrip", BottomPanel, sides.clone()),
        ("helios.queue", "Queue", BottomPanel, vec![]),
        ("helios.log", "Log", BottomPanel, vec![]),
    ] {
        panels
            .declare(PanelSpec {
                id: id.into(),
                title: title.into(),
                area,
                areas,
                commands: Vec::new(),
            })
            .expect("panel ids are unique");
    }
    panels
}
