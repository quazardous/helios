//! The question before quitting on egui-pop's bus (ADR 0029, slice 8):
//! the main window's `window.close` step carries Helios's before-hook,
//! which asks the quit's request (`helios.app.quit`): its listeners run
//! in order, and one refusing vetoes the close; while jobs run, the
//! running jobs' listener refuses and "Quit Helios?" is asked; its Quit
//! asks the close again, which goes. Every way
//! the main window closes takes this path: Helios's Quit, egui-pop's
//! `quit`, a command closing the main window, and the system's close
//! (the compositor's, as the desktop's host meets it).

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::api::windows::WindowId;
use helios_gui::shell::{Closing, LIBRARY_WINDOW, LIBRARY_WINDOW_COMMAND, LastWindow};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::Value;

const QUESTION: &str = "jobs are running";

fn harness() -> Harness<'static, Shell> {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    h.state_mut().set_last_window(LastWindow::Quit);
    h.run();
    h
}

/// A job running in this process, cancellable: a quit would cancel it.
fn running_job(h: &Harness<'static, Shell>) -> helios_core::jobs::Report {
    let report = h.state().jobs().start(helios_core::jobs::Start::new(
        "test-job",
        "A test job",
        "test",
    ));
    report.on_cancel(|| {});
    report
}

fn quit(h: &mut Harness<'static, Shell>) {
    h.state_mut()
        .run("workbench.quit", Value::Null, Origin::User)
        .unwrap();
    h.run_steps(2);
}

fn asked(h: &Harness<'static, Shell>) -> bool {
    h.query_by_label_contains(QUESTION).is_some()
}

fn windows(h: &Harness<'static, Shell>) -> Vec<String> {
    h.state()
        .app()
        .handle()
        .windows()
        .into_iter()
        .map(|w| w.id.to_string())
        .collect()
}

