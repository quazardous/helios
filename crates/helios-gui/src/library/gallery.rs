//! The gallery: one view of the Library (`docs/design/library-search.md`,
//! section 6).
//!
//! [`Gallery`] holds what one view of the photos keeps for itself: its
//! search field, filter, restriction (album, folder, import session, a
//! set shown alone), the order it shows, its selection and cursor, its
//! facet counts. [`super::Library`] keeps what every view reads (the
//! entries, albums, folders, thumbnails, the sort the entries are read
//! in) and one gallery per host, by id: the main window's Library panel
//! (`main`) and the Library window (`window`) each have their own, so
//! filtering or restricting one never changes the other (owner,
//! 2026-11-15). The workspace keeps each one's restriction and query
//! apart. The commander layout and the filmstrip add more later.
//!
//! [`GalleryOptions`] is what a host asks of the gallery it draws: its
//! id, and a more or less complete search depending on where it is
//! hosted.

use egui_pop::views::library::LibraryState;
use egui_pop::views::library::filter::{Filter, SearchState};
use helios_template::{LibraryDesc, ViewDesc};
use helios_vault::fields::Query;
use std::collections::HashSet;

/// The main window's Library panel's gallery.
pub const MAIN_GALLERY: &str = "main";

/// The Library window's gallery.
pub const WINDOW_GALLERY: &str = "window";

/// What the workspace keeps for the gallery `id`: the main panel's at the
/// top of `library` (where a workspace written before the views were
/// apart has it), another's under `library.views`.
pub fn view_of(library: Option<&LibraryDesc>, id: &str) -> ViewDesc {
    let Some(l) = library else {
        return ViewDesc::default();
    };
    if id == MAIN_GALLERY {
        ViewDesc {
            query: l.query.clone(),
            folder: l.folder.clone(),
            collection: l.collection.clone(),
        }
    } else {
        l.views.get(id).cloned().unwrap_or_default()
    }
}

/// Writes what the workspace keeps for the gallery `id` ([`view_of`]); an
/// empty view of another gallery is none at all.
pub fn set_view(library: &mut LibraryDesc, id: &str, view: ViewDesc) {
    if id == MAIN_GALLERY {
        library.query = view.query;
        library.folder = view.folder;
        library.collection = view.collection;
    } else if view.is_empty() {
        library.views.remove(id);
    } else {
        library.views.insert(id.to_string(), view);
    }
}

/// How much search a gallery offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMode {
    /// No search bar: the gallery shows its source, or follows the
    /// gallery it is linked to (the filmstrip).
    None,
    /// One field with its criteria (the main window's panel).
    Compact,
    /// The field, and the advanced rules once they are drawn inline
    /// (the Library window).
    Full,
}

/// Where a gallery offers its sources (folders, albums, keywords,
/// imports).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourcesPlace {
    /// None: the gallery shows what its host or its link gives it.
    None,
    /// Above the grid, in the gallery's own column (today's sections;
    /// a header menu once the bar is regrouped).
    Header,
    /// A panel of their own beside the grid (the Library window's
    /// commander layout, once it lands; drawn as `Header` until then).
    Panel,
}

/// What a host asks of the gallery it draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GalleryOptions {
    /// The gallery's id: the state it draws and the workspace keeps
    /// (`main`, `window`; later `pane-1`, `filmstrip`).
    pub id: &'static str,
    pub search: SearchMode,
    pub sources: SourcesPlace,
    /// The sort menu and the count of photos shown.
    pub sort_count: bool,
    /// The view options: auto-advance and Compare (a View menu once the
    /// bar is regrouped).
    pub view_options: bool,
    /// The current photo's marks, metadata and versions under the
    /// sources (they move to the metadata panel later).
    pub marks: bool,
}

impl GalleryOptions {
    /// The main window's Library panel.
    pub const MAIN_PANEL: GalleryOptions = GalleryOptions {
        id: MAIN_GALLERY,
        search: SearchMode::Compact,
        sources: SourcesPlace::Header,
        sort_count: true,
        view_options: true,
        marks: true,
    };

