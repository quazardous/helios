//! What the engine says a user should know (a GPU lost, a step fallen back
//! to the CPU) reaches the user whatever the engine published around it:
//! a burst of step events between two interface passes, longer than the
//! engine bus's journal, does not push it out unread.

use egui::vec2;
use helios_core::bus::JOURNAL;
use helios_core::engine::{GpuLost, StepComputed};
use helios_core::module::Failure;
use helios_core::{Message, NodeId, Region};
use helios_gui::{Shell, headless, helios_panels};

#[test]
fn a_gpu_lost_survives_a_burst_longer_than_the_journal() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let engine = h.state().viewport().engine().clone();
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
    // No interface pass between the notice and a burst longer than the
    // journal.
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
    h.run();
    let told: Vec<_> = h
        .state()
        .notices()
        .iter()
        .filter(|n| n.key == "engine-gpu-lost")
        .collect();
    assert_eq!(told.len(), 1, "the notice is shown once: {told:?}");
}