/// Who vetoed the main window's `window.close` steps so far.
fn vetoes(h: &Harness<'static, Shell>) -> Vec<String> {
    h.state()
        .app()
        .handle()
        .events_since(0)
        .into_iter()
        .filter(|e| e.name == "window.close.vetoed")
        .map(|e| e.data["by"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// Quit with a job running asks first; Cancel keeps Helios and its
/// windows as they were; asked again, its Quit ends Helios, every window
/// with it.
#[test]
fn quitting_with_a_job_running_asks_and_cancel_keeps_everything() {
    let mut h = harness();
    h.state_mut()
        .run(LIBRARY_WINDOW_COMMAND, Value::Null, Origin::User)
        .unwrap();
    h.run_steps(2);
    assert_eq!(windows(&h), ["main", LIBRARY_WINDOW]);
    let _job = running_job(&h);

    quit(&mut h);
    assert!(asked(&h), "the question about the running job");
    assert!(!h.state().quitting(), "asked first");
    assert_eq!(vetoes(&h), ["helios.quit"], "the step's before-hook asked");
    assert_eq!(windows(&h), ["main", LIBRARY_WINDOW], "nothing closed");
    assert_eq!(h.state().library_window_state()["open"], true);

    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(!asked(&h), "the question answered");
    assert!(!h.state().quitting(), "Cancel keeps Helios");
    assert_eq!(windows(&h), ["main", LIBRARY_WINDOW], "and its windows");
    assert_eq!(h.state().library_window_state()["open"], true);

    quit(&mut h);
    assert!(asked(&h), "asked again");
    // Its Quit button's command.
    h.state_mut()
        .run("helios.quit.confirmed", Value::Null, Origin::User)
        .unwrap();
    h.run_steps(2);
    assert!(h.state().quitting(), "Quit ends Helios");
    assert_eq!(
        h.state().library_window_state()["open"],
        false,
        "every window closed with the main one"
    );
    let ctx = h.ctx.clone();
    assert_eq!(
        h.state_mut().main_window_closing(&ctx),
        Closing::Quit,
        "the host shows the end splash and closes"
    );
}

/// Nothing running: Quit quits at once, nothing asked.
#[test]
fn quitting_with_no_job_quits_at_once() {
    let mut h = harness();
    quit(&mut h);
    assert!(!asked(&h));
    assert!(h.state().quitting());
    assert!(vetoes(&h).is_empty());
}

/// The system's close of the last window (the compositor's, Alt+F4), as
/// the desktop's host meets it: the same question, from the step; the
/// system's close cancelled until the answer. Nothing running, it quits.
#[test]
fn the_systems_close_takes_the_same_path() {
    let mut h = harness();
    let job = running_job(&h);
    let ctx = h.ctx.clone();
    assert_eq!(h.state_mut().main_window_closing(&ctx), Closing::Kept);
    h.run_steps(2);
    assert!(asked(&h), "the question about the running job");
    assert!(!h.state().quitting());
    assert_eq!(vetoes(&h), ["helios.quit"]);
    assert_eq!(windows(&h), ["main"], "the main window stays");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(!h.state().quitting());

    // The job ended: the next close quits at once.
    job.finished();
    assert_eq!(h.state_mut().main_window_closing(&ctx), Closing::Quit);
    assert!(h.state().quitting());
}

/// egui-pop's own Quit (`app.quit()`) and a command closing the main
/// window take the same step: the question, the close vetoed.
#[test]
fn egui_pops_quit_and_a_command_close_take_the_same_path() {
    let mut h = harness();
    let _job = running_job(&h);
    let app = h.state().app().handle().clone();
    app.quit();
    h.run_steps(2);
    assert!(asked(&h), "app.quit() asks");
    assert!(!h.state().quitting());
    assert!(!app.quitting(), "the close vetoed: not quitting");
    h.get_by_label("Cancel").click();
    h.run_steps(2);

    app.close_window(&WindowId::main());
    h.run_steps(2);
    assert!(asked(&h), "closing the main window by a command asks");
    assert!(!h.state().quitting());
    assert_eq!(vetoes(&h), ["helios.quit", "helios.quit"]);
}

/// `helios.app.quit`'s listeners on the desktop's bus, in their order
/// (`tests/bus/quit.txt`).
#[test]
fn the_quit_requests_listeners_run_in_a_declared_order() {
    let h = harness();
    assert_eq!(
        helios_gui::app_quit::order(&*h.state().app().handle().bus()),
        include_str!("../bus/quit.txt")
    );
}

/// A quit is one request (`helios.app.quit`): its listeners run in their
/// declared order, each saying what it checked; one refusing (an edit
/// not saved) vetoes the main window's close and keeps everything, its
/// name in the veto; once it lets the quit go, Helios quits.
#[test]
fn quitting_runs_the_listeners_in_order_and_a_veto_stops_it() {
    use egui_pop::api::bus::bus_contract::{HookError, ListenOpts};
    use helios_gui::app_quit::{AppQuit, JOBS};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let mut h = harness();
    let bus = h.state().app().handle().bus();
    let unsaved = Arc::new(AtomicBool::new(true));
    let edit = unsaved.clone();
    bus.listen::<AppQuit>(
        "helios.test.edit",
        ListenOpts::new().before(JOBS).owner("helios.test"),
        move |_, said| {
            if edit.load(Ordering::SeqCst) {
                return Err(HookError::refuse("an edit is not saved"));
            }
            said.push("helios.test.edit: saved".into());
            Ok(())
        },
    )
    .unwrap();
    bus.listen::<AppQuit>(
        "helios.test.after",
        ListenOpts::new().after(JOBS).owner("helios.test"),
        |_, said| {
            said.push("helios.test.after".into());
            Ok(())
        },
    )
    .unwrap();

    quit(&mut h);
    assert!(!h.state().quitting(), "the unsaved edit keeps Helios open");
    assert_eq!(vetoes(&h), ["helios.quit"], "the close vetoed");
    assert_eq!(windows(&h), ["main"], "nothing closed");
    let report = h.state().quit_report().expect("the quit was asked");
    assert_eq!(
        report.refused,
        Some(("helios.test.edit".into(), "an edit is not saved".into()))
    );
    assert!(report.said.is_empty(), "nothing after the refusal ran");

    unsaved.store(false, Ordering::SeqCst);
    quit(&mut h);
    assert!(h.state().quitting(), "nothing holds it: Helios quits");
    let report = h.state().quit_report().unwrap();
    assert!(report.goes());
    assert_eq!(
        report.said,
        [
            "helios.test.edit: saved",
            "helios.quit.jobs: no job running",
            "helios.test.after",
        ]
    );
    assert!(report.failures.is_empty());
}

/// A listener that never answers (threaded, its deadline 100 ms) does
/// not hold the quit: it is reported as late and Helios quits.
#[test]
fn a_listener_past_its_deadline_is_reported_and_does_not_hang_the_quit() {
    use egui_pop::api::bus::bus_contract::ListenOpts;
    use helios_gui::app_quit::AppQuit;
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    let mut h = harness();
    let bus = h.state().app().handle().bus();
    // Held until the test lets it go, after the quit.
    let (release, held) = mpsc::channel::<()>();
    let held = Arc::new(Mutex::new(held));
    bus.listen::<AppQuit>(
        "helios.test.stuck",
        ListenOpts::new()
            .threaded()
            .deadline(Duration::from_millis(100))
            .owner("helios.test"),
        move |_, _| {
            let _ = held.lock().unwrap().recv();
            Ok(())
        },
    )
    .unwrap();

    let started = Instant::now();
    quit(&mut h);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    assert!(h.state().quitting(), "the late listener does not hold it");
    let report = h.state().quit_report().unwrap();
    assert!(report.goes());
    assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
    assert!(
        report.failures[0].contains("helios.test.stuck"),
        "{:?}",
        report.failures
    );
    release.send(()).unwrap();
}

/// No sleep is left on the quit and close path: each wait there is woken
/// by what it waits for, with a deadline (a channel, a condition), never
/// a clock polled.
#[test]
fn no_sleep_is_left_on_the_quit_and_close_path() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let files = [
        "crates/helios-gui/src/app_quit.rs",
        "crates/helios-gui/src/ending.rs",
        "crates/helios-gui/src/quit.rs",
        "crates/helios-gui/src/shell/lifetime.rs",
        "crates/helios-node-core/src/node.rs",
        "crates/helios-node-core/src/job_promises.rs",
    ];
    let mut found = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(root.join(file)).unwrap();
        // The code, its test module left out.
        let code = text.split("#[cfg(test)]").next().unwrap_or_default();
        for (n, line) in code.lines().enumerate() {
            let line = line.trim_start();
            if !line.starts_with("//") && line.contains("thread::sleep(") {
                found.push(format!("{file}:{}: {line}", n + 1));
            }
        }
    }
    assert!(
        found.is_empty(),
        "sleeps on the quit path:\n{}",
        found.join("\n")
    );
}
