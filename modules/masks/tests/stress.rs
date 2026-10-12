//! The stress network: three masked modules over 54 mask nodes, chained
//! through their images and their masks. Tiles give the full render bit
//! for bit; an edit recomputes only the branch it dirties.

use std::sync::Arc;

use helios_core::{Bus, Describe, Engine, Image, ModuleRef, Params, Region, Value};
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

/// The network renders as it did when each distinct region asked of a
/// node was a step of its own: union planning changes what is computed,
/// not one bit of the result. Hashes of that engine's renders (x86-64
/// Linux: the picture uses `sin`, whose last bit may differ elsewhere).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn renders_are_bit_identical_to_planning_per_region() {
    let full = Region::sized(40, 28);
    let n = network(full);
    let e = engine();
    let out = e.render(&n.graph, n.output(), "output", full).unwrap();
    assert_eq!(
        out.content_hash().to_string(),
        "be5fc8fae33abbd5fa7911851bc609f4b789fafb144389d5f762df7c700aa447"
    );
    let masks = [
        "edf263826520723bde9fc239e6c7b61cda222f15b9968078dd8b0252691cf468",
        "15073ed52245dbfecdc5dc3fadd87c6443b4993d29a70cec37fcd2c7faac7e70",
        "b7d994b2f42c6df3944f9f37f34581178e30665f22f3db72c0620f042603f639",
    ];
    for (node, hash) in n.masks.iter().zip(masks) {
        let mask = e.render(&n.graph, *node, "mask", full).unwrap();
        assert_eq!(mask.content_hash().to_string(), hash);
    }
    let proxy = engine()
        .render_at(&n.graph, n.output(), "output", Region::sized(10, 7), 0.25)
        .unwrap();
    assert_eq!(
        proxy.content_hash().to_string(),
        "044d19e2fae662ca115e847a9ca67ab9dd905a6ecfae5ce8e468630742537151"
    );
}

#[test]
fn a_small_cache_budget_gives_the_same_result_and_computes_nothing_twice() {
    let full = Region::sized(40, 28);
    let n = network(full);
    let free = engine()
        .render(&n.graph, n.output(), "output", full)
        .unwrap();
    // Room for a handful of one-channel masks of this size.
    let budget = 8 * full.area() * 4;
    let e = Engine::with_budget(Arc::new(Bus::new()), Some(budget));
    let tight = e.render(&n.graph, n.output(), "output", full).unwrap();
    assert!(same_bits(&tight, &free));
    let c = e.counters();
    assert_eq!(
        c.computed,
        n.graph.ids().count() as u64,
        "what the render still needs is never evicted"
    );
    assert!(c.evictions > 0);
    let d = e.describe();
    assert!(
        d["cache"]["bytes"].as_u64().unwrap() <= budget as u64,
        "{d}"
    );
    // Tiles under the same budget still give the full render.
    for tile in full.tiles(16) {
        let part = e.render(&n.graph, n.output(), "output", tile).unwrap();
        assert!(same_bits(&part, &free.sample(tile)), "tile {tile}");
    }
}

#[test]
fn a_full_render_computes_each_node_once() {
    let r = Region::sized(48, 32);
    let n = network(r);
    let e = engine();
    let events = e.bus().subscribe();
    e.render(&n.graph, n.output(), "output", r).unwrap();
    let mut names: Vec<String> = events
        .try_iter()
        .filter(|ev| ev.kind == "engine.step_computed")
        .map(|ev| ev.payload["name"].as_str().unwrap().to_string())
        .collect();
    let steps = names.len();
    names.sort();
    names.dedup();
    assert_eq!(steps, names.len(), "one step per node");
    // Every node: the mask nodes, the processed modules and the source.
    assert_eq!(steps, n.graph.ids().count());
    let c = e.counters();
    assert_eq!(
        (c.computed, c.keys, c.source_reads),
        (steps as u64, steps as u64, 1)
    );
}
