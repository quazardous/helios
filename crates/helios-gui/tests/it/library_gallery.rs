//! The Library's gallery component (`docs/design/library-search.md`,
//! section 6, slice G1): each host gives its options and the gallery
//! draws what they ask (no search field without search, no sources
//! without sources, no sort or view options when hidden); the main
//! window's panel and the Library window each draw a gallery of their
//! own, so filtering, restricting or selecting in one leaves the other
//! as it was, and commands act on the one with input; and the facets bar
//! lists the filter keys declared, a module's key included, a click
//! adding its term.
//!
//! Slice G2 (owner, 2026-11-17): a gallery is an explorer of two linked
//! parts, its Filters (no photos) and its Results (the photos as
//! thumbnails at a size or as a list); each view has its own
//! auto-advance, a toggle in its Results bar.
//!
//! #5265: a Details mode (columns sorted as the gallery, kept per view);
//! the list and the details open a photo on Enter or a double click and
//! carry photos out on a drag; a narrow bar keeps Compare on one line.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable};
use egui_pop::views::library::filter::Filter;
use helios_core::Message;
use helios_gui::library::DraggedPhotos;
use helios_gui::library::{
    EXPLORER, Entry, GalleryOptions, Library, MAIN_GALLERY, SearchKey, WINDOW_GALLERY, explorer_of,
};
use helios_template::{ResultsColumn, ResultsDesc, ResultsMode};

/// The Results bar's auto-advance toggle's name (and tooltip).
const AUTO_ADVANCE: &str =
    "Auto-advance: after a rating, flag or reject from the keyboard, go to the next photo";

fn library() -> Rc<RefCell<Library>> {
    let mut lib = Library::default();
    lib.set_entries(
        ["a", "b", "c"]
            .iter()
            .map(|id| Entry {
                id: (*id).into(),
                name: format!("{id}.jpg"),
                ..Entry::default()
            })
            .collect(),
    );
    Rc::new(RefCell::new(lib))
}

fn harness(lib: &Rc<RefCell<Library>>, options: GalleryOptions) -> Harness<'static> {
    let lib = lib.clone();
    // egui's default fonts lack the sort arrow (the shell installs
    // Helios's); these tests read labels, not pixels.
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size(egui::vec2(480.0, 640.0))
        .build_ui(move |ui| lib.borrow_mut().show(ui, &options));
    h.run();
    h
}

fn has(h: &Harness, label: &str) -> bool {
    h.query_by_label(label).is_some()
}

#[test]
fn the_options_choose_what_the_gallery_draws() {
    let lib = library();
    let main = harness(&lib, GalleryOptions::MAIN_PANEL);
    assert!(has(&main, "Search images"), "the main panel has its field");
    assert!(has(&main, AUTO_ADVANCE), "and its view options");
    assert!(has(&main, "3 of 3"), "and its count");
    assert!(has(&main, "Folders"), "and its sources");
    drop(main);

    let bare = harness(&lib, GalleryOptions::BARE);
    assert!(!has(&bare, "Search images"), "no search: no field");
    assert!(!has(&bare, AUTO_ADVANCE), "no view options");
    assert!(!has(&bare, "3 of 3"), "no sort and count");
    assert!(!has(&bare, "Folders"), "no sources");
    assert!(has(&bare, "Images"), "the grid stays");
    drop(bare);

    let only_search = GalleryOptions {
        view_options: false,
        sort_count: false,
        ..GalleryOptions::MAIN_PANEL
    };
    let h = harness(&lib, only_search);
    assert!(has(&h, "Search images"));
    assert!(!has(&h, AUTO_ADVANCE));
    assert!(!has(&h, "3 of 3"));
}

