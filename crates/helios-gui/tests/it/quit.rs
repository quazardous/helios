//! The question before quitting on egui-pop's bus (ADR 0029, slice 8):
//! the main window's `window.close` step carries Helios's before-hook,
//! which vetoes the close that would end Helios while jobs run and asks
//! "Quit Helios?"; its Quit asks the close again, which goes. Every way
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
