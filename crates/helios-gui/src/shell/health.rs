//! The health check on the desktop ([`helios_health`]): Help > Health
//! check (`help.health`) opens its page, which lists the findings by
//! level, each with its detail, a "Fix…" button running its command and
//! "Ignore"; the ignored ones wait under "Ignored (n)" to be taken back.
//! "Check now" (`health.checkNow`) runs the checks again.
//!
//! The checks run as a background job ([`helios_health::runner`]): a
//! while after start, daily, and when a volume comes or goes (the
//! storage watch's listener). The status bar's badge shows the worst
//! finding needing attention and their count; it opens the page. Nothing
//! shows while all is well, and nothing is ever modal.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use egui::Context as Egui;
use egui_pop::model::ids::CommandId;
use helios_core::Message;
use helios_health::runner::Runner;
use helios_health::{Finding, Level};

use super::Shell;

/// Opens the Health page.
pub const COMMAND: &str = "help.health";
/// Runs the checks now.
pub const CHECK_NOW: &str = "health.checkNow";
/// The status bar badge's id.
pub const BADGE: &str = "helios.health.badge";

/// The page's state, kept by the shell.
#[derive(Default)]
pub(crate) struct HealthPage {
    runner: Option<Runner>,
    open: bool,
    /// The context the job's thread asks a frame of.
    ctx: Arc<Mutex<Option<Egui>>>,
    /// How many times a volume came or went, once listened to.
    volumes: Arc<AtomicU64>,
    listening: bool,
}

/// A finding as the page shows it, its words translated.
struct Row {
    id: String,
    level: Level,
    message: String,
    detail: String,
    fix: Option<(String, String, serde_json::Value)>,
}

/// What the page's buttons asked.
enum Asked {
    CheckNow,
    Fix(String, serde_json::Value),
    Ignore(String, bool),
}

