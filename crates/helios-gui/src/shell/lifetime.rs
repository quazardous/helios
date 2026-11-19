//! The application's life, apart from its windows' (docs/design/windows.md,
//! section 7).
//!
//! The process's core (the library, the jobs, the kernel) is Helios's
//! local node; every OS window, the main one included, is a view of it.
//! Quitting is the core's policy, not a window's:
//!
//! - **Quit** (Ctrl+Q, File > Quit, from any window) ends Helios: the main
//!   window is closed, and every other window with it. When jobs are
//!   running, one question is asked first, and only then.
//! - **Closing a window** (Ctrl+W, Alt+F4, its close button) closes that
//!   window only. Closing the main window while another is open moves that
//!   one into it (egui-pop's rule): Helios goes on. Window > Main window
//!   brings the main window back, that window going back to its own.
//! - **Closing the last window** quits, as Quit does, on Linux and
//!   Windows. On macOS ([`LastWindow::StayOpen`]) Helios stays running
//!   with no window, as document applications do there: the main window
//!   is hidden, and the Dock icon, File > New Window or Window > Main
//!   window bring it back; Cmd+Q quits.
//!
//! The host (the desktop's `main.rs`) asks [`Shell::main_window_closing`]
//! when the main window's close is requested, and quits only when told.
//!
//! Quitting is asked of the local node ([`crate::node`], attached with
//! [`Shell::attach_node`]): the node owns the vault, and closes it once
//! asked to quit and its last client (the GUI) is gone. No window saves
//! or closes the vault. The node also runs the work that outlives a
//! window (the exports): the question before quitting counts its jobs,
//! and a confirmed quit asks the node to cancel them.
//!
//! Whether a quit may go is one request on the bus,
//! [`crate::app_quit::AppQuit`] (`helios.app.quit`), asked by the main
//! window's `window.close` before-hook: its listeners, the running jobs'
//! question among them, refuse in order, and a refusal vetoes the close.
//! Once the close goes, the end splash asks the same request again, one
//! listener's turn per frame ([`Shell::quit_turn`]): telling the node to
//! quit and detaching the GUI client are its listeners
//! ([`crate::app_quit::NODE`], [`crate::app_quit::CLIENT`]).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use egui::{Context, ViewportCommand};
use egui_pop::api::bus::{Reason, STEP_WINDOW_CLOSE, WindowKind, WindowRequest};
use egui_pop::api::steps::Before;
use egui_pop::api::windows::WindowId;
use egui_pop::api::{AppHandle, Ctx, Plugin, PluginError};
use egui_pop::model::ids::PluginId;
use helios_core::Message;
use serde_json::Value;

use super::Shell;

/// Quits once the question about running jobs is answered.
pub(super) const QUIT_CONFIRMED: &str = "helios.quit.confirmed";
/// Brings the main window forward.
pub(super) const MAIN_WINDOW: &str = "helios.window.main";

/// The plugin holding the hooks of the main window's `window.close`
/// step: the question before quitting, and the quit once it goes.
pub const QUIT_PLUGIN: PluginId = PluginId::from_static("helios.quit");

/// The key of egui-pop's `window.close` step result when another window
/// moved into the main one instead of it closing.
const MOVED_IN: &str = "moved_in";

/// What closing the last window does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LastWindow {
    /// Helios quits (Linux, Windows).
    Quit,
    /// Helios stays running with no window (macOS).
    StayOpen,
}

impl LastWindow {
    /// The platform's habit.
    pub const fn platform() -> Self {
        if cfg!(target_os = "macos") {
            LastWindow::StayOpen
        } else {
            LastWindow::Quit
        }
    }
}

impl Default for LastWindow {
    fn default() -> Self {
        Self::platform()
    }
}

/// The main window, as the last window's policy sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MainWindow {
    #[default]
    Shown,
    /// The last window was closed and Helios stays running: hidden until
    /// it is asked back.
    Hidden,
    /// Asked back: shown at the next pass.
    Reopening,
}

/// Where the application's life stands.
#[derive(Debug)]
pub(super) struct Lifetime {
    /// Whether the main window is hidden, Helios running with no window.
    main: MainWindow,
    /// What the `window.close` step's hooks share with the shell.
    quit: Rc<Quit>,
}

