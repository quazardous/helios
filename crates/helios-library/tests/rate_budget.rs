//! One rate budget per account ([`helios_connect::budget`]): an import's
//! downloads and the import window's thumbnails, run together against a
//! fake album service that counts its requests, never ask more than the
//! connector declared, and a waiting download goes before the thumbnails.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use helios_connect::budget::{Budget, Use};
use helios_connect::fake_albums::AlbumsProvider;
use helios_connect::{
    Cancel, Connector, ConnectorProvider, ConnectorServices, Item, ItemRef, Limits, Settings,
    SourceRef,
};
use helios_core::jobs::Jobs;
use helios_library::fetch::{self, Remote};
use helios_library::import::{Duplicates, Layout, Options, Placement};
use helios_vault::{MemoryStore, Vault};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-rate-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The service, its connector, an album of `n` photos of `account`.
fn service(
    account: &str,
    n: usize,
    limits: Limits,
) -> (AlbumsProvider, Box<dyn Connector>, SourceRef, Vec<Item>) {
    let mut provider = AlbumsProvider::new(&[("Trip", n)], 100);
    provider.thumbnails = true;
    provider.limits = limits;
    let connector = provider
        .create(&Settings::Null, &ConnectorServices::default())
        .unwrap();
    let source = SourceRef {
        connector: helios_connect::fake_albums::ID.into(),
        id: format!("{account}/album-0"),
        label: "Trip".into(),
        root: None,
        capacity: None,
    };
    let items = connector
        .list(&source, None, 100, &Cancel::new())
        .unwrap()
        .items;
    assert_eq!(items.len(), n);
    (provider, connector, source, items)
}

/// Imports `items` on a thread of its own.
fn import(
    connector: Arc<dyn Connector>,
    source: SourceRef,
    items: Vec<Item>,
    root: PathBuf,
) -> std::thread::JoinHandle<u64> {
    std::thread::spawn(move || {
        let vault = Vault::new(MemoryStore::default());
        let options = Options {
            placement: Placement::Copy { into: Some(root) },
            layout: Layout::Flat,
            duplicates: Duplicates::Keep,
            ..Options::default()
        };
        let remote = Remote {
            connector: &*connector,
            source,
            items,
            new_only: false,
        };
        let jobs = Jobs::new(Arc::default());
        let report =
            fetch::import_remote(&vault, &jobs, &remote, &options, 1000, &mut |_| {}).unwrap();
        report.fetched
    })
}

/// Glances as the import window reads them: `n` thumbnails, each waiting
/// for its place in the account's budget.
fn glances(
    connector: Arc<dyn Connector>,
    source: SourceRef,
    items: Vec<Item>,
    n: usize,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let budget = Budget::of(&source, connector.describe().caps.limits);
        for item in items.iter().cycle().take(n) {
            let _permit = budget
                .take(Use::Thumbnail, &Instant::now, &|| false)
                .unwrap();
            let at = ItemRef {
                source: source.clone(),
                id: item.id.clone(),
            };
            connector.thumbnail(&at, 64).unwrap();
        }
    })
}

#[test]
fn downloads_and_thumbnails_together_stay_within_the_account_s_rate() {
    let limits = Limits {
        per_second: Some(8),
        parallel: Some(2),
        ..Limits::default()
    };
    let (provider, connector, source, items) = service("rate-ann", 5, limits);
    let connector: Arc<dyn Connector> = Arc::from(connector);
    let root = temp("together");
    let thumbs: Vec<_> = (0..3)
        .map(|_| glances(connector.clone(), source.clone(), items.clone(), 3))
        .collect();
    let fetched = import(connector.clone(), source.clone(), items, root.clone())
        .join()
        .unwrap();
    for t in thumbs {
        t.join().unwrap();
    }
    assert_eq!(fetched, 5);
    let requests = &provider.requests;
    assert!(requests.most_at_once.load(Ordering::SeqCst) <= 2);
    let log = requests.log.lock().unwrap().clone();
    assert_eq!(log.len(), 5 + 9);
    // Counted where the service sees them, a little after each place
    // was given: a second's window, less that margin.
    let window = Duration::from_millis(950);
    for (_, start) in &log {
        let within = log
            .iter()
            .filter(|(_, t)| *t >= *start && t.duration_since(*start) < window)
            .count();
        assert!(within <= 8, "{within} requests within a second: {log:?}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_waiting_download_goes_before_the_waiting_thumbnails() {
    let limits = Limits {
        parallel: Some(1),
        ..Limits::default()
    };
    let (provider, connector, source, items) = service("rate-bob", 2, limits);
    provider.hold_thumbnails.store(true, Ordering::SeqCst);
    let connector: Arc<dyn Connector> = Arc::from(connector);
    let root = temp("first");
    // A thumbnail holds the only place.
    let first = glances(connector.clone(), source.clone(), items.clone(), 1);
    while provider.thumbnails_asked.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(2));
    }
    // An import, then more thumbnails, wait behind it.
    let import = import(
        connector.clone(),
        source.clone(),
        items.clone(),
        root.clone(),
    );
    std::thread::sleep(Duration::from_millis(50));
    let more: Vec<_> = (0..3)
        .map(|_| glances(connector.clone(), source.clone(), items.clone(), 2))
        .collect();
    std::thread::sleep(Duration::from_millis(50));
    provider.hold_thumbnails.store(false, Ordering::SeqCst);
    first.join().unwrap();
    assert_eq!(import.join().unwrap(), 2);
    for t in more {
        t.join().unwrap();
    }
    let log = provider.requests.log.lock().unwrap().clone();
    let kinds: Vec<Use> = log.iter().map(|(u, _)| *u).collect();
    assert_eq!(kinds.len(), 1 + 2 + 6);
    assert_eq!(kinds[..2], [Use::Thumbnail, Use::Download], "{kinds:?}");
    assert!(provider.requests.most_at_once.load(Ordering::SeqCst) <= 1);
    let _ = std::fs::remove_dir_all(&root);
}
