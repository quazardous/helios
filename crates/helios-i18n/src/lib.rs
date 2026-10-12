//! Helios's translations, over egui-pop's translation plugin
//! (`egui-pop-i18n`: catalogues, fallback chains, the language setting,
//! the extractor and the checks).
//!
//! What stays here is Helios's own:
//!
//! - [`CATALOGUES`]: the repository's `i18n/<locale>/*.ftl`, built in,
//!   given to `I18nPlugin::catalogues` by the desktop;
//! - [`ids`]: the message ids of what Helios declares (commands, panels,
//!   module parameters), derived from stable ids, never from labels;
//! - [`extract`]: the English catalogue (`i18n/en`) generated from those
//!   declarations (`cargo xtask i18n extract`).
//!
//! Catalogues live in `i18n/<locale>/` as one file per area
//! (`commands.ftl`, `panels.ftl`, `modules.ftl`, `interface.ftl`).
//! `i18n/en` is generated and never edited; the other locales are
//! translations of it.

pub mod extract;
pub mod ids;

pub use egui_pop_i18n::{Catalogue, I18n};
pub use extract::Declarations;
pub use ids::MessageId;

mod builtin {
    include!(concat!(env!("OUT_DIR"), "/builtin.rs"));
}

/// Helios's catalogues, every locale's, built in.
pub static CATALOGUES: &[Catalogue] = builtin::BUILTIN;

/// The environment variable naming the language, before the system's,
/// when the language setting is `system`.
pub const ENV: &str = "HELIOS_LANG";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_catalogues_load_beside_egui_pops() {
        let i = I18n::with_egui_pop("fr");
        i.add_catalogues(CATALOGUES).unwrap();
        assert!(i.locales().iter().any(|l| l == "fr"));
        assert_eq!(i.t(&ids::panel_title("helios.graph").key(), &[]), "Graphe");
    }

    #[test]
    fn the_repository_catalogues_have_no_fault() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../i18n");
        let catalogs = egui_pop_i18n::check::Catalogs::read(&dir).unwrap();
        let r = egui_pop_i18n::check::check(&catalogs);
        assert!(r.faults.is_empty(), "{r}");
    }
}
