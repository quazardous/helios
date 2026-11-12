//! The cache service: the eviction order, pins, single flight, and the
//! stores' usage and clearing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use helios_cache::{
    Cost, Flight, FlightError, Lru, Pins, RENDER_STEP, Scope, Service, Store, THUMB_SMALL, Tier,
    Usage,
};

#[test]
fn the_least_recently_used_goes_first() {
    let mut lru = Lru::new();
    for (i, key) in ["a", "b", "c", "d"].into_iter().enumerate() {
        lru.insert(key, 10, i as u64, 0);
    }
    assert!(lru.touch(&"a", 10));
    let gone = lru.evict(20, |_| false);
    assert_eq!(gone, [("b", 10), ("c", 10)]);
    assert_eq!(lru.bytes(), 20);
    assert!(lru.contains(&"a") && lru.contains(&"d"));
}

#[test]
fn a_dear_miss_outlives_a_cheap_one_used_at_the_same_time() {
    let mut lru = Lru::new();
    let slow = Cost::Render.bonus(800_000_000, 4 << 20);
    let quick = Cost::Cheap.bonus(2_000_000, 4 << 20);
    assert!(slow > quick);
    lru.insert("render", 4 << 20, 1_000, slow);
    lru.insert("crop", 4 << 20, 2_000, quick);
    let gone = lru.evict(4 << 20, |_| false);
    assert_eq!(gone, [("crop", 4 << 20)]);
}

#[test]
fn eviction_skips_what_is_kept_and_stops_under_its_target() {
    let mut lru = Lru::new();
    for i in 0..10_000u64 {
        lru.insert(i, 1, i, 0);
    }
    let pins = Pins::new();
    let pin = pins.pin(0u64);
    let gone = lru.evict(9_990, |k| pins.is_pinned(k));
    assert_eq!(gone.len(), 10);
    assert_eq!(gone.first().unwrap().0, 1, "the pinned entry is skipped");
    assert!(lru.contains(&0));
    drop(pin);
    assert!(!pins.is_pinned(&0), "a dropped guard unpins");
    let gone = lru.evict(9_989, |k| pins.is_pinned(k));
    assert_eq!(gone, [(0, 1)]);
}

#[test]
fn concurrent_requests_for_one_key_compute_once() {
    let flight: Arc<Flight<&str, u64>> = Arc::default();
    let computed = Arc::new(AtomicU64::new(0));
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (flight, computed, barrier) = (flight.clone(), computed.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                flight.run(&"raw", Duration::from_secs(10), || {
                    computed.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(100));
                    Some(42)
                })
            })
        })
        .collect();
    let values: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(values.iter().all(|v| v.as_ref().unwrap().0 == 42));
    // Threads that came after the first computation ended compute again;
    // those that met it merged.
    let merged = values.iter().filter(|v| v.as_ref().unwrap().1).count() as u64;
    assert_eq!(computed.load(Ordering::SeqCst) + merged, 8);
    assert!(merged > 0, "some requests merged");
    assert_eq!(flight.merged(), merged);
    assert_eq!(flight.running(), 0);
}

#[test]
fn a_merged_wait_has_a_deadline_and_a_failed_leader_frees_its_waiters() {
    let flight: Arc<Flight<u8, u8>> = Arc::default();
    let started = Arc::new(Barrier::new(2));
    let leader = {
        let (flight, started) = (flight.clone(), started.clone());
        std::thread::spawn(move || {
            flight.run(&1, Duration::from_secs(1), || {
                started.wait();
                std::thread::sleep(Duration::from_millis(300));
                None
            })
        })
    };
    started.wait();
    assert_eq!(
        flight.run(&1, Duration::from_millis(10), || Some(9)),
        Err(FlightError::Timeout)
    );
    assert_eq!(
        flight.run(&1, Duration::from_secs(10), || Some(9)),
        Err(FlightError::Abandoned)
    );
    assert_eq!(leader.join().unwrap(), Err(FlightError::Abandoned));
    assert_eq!(
        flight.run(&1, Duration::from_secs(1), || Some(9)),
        Ok((9, false))
    );
}

/// A store of fake entries.
struct Fake {
    scope: Scope,
    entries: Mutex<Vec<(&'static str, u64)>>,
}

impl Store for Fake {
    fn usage(&self) -> Vec<Usage> {
        let entries = self.entries.lock().unwrap();
        [THUMB_SMALL, RENDER_STEP]
            .into_iter()
            .map(|kind| Usage {
                scope: self.scope.clone(),
                kind,
                tier: kind.tiers[0],
                entries: entries.iter().filter(|e| e.0 == kind.id).count() as u64,
                bytes: entries.iter().filter(|e| e.0 == kind.id).map(|e| e.1).sum(),
                pinned: 0,
                budget: None,
                shown: None,
            })
            .collect()
    }

    fn clear(&self, kind: Option<&str>) -> u64 {
        let mut entries = self.entries.lock().unwrap();
        let freed = entries
            .iter()
            .filter(|e| kind.is_none_or(|k| k == e.0))
            .map(|e| e.1)
            .sum();
        entries.retain(|e| kind.is_some_and(|k| k != e.0));
        freed
    }
}

#[test]
fn the_service_counts_its_stores_and_clears_them_by_kind() {
    let service = Service::new();
    let fake = Arc::new(Fake {
        scope: Scope::silo("library"),
        entries: Mutex::new(vec![
            ("thumb.256", 100),
            ("thumb.256", 50),
            ("render.step", 1000),
        ]),
    });
    service.attach(Arc::downgrade(&fake) as std::sync::Weak<dyn Store>);
    assert_eq!(service.used(Tier::Disk), 150);
    assert_eq!(service.used(Tier::Ram), 1000);
    let counters = service.counters(&Scope::silo("library"), &THUMB_SMALL);
    counters.hits.fetch_add(3, Ordering::Relaxed);
    let snapshot = service.snapshot();
    let thumbs = snapshot["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == "thumb.256")
        .unwrap();
    assert_eq!(thumbs["entries"], 2);
    assert_eq!(thumbs["scope"], "silo:library");
    assert_eq!(thumbs["stats"]["hits"], 3);
    assert_eq!(service.clear(Some("thumb.256")), 150);
    assert_eq!(service.used(Tier::Disk), 0);
    assert_eq!(service.used(Tier::Ram), 1000, "another kind is kept");
    assert_eq!(service.clear(None), 1000);
    drop(fake);
    assert!(
        service.usage().is_empty(),
        "a dropped store is no longer counted"
    );
}

#[test]
fn the_ram_tier_has_a_budget_by_default_and_takes_another() {
    let service = Service::new();
    assert!(service.budget(Tier::Ram).is_some());
    service.set_budget(Tier::Ram, Some(512 << 20));
    assert_eq!(service.budget(Tier::Ram), Some(512 << 20));
    assert_eq!(service.budget(Tier::Disk), None);
    assert_eq!(helios_cache::size_text(512 << 20), "512.0 MiB");
    assert_eq!(helios_cache::eviction_target(1000), 900);
}
