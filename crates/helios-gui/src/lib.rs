//! Helios's desktop: the shell of `helios-shell` drawn with egui, and the
//! framework's own pictures and recordings of it.

pub mod dock;
pub mod frame;
pub mod keys;
pub mod scopes;
pub mod shell;
pub mod snapshot;
pub mod theme;

pub use shell::{Shell, headless, name_rect};

use helios_shell::panels::{Area, PanelSpec, Panels};

/// The panels Helios declares today; each shows a placeholder until its
/// widget lands.
pub fn helios_panels() -> Panels {
    let mut panels = Panels::new();
    for (id, title, area) in [
        ("helios.viewport", "Image", Area::Editors),
        ("helios.graph", "Graph", Area::Editors),
        ("helios.vault", "Vault", Area::PrimarySideBar),
        ("helios.modules", "Modules", Area::PrimarySideBar),
        ("helios.params", "Parameters", Area::SecondarySideBar),
        ("helios.scopes", "Scopes", Area::SecondarySideBar),
        ("helios.queue", "Queue", Area::BottomPanel),
        ("helios.log", "Log", Area::BottomPanel),
    ] {
        panels
            .declare(PanelSpec {
                id: id.into(),
                title: title.into(),
                area,
                commands: Vec::new(),
            })
            .expect("panel ids are unique");
    }
    panels
}
