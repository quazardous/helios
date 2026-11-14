//! The boot and end splashes' snapshots (`tests/snapshots/`, thresholds
//! in the workspace's `kittest.toml`): the boot splash with a step done,
//! one running and one waiting its turn; the end splash with a step that
//! lasts, its reason and "Quit now". egui's own dark style: the splash
//! draws with whatever style the window has. A fixed version: the build's
//! changes with each commit. Regenerate with
//! `UPDATE_SNAPSHOTS=1 cargo test -p helios-gui --test splash_snapshots`
//! (the box's `gui-snapshots` job).

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use helios_core::Message;
use helios_gui::splash::{self, Kind};
use helios_node_core::phases::Phase;

fn harness(kind: Kind, phase: Phase, stuck_after: Duration) -> Harness<'static> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(800.0, 500.0))
        .build_ui(move |ui| {
            splash::show_as(ui, kind, &phase, Instant::now(), stuck_after, "1.0.0");
        });
    h.run_steps(3);
    h
}

#[test]
fn the_boot_splash() {
    let phase = Phase::new();
    phase.begin(Message::plain("Opening the library")).done();
    let _running = phase.begin(Message::plain("Building the interface"));
    let _waiting = phase.declare(Message::plain("Reading the library"));
    let mut h = harness(Kind::Boot, phase, splash::STUCK);
    h.snapshot("splash_boot");
}

#[test]
fn the_end_splash_with_a_step_that_lasts() {
    let phase = Phase::new();
    phase.begin(Message::plain("Saving your changes")).done();
    let jobs = phase
        .begin(Message::plain("Stopping background jobs"))
        .can_skip();
    jobs.why(Message::plain("Waiting for 2 running jobs to finish"));
    let _waiting = phase.declare(Message::plain("Saving and closing the library"));
    let mut h = harness(Kind::End, phase, Duration::ZERO);
    h.snapshot("splash_end_stuck");
    drop(jobs);
}