impl Lifetime {
    pub(super) fn new(quit: Rc<Quit>) -> Self {
        Self {
            main: MainWindow::default(),
            quit,
        }
    }
}

/// Quitting, as the hooks of the main window's `window.close` step and
/// the shell share it (on the interface's thread: the hooks run in
/// egui-pop's call).
pub(crate) struct Quit {
    /// What closing the last window does.
    last_window: Cell<LastWindow>,
    /// Quit was asked and nothing holds it: the main window's close ends
    /// the process.
    quitting: Cell<bool>,
    /// The quit cancels the running jobs (asked, or nothing to ask).
    cancel_jobs: Cell<bool>,
    /// The question about the running jobs was answered Quit: the next
    /// quit's request does not ask it again.
    jobs_confirmed: Cell<bool>,
    /// What the last quit's request did.
    report: RefCell<Option<crate::app_quit::QuitReport>>,
    /// Confirmations are answered yes by policy: nothing is asked.
    assume_yes: Cell<bool>,
    /// The local node and the GUI client, told to quit and detached by
    /// the end's listeners.
    slots: Arc<crate::app_quit::EndSlots>,
    /// What the end's steps did so far.
    ended: RefCell<crate::app_quit::ShutdownReport>,
    /// The jobs running in this process.
    jobs: helios_core::jobs::Jobs,
    /// The question's language.
    i18n: egui_pop::i18n::I18n,
}

impl std::fmt::Debug for Quit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Quit")
            .field("last_window", &self.last_window.get())
            .field("quitting", &self.quitting.get())
            .finish_non_exhaustive()
    }
}

impl Quit {
    pub(crate) fn new(jobs: helios_core::jobs::Jobs, i18n: egui_pop::i18n::I18n) -> Self {
        Self {
            last_window: Cell::new(LastWindow::platform()),
            quitting: Cell::new(false),
            cancel_jobs: Cell::new(false),
            jobs_confirmed: Cell::new(false),
            report: RefCell::new(None),
            assume_yes: Cell::new(false),
            slots: Arc::default(),
            ended: RefCell::default(),
            jobs,
            i18n,
        }
    }

    /// Helios quits, cancelling its jobs when `cancel_jobs` (the
    /// question answered): the end splash's turns tell the node
    /// ([`crate::app_quit::NODE`]); dropped before, the shell tells it.
    fn quitting(&self, cancel_jobs: bool) {
        self.quitting.set(true);
        self.cancel_jobs.set(self.cancel_jobs.get() | cancel_jobs);
    }

    /// The jobs a quit would cancel: the node's (the exports, which
    /// outlive their window) and the others running in this process.
    fn running_jobs(&self) -> usize {
        let mut running = self.jobs.cancellable();
        if let Some(node) = self.slots.node() {
            running.extend(node.running_jobs());
        }
        running.sort_unstable();
        running.dedup();
        running.len()
    }

    /// Whether a main window's close ends Helios: Quit, or the last
    /// window closed where that quits. Another window moving into the
    /// main one, or the last window hidden on macOS, does not.
    fn ends(&self, app: &AppHandle, req: &WindowRequest) -> bool {
        req.reason == Reason::Quit
            || (app.windows().len() <= 1 && self.last_window.get() == LastWindow::Quit)
    }

    /// The before-hook of `window.close`: the main window's close that
    /// would end Helios asks the quit's request
    /// ([`crate::app_quit::AppQuit`]); a listener refusing vetoes the
    /// close. Refused by the running jobs' listener, the question is
    /// asked; its Quit ([`QUIT_CONFIRMED`]) asks the close again, which
    /// goes.
    fn before_close(&self, app: &AppHandle, args: &Value) -> Before {
        use crate::app_quit::{self, QuitAsk, QuitWhy};
        let Ok(req) = serde_json::from_value::<WindowRequest>(args.clone()) else {
            return Before::Go;
        };
        if req.kind != WindowKind::Main || self.quitting.get() || !self.ends(app, &req) {
            return Before::Go;
        }
        let running = self.running_jobs();
        let ask = QuitAsk {
            why: if req.reason == Reason::Quit {
                QuitWhy::Quit
            } else {
                QuitWhy::LastWindow
            },
            stage: crate::app_quit::QuitStage::Ask,
            running_jobs: running,
            jobs_confirmed: self.jobs_confirmed.get() || self.assume_yes.get(),
        };
        let report = app_quit::ask(&*app.bus(), ask);
        let refused = report.refused.clone();
        *self.report.borrow_mut() = Some(report);
        let Some((listener, reason)) = refused else {
            return Before::Go;
        };
        if listener == app_quit::JOBS {
            self.ask(app, running);
        } else {
            // Kept open for another reason: the jobs are asked about
            // again at the next quit.
            self.jobs_confirmed.set(false);
        }
        Before::Veto(format!("{listener}: {reason}"))
    }

