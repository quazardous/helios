//! The connector contract, run against the registry's folder and card
//! connectors over temporary trees.

use std::path::PathBuf;
use std::sync::Arc;

use helios_connect::card::CardConnector;
use helios_connect::contract::{Fixture, check};
use helios_connect::folder::Folder;
use helios_connect::{
    Cancel, Connector, ConnectorError, ConnectorServices, PAGE, Settings, registry,
};
use helios_storage::Card;

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("helios-connect-{name}-{}", std::process::id()))
}

fn make(id: &'static str, services: ConnectorServices) -> impl Fn() -> Box<dyn Connector> {
    move || {
        registry()[id]
            .create(&Settings::Null, &services)
            .expect("created")
    }
}

#[test]
fn the_folder_connector_keeps_the_contract() {
    let root = temp("folder");
    let fixture = Fixture::write(&root, false).unwrap();
    let places = root.clone();
    let services = ConnectorServices {
        places: Some(Arc::new(move || vec![places.clone()])),
        ..ConnectorServices::default()
    };
    check(&make("folder", services), &Folder::source(&root), &fixture);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_folder_holding_dcim_is_read_from_there_as_today() {
    let root = temp("folder-dcim");
    let fixture = Fixture::write(&root, true).unwrap();
    let places = root.clone();
    let services = ConnectorServices {
        places: Some(Arc::new(move || vec![places.clone()])),
        ..ConnectorServices::default()
    };
    check(&make("folder", services), &Folder::source(&root), &fixture);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_card_connector_keeps_the_contract() {
    let root = temp("card");
    let fixture = Fixture::write(&root, true).unwrap();
    let card = Card {
        label: "EOS_DIGITAL".into(),
        root: root.clone(),
        capacity: Some(64_000_000_000),
    };
    let polled = card.clone();
    let services = ConnectorServices {
        cards: Some(Arc::new(move || vec![polled.clone()])),
        ..ConnectorServices::default()
    };
    let source = CardConnector::source(&card);
    assert_eq!(source.label, "EOS_DIGITAL");
    assert_eq!(source.capacity, Some(64_000_000_000));
    check(&make("card", services), &source, &fixture);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_listing_is_the_one_the_import_reads() {
    // The import operation lists a source through the same walk.
    let root = temp("same-walk");
    let fixture = Fixture::write(&root, true).unwrap();
    let files = helios_connect::local::files_of(&root).unwrap();
    let want: Vec<PathBuf> = fixture.expected.iter().map(|e| root.join(&e.id)).collect();
    assert_eq!(files, want);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_source_gone_says_so() {
    let root = temp("gone");
    let _ = std::fs::remove_dir_all(&root);
    let folder = make("folder", ConnectorServices::default())();
    assert_eq!(
        folder.list(&Folder::source(&root), None, PAGE, &Cancel::new()),
        Err(ConnectorError::Gone)
    );
}