#[test]
fn the_filters_panel_holds_no_photos_and_the_results_panel_shows_them() {
    let lib = library();
    let filters = harness(&lib, GalleryOptions::FILTERS_PANEL);
    assert!(has(&filters, "Search images"), "the Filters search");
    assert!(has(&filters, "Folders"), "and hold the sources");
    assert!(!has(&filters, "Images"), "no grid in the Filters");
    assert!(!has(&filters, "a.jpg"), "no photo in the Filters");
    assert!(!has(&filters, AUTO_ADVANCE), "no view options");
    drop(filters);

    let results = harness(&lib, GalleryOptions::RESULTS_PANEL);
    assert!(has(&results, "Images"), "the Results show the grid");
    assert!(has(&results, "a.jpg"), "and its photos");
    assert!(has(&results, "3 of 3"), "with the count");
    assert!(has(&results, "View as"), "how they show");
    assert!(has(&results, AUTO_ADVANCE), "and auto-advance");
    assert!(!has(&results, "Search images"), "no search in the Results");
    assert!(!has(&results, "Folders"), "no sources in the Results");
}

#[test]
fn the_filters_feed_the_results_of_their_explorer() {
    // Linked by the gallery both draw: what the Filters panel filters is
    // what its Results panel shows, the Library window's untouched.
    assert_eq!(GalleryOptions::FILTERS_PANEL.id, EXPLORER.gallery);
    assert_eq!(GalleryOptions::RESULTS_PANEL.id, EXPLORER.gallery);
    assert_eq!(explorer_of(EXPLORER.filters), Some(EXPLORER));
    assert_eq!(explorer_of(EXPLORER.results), Some(EXPLORER));
    assert_eq!(explorer_of("helios.viewport"), None);

    let lib = library();
    let mut filters = harness(&lib, GalleryOptions::FILTERS_PANEL);
    // Typed as a hand types: the field focused by a click, then a key.
    filters.get_by_label("Search images").click();
    filters.run();
    filters.event(egui::Event::Text("b".into()));
    filters.run();
    filters.run();
    drop(filters);
    let results = harness(&lib, GalleryOptions::RESULTS_PANEL);
    drop(results);
    let mut lib = lib.borrow_mut();
    assert_eq!(lib.gallery_id(), MAIN_GALLERY);
    assert!(
        !lib.filter().is_empty(),
        "the Results' gallery has the filter typed in its Filters"
    );
    lib.with_gallery(WINDOW_GALLERY, |lib| {
        assert!(lib.filter().is_empty(), "the window's stays as it was");
    });
}

#[test]
fn the_results_mode_and_size_are_each_view_s_and_kept() {
    let lib = library();
    let thumbs = |lib: &Library| lib.results();
    assert_eq!(thumbs(&lib.borrow()), ResultsDesc::default());
    {
        let mut lib = lib.borrow_mut();
        lib.set_results(&ResultsDesc {
            mode: ResultsMode::List,
            size: Some(192),
            ..ResultsDesc::default()
        });
        assert!(
            lib.take_results_changes().is_empty(),
            "set from the workspace"
        );
    }
    // Shown as a list: the photos are rows, the grid is gone.
    let results = harness(&lib, GalleryOptions::RESULTS_PANEL);
    assert!(has(&results, "a.jpg"), "a row per photo");
    drop(results);
    let mut lib = lib.borrow_mut();
    assert_eq!(
        lib.results(),
        ResultsDesc {
            mode: ResultsMode::List,
            size: Some(192),
            ..ResultsDesc::default()
        }
    );
    lib.with_gallery(WINDOW_GALLERY, |lib| {
        assert_eq!(lib.results(), ResultsDesc::default(), "the window's own");
    });
}

#[test]
fn the_view_menu_changes_the_results_and_asks_to_keep_them() {
    let lib = library();
    let mut results = harness(&lib, GalleryOptions::RESULTS_PANEL);
    results.get_by_label("View as").click();
    results.run();
    results.get_by_label("List").click();
    results.run();
    assert!(has(&results, "a.jpg"), "a row per photo");
    drop(results);
    let mut lib = lib.borrow_mut();
    assert_eq!(lib.results().mode, ResultsMode::List);
    assert_eq!(
        lib.take_results_changes(),
        [(MAIN_GALLERY.to_string(), lib.results())],
        "the host keeps it with the view"
    );
}

