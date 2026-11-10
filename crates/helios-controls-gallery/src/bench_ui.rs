//! The gallery's bench page: `egui_hud`'s (`egui_hud::bench::ui`) over
//! Helios's kit ([`crate::bench::kit`]).

pub use egui_hud::bench::ui::*;

/// The bench page over an empty bench of Helios's kit.
pub fn page() -> BenchPage {
    BenchPage::new(crate::bench::standard())
}