    /// The Library window: a gallery of its own (its search, filter,
    /// restriction and selection), with the full search.
    pub const LIBRARY_WINDOW: GalleryOptions = GalleryOptions {
        id: WINDOW_GALLERY,
        search: SearchMode::Full,
        sources: SourcesPlace::Panel,
        sort_count: true,
        view_options: true,
        marks: true,
    };

    /// A grid with nothing around it: no search, no sources, no options.
    pub const BARE: GalleryOptions = GalleryOptions {
        id: "bare",
        search: SearchMode::None,
        sources: SourcesPlace::None,
        sort_count: false,
        view_options: false,
        marks: false,
    };

    /// Whether the gallery draws its search field.
    pub fn has_search(&self) -> bool {
        self.search != SearchMode::None
    }

    /// Whether the gallery draws its sources.
    pub fn has_sources(&self) -> bool {
        self.sources != SourcesPlace::None
    }
}

/// One view's own state: what it searches, what it shows and in which
/// order, what is selected.
pub struct Gallery {
    /// The grid's selection, cursor and scroll.
    pub(super) state: LibraryState<String>,
    /// The search field's text and criteria.
    pub(super) search: SearchState,
    pub(super) filter: Filter,
    /// What the views show, in order: the filter's matches, a stack's
    /// members right after its top image.
    pub(super) order: Vec<usize>,
    /// The images the vault matched for the filter, once the host ran its
    /// query.
    pub(super) matches: Option<HashSet<String>>,
    /// The set of photos shown alone (`Library::show_ids`: a job's
    /// photos), whatever the filter; the next filter set drops it.
    pub(super) only: Option<HashSet<String>>,
    /// A query for the host to run.
    pub(super) query: Option<Query>,
    /// How many photos of the filter carry each value of each facet, as
    /// the vault last counted them.
    pub(super) facets: Option<helios_vault::Facets>,
    /// A filter typed in the search field and when (the input's time):
    /// it counts as used once it rests ([`crate::recent_filters::TYPED_REST`]).
    pub(super) typed: Option<(Filter, f64)>,
    /// The order the vault reads the photos in: this gallery's sort.
    pub(super) sort: helios_vault::Order,
    /// The sort the entries must be read again in, once it changed
    /// (`Library::take_resort`).
    pub(super) resort: Option<helios_vault::Order>,
}

impl Gallery {
    /// A gallery showing everything, in the default sort.
    pub fn new(id: &str) -> Self {
        Gallery {
            state: LibraryState::new(id),
            search: SearchState::new(),
            filter: Filter::default(),
            order: Vec::new(),
            matches: None,
            only: None,
            query: None,
            facets: None,
            typed: None,
            sort: super::DEFAULT_SORT,
            resort: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_window_draws_its_own_gallery() {
        // Separate filters and restrictions per view (owner, 2026-11-15):
        // the two hosts name different galleries.
        assert_ne!(
            GalleryOptions::MAIN_PANEL.id,
            GalleryOptions::LIBRARY_WINDOW.id
        );
        assert_eq!(GalleryOptions::MAIN_PANEL.search, SearchMode::Compact);
        assert_eq!(GalleryOptions::LIBRARY_WINDOW.search, SearchMode::Full);
        assert!(!GalleryOptions::BARE.has_search());
        assert!(!GalleryOptions::BARE.has_sources());
    }

    #[test]
    fn an_old_workspace_s_scope_is_the_main_panel_s() {
        // Written before the views were apart: `library.collection` alone.
        let old: LibraryDesc =
            serde_json::from_value(serde_json::json!({ "collection": "test" })).unwrap();
        assert_eq!(
            view_of(Some(&old), MAIN_GALLERY).collection.as_deref(),
            Some("test")
        );
        assert!(view_of(Some(&old), WINDOW_GALLERY).is_empty());

        // The window's scope goes under `views`, the panel's stays.
        let mut l = old.clone();
        let window = ViewDesc {
            folder: Some("/photos".into()),
            ..ViewDesc::default()
        };
        set_view(&mut l, WINDOW_GALLERY, window.clone());
        assert_eq!(view_of(Some(&l), WINDOW_GALLERY), window);
        assert_eq!(l.collection.as_deref(), Some("test"));
        set_view(&mut l, WINDOW_GALLERY, ViewDesc::default());
        assert!(l.views.is_empty(), "an empty view is none at all");
        assert_eq!(l, old);
    }
}
