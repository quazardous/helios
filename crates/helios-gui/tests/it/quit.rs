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

/// The end splash's steps on a shell `shell`: the test listeners
/// `first` (between the jobs and the node) and `second` (after the node),
/// each noting the frame its turn ran at.
fn end_listeners(
    shell: &Shell,
    frame: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> std::sync::Arc<std::sync::Mutex<Vec<(String, usize)>>> {
    use egui_pop::api::bus::bus_contract::ListenOpts;
    use helios_gui::app_quit::{AppQuit, CLIENT, JOBS, NODE};
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};

    let bus = shell.app().handle().bus();
    let heard = Arc::new(Mutex::new(Vec::new()));
    for (id, opts) in [
        (
            "helios.test.first",
            ListenOpts::new().after(JOBS).before(NODE),
        ),
        (
            "helios.test.second",
            ListenOpts::new().after(NODE).before(CLIENT),
        ),
    ] {
        let (frame, heard) = (frame.clone(), heard.clone());
        bus.listen::<AppQuit>(id, opts.owner("helios.test"), move |ask, said| {
            if ask.stage.turn_of(id) {
                heard
                    .lock()
                    .unwrap()
                    .push((id.to_owned(), frame.load(Ordering::SeqCst)));
                said.push(id.to_owned());
            }
            Ok(())
        })
        .unwrap();
    }
    heard
}

/// The end splash asks the quit's request again, one listener's turn per
/// frame (through `Defer`), in the declared order; the host's steps are
/// listed with them, before the GUI client detaches, marked as the
/// host's; the splash's rows are those steps, in that order, and the
/// end's report has every step finished.
#[test]
fn the_end_runs_one_listener_per_frame_in_order_with_matching_rows() {
    use helios_core::Message;
    use helios_gui::app_quit::{self, CLIENT, JOBS, NODE};
    use helios_gui::splash::{End, Ending};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let shell = Shell::new(helios_panels(), None);
    let frame = Arc::new(AtomicUsize::new(0));
    let heard = end_listeners(&shell, &frame);
    let host = "helios.test.host";
    let steps = shell.end_steps(&[host]);
    let ids: Vec<String> = steps.iter().map(|s| s.id.clone()).collect();
    assert_eq!(
        ids,
        [
            JOBS,
            "helios.test.first",
            NODE,
            "helios.test.second",
            host,
            CLIENT
        ]
    );
    assert_eq!(
        steps
            .iter()
            .filter(|s| s.host)
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        [host],
        "the host's step marked"
    );
    let mut end = End::new(helios_node_core::phases::Phase::new());
    for (turn, step) in steps.iter().enumerate() {
        let id = step.id.clone();
        end = if step.host {
            end.then(
                Message::plain(format!("{id} (window)")),
                move |s: &mut Shell| {
                    s.quit_host_step_done(&id);
                },
            )
        } else {
            end.then(app_quit::label(&id), move |s: &mut Shell| {
                s.quit_turn(turn, &id);
            })
        };
    }
    let counted = frame.clone();
    let mut h = egui_kittest::Harness::builder()
        .with_size(vec2(640.0, 600.0))
        .build_ui_state(
            move |ui, (end, shell, outcome): &mut (End<Shell>, Shell, Option<Ending>)| {
                counted.fetch_add(1, Ordering::SeqCst);
                *outcome = Some(end.pass(ui, shell));
            },
            (end, shell, None),
        );
    h.run_steps(1);
    // Every step a row, in order.
    let rows = [
        "Checking the running jobs".to_owned(),
        "helios.test.first".to_owned(),
        "Asking the library's node to stop".to_owned(),
        "helios.test.second".to_owned(),
        format!("{host} (window)"),
        "Disconnecting from the library's node".to_owned(),
    ];
    let mut tops = Vec::new();
    for row in &rows {
        tops.push(h.get_by_label(row).rect().min.y);
    }
    assert!(tops.is_sorted(), "the rows in the steps' order: {tops:?}");
    for _ in 0..steps.len() {
        h.run_steps(1);
    }
    h.run_steps(1);
    assert_eq!(h.state().2, Some(Ending::Done));
    let heard = heard.lock().unwrap().clone();
    assert_eq!(heard.len(), 2, "each test listener's turn once: {heard:?}");
    assert_eq!(heard[0].0, "helios.test.first");
    assert_eq!(heard[1].0, "helios.test.second");
    // Turns 1 and 3, the node's between them: one frame each.
    assert_eq!(
        heard[1].1 - heard[0].1,
        2,
        "one listener per frame: {heard:?}"
    );
    let names: Vec<&str> = ids.iter().map(String::as_str).collect();
    let report = h.state().1.end_report(&names);
    assert_eq!(report.finished, ids, "every step, in order");
    assert!(report.clean() && !report.may_have_lost_work, "{report:?}");
}