#[test]
fn auto_advance_is_each_view_s_own() {
    // #5252 A: ticking it in one view leaves the other's as it was, and
    // a mark from the keyboard advances only where it is on.
    let lib = library();
    {
        let mut lib = lib.borrow_mut();
        lib.with_gallery(WINDOW_GALLERY, |lib| lib.set_auto_advance(true));
        assert!(!lib.auto_advance(), "the main view's is off");
    }
    let results = harness(&lib, GalleryOptions::RESULTS_PANEL);
    let toggled = |h: &Harness| h.get_by_label(AUTO_ADVANCE).accesskit_node().toggled();
    assert_eq!(
        toggled(&results),
        Some(egui::accesskit::Toggled::False),
        "the main Results bar says off"
    );
    drop(results);
    let window = harness(&lib, GalleryOptions::LIBRARY_WINDOW);
    assert_eq!(
        toggled(&window),
        Some(egui::accesskit::Toggled::True),
        "the window's says on"
    );
    drop(window);
    let mut lib = lib.borrow_mut();
    lib.activate(MAIN_GALLERY);
    lib.select(0);
    lib.rate(3).unwrap();
    assert_eq!(lib.selected(), Some(0), "no advance in the main view");
    lib.with_gallery(WINDOW_GALLERY, |lib| {
        lib.select(0);
        lib.rate(2).unwrap();
        assert_eq!(lib.selected(), Some(1), "the window's advances");
    });
}

#[test]
fn each_host_keeps_its_own_filter_restriction_and_selection() {
    // Owner, 2026-11-15: the Library views' active filters are apart.
    let lib = library();
    {
        let mut lib = lib.borrow_mut();
        lib.with_gallery(WINDOW_GALLERY, |lib| {
            lib.apply_filter(Filter::parse("a"));
            lib.set_collection(Some("test"));
            lib.select_ids(&["b".to_string()]);
        });
        lib.select_ids(&["c".to_string()]);
    }
    let main = harness(&lib, GalleryOptions::MAIN_PANEL);
    assert!(has(&main, "3 of 3"), "the main panel shows every photo");
    assert!(
        !has(&main, "Album: test"),
        "and no restriction of the window's"
    );
    drop(main);
    let window = harness(&lib, GalleryOptions::LIBRARY_WINDOW);
    assert!(has(&window, "Album: test"), "the window shows its own");
    drop(window);

    let mut lib = lib.borrow_mut();
    assert_eq!(
        lib.gallery_id(),
        MAIN_GALLERY,
        "commands act on the panel's"
    );
    assert!(lib.filter().text.is_empty());
    assert_eq!(lib.collection(), None);
    assert_eq!(lib.selection(), ["c".to_string()]);
    lib.with_gallery(WINDOW_GALLERY, |lib| {
        assert_eq!(lib.filter().text.split_whitespace().next(), Some("a"));
        assert_eq!(lib.collection().as_deref(), Some("test"));
        assert_eq!(lib.selection(), ["b".to_string()]);
    });
    // Lifting the panel's scope leaves the window's.
    lib.show_all();
    lib.with_gallery(WINDOW_GALLERY, |lib| {
        assert_eq!(lib.collection().as_deref(), Some("test"));
    });
}

#[test]
fn the_host_with_input_is_the_one_commands_act_on() {
    let lib = library();
    let mut window = harness(&lib, GalleryOptions::LIBRARY_WINDOW);
    // A click in the window's grid: its gallery is the one in force.
    window.get_by_label("b.jpg").click();
    window.run();
    drop(window);
    let mut lib = lib.borrow_mut();
    assert_eq!(lib.focused_gallery(), WINDOW_GALLERY);
    assert_eq!(lib.gallery_id(), WINDOW_GALLERY);
    assert_eq!(lib.selection(), ["b".to_string()]);
    lib.with_gallery(MAIN_GALLERY, |lib| {
        assert!(lib.selection().is_empty(), "the panel's selection stays");
    });
}

