//! One rate budget per account ([`helios_connect::budget`]): an import's
//! downloads and the import window's thumbnails, run together against a
//! fake album service that counts its requests, never ask more than the
//! connector declared, and a waiting download goes before the thumbnails.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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
            holder: None,
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
fn an_import_and_thumbnails_together_stay_within_the_parallel_places() {
    let limits = Limits {
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
    assert!(provider.requests.most_at_once.load(Ordering::SeqCst) <= 2);
    assert_eq!(provider.requests.log.lock().unwrap().len(), 5 + 9);
    let _ = std::fs::remove_dir_all(&root);
}

/// A clock the test moves: milliseconds after a start.
struct FakeClock(Instant, AtomicU64);

impl FakeClock {
    fn now(&self) -> Instant {
        self.0 + Duration::from_millis(self.1.load(Ordering::SeqCst))
    }
}

#[test]
fn downloads_and_thumbnails_together_stay_within_the_rate_on_a_fake_clock() {
    let limits = Limits {
        per_second: Some(8),
        parallel: Some(2),
        ..Limits::default()
    };
    let (provider, connector, source, items) = service("rate-cat", 5, limits);
    let connector: Arc<dyn Connector> = Arc::from(connector);
    let budget = Budget::of(&source, limits);
    let clock = Arc::new(FakeClock(Instant::now(), AtomicU64::new(0)));
    // Each request's use and the fake time its place was given.
    let granted = Arc::new(Mutex::new(Vec::new()));
    let worker = |use_: Use, n: usize| {
        let (connector, source, items, budget, clock, granted) = (
            connector.clone(),
            source.clone(),
            items.clone(),
            budget.clone(),
            clock.clone(),
            granted.clone(),
        );
        std::thread::spawn(move || {
            for item in items.iter().cycle().take(n) {
                // The time the budget counted the place at: the last asked.
                let last = std::cell::Cell::new(clock.now());
                let now = || {
                    last.set(clock.now());
                    last.get()
                };
                let _permit = budget.take(use_, &now, &|| false).unwrap();
                granted.lock().unwrap().push((use_, last.get()));
                let at = ItemRef {
                    source: source.clone(),
                    id: item.id.clone(),
                };
                match use_ {
                    Use::Download => {
                        let mut sink = Vec::new();
                        connector.fetch(&at, 0, &mut sink, &Cancel::new()).unwrap();
                    }
                    Use::Thumbnail => {
                        connector.thumbnail(&at, 64).unwrap();
                    }
                }
            }
        })
    };
    let threads = vec![
        worker(Use::Download, 10),
        worker(Use::Download, 10),
        worker(Use::Thumbnail, 10),
        worker(Use::Thumbnail, 10),
    ];
    // The clock moves a tenth of a second at a time until all is asked.
    while granted.lock().unwrap().len() < 40 {
        std::thread::sleep(Duration::from_millis(2));
        clock.1.fetch_add(100, Ordering::SeqCst);
    }
    for t in threads {
        t.join().unwrap();
    }
    let granted = granted.lock().unwrap().clone();
    for (_, start) in &granted {
        let within = granted
            .iter()
            .filter(|(_, t)| *t >= *start && t.duration_since(*start) < Duration::from_secs(1))
            .count();
        assert!(
            within <= 8,
            "{within} requests within a second: {granted:?}"
        );
    }
    assert!(provider.requests.most_at_once.load(Ordering::SeqCst) <= 2);
    assert_eq!(provider.requests.log.lock().unwrap().len(), 40);
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