/// A listener past its deadline at its end turn (threaded, 100 ms, never
/// answering) is cut and reported, the work it may have lost flagged,
/// and the end goes on to the next turn.
#[test]
fn an_end_listener_past_its_deadline_is_cut_reported_and_the_quit_goes_on() {
    use egui_pop::api::bus::bus_contract::ListenOpts;
    use helios_gui::app_quit::{AppQuit, CLIENT, TurnOutcome};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    let mut h = harness();
    let bus = h.state().app().handle().bus();
    let (release, held) = mpsc::channel::<()>();
    let held = Arc::new(Mutex::new(held));
    let stuck = "helios.test.stuck";
    bus.listen::<AppQuit>(
        stuck,
        ListenOpts::new()
            .threaded()
            .deadline(Duration::from_millis(100))
            .before(CLIENT)
            .owner("helios.test"),
        move |ask, _| {
            if ask.stage.turn_of(stuck) {
                let _ = held.lock().unwrap().recv();
            }
            Ok(())
        },
    )
    .unwrap();
    quit(&mut h);
    assert!(h.state().quitting());
    let ids: Vec<String> = h.state().end_steps(&[]).into_iter().map(|s| s.id).collect();
    let at = ids.iter().position(|id| id == stuck).unwrap();
    let started = Instant::now();
    let outcome = h.state().quit_turn(at, stuck);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    assert!(
        matches!(outcome, TurnOutcome::Late { cut: true, .. }),
        "{outcome:?}"
    );
    h.run_steps(1);
    let next = ids.iter().position(|id| id == CLIENT).unwrap();
    assert_eq!(h.state().quit_turn(next, CLIENT), TurnOutcome::Finished);
    let names: Vec<&str> = ids.iter().map(String::as_str).collect();
    let report = h.state().end_report(&names);
    assert_eq!(report.late.len(), 1, "{report:?}");
    assert_eq!(report.late[0].listener, stuck);
    assert_eq!(report.late[0].deadline_ms, 100);
    assert!(report.may_have_lost_work);
    assert!(
        report.finished.contains(&CLIENT.to_owned()),
        "the quit went on"
    );
    assert!(
        report
            .lines()
            .iter()
            .any(|l| l.contains("stopped `helios.test.stuck` after its 0.1 s deadline")),
        "{:?}",
        report.lines()
    );
    release.send(()).unwrap();
}