#[test]
fn a_module_s_filter_key_shows_in_the_facets_bar() {
    let lib = library();
    {
        let mut lib = lib.borrow_mut();
        lib.search_keys_mut()
            .register(
                SearchKey::new("test-culling", "sharpness", Message::plain("Sharpness"))
                    .facet("sharpness")
                    .finite(&["high", "low"]),
            )
            .unwrap();
        lib.apply_filter(Filter::parse("rating>=1"));
        let mut counts = BTreeMap::new();
        counts.insert(
            "sharpness".to_string(),
            BTreeMap::from([("high".to_string(), 2u64)]),
        );
        lib.set_facets(helios_vault::Facets(counts));
    }
    let mut h = harness(&lib, GalleryOptions::MAIN_PANEL);
    assert!(has(&h, "Sharpness"), "the key's label heads its line");
    h.get_by_label("high 2").click();
    h.run();
    drop(h);
    let filter = lib.borrow().filter().clone();
    assert_eq!(filter.min_rating, Some(1), "the filter keeps its terms");
    assert!(
        filter.text.contains("sharpness:high"),
        "the click adds the module's term: {filter:?}"
    );
}

/// Three photos with details.
fn detailed() -> Vec<Entry> {
    [("a", 3, 2_000_000), ("b", 5, 1_000), ("c", 1, 30_000_000)]
        .iter()
        .map(|&(id, rating, bytes)| Entry {
            id: id.into(),
            name: format!("{id}.jpg"),
            rating,
            bytes: Some(bytes),
            camera_make: Some("Fujifilm".into()),
            camera_model: Some("X-T5".into()),
            ..Entry::default()
        })
        .collect()
}

/// A library of [`detailed`] photos, shown as `mode`.
fn library_as(mode: ResultsMode) -> Rc<RefCell<Library>> {
    let lib = library();
    {
        let mut lib = lib.borrow_mut();
        lib.set_entries(detailed());
        lib.set_results(&ResultsDesc {
            mode,
            ..ResultsDesc::default()
        });
    }
    lib
}

/// The rect of the node named `label`.
fn rect_of(h: &Harness, label: &str) -> egui::Rect {
    h.get_by_label(label).rect()
}

#[test]
fn details_show_the_columns_and_sort_as_the_gallery() {
    let lib = library_as(ResultsMode::Details);
    let h = harness(&lib, GalleryOptions::RESULTS_PANEL);
    // The default columns' headers, named for screen readers; the
    // gallery's sort (capture time, up) on its column.
    assert!(has(&h, "File name"));
    assert!(has(&h, "Rating"));
    assert!(has(&h, "File size"));
    assert!(has(&h, "Camera"));
    assert!(
        h.query_all_by_label_contains("Capture time")
            .any(|n| n.accesskit_node().role() == egui::accesskit::Role::Button),
        "the capture time's header"
    );
    assert!(has(&h, "a.jpg"), "a row per photo");
    assert!(has(&h, "2.0 MB"), "its size");
    assert!(has(&h, "Fujifilm X-T5"), "its camera");
    drop(h);

    // In the window: a click on the Rating header sorts the gallery by
    // rating, its header then says so.
    let mut shell = helios_gui::Shell::new(helios_gui::helios_panels(), None);
    shell.set_library(detailed());
    shell.library_mut().set_results(&ResultsDesc {
        mode: ResultsMode::Details,
        ..ResultsDesc::default()
    });
    let mut h = helios_gui::headless::harness(shell, egui::vec2(1600.0, 1000.0));
    h.run_steps(4);
    h.get_by_label("Rating").click();
    h.run_steps(4);
    assert_eq!(
        h.state().library().sort(),
        helios_vault::Order::asc(helios_vault::Sort::Rating),
        "the header ran the gallery's sort"
    );
    assert!(
        h.query_by_label("Rating").is_none() && h.query_by_label_contains("Rating").is_some(),
        "the Rating header says it sorts"
    );
}

