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
//! id, which of its two parts, and a more or less complete search
//! depending on where it is hosted.
//!
//! A gallery is an explorer of two linked parts (owner, 2026-11-17): its
//! **Filters** (the search, the filters and the sources, nothing else)
//! and its **Results** (the photos, shown as thumbnails at a size, as a
//! list or as details in columns, with the view's options in its bar). The main window draws
//! them as two panels ([`EXPLORER`]: `helios.filters` and
//! `helios.vault`), linked by the gallery id they both draw; the Library
//! window draws its gallery's two parts side by side. A second explorer
//! is a second pair naming another gallery id.

use egui_pop::views::library::LibraryState;
use egui_pop::views::library::filter::{Filter, SearchState};
use egui_pop::views::list::ListState;
use egui_pop::views::table::TableState;
use helios_template::{LibraryDesc, ResultsColumn, ResultsDesc, ResultsMode, ViewDesc};
use helios_vault::fields::Query;
use std::collections::HashSet;

/// The main window's Library panel's gallery.
pub const MAIN_GALLERY: &str = "main";

/// The Library window's gallery.
pub const WINDOW_GALLERY: &str = "window";

/// The main window's Filters panel: the main gallery's search, filters
/// and sources.
pub const FILTERS_PANEL: &str = "helios.filters";

/// The main window's Results panel: the main gallery's photos.
pub const RESULTS_PANEL: &str = "helios.vault";

/// One explorer of the main window: a Filters panel and the Results
/// panel it feeds, linked by the gallery both draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Explorer {
    /// The gallery both panels draw.
    pub gallery: &'static str,
    /// The Filters panel's id.
    pub filters: &'static str,
    /// The Results panel's id.
    pub results: &'static str,
}

/// The main window's explorer (more come with gallery instances).
pub const EXPLORER: Explorer = Explorer {
    gallery: MAIN_GALLERY,
    filters: FILTERS_PANEL,
    results: RESULTS_PANEL,
};

/// The explorers of the main window, by panel: the one whose Filters or
/// Results panel `panel` is.
pub fn explorer_of(panel: &str) -> Option<Explorer> {
    [EXPLORER]
        .into_iter()
        .find(|e| e.filters == panel || e.results == panel)
}

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
            auto_advance: None,
            results: l.results.clone(),
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
        library.results = view.results;
        if let Some(on) = view.auto_advance {
            library.auto_advance = on;
        }
    } else if view.is_empty() {
        library.views.remove(id);
    } else {
        library.views.insert(id.to_string(), view);
    }
}

/// Whether culling auto-advances in the gallery `id`, as the workspace
/// keeps it: its own value, else the library's (the main view's, which
/// a workspace written before each view had its own shares with all).
pub fn auto_advance_of(library: Option<&LibraryDesc>, id: &str) -> bool {
    let Some(l) = library else {
        return false;
    };
    if id == MAIN_GALLERY {
        return l.auto_advance;
    }
    l.views
        .get(id)
        .and_then(|v| v.auto_advance)
        .unwrap_or(l.auto_advance)
}

/// Writes whether culling auto-advances in the gallery `id`. The main
/// view's value is the library's, which the other views without their
/// own follow: changing it gives each known view its own first, so it
/// changes the main view only.
pub fn set_auto_advance(library: &mut LibraryDesc, id: &str, on: bool) {
    if id == MAIN_GALLERY {
        // The other views known today (more with gallery instances).
        let mut view = view_of(Some(library), WINDOW_GALLERY);
        if view.auto_advance.is_none() && library.auto_advance != on {
            view.auto_advance = Some(library.auto_advance);
            set_view(library, WINDOW_GALLERY, view);
        }
        library.auto_advance = on;
    } else {
        let mut view = view_of(Some(library), id);
        view.auto_advance = (on != library.auto_advance).then_some(on);
        set_view(library, id, view);
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
    /// Which of the gallery's parts the host draws.
    pub parts: Parts,
    pub search: SearchMode,
    pub sources: SourcesPlace,
    /// The sort menu and the count of photos shown.
    pub sort_count: bool,
    /// The view options in the Results bar: how the results show
    /// (thumbnails or a list, their size), auto-advance and Compare.
    pub view_options: bool,
    /// The current photo's marks, metadata and versions under the
    /// sources (they move to the metadata panel later).
    pub marks: bool,
}

/// Which parts of a gallery a host draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parts {
    /// The search, the filters and the sources: no photos.
    Filters,
    /// The photos and the view's bar.
    Results,
    /// Both, side by side: the Filters in a column at the start, the
    /// Results taking the rest.
    Both,
}