/// The end's report is said on the journal, written for the next start
/// and read back there once: the steps the end left are abandoned.
#[test]
fn the_quit_report_is_journaled_and_read_back_at_the_next_start() {
    use helios_gui::app_quit::{REPORT_EVENT, REPORT_FILE, ShutdownReport};

    let mut h = harness();
    quit(&mut h);
    let ids: Vec<String> = h.state().end_steps(&[]).into_iter().map(|s| s.id).collect();
    // The end left before its last turn ("Quit now").
    for (turn, id) in ids.iter().enumerate().take(ids.len() - 1) {
        h.state().quit_turn(turn, id);
        h.run_steps(1);
    }
    let names: Vec<&str> = ids.iter().map(String::as_str).collect();
    let report = h.state().end_report(&names);
    assert_eq!(report.abandoned, [ids.last().unwrap().clone()]);
    assert!(report.may_have_lost_work);
    let said: Vec<_> = h
        .state()
        .app()
        .handle()
        .events_since(0)
        .into_iter()
        .filter(|e| e.name == REPORT_EVENT)
        .collect();
    assert_eq!(said.len(), 1, "said once on the journal");
    assert_eq!(said[0].data["abandoned"][0], ids.last().unwrap().as_str());

    let dir = std::env::temp_dir().join(format!("helios-quit-report-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    report.write(&dir).unwrap();
    let read = ShutdownReport::take(&dir).expect("read back at the next start");
    assert_eq!(read, report);
    assert!(
        read.lines()
            .iter()
            .any(|l| l.contains("left `helios.quit.client` before it ran")),
        "{:?}",
        read.lines()
    );
    assert!(!dir.join(REPORT_FILE).exists(), "taken once");
    assert!(ShutdownReport::take(&dir).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// No sleep is left on the quit and close path, the node's side included
/// (its quit request, its jobs, the import's cancel watch): each wait there is woken
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
        "crates/helios-node-core/src/node_quit.rs",
        "crates/helios-node-core/src/job_promises.rs",
        "crates/helios-node-core/src/import_promise.rs",
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

/// Exports that stop when let go: the node's quit waits on them.
struct HeldExports {
    began: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>,
    gate: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

impl helios_node_core::node::NodeJobs for HeldExports {
    fn name(&self) -> helios_core::Message {
        helios_core::Message::new("node-end-exports", "Stopping the exports")
    }
    fn running(&self) -> Vec<String> {
        Vec::new()
    }
    fn cancel_all(&self) {
        if let Some(began) = self.began.lock().unwrap().take() {
            let _ = began.send(());
        }
    }
    fn wait(&self, deadline: std::time::Duration) -> bool {
        let (open, opened) = &*self.gate;
        let held = open.lock().unwrap();
        *opened
            .wait_timeout_while(held, deadline, |o| !*o)
            .unwrap()
            .0
    }
    fn stop_within(&self) -> std::time::Duration {
        std::time::Duration::from_secs(5)
    }
}

/// The node's quit, told over its link, shows on the end splash: the row
/// of the node's listener running names what it waits for, and what the
/// node's quit did goes in the end's report.
#[test]
fn the_end_splash_shows_the_node_s_listener_running() {
    use helios_gui::app_quit::{NodeEnd, ShutdownReport};
    use helios_node_core::node::LocalNode;
    use helios_node_core::node_quit::VAULT;
    use helios_vault_sqlite::SqliteStore;
    use std::sync::Arc;
    use std::time::Duration;

    let dir = std::env::temp_dir().join(format!("helios-quit-node-end-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let node = LocalNode::open(
        SqliteStore::open(&dir.join("vault.db")).unwrap(),
        Arc::new(helios_core::Bus::new()),
        Duration::from_hours(1),
    );
    let link = node.link();
    let (began, begun) = std::sync::mpsc::channel();
    let gate: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)> = Arc::default();
    link.adopt(Arc::new(HeldExports {
        began: std::sync::Mutex::new(Some(began)),
        gate: gate.clone(),
    }));
    let phase = helios_node_core::phases::Phase::new();
    let mut follow = NodeEnd::new(link.follow_quit().expect("the node's facts"), phase.clone());
    let client = node.connect();
    link.quit_cancelling_jobs();
    drop(client);
    begun
        .recv_timeout(Duration::from_secs(10))
        .expect("the exports' listener began");
    follow.pump();
    let steps = phase.steps();
    let running: Vec<_> = steps.iter().filter(|s| s.running()).collect();
    assert_eq!(running.len(), 1, "{steps:?}");
    assert_eq!(running[0].label.key, "node-end-exports");
    assert_eq!(
        running[0].why.as_ref().map(|w| w.english.as_str()),
        Some("Waiting for: exports")
    );
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    assert!(
        link.wait_stopped(Duration::from_secs(10)),
        "the node stopped"
    );
    follow.pump();
    assert!(!phase.busy(), "every row done: {:?}", phase.steps());
    let ended = follow.ended().expect("the node's quit ended, said");
    assert_eq!(ended.finished, ["helios.node.quit.exports", VAULT]);
    let mut report = ShutdownReport::default();
    report.record_node(ended);
    assert!(report.clean());
    assert!(report.finished.contains(&VAULT.to_owned()));
    node.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A node listener cut at its deadline is named in the end's report,
/// what the next start says.
#[test]
fn a_node_listener_cut_is_named_in_the_quit_report() {
    use helios_gui::app_quit::ShutdownReport;
    use helios_node_core::node_quit::{LateListener, QuitEnded, VAULT};

    let ended = QuitEnded {
        finished: vec![VAULT.to_owned()],
        late: vec![LateListener {
            listener: "helios.node.quit.exports".into(),
            deadline_ms: 2000,
            cut: true,
        }],
        failed: Vec::new(),
    };
    let mut report = ShutdownReport::default();
    report.record_node(&ended);
    assert!(report.may_have_lost_work);
    assert!(
        report
            .lines()
            .iter()
            .any(|l| l.contains("stopped `helios.node.quit.exports` after its 2 s deadline")),
        "{:?}",
        report.lines()
    );
}
