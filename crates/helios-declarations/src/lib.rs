//! Everything Helios declares with English text: the desktop's commands
//! (the shell's registry, values commands included) and panels, and every
//! module's parameters. The English catalog (`i18n/en`) is generated from
//! it, so no list of the ids built at run time is kept by hand.
//!
//! It lives apart from `helios-i18n`, which the desktop will depend on,
//! and from `xtask`, which stays light: `cargo xtask i18n extract` runs
//! this crate's binary, which prints the declarations as JSON.

use helios_i18n::Declarations;

/// Helios's declarations, from a headless desktop shell and the node's
/// module registry.
pub fn collect() -> Declarations {
    let shell = helios_gui::Shell::new(helios_gui::helios_panels(), None);
    let panels = helios_gui::helios_panels();
    let modules = helios_node::registry();
    let modules: Vec<_> = modules
        .values()
        .map(|m| (m.manifest(), m.param_hints()))
        .collect();
    Declarations::from_specs(shell.registry.specs(), panels.all(), modules)
}
