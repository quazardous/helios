//! A service's albums in the import window: an account whose connector
//! lists its albums (here a fake one, two albums a page) shows them under
//! its item in the Services panel, page by page ("More albums"); clicking
//! one selects that album as the source, its photos fill the grid ("in
//! your Library" marked) and the footer's Import fetches the ones ticked
//! under the album's name. A connector without albums shows none, even
//! one declaring the `collections` browser.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use helios_connect::account::{
    Account, AccountBook as _, AccountState, MemoryAccounts, MemorySecrets,
};
use helios_connect::fake_albums::AlbumsProvider;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

fn until(h: &mut Harness<'_, Shell>, what: &str, done: impl Fn(&mut Harness<'_, Shell>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let labels: Vec<String> = h
        .query_all_by_label_contains("")
        .filter_map(|n| n.accesskit_node().label())
        .collect();
    panic!("never got there: {what}; shown: {labels:?}");
}

fn shown(h: &Harness<'_, Shell>, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

fn click(h: &mut Harness<'_, Shell>, label: &str) {
    h.query_all_by_label(label)
        .next()
        .unwrap_or_else(|| panic!("no {label}"))
        .click();
    h.run_steps(3);
}

struct Folder;

impl helios_gui::shell::LibraryFolder for Folder {
    fn get(&self) -> Result<Option<std::path::PathBuf>, String> {
        Ok(Some(std::env::temp_dir()))
    }

    fn set(&self, _dir: &std::path::Path) -> Result<(), String> {
        Ok(())
    }
}

/// Each import: the source's name, and the items' ids.
type Imported = Arc<Mutex<Vec<(String, Vec<String>)>>>;

/// The import window open on a connected account of `provider`'s
/// connector, selected.
fn world(
    provider: Arc<dyn helios_connect::ConnectorProvider>,
) -> (Harness<'static, Shell>, Imported) {
    let book = Arc::new(MemoryAccounts::default());
    let mut account = Account::new(&provider.manifest().id, "ann@example.org", &[]);
    account.state = AccountState::Connected;
    book.save(&account).unwrap();
    let imported = Imported::default();
    let mut shell = Shell::new(helios_panels(), None);
    shell.add_import_connector(provider);
    let seen = imported.clone();
    shell.set_import_remote(Arc::new(
        move |job: helios_gui::shell::RemoteImport, emit: &mut dyn FnMut(&Value)| {
            let ids: Vec<String> = job.items.iter().map(|i| i.id.0.clone()).collect();
            for id in &ids {
                emit(&json!({ "item": id, "status": "added" }));
            }
            let added = ids.len();
            seen.lock().unwrap().push((job.source.label.clone(), ids));
            Ok(json!({ "added": added, "files": added }))
        },
    ));
    // The first photo of the first album is in the Library already.
    shell.set_import_remote_known(Arc::new(|_, items| {
        items.iter().map(|i| i.id.0 == "a0-p0").collect()
    }));
    shell.set_import_library_folder(Arc::new(Folder));
    shell.set_import_accounts(book, Arc::new(MemorySecrets::default()));
    let mut h = headless::harness(shell, vec2(1280.0, 1100.0));
    h.run();
    h.state_mut()
        .run("library.importWindow", Value::Null, Origin::User)
        .unwrap();
    h.run_steps(2);
    until(&mut h, "the account listed", |h| {
        shown(h, "ann@example.org")
    });
    click(&mut h, "ann@example.org");
    (h, imported)
}

#[test]
fn an_account_s_albums_are_listed_page_by_page_and_one_imports_as_a_source() {
    let provider = AlbumsProvider::new(&[("Trip", 3), ("Family", 1), ("Empty", 0)], 2);
    let pages = provider.album_pages.clone();
    let (mut h, imported) = world(Arc::new(provider));
    until(&mut h, "the first page of albums", |h| {
        shown(h, "Trip") && shown(h, "Family")
    });
    assert!(!shown(&h, "Empty"), "on the second page");
    assert_eq!(pages.load(std::sync::atomic::Ordering::SeqCst), 1);
    click(&mut h, "More albums");
    until(&mut h, "the second page", |h| shown(h, "Empty"));
    assert!(!shown(&h, "More albums"), "every album read");
    assert_eq!(pages.load(std::sync::atomic::Ordering::SeqCst), 2);
    // Nothing in the grid before an album is selected.
    assert!(h.state().import_cells().is_empty());
    click(&mut h, "Trip");
    until(&mut h, "the album's photos in the grid", |h| {
        h.state().import_cells().len() == 3
    });
    let cells = h.state().import_cells();
    let names: Vec<&str> = cells.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(names, ["IMG_0000.JPG", "IMG_0001.JPG", "IMG_0002.JPG"]);
    // The first is in the Library: marked, and left out by New only.
    assert_eq!(
        cells.iter().map(|c| c.2).collect::<Vec<_>>(),
        [true, false, false]
    );
    until(&mut h, "two taken", |h| shown(h, "Import 2"));
    assert!(imported.lock().unwrap().is_empty(), "nothing fetched yet");
    click(&mut h, "Import 2");
    until(&mut h, "the album imported", |_| {
        !imported.lock().unwrap().is_empty()
    });
    // Under the album's name: the import history names it.
    assert_eq!(
        imported.lock().unwrap()[0],
        ("Trip".into(), vec!["a0-p1".into(), "a0-p2".into()])
    );
    assert!(!h.state().import_window_open(), "closed as a card's import");
}

#[test]
fn another_album_or_the_account_clicked_leaves_the_album() {
    let provider = AlbumsProvider::new(&[("Trip", 3), ("Family", 1)], 5);
    let (mut h, _) = world(Arc::new(provider));
    until(&mut h, "the albums", |h| shown(h, "Family"));
    click(&mut h, "Trip");
    until(&mut h, "Trip's photos", |h| {
        h.state().import_cells().len() == 3
    });
    click(&mut h, "Family");
    until(&mut h, "Family's photo alone", |h| {
        h.state().import_cells().len() == 1
    });
    click(&mut h, "ann@example.org");
    until(&mut h, "the account alone, no album", |h| {
        h.state().import_cells().is_empty()
            && shown(h, "Choose an album under the account, in Sources.")
    });
}

#[test]
fn a_connector_without_albums_lists_none() {
    // The test server declares the `collections` browser, not the caps.
    let (mut h, _) = world(Arc::new(helios_connect::fake::FakeProvider));
    h.run_steps(5);
    for label in ["Reading the albums…", "More albums", "No albums"] {
        assert!(!shown(&h, label), "{label}");
    }
    assert!(h.state().import_cells().is_empty());
}
