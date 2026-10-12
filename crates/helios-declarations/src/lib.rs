//! Everything Helios declares with English text: the desktop's commands
//! (the shell's registry, values commands included) and panels, every
//! module's parameters, and the rest of what the desktop declares to
//! egui-pop (categories, submenus, settings, plugins). The English
//! catalogue (`i18n/en`) is generated from it, so no list of the ids built
//! at run time is kept by hand.
//!
//! It lives apart from `helios-i18n`, which the desktop depends on, and
//! from `xtask`, which stays light: `cargo xtask i18n extract` runs this
//! crate's binary, which prints the declarations as JSON.

use std::collections::BTreeSet;

use egui_pop::i18n::extract;
use egui_pop::model::ids::MenuId;
use helios_i18n::Declarations;

/// Menus Helios opens itself, besides the menu bar.
const MENUS: [MenuId; 2] = [
    MenuId::from_static("helios.system"),
    MenuId::from_static("helios.param"),
];

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
    let commands = shell.specs();
    let mut d = Declarations::from_specs(&commands, panels.all(), modules);
    // What egui-pop's catalogues translate, and what the areas above
    // already hold, is not Helios's to write again.
    let egui_pop = egui_pop::i18n::I18n::with_egui_pop("en").keys("en");
    let ours: BTreeSet<String> = d.english().into_keys().map(|id| id.key()).collect();
    for f in extract::declarations(shell.app().handle(), &MENUS) {
        if egui_pop.contains(&f.key) || ours.contains(&f.key) {
            continue;
        }
        // Helios's command titles are in commands.ftl; any other command
        // title here is one egui-pop derives for a zone of its own.
        if f.key.starts_with("command-") && f.key.split_once('.').is_some_and(|(_, a)| a == "title")
        {
            continue;
        }
        // The language setting's choices are each language's own name;
        // the focus setting's description quotes what this machine's
        // system answered.
        if f.key.starts_with("setting-appearance-language-choice-")
            || f.key == "setting-appearance-focus.description"
        {
            continue;
        }
        if let extract::English::Plain(text) = f.english {
            d.interface.entry(f.key).or_insert(text);
        }
    }
    d
}
