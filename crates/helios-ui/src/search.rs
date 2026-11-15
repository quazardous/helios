//! Search fields by what they search: one field everywhere, a magnifier,
//! a placeholder from its scope, a clear button.

use egui_pop::core::input::{Input, InputOutput};
use egui_pop::egui::Ui;

/// What a search field searches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Search {
    /// The photos of the Library.
    Library,
    /// A list of a screen of its own.
    Find(Scope),
}

/// The lists a [`Search::Find`] field narrows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// The parameters of a macro node's interface.
    MacroParameters,
    /// The workshops, by what they do.
    Workshops,
}

impl Search {
    pub const ALL: [Search; 3] = [
        Search::Library,
        Search::Find(Scope::MacroParameters),
        Search::Find(Scope::Workshops),
    ];

    /// Its accessible name, for assistive technologies and agents.
    pub fn label(self) -> &'static str {
        match self {
            Search::Library => "Search the library",
            Search::Find(Scope::MacroParameters) => "Filter the parameters",
            Search::Find(Scope::Workshops) => "Find a workshop",
        }
    }

    /// The hint shown while the field is empty.
    pub fn placeholder(self) -> &'static str {
        match self {
            Search::Library => "Search photos…",
            Search::Find(Scope::MacroParameters) => "Filter parameters…",
            Search::Find(Scope::Workshops) => "What you want: brighter, crop…",
        }
    }
}

/// A search field over `query` for `search`: egui-pop's search input
/// (magnifier, clearable), named and hinted by its scope, as wide as
/// the space given.
///
/// Need (input): a search field.
/// Need (input): a filter box.
pub fn search(ui: &mut Ui, query: &mut String, search: Search) -> InputOutput {
    Input::search(query, search.label())
        .placeholder(search.placeholder())
        .id_salt(("helios_ui.search", search))
        .show(ui)
}