    /// The after-hook of `window.close`: the main window closed, Helios
    /// quits (its running jobs cancelled: asked, or nothing to ask).
    fn after_close(&self, args: &Value, result: &Result<Value, String>) {
        let Ok(req) = serde_json::from_value::<WindowRequest>(args.clone()) else {
            return;
        };
        let Ok(done) = result else { return };
        if req.kind != WindowKind::Main || done.get(MOVED_IN).is_some() {
            return;
        }
        // The last window hidden on macOS is the host's, not a quit.
        if req.reason != Reason::Quit && self.last_window.get() == LastWindow::StayOpen {
            return;
        }
        self.quitting(true);
    }

    /// The one question before quitting: what it would cancel.
    fn ask(&self, app: &AppHandle, running: usize) {
        let text = |m: Message| helios_i18n::message(&self.i18n, &m);
        let confirm = crate::dialogs::Confirm {
            title: text(Message::new("gui-quit-title", "Quit Helios?")),
            detail: text(
                Message::new(
                    "gui-quit-jobs-detail",
                    "{ $count } jobs are running; quitting cancels them",
                )
                .with("count", running),
            ),
            verb: text(Message::new("gui-quit-verb", "Quit")),
            danger: true,
            command: QUIT_CONFIRMED.to_owned(),
            args: Value::Null,
        };
        match (
            app.service(&egui_pop::api::dialogs::DIALOGS),
            super::confirm_dialog(confirm),
        ) {
            (Some(dialogs), Ok(dialog)) => {
                dialogs.open(dialog);
            }
            (None, _) => log::warn!(target: "helios", "no dialogs to ask before quitting"),
            (_, Err(e)) => log::warn!(target: "helios", "the question before quitting: {e}"),
        }
    }
}

impl Drop for Quit {
    /// The shell goes while Helios quits and the end's turns did not tell
    /// the node ("Quit now", the end's limit, a host without the end
    /// splash): told now, so it still closes the vault once its last
    /// client is gone.
    fn drop(&mut self) {
        if self.quitting.get() {
            self.slots.tell(self.cancel_jobs.get());
        }
    }
}

/// The hooks of egui-pop's `window.close` step for the main window: the
/// question before quitting while jobs run (a before-hook vetoing the
/// close), and Helios quitting once the close went (an after-hook).
/// Every way the main window closes takes this path: its close button,
/// the system's close, Ctrl+W, egui-pop's Quit and Helios's.
pub(crate) struct QuitHooks(pub(crate) Rc<Quit>);

impl Plugin for QuitHooks {
    fn id(&self) -> PluginId {
        QUIT_PLUGIN
    }

    fn title(&self) -> String {
        "Helios quit".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        let before = self.0.clone();
        let after = self.0.clone();
        // Helios's own listeners on the quit's request.
        if let Err(e) = crate::app_quit::listen(&*ctx.app().bus(), &self.0.slots) {
            log::error!(target: "helios", "{}: {e}", crate::app_quit::QUIT);
        }
        ctx.before(STEP_WINDOW_CLOSE, move |app, args| {
            before.before_close(app, args)
        })
        .after(STEP_WINDOW_CLOSE, move |_, args, result| {
            after.after_close(args, result);
        });
        Ok(())
    }
}