#[test]
fn the_details_columns_are_each_view_s_and_kept() {
    let lib = library_as(ResultsMode::Details);
    let columns = vec![
        ResultsColumn {
            id: "name".into(),
            width: Some(240),
        },
        ResultsColumn {
            id: "iso".into(),
            width: None,
        },
    ];
    lib.borrow_mut().set_results(&ResultsDesc {
        mode: ResultsMode::Details,
        columns: columns.clone(),
        ..ResultsDesc::default()
    });
    let h = harness(&lib, GalleryOptions::RESULTS_PANEL);
    assert!(has(&h, "ISO"), "the chosen column");
    assert!(!has(&h, "Camera"), "a column not chosen");
    drop(h);
    let mut lib = lib.borrow_mut();
    assert_eq!(lib.results().columns, columns);
    assert_eq!(lib.results().mode, ResultsMode::Details);
    lib.with_gallery(WINDOW_GALLERY, |lib| {
        assert_eq!(lib.results(), ResultsDesc::default(), "the window's own");
    });
}

/// Enter on the focused view, then a double click on a row, open a
/// photo, in the list and in the details.
#[test]
fn list_and_details_open_a_photo_on_enter_and_double_click() {
    for mode in [ResultsMode::List, ResultsMode::Details] {
        let lib = library_as(mode);
        let mut h = harness(&lib, GalleryOptions::RESULTS_PANEL);
        h.get_by_label("b.jpg").click();
        h.run();
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(
            lib.borrow_mut().take_activated().as_deref(),
            Some("b"),
            "{mode:?}: Enter opens the cursor's photo"
        );
        let at = rect_of(&h, "c.jpg").center();
        for _ in 0..2 {
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
            h.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
            h.run();
        }
        assert_eq!(
            lib.borrow_mut().take_activated().as_deref(),
            Some("c"),
            "{mode:?}: a double click opens the row's photo"
        );
    }
}

/// A drag from a row carries the photos out, as from a thumbnail: the
/// selection when the row is in it.
#[test]
fn a_drag_from_a_row_carries_the_photos_out() {
    for mode in [ResultsMode::List, ResultsMode::Details] {
        let lib = library_as(mode);
        lib.borrow_mut()
            .select_ids(&["a".to_string(), "b".to_string()]);
        let mut h = harness(&lib, GalleryOptions::RESULTS_PANEL);
        let from = rect_of(&h, "a.jpg").center();
        h.event(egui::Event::PointerMoved(from));
        h.event(egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
        for step in 1..=4 {
            h.event(egui::Event::PointerMoved(
                from + egui::vec2(0.0, 20.0 * step as f32),
            ));
            h.run();
        }
        let carried =
            egui_pop::api::bus::drag::payload::<DraggedPhotos>(&h.ctx).map(|p| p.0.clone());
        assert_eq!(
            carried,
            Some(vec!["a".to_string(), "b".to_string()]),
            "{mode:?}: the selection goes"
        );
        h.event(egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.run();
    }
}

/// In a narrow panel the Results bar keeps Compare on one line: its icon
/// alone, named, beside the view menu.
#[test]
fn a_narrow_bar_keeps_compare_on_one_line() {
    let lib = library();
    lib.borrow_mut()
        .select_ids(&["a".to_string(), "b".to_string()]);
    let lib2 = lib.clone();
    let options = GalleryOptions::RESULTS_PANEL;
    let mut h = Harness::builder()
        .allow_missing_glyphs()
        .with_size(egui::vec2(230.0, 400.0))
        .build_ui(move |ui| lib2.borrow_mut().show(ui, &options));
    h.run();
    let compare = rect_of(&h, "Compare");
    let line = rect_of(&h, "View as").height();
    assert!(
        compare.height() <= line + 1.0,
        "Compare on one line: {compare:?}, a line {line}"
    );
    drop(h);
    // Wide, its words.
    let h = harness(&lib, GalleryOptions::RESULTS_PANEL);
    assert!(rect_of(&h, "Compare").width() > rect_of(&h, "Compare").height() * 1.5);
}