impl Shell {
    /// Runs the health checks with `runner`, reported in the jobs centre;
    /// the desktop gives Helios's checks over the machine and the vault,
    /// tests their own.
    pub fn set_health(&mut self, mut runner: Runner) {
        runner.report_to(self.jobs());
        let ctx = self.health.ctx.clone();
        runner.on_done(move || {
            if let Some(ctx) = ctx.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                ctx.request_repaint();
            }
        });
        self.health.runner = Some(runner);
    }

    /// The health checks' runner, when set.
    pub fn health(&self) -> Option<&Runner> {
        self.health.runner.as_ref()
    }

    /// Whether the Health page is open.
    pub fn health_page_open(&self) -> bool {
        self.health.open
    }

    pub(super) fn open_health(&mut self) {
        self.health.open = true;
    }

    /// `health.checkNow`: whether a run started (none while one runs).
    pub(super) fn health_check_now(&mut self) -> bool {
        self.health.runner.as_mut().is_some_and(Runner::run_now)
    }

    /// Before the bar is drawn: a run when one is due, and the badge's
    /// worst finding.
    pub(super) fn health_tick(&mut self, ctx: &Egui) {
        {
            let mut held = self
                .health
                .ctx
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if held.is_none() {
                *held = Some(ctx.clone());
            }
        }
        if !self.health.listening && self.watching_volumes() {
            let (volumes, ctx) = (self.health.volumes.clone(), ctx.clone());
            self.on_volumes_changed(move || {
                volumes.fetch_add(1, Ordering::SeqCst);
                ctx.request_repaint();
            });
            self.health.listening = true;
        }
        let generation = self
            .health
            .listening
            .then(|| self.health.volumes.load(Ordering::SeqCst));
        let Some(runner) = self.health.runner.as_mut() else {
            return;
        };
        runner.tick(Instant::now(), generation);
        let worst = runner.findings().worst();
        self.status.borrow_mut().health = worst;
    }

    /// After the panels: the page, when open.
    pub(super) fn health_window(&mut self, ctx: &Egui) {
        if !self.health.open {
            return;
        }
        let Some(runner) = self.health.runner.as_ref() else {
            self.health.open = false;
            return;
        };
        let t = |m: Message| crate::i18n::text(ctx, m);
        let (rows, ignored, ran) = {
            let findings = runner.findings();
            let row = |f: &Finding| Row {
                id: f.id.clone(),
                level: f.level,
                message: t(f.message.clone()),
                detail: t(f.detail.clone()),
                fix: f
                    .fix
                    .as_ref()
                    .map(|x| (t(x.label.clone()), x.command.clone(), x.args.clone())),
            };
            (
                findings.shown().into_iter().map(row).collect::<Vec<_>>(),
                findings
                    .ignored_findings()
                    .into_iter()
                    .map(row)
                    .collect::<Vec<_>>(),
                findings.ran(),
            )
        };
        let running = runner.running();
        let mut open = true;
        let mut asked = None;
        let max = ctx.content_rect().size() - egui::vec2(32.0, 32.0);
        egui::Window::new(t(Message::new("gui-health-title", "Health check")))
            .id(egui::Id::unique("helios.health.window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(560.0, 420.0).min(max))
            .max_size(max)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let label = if running {
                        t(Message::new("gui-health-checking", "Checking…"))
                    } else {
                        t(Message::new("gui-health-check-now", "Check now"))
                    };
                    if ui.add_enabled(!running, egui::Button::new(label)).clicked() {
                        asked = Some(Asked::CheckNow);
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if !ran {
                        ui.label(t(Message::new(
                            "gui-health-not-yet",
                            "Not checked yet: the checks run in the background.",
                        )));
                    }
                    for (level, heading) in [
                        (
                            Level::Problem,
                            Message::new("gui-health-problems", "Problems ({ $count })"),
                        ),
                        (
                            Level::Warning,
                            Message::new("gui-health-warnings", "Warnings ({ $count })"),
                        ),
                        (
                            Level::Ok,
                            Message::new("gui-health-ok", "All good ({ $count })"),
                        ),
                    ] {
                        let these: Vec<&Row> = rows.iter().filter(|r| r.level == level).collect();
                        if these.is_empty() {
                            continue;
                        }
                        ui.heading(t(heading.with("count", these.len())));
                        for row in these {
                            row_ui(ui, row, &t, &mut asked, false);
                        }
                    }
                    if !ignored.is_empty() {
                        ui.separator();
                        ui.heading(t(Message::new(
                            "gui-health-ignored",
                            "Ignored ({ $count })",
                        )
                        .with("count", ignored.len())));
                        for row in &ignored {
                            row_ui(ui, row, &t, &mut asked, true);
                        }
                    }
                });
            });
        if !open {
            self.health.open = false;
        }
        match asked {
            Some(Asked::CheckNow) => {
                self.health_check_now();
            }
            Some(Asked::Fix(command, args)) => match CommandId::new(&command) {
                Ok(id) => {
                    self.app.handle().run(id, args);
                    // Then the checks again, so a fix done at once (an
                    // acknowledgement) clears its finding.
                    if let Ok(again) = CommandId::new(CHECK_NOW) {
                        self.app.handle().run(again, serde_json::Value::Null);
                    }
                }
                Err(e) => self.warn(format!("health: {command}: {e}")),
            },
            Some(Asked::Ignore(id, on)) => {
                let saved = self
                    .health
                    .runner
                    .as_ref()
                    .map(|r| r.findings().ignored.set(&id, on));
                if let Some(Err(e)) = saved {
                    self.warn(format!("health: {e}"));
                }
                let worst = self
                    .health
                    .runner
                    .as_ref()
                    .and_then(|r| r.findings().worst());
                self.status.borrow_mut().health = worst;
            }
            None => {}
        }
    }
}

/// One finding: its message, its detail, its fix and Ignore (or Undo,
/// ignored).
fn row_ui(
    ui: &mut egui::Ui,
    row: &Row,
    t: &dyn Fn(Message) -> String,
    asked: &mut Option<Asked>,
    ignored: bool,
) {
    ui.group(|ui| {
        ui.set_width(ui.available_width());
        let colour = match row.level {
            Level::Problem => ui.visuals().error_fg_color,
            Level::Warning => ui.visuals().warn_fg_color,
            Level::Ok => ui.visuals().text_color(),
        };
        ui.colored_label(colour, egui::RichText::new(&row.message).strong());
        ui.label(egui::RichText::new(&row.detail).small());
        ui.horizontal(|ui| {
            if ignored {
                if ui
                    .button(t(Message::new("gui-health-undo-ignore", "Show again")))
                    .clicked()
                {
                    *asked = Some(Asked::Ignore(row.id.clone(), false));
                }
                return;
            }
            if let Some((label, command, args)) = &row.fix
                && ui
                    .button(t(Message::new("gui-health-fix", "Fix…")))
                    .on_hover_text(label)
                    .clicked()
            {
                *asked = Some(Asked::Fix(command.clone(), args.clone()));
            }
            if row.level > Level::Ok
                && ui
                    .button(t(Message::new("gui-health-ignore", "Ignore")))
                    .clicked()
            {
                *asked = Some(Asked::Ignore(row.id.clone(), true));
            }
        });
    });
}
