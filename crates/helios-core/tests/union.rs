//! Union planning: a node read by several consumers over different regions
//! is computed once over their union, each consumer reading a crop of it;
//! reads far apart stay apart; a later request inside cached content is
//! cropped from it.

use std::sync::{Arc, Mutex};

use helios_core::engine::unions;
use helios_core::*;

/// The inputs a reader got.
type Seen = Arc<Mutex<Vec<Image>>>;

/// Reads its input at `(x + shift, y)`, with `reach` pixels around it on
/// each side; remembers whether the input it got shares a buffer with the
/// previous one (to see crops).
struct Reader {
    name: &'static str,
    shift: i64,
    reach: u32,
    inputs: Seen,
}

impl Reader {
    fn new(name: &'static str, shift: i64, reach: u32) -> Self {
        Self {
            name,
            shift,
            reach,
            inputs: Arc::default(),
        }
    }
}

impl Module for Reader {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: self.name.into(),
            version: 1,
            description: String::new(),
            params: vec![],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 1)],
            context: vec![],
            needs: Needs {
                overlap: self.reach,
            },
            kind: ModuleKind::Process,
            family: Family::Other,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, _: &Context) -> Result<(), String> {
        Ok(())
    }

    fn input_region(&self, (): &(), output: Region, overlap: u32) -> Region {
        let r = output.grow(overlap);
        Region::new(r.x + self.shift, r.y, r.width, r.height)
    }

    fn process(&self, (): &(), inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        self.inputs.lock().unwrap().push(input.clone());
        let reach = i64::from(self.reach);
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            let x = x + self.shift;
            (-reach..=reach).map(|d| input.pixel(x + d, y)[0]).sum()
        })])
    }
}

/// Adds its two inputs.
struct Sum;

impl Module for Sum {
    type Plan = ();

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "sum".into(),
            version: 1,
            description: String::new(),
            params: vec![],
            inputs: vec![PortSpec::image("a", 1), PortSpec::image("b", 1)],
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
        Ok(vec![Image::from_fn(output, 1, |x, y, _| {
            inputs[0].pixel(x, y)[0] + inputs[1].pixel(x, y)[0]
        })])
    }
}

fn picture() -> Image {
    Image::from_fn(Region::sized(400, 30), 1, |x, y, _| {
        ((x * 7 + y * 13) % 31) as f32
    })
}

/// A source read by two readers, summed. Returns the graph, the source,
/// the sum and the readers' recorded inputs.
fn network(a: Reader, b: Reader) -> (Graph, NodeId, NodeId, [Seen; 2]) {
    let seen = [a.inputs.clone(), b.inputs.clone()];
    let mut g = Graph::new();
    let src = g.add_source("src", picture());
    let a = g
        .add_module("a", ModuleRef::new(a), &Params::new())
        .unwrap();
    let b = g
        .add_module("b", ModuleRef::new(b), &Params::new())
        .unwrap();
    let sum = g
        .add_module("sum", ModuleRef::new(Sum), &Params::new())
        .unwrap();
    g.connect(src, "image", a, "input").unwrap();
    g.connect(src, "image", b, "input").unwrap();
    g.connect(a, "output", sum, "a").unwrap();
    g.connect(b, "output", sum, "b").unwrap();
    (g, src, sum, seen)
}

fn engine() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

fn same_bits(a: &Image, b: &Image) -> bool {
    a.region() == b.region()
        && a.data()
            .iter()
            .zip(b.data())
            .all(|(x, y)| x.to_bits() == y.to_bits())
}

#[test]
fn overlapping_reads_merge_and_far_reads_stay_apart() {
    let full = Region::sized(10, 10);
    // A region and the same grown: one union, the larger.
    assert_eq!(unions(vec![full, full.grow(2), full]), [full.grow(2)]);
    // Side by side: the box costs no more than the two.
    assert_eq!(
        unions(vec![Region::new(0, 0, 5, 4), Region::new(5, 0, 5, 4)]),
        [Region::new(0, 0, 10, 4)]
    );
    // Far apart: the box would compute the gap.
    let far = vec![Region::new(0, 0, 4, 4), Region::new(100, 0, 4, 4)];
    assert_eq!(unions(far.clone()), far);
    // A third read bridging both makes one box again.
    let bridge = Region::new(0, 0, 104, 4);
    assert_eq!(unions(vec![far[0], far[1], bridge]), [bridge]);
}