impl Parts {
    /// Whether the host draws the Filters.
    pub fn filters(self) -> bool {
        self != Parts::Results
    }

    /// Whether the host draws the Results.
    pub fn results(self) -> bool {
        self != Parts::Filters
    }
}

impl GalleryOptions {
    /// The main window's Filters panel.
    pub const FILTERS_PANEL: GalleryOptions = GalleryOptions {
        id: MAIN_GALLERY,
        parts: Parts::Filters,
        search: SearchMode::Compact,
        sources: SourcesPlace::Header,
        sort_count: false,
        view_options: false,
        marks: true,
    };

    /// The main window's Results panel.
    pub const RESULTS_PANEL: GalleryOptions = GalleryOptions {
        id: MAIN_GALLERY,
        parts: Parts::Results,
        search: SearchMode::None,
        sources: SourcesPlace::None,
        sort_count: true,
        view_options: true,
        marks: false,
    };

    /// Both parts of the main gallery in one place (a test, an agent's
    /// look at the whole view).
    pub const MAIN_PANEL: GalleryOptions = GalleryOptions {
        id: MAIN_GALLERY,
        parts: Parts::Both,
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
        parts: Parts::Both,
        search: SearchMode::Full,
        sources: SourcesPlace::Panel,
        sort_count: true,
        view_options: true,
        marks: true,
    };

    /// A grid with nothing around it: no search, no sources, no options.
    pub const BARE: GalleryOptions = GalleryOptions {
        id: "bare",
        parts: Parts::Results,
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
    /// What is typed in the Metadata section, for this gallery's
    /// selection.
    pub(super) meta: super::metadata::MetaDrafts,
    /// Culling: after a mark from the keyboard, the cursor moves to the
    /// next image of this gallery.
    pub(super) auto_advance: bool,
    /// How the Results show the photos: thumbnails (their size is
    /// `state`'s) or a list.
    pub(super) mode: ResultsMode,
    /// The list's state, in list mode (its selection follows `state`'s).
    pub(super) list: ListState<String>,
    /// The details' columns as the user set them (order, widths); the
    /// default columns when empty.
    pub(super) columns: Vec<ResultsColumn>,
    /// The details' table's state, in details mode (its selection
    /// follows `state`'s).
    pub(super) table: TableState<String>,
    /// The value store the details' table reads its sort and widths
    /// from, for the columns it was made for (see
    /// [`super::details::TableStore`]).
    pub(super) table_store: Option<super::details::TableStore>,
    /// How the results show changed since the workspace last kept it
    /// ([`super::Library::take_results_changes`]).
    pub(super) results_changed: bool,
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
            meta: super::metadata::MetaDrafts::default(),
            auto_advance: false,
            mode: ResultsMode::Thumbnails,
            list: ListState::new(),
            columns: Vec::new(),
            table: TableState::new(),
            table_store: None,
            results_changed: false,
        }
    }

    /// How the Results show the photos, as the workspace keeps it.
    pub fn results(&self) -> ResultsDesc {
        let size = self.state.size().round() as u16;
        let default = egui_pop::views::library::grid::DEFAULT_SIZE.round() as u16;
        ResultsDesc {
            mode: self.mode,
            size: (size != default).then_some(size),
            columns: self.columns.clone(),
        }
    }

    /// Shows the photos as `results` says (thumbnails at the default
    /// size when it says nothing).
    pub fn set_results(&mut self, results: &ResultsDesc) {
        self.mode = results.mode;
        if self.columns != results.columns {
            self.columns = results.columns.clone();
            self.table_store = None;
        }
        self.state.set_size(
            results
                .size
                .map_or(egui_pop::views::library::grid::DEFAULT_SIZE, f32::from),
        );
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

    #[test]
    fn auto_advance_is_kept_per_view_and_an_old_workspace_shares_it() {
        // Written before each view had its own: every view takes it.
        let mut l: LibraryDesc =
            serde_json::from_value(serde_json::json!({ "auto_advance": true })).unwrap();
        assert!(auto_advance_of(Some(&l), MAIN_GALLERY));
        assert!(auto_advance_of(Some(&l), WINDOW_GALLERY));
        // Off in the main view: the window keeps its on.
        set_auto_advance(&mut l, MAIN_GALLERY, false);
        assert!(!auto_advance_of(Some(&l), MAIN_GALLERY));
        assert!(auto_advance_of(Some(&l), WINDOW_GALLERY));
        // Off in the window too: nothing of its own left to keep.
        set_auto_advance(&mut l, WINDOW_GALLERY, false);
        assert!(!auto_advance_of(Some(&l), WINDOW_GALLERY));
        assert_eq!(l, LibraryDesc::default());
    }
}