/// What the main window's close request does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closing {
    /// Helios quits after this pass, every window with it.
    Quit,
    /// Another window moves into the main one (egui-pop's rule): Helios
    /// goes on.
    Moved,
    /// The close is cancelled: a question is asked first.
    Kept,
    /// The last window closed and Helios stays running with no window
    /// ([`LastWindow::StayOpen`]): the host hides the main window
    /// instead of closing it.
    Hidden,
}

impl Shell {
    /// Attaches the local node: Helios quitting asks it to quit.
    pub fn attach_node(&mut self, node: crate::node::Link) {
        let quit = &self.lifetime.quit;
        quit.slots.set_node(node);
        // Attached late (the start handing the library over after the
        // close): told at once.
        if quit.quitting.get() {
            quit.slots.tell(quit.cancel_jobs.get());
        }
    }

    /// The GUI's client of the local node: detached at the end's turn of
    /// [`crate::app_quit::CLIENT`], after the host's steps.
    pub fn attach_client(&mut self, client: crate::node::Client) {
        self.lifetime.quit.slots.set_client(client);
    }

    /// The local node, when attached.
    pub fn node_link(&self) -> Option<crate::node::Link> {
        self.lifetime.quit.slots.node()
    }

    /// The end splash's steps, in order: the quit request's listeners,
    /// and the host's steps `host` placed before the GUI client detaches
    /// ([`crate::app_quit::end_steps`]).
    pub fn end_steps(&self, host: &[&str]) -> Vec<crate::app_quit::EndStep> {
        crate::app_quit::end_steps(&*self.core.app.handle().bus(), host)
    }

    /// The end's `turn`-th step, the listener `listener`'s: the quit
    /// request asked through `Defer` and heard at this drain. Kept in the
    /// end's report; the quit goes on whatever it did.
    pub fn quit_turn(&self, turn: usize, listener: &str) -> crate::app_quit::TurnOutcome {
        use crate::app_quit::{QuitAsk, QuitStage, QuitWhy, turn as ask_turn};
        let quit = &self.lifetime.quit;
        let handle = self.core.app.handle().clone();
        let ask = QuitAsk {
            why: QuitWhy::Quit,
            stage: QuitStage::End {
                turn,
                listener: listener.to_owned(),
            },
            running_jobs: 0,
            jobs_confirmed: quit.cancel_jobs.get(),
        };
        let outcome = ask_turn(&*handle.bus(), || handle.drain_asks(), ask);
        quit.ended.borrow_mut().record(listener, &outcome);
        outcome
    }

    /// The host's end step `id` did its part.
    pub fn quit_host_step_done(&self, id: &str) {
        let done = crate::app_quit::TurnOutcome::Finished;
        self.lifetime.quit.ended.borrow_mut().record(id, &done);
    }

    /// The end is over: the steps of `steps` that did not run are
    /// abandoned. The end's report, said on the journal
    /// ([`crate::app_quit::REPORT_EVENT`]).
    pub fn end_report(&self, steps: &[&str]) -> crate::app_quit::ShutdownReport {
        let mut report = self.lifetime.quit.ended.borrow().clone();
        let ran: Vec<String> = report
            .finished
            .iter()
            .chain(report.late.iter().map(|l| &l.listener))
            .chain(report.failed.iter().map(|(id, _)| id))
            .cloned()
            .collect();
        let left: Vec<&str> = steps
            .iter()
            .copied()
            .filter(|id| !ran.iter().any(|r| r == id))
            .collect();
        report.abandon(left);
        if !report.clean() {
            for line in report.lines() {
                log::warn!(target: "helios::quit", "{line}");
            }
        }
        self.core.app.handle().publish(
            "helios",
            None,
            crate::app_quit::REPORT_EVENT,
            serde_json::to_value(&report).unwrap_or_default(),
        );
        report
    }

    /// The system ends the session: Helios quits at once, asking nothing.
    pub fn session_ending(&mut self) {
        self.lifetime.quit.quitting(true);
    }

    /// Quits Helios, from any window: egui-pop's Quit, the main window's
    /// `window.close` with the reason `quit`, whose before-hook asks
    /// first when jobs are running ([`QuitHooks`]).
    pub(super) fn quit(&mut self) {
        self.core.app.handle().quit();
    }