#[test]
fn a_node_read_over_different_regions_is_computed_once_over_their_union() {
    let (g, src, sum, seen) = network(Reader::new("a", 0, 1), Reader::new("b", 0, 3));
    let e = engine();
    let r = Region::new(50, 5, 40, 10);
    let events = e.bus().subscribe();
    let out = e.render(&g, sum, "output", r).unwrap();
    let steps: Vec<(String, String)> = events
        .try_iter()
        .filter(|ev| ev.kind == "engine.step_computed")
        .map(|ev| {
            let region: Region = serde_json::from_value(ev.payload["region"].clone()).unwrap();
            (
                ev.payload["name"].as_str().unwrap().into(),
                region.to_string(),
            )
        })
        .collect();
    // One step per node; the source over the widest read.
    assert_eq!(steps.len(), 4, "{steps:?}");
    assert!(
        steps.contains(&("src".into(), r.grow(3).to_string())),
        "{steps:?}"
    );
    let c = e.counters();
    assert_eq!((c.computed, c.source_reads, c.keys), (4, 1, 4));
    // Each reader got its own region, a view into the one source buffer.
    let (a, b) = (&seen[0].lock().unwrap()[0], &seen[1].lock().unwrap()[0]);
    assert_eq!((a.region(), b.region()), (r.grow(1), r.grow(3)));
    assert!(a.same_buffer(b), "crops of one buffer, not copies");
    // The same pixels as reading each region apart.
    let expected = Image::from_fn(r, 1, |x, y, _| {
        let p = picture();
        let at = |x: i64| p.sample(Region::new(x, y, 1, 1)).data()[0];
        (-1..=1).map(|d| at(x + d)).sum::<f32>() + (-3..=3).map(|d| at(x + d)).sum::<f32>()
    });
    assert!(same_bits(&out, &expected));
    let _ = src;
}

#[test]
fn reads_far_apart_are_computed_apart() {
    let (g, _, sum, seen) = network(Reader::new("a", -120, 0), Reader::new("b", 120, 0));
    let e = engine();
    let r = Region::new(150, 0, 20, 10);
    let out = e.render(&g, sum, "output", r).unwrap();
    let c = e.counters();
    assert_eq!(c.source_reads, 2, "two unions, not the 260-pixel box");
    assert_eq!(c.computed, 5);
    let (a, b) = (&seen[0].lock().unwrap()[0], &seen[1].lock().unwrap()[0]);
    assert_eq!(a.region(), Region::new(30, 0, 20, 10));
    assert_eq!(b.region(), Region::new(270, 0, 20, 10));
    let p = picture();
    assert_eq!(out.pixel(160, 3)[0], p.pixel(40, 3)[0] + p.pixel(280, 3)[0]);
}

#[test]
fn tiles_give_the_full_render_bit_for_bit() {
    let (g, _, sum, _) = network(Reader::new("a", -2, 1), Reader::new("b", 5, 3));
    let full = Region::sized(400, 30);
    let whole = engine().render(&g, sum, "output", full).unwrap();
    let tiled = engine();
    for tile in full.tiles(16) {
        let part = tiled.render(&g, sum, "output", tile).unwrap();
        assert!(same_bits(&part, &whole.sample(tile)), "{tile}");
    }
}

#[test]
fn a_request_inside_cached_content_is_cropped_from_it() {
    let (g, src, sum, _) = network(Reader::new("a", 0, 1), Reader::new("b", 0, 3));
    let e = engine();
    let wide = Region::sized(200, 30);
    let whole = e.render(&g, sum, "output", wide).unwrap();
    let before = e.counters();
    let part = Region::new(20, 4, 30, 8);
    let inside = e.render(&g, sum, "output", part).unwrap();
    let d = e.counters().since(&before);
    assert_eq!(d.computed, 0, "nothing computed");
    assert_eq!(d.crops, 1, "the sum's step cropped from the wider one");
    assert!(inside.same_buffer(&whole), "a view, not a copy");
    assert!(same_bits(&inside, &whole.sample(part)));
    // The status says the cropped content is there.
    let status = e.status(&g, sum, part).unwrap();
    assert!(
        status.iter().all(|s| s.state == NodeState::Done),
        "{status:?}"
    );
    // A source request inside what the readers read is cropped too.
    let before = e.counters();
    e.render(&g, src, "image", Region::new(0, 0, 10, 10))
        .unwrap();
    assert_eq!(e.counters().since(&before).source_reads, 0);
}
