//! bus-kernel's conformance suite on the bus Helios builds: egui-pop's
//! hook kernel with Helios's fact bus (`BusJournal`) as its journal and
//! relay, as the node's and the desktop's `main` build it.

use std::sync::Arc;

use bus_contract::{Bus as _, Capability, offers};
use bus_kernel::conformance::{self, Outcome};
use egui_pop_hooks::Kernel;
use helios_core::{Bus, BusJournal};

/// A fresh application bus as the node builds it: a fact bus with the
/// core's fact tables declared, attached as the kernel's journal and
/// relay, the typed kinds declared as the kernel's journaled points.
fn node_bus() -> Arc<dyn bus_contract::Bus> {
    let facts = Arc::new(Bus::new());
    facts.declare(helios_core::bus::FACTS.iter().copied());
    let (kernel, journal) = BusJournal::attach(
        facts,
        |j| Kernel::new().with_journal(j.clone()).with_relay(j),
        Kernel::declaration,
    );
    let refused = journal.declare(&*kernel, helios_core::bus::FACTS.iter().copied());
    assert!(refused.is_empty(), "{refused:?}");
    kernel
}

#[test]
fn the_node_bus_offers_a_journal_and_a_relay() {
    let bus = node_bus();
    assert!(offers(&*bus, Capability::Journal));
    assert!(offers(&*bus, Capability::Relay));
    assert!(bus.journal().is_some() && bus.relay().is_some());
}

#[test]
fn the_node_bus_keeps_the_bus_contract() {
    let report = conformance::report(node_bus);
    for (name, outcome) in &report {
        println!("{name}: {outcome:?}");
    }
    // The journal and relay groups run: the bus offers both.
    for (name, outcome) in &report {
        if name.starts_with("journal.") || name.starts_with("relay.") {
            assert!(
                !matches!(outcome, Outcome::Skipped(_)),
                "{name} was skipped"
            );
        }
    }
    let failed: Vec<String> = report
        .iter()
        .filter_map(|(name, outcome)| match outcome {
            Outcome::Failed(why) => Some(format!("- {name}: {why}")),
            _ => None,
        })
        .collect();
    assert!(
        failed.is_empty(),
        "{} conformance check(s) failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