    /// Quits, cancelling what runs (the question answered Quit): the
    /// quit's request is asked again, its running jobs' listener lets it
    /// go, and egui-pop closes the main window, every other window with
    /// it, none moving into the main one. Another listener may still
    /// refuse (an edit not saved).
    pub(super) fn quit_confirmed(&mut self) {
        self.lifetime.quit.jobs_confirmed.set(true);
        self.core.app.handle().quit();
    }

    /// What the last quit's request did (`helios.app.quit`): each
    /// listener's line, the failures, the refusal.
    pub fn quit_report(&self) -> Option<crate::app_quit::QuitReport> {
        self.lifetime.quit.report.borrow().clone()
    }

    /// Whether Helios is quitting: the main window's close ends it.
    pub fn quitting(&self) -> bool {
        self.lifetime.quit.quitting.get()
    }

    /// Confirmations answered yes by policy: quitting asks nothing.
    pub(super) fn quit_assumes_yes(&self, on: bool) {
        self.lifetime.quit.assume_yes.set(on);
    }

    /// The main window's close was requested by the system in this pass
    /// (its close button, Alt+F4, the window manager, or the close a
    /// `window.close` of the main window asked): what the host does with
    /// it. The host quits on [`Closing::Quit`] only, and draws nothing in
    /// this pass on [`Closing::Kept`].
    ///
    /// The last window's close is the main window's `window.close` on the
    /// bus, asked here: its before-hook asks first when jobs are running
    /// (the close vetoed, the system's close cancelled), and the close
    /// that goes quits.
    pub fn main_window_closing(&mut self, ctx: &Context) -> Closing {
        if self.lifetime.quit.quitting.get() {
            return Closing::Quit;
        }
        let app = self.core.app.handle().clone();
        // egui-pop's pass moves the window worked in last into the main
        // one (its `window.close` step).
        if app.windows().len() > 1 {
            return Closing::Moved;
        }
        // The last window on macOS: Helios stays running with no window,
        // its jobs going on.
        if self.lifetime.quit.last_window.get() == LastWindow::StayOpen {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            self.lifetime.main = MainWindow::Hidden;
            return Closing::Hidden;
        }
        // The last window: its `window.close`, which quits, as Quit does,
        // unless its before-hook asked first.
        app.request_window_close(WindowRequest::new(
            WindowId::MAIN,
            WindowKind::Main,
            "app",
            Reason::User,
        ));
        if self.lifetime.quit.quitting.get() {
            return Closing::Quit;
        }
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        ctx.request_repaint();
        Closing::Kept
    }

    /// Window > Main window: brings it forward. One closed while another
    /// window took its place comes back: that window's content goes back
    /// to a window of its own, and the main window draws the shell again.
    pub(super) fn main_window(&mut self) {
        if self.window_hidden() {
            self.reopen();
            return;
        }
        self.core.app.handle().raise_window(&WindowId::main());
    }

    /// What closing the last window does: the platform's habit unless
    /// set (tests set macOS's on any platform).
    pub fn set_last_window(&mut self, last_window: LastWindow) {
        self.lifetime.quit.last_window.set(last_window);
    }

    /// Whether Helios runs with no window (the last one closed on macOS).
    pub fn window_hidden(&self) -> bool {
        self.lifetime.main != MainWindow::Shown
    }

    /// Brings the hidden main window back (the Dock icon, File > New
    /// Window): shown at the next pass, by [`Shell::show_reopened`].
    pub fn reopen(&mut self) {
        if self.lifetime.main == MainWindow::Hidden {
            self.lifetime.main = MainWindow::Reopening;
        }
    }

    /// Shows the main window again when it was asked back; whether it
    /// was. Runs in the main window's pass, which goes on while it is
    /// hidden.
    pub fn show_reopened(&mut self, ctx: &Context) -> bool {
        if self.lifetime.main != MainWindow::Reopening {
            return false;
        }
        self.lifetime.main = MainWindow::Shown;
        let root = egui::ViewportId::ROOT;
        ctx.send_viewport_cmd_to(root, ViewportCommand::Visible(true));
        ctx.send_viewport_cmd_to(root, ViewportCommand::Minimized(false));
        // The focus is asked on the bus (`window.activate`, `command`).
        super::raise_main_window(self.core.app.handle());
        ctx.request_repaint();
        true
    }
}
