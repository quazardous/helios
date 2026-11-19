//! bus-kernel's conformance suite on the desktop's application bus, as
//! `main` and the shell build it: egui-pop's hook kernel with a desktop
//! fact bus (`helios_gui::bus`, every table of `helios_gui::FACTS`
//! declared) as its journal and relay (`BusJournal`), the typed kinds its
//! journaled points, then a shell built on it, which declares egui-pop's
//! points and Helios's, attaches the node events to the relay and adds
//! its local and deferred listeners (`Shell::assemble`).

use std::cell::RefCell;
use std::sync::Arc;

use bus_kernel::conformance::{self, Outcome};
use egui_pop::api::bus::bus_contract::{self, Capability, offers};
use egui_pop::api::bus::egui_pop_hooks::Kernel;
use helios_core::BusJournal;
use helios_gui::{Shell, helios_panels};

thread_local! {
    /// The shells built on the buses the suite asked for, kept alive
    /// for the test's length: their listeners stay as the desktop has
    /// them while the checks run.
    static SHELLS: RefCell<Vec<Shell>> = const { RefCell::new(Vec::new()) };
}

/// A fresh application bus as the desktop builds it (`journaled_bus` in
/// `main`), and the fact bus that is its journal: the one the desktop's
/// own node publishes on.
pub fn journaled() -> (Arc<dyn bus_contract::Bus>, Arc<helios_core::Bus>) {
    let facts = helios_gui::bus();
    let (kernel, journal) = BusJournal::attach(
        facts.clone(),
        |j| Kernel::new().with_journal(j.clone()).with_relay(j),
        Kernel::declaration,
    );
    let refused = journal.declare(&*kernel, helios_gui::FACTS.iter().copied());
    assert!(refused.is_empty(), "{refused:?}");
    (kernel, facts)
}

/// A fresh application bus as the desktop builds it, with a shell built
/// on it.
fn desktop_bus() -> Arc<dyn bus_contract::Bus> {
    let (kernel, _) = journaled();
    let shell = Shell::builder(helios_panels(), None)
        .bus(kernel.clone())
        .build();
    SHELLS.with(|s| s.borrow_mut().push(shell));
    kernel
}

#[test]
fn the_desktop_bus_keeps_the_bus_contract() {
    let bus = desktop_bus();
    assert!(offers(&*bus, Capability::Journal));
    assert!(offers(&*bus, Capability::Relay));
    drop(bus);

    let report = conformance::report(desktop_bus);
    for (name, outcome) in &report {
        println!("{name}: {outcome:?}");
    }
    // The journal and relay groups run: the bus offers both.
    let skipped: Vec<&str> = report
        .iter()
        .filter(|(name, outcome)| {
            (name.starts_with("journal.") || name.starts_with("relay."))
                && matches!(outcome, Outcome::Skipped(_))
        })
        .map(|(name, _)| *name)
        .collect();
    assert!(skipped.is_empty(), "skipped: {skipped:?}");
    let failed: Vec<String> = report
        .iter()
        .filter_map(|(name, outcome)| match outcome {
            Outcome::Failed(why) => Some(format!("- {name}: {why}")),
            _ => None,
        })
        .collect();
    SHELLS.with(|s| s.borrow_mut().clear());
    assert!(
        failed.is_empty(),
        "{} conformance check(s) failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
