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
//!   one into it (egui-pop's rule): Helios goes on.
//! - **Closing the last window** quits, as Quit does.
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

use egui::{Context, ViewportCommand, ViewportId};
use egui_pop::api::windows::WindowId;
use helios_core::Message;

use super::Shell;
use crate::dialogs::Level;

/// Quits once the question about running jobs is answered.
pub(super) const QUIT_CONFIRMED: &str = "helios.quit.confirmed";
/// Brings the main window forward.
pub(super) const MAIN_WINDOW: &str = "helios.window.main";

/// How many passes a quit question holds back the main window's close
/// that came with it (egui-pop's Quit closes the window it runs in).
const HOLD_PASSES: u64 = 3;

/// Where the application's life stands.
#[derive(Debug, Default)]
pub(super) struct Lifetime {
    /// Quit was asked and nothing holds it: the main window's close ends
    /// the process.
    quitting: bool,
    /// A quit question was opened at this pass: the main window's close
    /// sent with the same Quit is cancelled, not taken for a close.
    asked_at: Option<u64>,
    /// The local node, asked to quit when Helios quits.
    node: Option<crate::node::Link>,
    /// The quit cancels the running jobs (asked, or nothing to ask).
    cancel_jobs: bool,
}

impl Lifetime {
    /// Helios quits: the node is asked to quit too, cancelling its jobs
    /// when `cancel_jobs` (the question answered), waiting for them if
    /// not.
    fn quitting(&mut self, cancel_jobs: bool) {
        self.quitting = true;
        self.cancel_jobs |= cancel_jobs;
        if let Some(node) = &self.node {
            tell(node, self.cancel_jobs);
        }
    }
}

fn tell(node: &crate::node::Link, cancel_jobs: bool) {
    if cancel_jobs {
        node.quit_cancelling_jobs();
    } else {
        node.quit();
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
}

impl Shell {
    /// Attaches the local node: Helios quitting asks it to quit.
    pub fn attach_node(&mut self, node: crate::node::Link) {
        if self.lifetime.quitting {
            tell(&node, self.lifetime.cancel_jobs);
        }
        self.lifetime.node = Some(node);
    }

    /// The system ends the session: Helios quits at once, asking nothing.
    pub fn session_ending(&mut self) {
        self.lifetime.quitting(true);
    }

    /// Quits Helios, from any window: asks first when jobs are running.
    pub(super) fn quit(&mut self, ctx: &Context) {
        if self.lifetime.quitting {
            close_main_window(ctx);
            return;
        }
        let running = self.running_jobs();
        if running == 0 || self.core.assume_yes {
            self.quit_confirmed(ctx);
        } else {
            self.lifetime.asked_at = Some(ctx.cumulative_pass_nr());
            self.ask_before_quitting(running);
        }
    }

    /// Quits, whatever runs: the running jobs cancelled.
    pub(super) fn quit_confirmed(&mut self, ctx: &Context) {
        self.lifetime.quitting(true);
        close_main_window(ctx);
    }

    /// Whether Helios is quitting: the main window's close ends it.
    pub fn quitting(&self) -> bool {
        self.lifetime.quitting
    }

    /// The main window's close was requested in this pass (its close
    /// button, Alt+F4, Ctrl+W, the window manager, Quit): what the host
    /// does with it. The host quits on [`Closing::Quit`] only, and draws
    /// nothing in this pass on [`Closing::Kept`].
    pub fn main_window_closing(&mut self, ctx: &Context) -> Closing {
        if self.lifetime.quitting {
            return Closing::Quit;
        }
        // The close that came with a Quit whose question is open.
        if let Some(at) = self.lifetime.asked_at.take()
            && ctx.cumulative_pass_nr() <= at + HOLD_PASSES
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.request_repaint();
            return Closing::Kept;
        }
        if self.core.app.handle().windows().len() > 1 {
            return Closing::Moved;
        }
        // The last window: closing it quits, as Quit does.
        let running = self.running_jobs();
        if running == 0 || self.core.assume_yes {
            self.lifetime.quitting(true);
            return Closing::Quit;
        }
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        ctx.request_repaint();
        self.ask_before_quitting(running);
        Closing::Kept
    }

    /// Window > Main window: brings it forward. One closed while another
    /// window took its place comes back once that window closes.
    pub(super) fn main_window(&mut self) {
        let app = self.core.app.handle().clone();
        let main = WindowId::main();
        if app.window_viewport(&main).is_some() {
            app.raise_window(&main);
        } else {
            self.notify(
                Level::Info,
                Message::new(
                    "gui-main-window-in-use",
                    "The main window shows this window now; it comes back when this window closes",
                ),
            );
        }
    }

    /// The jobs a quit would cancel: the node's (the exports, which
    /// outlive their window) and the others running in this process.
    fn running_jobs(&self) -> usize {
        let mut running = self.core.jobs.cancellable();
        if let Some(node) = &self.lifetime.node {
            running.extend(node.running_jobs());
        }
        running.sort_unstable();
        running.dedup();
        running.len()
    }

    /// The one question before quitting: what it would cancel.
    fn ask_before_quitting(&mut self, running: usize) {
        let confirm = crate::dialogs::Confirm {
            title: self.text(Message::new("gui-quit-title", "Quit Helios?")),
            detail: self.text(
                Message::new(
                    "gui-quit-jobs-detail",
                    "{ $count } jobs are running; quitting cancels them",
                )
                .with("count", running),
            ),
            verb: self.text(Message::new("gui-quit-verb", "Quit")),
            danger: true,
            command: QUIT_CONFIRMED.to_owned(),
            args: serde_json::Value::Null,
        };
        self.confirm(confirm);
    }
}

/// Closes the main window, wherever the quit was asked: the process ends
/// with it.
fn close_main_window(ctx: &Context) {
    ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
    ctx.request_repaint_of(ViewportId::ROOT);
}
