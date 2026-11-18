//! What the engine says a user should know (a GPU lost, a step fallen back
//! to the CPU) reaches the user whatever the engine published around it:
//! a burst of step events between two interface passes, longer than the
//! engine bus's journal, does not push it out unread.

use std::time::Duration;

use egui::vec2;
use egui_pop::api::bus::bus_contract::Journal;
use egui_pop::api::bus::egui_pop_hooks::Kernel;
use helios_core::bus::JOURNAL;
use helios_core::engine::{GpuLost, StepComputed};
use helios_core::module::Failure;
use helios_core::{BusJournal, Event, Message, NodeId, Region};
use helios_gui::{Shell, headless, helios_panels};

/// Publishes a GPU lost, then a burst of step events longer than the
/// engine bus's journal, with no interface pass between them.
fn lost_then_burst(engine: &helios_core::Engine) {
    engine.bus().publish(
        "engine",
        None,
        &GpuLost {
            failure: Failure::DeviceLost,
            reason: "driver reset".into(),
            recovered: false,
            message: Message::new("engine-gpu-lost", "The GPU failed."),
        },
    );
    for n in 0..JOURNAL + 100 {
        engine.bus().publish(
            "engine",
            None,
            &StepComputed {
                node: NodeId(1),
                name: format!("s{n}"),
                key: String::new(),
                region: Region::new(0, 0, 1, 1),
                ok: true,
            },
        );
    }
}

#[test]
fn a_gpu_lost_survives_a_burst_longer_than_the_journal() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    lost_then_burst(&h.state().viewport().engine().clone());
    h.run();
    let told: Vec<_> = h
        .state()
        .notices()
        .iter()
        .filter(|n| n.key == "engine-gpu-lost")
        .collect();
    assert_eq!(told.len(), 1, "the notice is shown once: {told:?}");
}

/// On the desktop's bus (journaled, as `main` builds it), the notice is
/// relayed into the journal and heard by the shell's named listener
/// once; the engine's per-render kinds stay off the journal.
#[test]
fn a_gpu_lost_reaches_its_named_listener_once_through_the_journal() {
    let facts = helios_gui::bus();
    let (kernel, journal) = BusJournal::attach(
        facts.clone(),
        |j| Kernel::new().with_journal(j.clone()).with_relay(j),
        Kernel::declaration,
    );
    assert!(
        journal
            .declare(&*kernel, helios_gui::FACTS.iter().copied())
            .is_empty()
    );
    let shell = Shell::builder(helios_panels(), None)
        .bus(kernel.clone())
        .build();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    let listeners: Vec<String> = kernel
        .listeners(GpuLost::KIND)
        .unwrap_or_default()
        .into_iter()
        .map(|l| l.id)
        .collect();
    assert_eq!(listeners, [helios_gui::shell::ENGINE_NOTICES]);
    let before = facts.last_seq();
    lost_then_burst(&h.state().viewport().engine().clone());
    assert!(Journal::flush(&*journal, Duration::from_secs(10)));
    let on_journal: Vec<&str> = facts
        .since(before)
        .iter()
        .map(|e| e.kind)
        .filter(|k| k.starts_with("engine."))
        .collect();
    assert_eq!(on_journal, [GpuLost::KIND], "only the notice is relayed");
    h.run();
    let told = h
        .state()
        .notices()
        .iter()
        .filter(|n| n.key == "engine-gpu-lost")
        .count();
    assert_eq!(told, 1, "the notice is shown once");
}
