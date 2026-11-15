//! The Library's gallery component (`docs/design/library-search.md`,
//! section 6, slice G1): each host gives its options and the gallery
//! draws what they ask (no search field without search, no sources
//! without sources, no sort or view options when hidden); the main
//! window's panel and the Library window draw the same `main` gallery,
//! so one filter and one selection; and the facets bar lists the filter
//! keys declared, a module's key included, a click adding its term.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_pop::views::library::filter::Filter;
use helios_core::Message;
use helios_gui::library::{Entry, GalleryOptions, Library, SearchKey};

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
    let mut h = Harness::builder()
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
    assert!(has(&main, "Auto-advance"), "and its view options");
    assert!(has(&main, "3 of 3"), "and its count");
    assert!(has(&main, "Folders"), "and its sources");
    drop(main);

    let bare = harness(&lib, GalleryOptions::BARE);
    assert!(!has(&bare, "Search images"), "no search: no field");
    assert!(!has(&bare, "Auto-advance"), "no view options");
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
    assert!(!has(&h, "Auto-advance"));
    assert!(!has(&h, "3 of 3"));
}

#[test]
fn both_hosts_share_the_main_gallery_s_filter_and_selection() {
    let lib = library();
    lib.borrow_mut().apply_filter(Filter::parse("a"));
    lib.borrow_mut().select_ids(&["a".to_string()]);
    let window = harness(&lib, GalleryOptions::LIBRARY_WINDOW);
    drop(window);
    let main = harness(&lib, GalleryOptions::MAIN_PANEL);
    drop(main);
    let lib = lib.borrow();
    assert_eq!(lib.filter().text, "a");
    assert_eq!(lib.selection(), ["a".to_string()]);
}

#[test]
fn a_module_s_filter_key_shows_in_the_facets_bar() {
    let lib = library();
    {
        let mut lib = lib.borrow_mut();
        lib.search_keys_mut()
            .register(
                SearchKey::new(
                    "test-culling",
                    "sharpness",
                    Message::new("test-sharpness", "Sharpness"),
                )
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
