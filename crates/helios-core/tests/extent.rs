//! Extents: a module whose outputs are a constant away from a region (a
//! drawn shape) is filled there by the engine, without running it or
//! reading its inputs, and tiles still equal one render.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use helios_core::*;

/// A square of side `side` at `(x, y)`: `inside` there, `fill` elsewhere;
/// with an input, it adds the input inside the square (and reads it
/// nowhere else). Counts its runs.
struct Square {
    x: i64,
    y: i64,
    side: u32,
    fill: f32,
    with_input: bool,
    runs: Arc<AtomicU64>,
}

impl Module for Square {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "square".into(),
            version: 1,
            description: String::new(),
            params: vec![],
            inputs: if self.with_input {
                vec![PortSpec::image("input", 1)]
            } else {
                vec![]
            },
            outputs: vec![PortSpec::image("output", 1), PortSpec::image("pair", 2)],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Mask,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn extent(&self, (): &()) -> Option<Extent> {
        Some(Extent {
            region: Region::new(self.x, self.y, self.side, self.side),
            fill: self.fill,
        })
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let square = Region::new(self.x, self.y, self.side, self.side);
        let value = |x: i64, y: i64| {
            if square.contains(&Region::new(x, y, 1, 1)) {
                let base = (x - self.x + y - self.y) as f32 * 0.1;
                base + inputs.first().map_or(0.0, |i| i.pixel(x, y)[0])
            } else {
                self.fill
            }
        };
        Ok(vec![
            Image::from_fn(output, 1, |x, y, _| value(x, y)),
            Image::from_fn(output, 2, |x, y, _| value(x, y)),
        ])
    }
}

fn square(with_input: bool) -> (Square, Arc<AtomicU64>) {
    let runs = Arc::new(AtomicU64::new(0));
    (
        Square {
            x: 20,
            y: 12,
            side: 10,
            fill: 0.25,
            with_input,
            runs: runs.clone(),
        },
        runs,
    )
}

#[test]
fn tiles_outside_the_extent_are_filled_and_equal_one_render() {
    let full = Region::sized(64, 48);
    let (module, runs) = square(false);
    let mut g = Graph::new();
    let node = g
        .add_module("square", ModuleRef::new(module), &Params::new())
        .unwrap();

    let one = Engine::new(Arc::new(Bus::new()));
    let whole = one.render(&g, node, "output", full).unwrap();
    let whole_pair = one.render(&g, node, "pair", full).unwrap();
    assert_eq!(one.counters().filled, 0, "the render overlaps the square");

    let tiled = Engine::new(Arc::new(Bus::new()));
    let ran = runs.load(Ordering::SeqCst);
    let tiles = full.tiles(16);
    for tile in &tiles {
        let part = tiled.render(&g, node, "output", *tile).unwrap();
        assert_eq!(part.data(), whole.sample(*tile).data(), "tile {tile}");
        let pair = tiled.render(&g, node, "pair", *tile).unwrap();
        assert_eq!(pair.data(), whole_pair.sample(*tile).data(), "tile {tile}");
    }
    // The square (20..30, 12..22) lies across tiles (16..32, 0..16) and
    // (16..32, 16..32): only those two run the module.
    let c = tiled.counters();
    assert_eq!(runs.load(Ordering::SeqCst) - ran, 2);
    assert_eq!(c.filled, tiles.len() as u64 - 2);
    assert_eq!(c.computed, tiles.len() as u64, "a fill is a computed step");
}

#[test]
fn a_filled_step_reads_none_of_its_inputs() {
    let (module, runs) = square(true);
    let mut g = Graph::new();
    let src = g.add_source(
        "image",
        Image::from_fn(Region::sized(64, 48), 1, |x, _, _| x as f32),
    );
    let node = g
        .add_module("square", ModuleRef::new(module), &Params::new())
        .unwrap();
    g.connect(src, graph::SOURCE_PORT, node, "input").unwrap();

    let engine = Engine::new(Arc::new(Bus::new()));
    let far = Region::new(40, 30, 8, 8);
    let out = engine.render(&g, node, "output", far).unwrap();
    assert!(out.data().iter().all(|v| *v == 0.25));
    let c = engine.counters();
    assert_eq!((c.filled, c.source_reads), (1, 0), "{c:?}");
    assert_eq!(runs.load(Ordering::SeqCst), 0);

    let near = Region::new(16, 8, 8, 8);
    let out = engine.render(&g, node, "output", near).unwrap();
    assert_eq!(out.pixel(20, 12)[0], 20.0, "the input added inside");
    assert_eq!(engine.counters().source_reads, 1);
}
