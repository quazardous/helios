//! The stress network: three masked modules over 54 mask nodes, chained
//! through their images and their masks. Tiles give the full render bit
//! for bit; an edit recomputes only the branch it dirties.

use std::sync::Arc;

use helios_core::{Bus, Engine, Image, ModuleRef, Params, Region, Value};
use helios_module_exposure::Exposure;
use helios_module_masks::stress::{self, MASK_NODES_PER_MODULE, Stress};

fn engine() -> Engine {
    Engine::new(Arc::new(Bus::new()))
}

fn network(region: Region) -> Stress {
    stress::build(stress::picture(region), 3, |k| {
        (
            ModuleRef::new(Exposure),
            Params::new().with("exposure", Value::Float(0.5 - k as f64 * 0.4)),
        )
    })
    .unwrap()
}

fn same_bits(a: &Image, b: &Image) -> bool {
    a.region() == b.region()
        && a.data()
            .iter()
            .zip(b.data())
            .all(|(x, y)| x.to_bits() == y.to_bits())
}

#[test]
fn the_network_has_over_fifty_mask_nodes() {
    let n = network(Region::sized(8, 6));
    assert_eq!(n.mask_nodes, 3 * MASK_NODES_PER_MODULE);
    assert!(n.mask_nodes > 50);
    // Every mask node, every processed module and the source.
    assert_eq!(n.graph.ids().count(), n.mask_nodes + 3 + 1);
}

#[test]
fn a_tiled_render_equals_the_full_render_bit_for_bit() {
    let full = Region::sized(40, 28);
    let n = network(full);
    let whole = engine()
        .render(&n.graph, n.output(), "output", full)
        .unwrap();
    let masks: Vec<Image> = n
        .masks
        .iter()
        .map(|m| engine().render(&n.graph, *m, "mask", full).unwrap())
        .collect();
    // The masks are not trivial: each takes values across its range.
    for mask in &masks {
        let (lo, hi) = mask
            .data()
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        assert!(hi - lo > 0.2, "mask from {lo} to {hi}");
    }
    let tiled = engine();
    for tile in full.tiles(16) {
        let part = tiled.render(&n.graph, n.output(), "output", tile).unwrap();
        assert!(same_bits(&part, &whole.sample(tile)), "output tile {tile}");
        for (node, mask) in n.masks.iter().zip(&masks) {
            let part = tiled.render(&n.graph, *node, "mask", tile).unwrap();
            assert!(same_bits(&part, &mask.sample(tile)), "mask tile {tile}");
        }
    }
}

#[test]
fn a_one_value_edit_recomputes_only_the_dirty_branch() {
    let r = Region::sized(32, 24);
    let mut n = network(r);
    let e = engine();
    let first = e.render(&n.graph, n.output(), "output", r).unwrap();
    let rendered = e.counters();
    // Planning computes one key per distinct step, each computed once: the
    // nodes every module shares (its input) are not planned per path.
    assert_eq!(rendered.keys, rendered.computed);
    // Rendering again plans again but computes nothing.
    e.render(&n.graph, n.output(), "output", r).unwrap();
    let again = e.counters().since(&rendered);
    assert_eq!(again.computed, 0);
    assert_eq!(
        again.keys, rendered.keys,
        "planning does not depend on the cache"
    );

    // One value of the last module's chroma member.
    let chroma = n.graph.find("m2.chroma").unwrap();
    n.graph
        .set_param(chroma, "chroma", Value::List(vec![0.0, 0.3, 0.0, 0.1]))
        .unwrap();
    let events = e.bus().subscribe();
    let before = e.counters();
    let edited = e.render(&n.graph, n.output(), "output", r).unwrap();
    let delta = e.counters().since(&before);
    let mut computed: Vec<String> = events
        .try_iter()
        .filter(|ev| ev.kind == "engine.step_computed")
        .map(|ev| ev.payload["name"].as_str().unwrap().to_string())
        .collect();
    computed.sort();
    // The member, then every node between it and the output: its group,
    // the group's refinement, the root, the root's levels, the blend.
    assert_eq!(
        computed,
        [
            "m2.a",
            "m2.a.blur",
            "m2.a.levels",
            "m2.blend",
            "m2.chroma",
            "m2.root",
            "m2.root.levels",
        ]
    );
    assert_eq!(delta.computed, computed.len() as u64);
    assert_eq!(delta.source_reads, 0);
    // Planning cost does not move with an edit.
    assert_eq!(delta.keys, rendered.keys);
    assert!(!same_bits(&first, &edited), "the edit shows");
}
