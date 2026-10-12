//! The engine cache within a memory budget: least recently used content is
//! evicted, evicted content is computed again identically, and content
//! still being computed is never evicted.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use helios_core::*;

/// Says when a computation starts, then holds it until released.
type Gate = (Mutex<Sender<()>>, Mutex<Receiver<()>>);

/// Copies its input; optionally held by a gate.
struct Copy {
    gate: Option<Gate>,
}

impl Module for Copy {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "copy".into(),
            version: 1,
            description: String::new(),
            params: vec![],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Other,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        if let Some((started, release)) = &self.gate {
            started.lock().unwrap().send(()).unwrap();
            release.lock().unwrap().recv().unwrap();
        }
        Ok(vec![inputs[0].sample(output)])
    }
}

/// 16 x 16 pixels, one channel of f32: 1 KiB per region.
const TILE: u32 = 16;
const TILE_BYTES: usize = (TILE * TILE * 4) as usize;

fn network(module: Copy) -> (Graph, NodeId) {
    let mut g = Graph::new();
    let src = g.add_source(
        "src",
        Image::from_fn(Region::sized(TILE * 8, TILE), 1, |x, y, _| {
            (x * 7 + y) as f32
        }),
    );
    let copy = g
        .add_module("copy", ModuleRef::new(module), &Params::new())
        .unwrap();
    g.connect(src, "image", copy, "input").unwrap();
    (g, copy)
}

fn tile(i: i64) -> Region {
    Region::new(i * i64::from(TILE), 0, TILE, TILE)
}

#[test]
fn the_cache_stays_within_its_budget_and_recomputes_what_it_evicted() {
    let (g, copy) = network(Copy { gate: None });
    // Room for four regions: each render holds two (the source region and the copy).
    let e = Engine::with_budget(Arc::new(Bus::new()), Some(4 * TILE_BYTES));
    let first = e.render(&g, copy, "output", tile(0)).unwrap();
    for i in 1..5 {
        e.render(&g, copy, "output", tile(i)).unwrap();
    }
    let d = e.describe();
    assert!(
        d["cache"]["bytes"].as_u64().unwrap() <= 4 * TILE_BYTES as u64,
        "{d}"
    );
    assert_eq!(
        e.counters().evictions,
        6,
        "five renders of two entries, four fit"
    );

    let before = e.counters();
    let again = e.render(&g, copy, "output", tile(0)).unwrap();
    assert_eq!(
        e.counters().since(&before).computed,
        2,
        "tile 0 was evicted: computed again"
    );
    assert_eq!(again.content_hash(), first.content_hash());

    let before = e.counters();
    e.render(&g, copy, "output", tile(0)).unwrap();
    assert_eq!(
        e.counters().since(&before).computed,
        0,
        "the most recent content stays"
    );
}

#[test]
fn without_a_budget_nothing_is_evicted() {
    let (g, copy) = network(Copy { gate: None });
    let e = Engine::new(Arc::new(Bus::new()));
    for i in 0..8 {
        e.render(&g, copy, "output", tile(i)).unwrap();
    }
    assert_eq!(e.counters().evictions, 0);
    assert_eq!(e.describe()["cache"]["bytes"], 16 * TILE_BYTES as u64);
}

#[test]
fn content_being_computed_is_never_evicted() {
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let (gated, gated_copy) = network(Copy {
        gate: Some((Mutex::new(started_tx), Mutex::new(release_rx))),
    });
    let (free, free_copy) = network(Copy { gate: None });
    let e = Engine::with_budget(Arc::new(Bus::new()), Some(2 * TILE_BYTES));
    thread::scope(|s| {
        let held = s.spawn(|| e.render(&gated, gated_copy, "output", tile(0)).unwrap());
        started_rx.recv().unwrap();
        // Fill the cache far past its budget while the gated render runs.
        for i in 1..8 {
            e.render(&free, free_copy, "output", tile(i)).unwrap();
        }
        let status = e.status(&gated, gated_copy, tile(0)).unwrap();
        assert_eq!(
            status[1].state,
            NodeState::Running,
            "still in the cache, still running"
        );
        release_tx.send(()).unwrap();
        let image = held.join().unwrap();
        assert_eq!(image.pixel(3, 2)[0], (3 * 7 + 2) as f32);
    });
}

/// A chain of copies: `n` steps after the source, each reading the one
/// before.
fn chain(n: usize) -> (Graph, NodeId) {
    let mut g = Graph::new();
    let mut last = g.add_source(
        "src",
        Image::from_fn(Region::sized(TILE * 8, TILE), 1, |x, y, _| {
            (x * 7 + y) as f32
        }),
    );
    let mut port = "image";
    for i in 0..n {
        let copy = g
            .add_module(
                &format!("copy{i}"),
                ModuleRef::new(Copy { gate: None }),
                &Params::new(),
            )
            .unwrap();
        g.connect(last, port, copy, "input").unwrap();
        (last, port) = (copy, "output");
    }
    (g, last)
}

#[test]
fn a_render_never_evicts_what_it_still_needs() {
    // A budget smaller than one step: without pins, every input would be
    // evicted as soon as it is computed, before its reader ran.
    let (g, last) = chain(6);
    let e = Engine::with_budget(Arc::new(Bus::new()), Some(TILE_BYTES / 2));
    let events = e.bus().subscribe();
    let image = e.render(&g, last, "output", tile(2)).unwrap();
    let c = e.counters();
    assert_eq!(c.computed, 7, "each step once: none computed again");
    assert_eq!(image.pixel(40, 3)[0], (40 * 7 + 3) as f32);
    // Once the render is done its pins go: the cache is back within budget,
    // and the event says so.
    let d = e.describe();
    assert!(
        d["cache"]["bytes"].as_u64().unwrap() <= TILE_BYTES as u64 / 2,
        "{d}"
    );
    assert_eq!(d["cache"]["pinned"], 0);
    let settled: Vec<_> = events
        .try_iter()
        .filter(|ev| ev.kind == "engine.cache_settled")
        .collect();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].payload["evicted"], 7);
    assert_eq!(settled[0].payload["bytes"], 0);
    assert_eq!(settled[0].payload["budget"], TILE_BYTES / 2);
}

#[test]
fn a_small_budget_gives_the_same_result() {
    let (g, last) = chain(5);
    let full = Region::sized(TILE * 8, TILE);
    let free = Engine::new(Arc::new(Bus::new()))
        .render(&g, last, "output", full)
        .unwrap();
    let tight = Engine::with_budget(Arc::new(Bus::new()), Some(TILE_BYTES));
    for t in full.tiles(TILE / 2) {
        let part = tight.render(&g, last, "output", t).unwrap();
        assert_eq!(part.content_hash(), free.sample(t).content_hash(), "{t}");
    }
    assert!(tight.counters().evictions > 0);
}

#[test]
fn the_default_budget_is_a_share_of_memory() {
    use helios_core::engine::{DEFAULT_CACHE_SHARE, memory_share};
    assert_eq!(memory_share(25.0, Some(16 << 30)), Some(4 << 30));
    assert_eq!(memory_share(0.0, Some(16 << 30)), None, "0: no limit");
    assert_eq!(memory_share(DEFAULT_CACHE_SHARE, None), Some(2 << 30));
    const { assert!(DEFAULT_CACHE_SHARE > 0.0 && DEFAULT_CACHE_SHARE < 100.0) };
}
