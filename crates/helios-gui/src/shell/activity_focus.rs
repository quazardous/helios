//! The Activity focus: every background job in one place, as two panels.
//!
//! - **Jobs** (left): one row per job, those running first, then the
//!   recent ones, newest first. Each row has an icon by kind, the label
//!   and the source; a progress bar when the job reports how far it is
//!   (`done` of `total`), else an activity mark and the time elapsed (no
//!   fake bar); its state; Cancel while it runs, on the row hovered or
//!   selected. Filters by kind and state, and "Clear finished".
//! - **Job detail** (right): the selected job's label, kind, source and
//!   who started it (the person, an agent, the command line, the node on
//!   its own); its start, elapsed time and time left; its progress and
//!   current step; the per-item outcome when it has items (an import's
//!   added, skipped and failed files, with the reasons); its log (the
//!   notes and states followed, [`crate::activity::LogLine`]); Cancel,
//!   Retry when the failure may be retried, and "Show in Library" once
//!   it is done.
//!
//! Both read the node's promises (`promises.list`, followed by
//! [`crate::activity::Activity`]), so a job an agent or the command line
//! started shows the same way as one started here. The status bar's
//! indicator opens the focus on the job clicked (`activity.show` with
//! `id`); View › Activity and its key open it too.
//!
//! One window is one focus (ROADMAP, "One window is one focus"): the
//! focus opens in an OS window of its own (`activity`), as the
//! Calibration focus does, and comes to the front when it is open
//! already; where the backend has no windows of its own (kittest, the
//! web), in a window of the main one. The window's pass only queues what
//! was asked ([`Event`]); the main window's pass runs it.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use egui::{Context as Egui, Rect, Sense, Vec2, ViewportId};
use egui_pop::api::windows::{WindowId, WindowSpec};
use egui_pop::core::icon::{IconWidget, Source};
use helios_core::Message;
use helios_vault::promises::{Promise, State};
use serde_json::{Value, json};

use super::Shell;
use crate::activity::{LogLine, Logged};
use crate::promise_line::Action;

/// The focus's OS window: its id, under which egui-pop keeps its size.
pub const WINDOW: &str = "activity";

/// The id of the window of the main one the focus is drawn in where the
/// backend has no windows of its own.
const EMBEDDED_ID: &str = "helios.activity.embedded";

const MIN_SIZE: Vec2 = egui::vec2(640.0, 360.0);
const MAX_DEFAULT_SIZE: Vec2 = egui::vec2(1100.0, 720.0);
/// A row's height in the Jobs list.
const ROW: f32 = 44.0;
/// How many failed items the detail lists.
const FAILED_SHOWN: usize = 20;

/// What the window asked of the shell, run at the main window's pass.
#[derive(Clone, Debug, PartialEq)]
enum Event {
    Ask(Action, String),
    /// The main window to the front, its Library shown.
    ShowLibrary,
}

/// Which jobs the list shows, by state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StateFilter {
    #[default]
    All,
    Running,
    Done,
    Failed,
}

impl StateFilter {
    fn keeps(self, s: &State) -> bool {
        match self {
            Self::All => true,
            Self::Running => !s.is_settled(),
            Self::Done => matches!(s, State::Done { .. }),
            Self::Failed => matches!(s, State::Failed { .. } | State::Cancelled),
        }
    }

    fn message(self) -> Message {
        match self {
            Self::All => Message::new("gui-jobs-state-all", "Every state"),
            Self::Running => Message::new("gui-jobs-state-running", "Running"),
            Self::Done => Message::new("gui-jobs-state-done", "Done"),
            Self::Failed => Message::new("gui-jobs-state-failed", "Failed or cancelled"),
        }
    }
}

/// The focus's state, shared by the shell and the window's content.
#[derive(Default)]
pub(crate) struct Jobs {
    open: bool,
    /// The OS window opened for it, not seen closed since.
    shown: Option<WindowId>,
    /// The promises as last read, newest first.
    promises: Vec<Promise>,
    /// The selected job's log.
    log: Vec<LogLine>,
    selected: Option<String>,
    /// The row under the pointer last frame.
    hovered: Option<String>,
    kind: Option<String>,
    state: StateFilter,
    /// The settled jobs "Clear finished" took out of the list.
    cleared: BTreeSet<String>,
    /// The rows drawn last frame, and whether each had a progress bar.
    drawn: Vec<(String, bool)>,
    /// Now, in seconds since the epoch.
    now: u64,
    /// The clock, frozen (pictures of the focus).
    frozen: Option<u64>,
    events: Vec<Event>,
}

/// The focus: its state shared with the window's content.
#[derive(Default)]
pub(super) struct ActivityFocus {
    state: Rc<RefCell<Jobs>>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn text(ui: &egui::Ui, m: Message) -> String {
    crate::i18n::text(ui.ctx(), m)
}

/// A duration in seconds, short: "42 s", "3 min 05 s", "1 h 02 min".
fn span(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min {:02} s", s / 60, s % 60),
        s => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

/// The icon of a kind of job.
fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "import" => "phosphor:tray-arrow-down",
        "export" => "phosphor:export",
        "trash" => "phosphor:trash",
        "backup" => "phosphor:hard-drives",
        "snapshot" => "phosphor:floppy-disk",
        "render" => "phosphor:image",
        _ => "phosphor:pulse",
    }
}

/// How far `done` of `total` is, from 0 to 1, when the job says.
#[allow(clippy::cast_precision_loss)]
fn fraction(state: &State) -> Option<f32> {
    match state {
        State::Running {
            done,
            total: Some(total),
            ..
        } if *total > 0 => Some((*done as f64 / *total as f64).min(1.0) as f32),
        _ => None,
    }
}

/// Where a job reads from, when its request says.
fn source(p: &Promise) -> Option<String> {
    match &p.request["source"] {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Object(o) => o
            .get("label")
            .or_else(|| o.get("id"))
            .and_then(Value::as_str)
            .map(String::from),
        _ => None,
    }
}

/// Who started a job, from its request's `origin`.
fn started_by(p: &Promise) -> Message {
    match p.request["origin"].as_str() {
        Some("user") => Message::new("gui-jobs-by-user", "You"),
        Some("agent") => Message::new("gui-jobs-by-agent", "An agent"),
        Some("cli") => Message::new("gui-jobs-by-cli", "The command line"),
        _ => Message::new("gui-jobs-by-node", "Automatic"),
    }
}

fn state_message(s: &State) -> Message {
    match s {
        State::Pending => Message::new("gui-promise-pending", "Waiting to start"),
        State::Running { .. } => Message::new("gui-jobs-state-running", "Running"),
        State::Done { .. } => Message::new("gui-promise-done", "Done"),
        State::Failed { reason, .. } => Message::new("gui-promise-failed", "Failed: { $reason }")
            .with("reason", reason.as_str()),
        State::Cancelled => Message::new("gui-promise-cancelled", "Cancelled"),
    }
}

fn state_colour(ui: &egui::Ui, s: &State) -> egui::Color32 {
    match s {
        State::Failed { .. } => ui.visuals().error_fg_color,
        State::Cancelled => ui.visuals().weak_text_color(),
        _ => ui.visuals().text_color(),
    }
}

impl Jobs {
    /// The rows the list shows, in its order: running first, then the
    /// rest, newest first.
    fn rows(&self) -> Vec<&Promise> {
        let kept = |p: &&Promise| {
            !(p.state.is_settled() && self.cleared.contains(&p.id))
                && self.kind.as_ref().is_none_or(|k| &p.kind == k)
                && self.state.keeps(&p.state)
        };
        let running = self
            .promises
            .iter()
            .filter(|p| !p.state.is_settled())
            .filter(kept);
        let settled = self
            .promises
            .iter()
            .filter(|p| p.state.is_settled())
            .filter(kept);
        running.chain(settled).collect()
    }

    /// The selected job, the first row's when none is or it is gone.
    fn fix_selection(&mut self) {
        let rows = self.rows();
        let kept = self
            .selected
            .as_ref()
            .is_some_and(|s| rows.iter().any(|p| &p.id == s));
        if !kept {
            self.selected = rows.first().map(|p| p.id.clone());
        }
    }

    fn selected(&self) -> Option<&Promise> {
        let id = self.selected.as_ref()?;
        self.promises.iter().find(|p| &p.id == id)
    }

    fn window_ui(&mut self, ui: &mut egui::Ui) {
        let full = ui.available_rect_before_wrap();
        let left_w = (full.width() * 0.45).clamp(280.0, 520.0);
        let left = Rect::from_min_size(full.min, egui::vec2(left_w, full.height()));
        let right = Rect::from_min_max(egui::pos2(left.max.x + 12.0, full.min.y), full.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
            self.list_ui(ui);
        });
        ui.painter().vline(
            left.max.x + 6.0,
            full.y_range(),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("helios.activity.detail")
                .show(ui, |ui| self.detail_ui(ui));
        });
        ui.allocate_rect(full, Sense::hover());
    }

    /// The Jobs panel: its filters, then one row per job.
    fn list_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading(text(ui, Message::new("gui-jobs-title", "Jobs")));
        ui.horizontal(|ui| {
            let mut kinds: Vec<String> = self.promises.iter().map(|p| p.kind.clone()).collect();
            kinds.sort();
            kinds.dedup();
            let all = text(ui, Message::new("gui-jobs-kind-all", "Every kind"));
            egui::ComboBox::from_id_salt("helios.activity.kind")
                .selected_text(self.kind.clone().unwrap_or_else(|| all.clone()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.kind, None, all);
                    for k in kinds {
                        ui.selectable_value(&mut self.kind, Some(k.clone()), k);
                    }
                });
            egui::ComboBox::from_id_salt("helios.activity.state")
                .selected_text(text(ui, self.state.message()))
                .show_ui(ui, |ui| {
                    for s in [
                        StateFilter::All,
                        StateFilter::Running,
                        StateFilter::Done,
                        StateFilter::Failed,
                    ] {
                        let label = text(ui, s.message());
                        ui.selectable_value(&mut self.state, s, label);
                    }
                });
            let finished: Vec<String> = self
                .promises
                .iter()
                .filter(|p| p.state.is_settled() && !self.cleared.contains(&p.id))
                .map(|p| p.id.clone())
                .collect();
            let clear = text(ui, Message::new("gui-jobs-clear", "Clear finished"));
            if ui
                .add_enabled(!finished.is_empty(), egui::Button::new(clear))
                .clicked()
            {
                self.cleared.extend(finished);
            }
        });
        ui.separator();
        self.fix_selection();
        let rows: Vec<Promise> = self.rows().into_iter().cloned().collect();
        self.drawn.clear();
        let mut hovered = None;
        egui::ScrollArea::vertical()
            .id_salt("helios.activity.jobs")
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.weak(text(
                        ui,
                        Message::new("gui-jobs-none", "No background jobs"),
                    ));
                }
                for p in &rows {
                    if self.row_ui(ui, p) {
                        hovered = Some(p.id.clone());
                    }
                }
            });
        self.hovered = hovered;
    }

    /// One job's row; whether the pointer is over it.
    fn row_ui(&mut self, ui: &mut egui::Ui, p: &Promise) -> bool {
        let selected = self.selected.as_ref() == Some(&p.id);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), Sense::click());
        if response.clicked() {
            self.selected = Some(p.id.clone());
        }
        if selected || response.hovered() {
            let fill = if selected {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().widgets.hovered.weak_bg_fill
            };
            ui.painter().rect_filled(rect, 4.0, fill);
        }
        let bar = fraction(&p.state);
        let actions = selected || self.hovered.as_ref() == Some(&p.id);
        let inner = rect.shrink2(egui::vec2(6.0, 3.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.horizontal(|ui| {
                ui.add(IconWidget::new(Source::named(
                    kind_icon(&p.kind),
                    "activity jobs",
                )));
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(&p.label);
                        if let Some(s) = source(p) {
                            ui.weak(s);
                        }
                    });
                    ui.horizontal(|ui| {
                        self.progress_ui(ui, p, bar);
                        if actions && !p.state.is_settled() && !p.cancel_asked {
                            let cancel = text(ui, Message::new("gui-promise-cancel", "Cancel"));
                            if ui.small_button(cancel).clicked() {
                                self.events.push(Event::Ask(Action::Cancel, p.id.clone()));
                            }
                        }
                    });
                });
            });
        });
        self.drawn.push((p.id.clone(), bar.is_some()));
        response.hovered() || ui.rect_contains_pointer(rect)
    }

    /// A row's progress: a bar when the job says how far it is, else an
    /// activity mark and the time elapsed; its state once settled.
    fn progress_ui(&self, ui: &mut egui::Ui, p: &Promise, bar: Option<f32>) {
        match (&p.state, bar) {
            (State::Running { done, total, .. }, Some(f)) => {
                ui.add(egui::ProgressBar::new(f).desired_width(140.0));
                ui.label(text(
                    ui,
                    Message::new("gui-promise-counts", "{ $done } of { $total }")
                        .with("done", *done)
                        .with("total", total.unwrap_or(0)),
                ));
            }
            (s, _) if !s.is_settled() => {
                ui.spinner();
                ui.weak(span(self.now.saturating_sub(p.created)));
                if p.cancel_asked {
                    ui.weak(text(
                        ui,
                        Message::new("gui-promise-cancelling", "Cancelling…"),
                    ));
                }
            }
            (s, _) => {
                ui.colored_label(state_colour(ui, s), text(ui, state_message(s)));
            }
        }
    }

    /// The Job detail panel, for the job selected.
    #[allow(
        clippy::too_many_lines,
        reason = "the detail's sections in their order"
    )]
    fn detail_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading(text(ui, Message::new("gui-jobs-detail", "Job detail")));
        let Some(p) = self.selected().cloned() else {
            ui.weak(text(
                ui,
                Message::new("gui-jobs-none-selected", "No job selected"),
            ));
            return;
        };
        ui.separator();
        ui.horizontal(|ui| {
            ui.add(IconWidget::new(Source::named(
                kind_icon(&p.kind),
                "activity detail",
            )));
            ui.strong(&p.label);
        });
        let field = |ui: &mut egui::Ui, name: Message, value: String| {
            ui.label(text(ui, name));
            ui.label(value);
            ui.end_row();
        };
        egui::Grid::new("helios.activity.fields")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                field(ui, Message::new("gui-jobs-kind", "Kind"), p.kind.clone());
                if let Some(s) = source(&p) {
                    field(ui, Message::new("gui-jobs-source", "Source"), s);
                }
                let by = text(ui, started_by(&p));
                field(ui, Message::new("gui-jobs-started-by", "Started by"), by);
                let state = text(ui, state_message(&p.state));
                field(ui, Message::new("gui-jobs-state", "State"), state);
                let start = chrono::DateTime::from_timestamp(p.created.cast_signed(), 0)
                    .map(|t| {
                        t.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_default();
                field(ui, Message::new("gui-jobs-started", "Started"), start);
                let end = if p.state.is_settled() {
                    p.updated
                } else {
                    self.now
                };
                let elapsed = end.saturating_sub(p.created);
                field(
                    ui,
                    Message::new("gui-jobs-elapsed", "Elapsed"),
                    span(elapsed),
                );
                if let State::Running {
                    done,
                    total: Some(total),
                    ..
                } = &p.state
                    && *done > 0
                    && total > done
                {
                    let left = elapsed.saturating_mul(total - done) / done;
                    field(ui, Message::new("gui-jobs-left", "Time left"), span(left));
                }
                if p.attempts > 1 {
                    field(
                        ui,
                        Message::new("gui-jobs-attempts", "Attempts"),
                        p.attempts.to_string(),
                    );
                }
            });
        ui.add_space(6.0);
        match &p.state {
            State::Running { done, total, note } => {
                if let Some(f) = fraction(&p.state) {
                    ui.add(egui::ProgressBar::new(f).show_percentage());
                } else {
                    ui.spinner();
                }
                let counts = match total {
                    Some(total) => Message::new("gui-promise-counts", "{ $done } of { $total }")
                        .with("done", *done)
                        .with("total", *total),
                    None => {
                        Message::new("gui-promise-so-far", "{ $done } so far").with("done", *done)
                    }
                };
                ui.label(text(ui, counts));
                if let Some(note) = note {
                    ui.horizontal(|ui| {
                        ui.label(text(ui, Message::new("gui-jobs-step", "Now")));
                        ui.weak(note);
                    });
                }
            }
            State::Done { result } => outcome_ui(ui, result),
            // The state's line above says the rest.
            _ => {}
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if !p.state.is_settled() && !p.cancel_asked {
                let cancel = text(ui, Message::new("gui-promise-cancel", "Cancel"));
                if ui.button(cancel).clicked() {
                    self.events.push(Event::Ask(Action::Cancel, p.id.clone()));
                }
            }
            if matches!(
                p.state,
                State::Failed {
                    retryable: true,
                    ..
                }
            ) && ui
                .button(text(ui, Message::new("gui-promise-retry", "Retry")))
                .clicked()
            {
                self.events.push(Event::Ask(Action::Retry, p.id.clone()));
            }
            if matches!(p.state, State::Done { .. })
                && ui
                    .button(text(
                        ui,
                        Message::new("gui-jobs-show-library", "Show in Library"),
                    ))
                    .clicked()
            {
                self.events.push(Event::ShowLibrary);
            }
        });
        ui.add_space(6.0);
        ui.label(text(ui, Message::new("gui-jobs-log", "Log")));
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if self.log.is_empty() {
                ui.weak("—");
            }
            for line in &self.log {
                let at = chrono::DateTime::from_timestamp(line.at.cast_signed(), 0)
                    .map(|t| {
                        t.with_timezone(&chrono::Local)
                            .format("%H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_default();
                let said = match &line.what {
                    Logged::Note(n) => n.clone(),
                    Logged::State(name, reason) => match (*name, reason) {
                        ("failed", Some(r)) => text(
                            ui,
                            Message::new("gui-promise-failed", "Failed: { $reason }")
                                .with("reason", r.as_str()),
                        ),
                        ("pending", _) => {
                            text(ui, Message::new("gui-promise-pending", "Waiting to start"))
                        }
                        ("running", _) => {
                            text(ui, Message::new("gui-jobs-state-running", "Running"))
                        }
                        ("done", _) => text(ui, Message::new("gui-promise-done", "Done")),
                        ("cancelled", _) => {
                            text(ui, Message::new("gui-promise-cancelled", "Cancelled"))
                        }
                        (other, _) => other.to_string(),
                    },
                };
                ui.horizontal(|ui| {
                    ui.weak(at);
                    ui.monospace(said);
                });
            }
        });
    }
}

/// A finished job's per-item outcome: an import's counts and the files
/// that failed with their reasons.
fn outcome_ui(ui: &mut egui::Ui, result: &Value) {
    let report = &result["report"];
    if report.is_object() {
        let count = |k: &str| report[k].as_u64().unwrap_or(0);
        egui::Grid::new("helios.activity.outcome")
            .num_columns(2)
            .spacing([12.0, 2.0])
            .show(ui, |ui| {
                for (key, m) in [
                    ("added", Message::new("gui-jobs-added", "Added")),
                    ("skipped", Message::new("gui-jobs-skipped", "Skipped")),
                    (
                        "unchanged",
                        Message::new("gui-jobs-unchanged", "Already there"),
                    ),
                    ("failed", Message::new("gui-jobs-items-failed", "Failed")),
                ] {
                    if key == "added" || count(key) > 0 {
                        ui.label(text(ui, m));
                        ui.label(count(key).to_string());
                        ui.end_row();
                    }
                }
            });
    } else {
        ui.label(text(ui, Message::new("gui-promise-done", "Done")));
    }
    let failed = result["failed"].as_array().map_or(&[][..], Vec::as_slice);
    for f in failed.iter().take(FAILED_SHOWN) {
        let path = f["path"].as_str().unwrap_or_default();
        let name = std::path::Path::new(path)
            .file_name()
            .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
        ui.horizontal(|ui| {
            ui.label(name);
            ui.colored_label(
                ui.visuals().error_fg_color,
                f["error"].as_str().unwrap_or_default(),
            );
        });
    }
}

/// The window's title.
fn window_title() -> Message {
    Message::new("gui-jobs-window", "Activity")
}

fn default_size(main: Vec2) -> Vec2 {
    let max = (main - egui::vec2(32.0, 32.0)).max(Vec2::ZERO);
    (main * 0.8).min(MAX_DEFAULT_SIZE).max(MIN_SIZE).min(max)
}

/// Whether the focus's content holds the main window: moved into it when
/// the main window was closed while it was open.
fn holds_main_window(app: &egui_pop::api::app::AppHandle) -> bool {
    app.window_viewport(&WindowId::new(WINDOW)) == Some(ViewportId::ROOT)
}

impl Shell {
    /// `activity.show`: opens the Activity focus, job `id` selected when
    /// given. Open already, its window comes forward.
    pub(super) fn open_activity(&mut self, args: &Value) {
        self.core.activity.acknowledge();
        if let Some(id) = &self.activity_focus.state.borrow().shown {
            self.core.app.handle().raise_window(id);
        }
        let mut j = self.activity_focus.state.borrow_mut();
        j.open = true;
        if let Some(id) = args["id"].as_str() {
            j.selected = Some(id.to_string());
            // The job asked for shows, whatever the filters said.
            j.kind = None;
            j.state = StateFilter::All;
            j.cleared.remove(id);
        }
    }

    /// Shows `promises` in the focus at a frozen time `now`, as if the
    /// node had answered them then: for pictures of the focus.
    pub fn show_activity_at(&mut self, promises: Vec<helios_vault::promises::Promise>, now: u64) {
        self.core.activity.show_promises(promises, now);
        self.activity_focus.state.borrow_mut().frozen = Some(now);
    }

    /// The focus as it is, for tests and agents: open, the job selected,
    /// the rows in their order (each with whether it shows a bar).
    pub fn activity_focus_state(&self) -> Value {
        let j = self.activity_focus.state.borrow();
        let rows: Vec<Value> = j
            .drawn
            .iter()
            .map(|(id, bar)| json!({ "id": id, "bar": bar }))
            .collect();
        json!({ "open": j.open, "selected": j.selected, "rows": rows })
    }

    /// Each frame of the main window: the promises read, what the window
    /// asked run, and its OS window opened or closed as the focus is.
    pub(super) fn activity_frame(&mut self, ctx: &Egui) {
        let events = {
            let mut j = self.activity_focus.state.borrow_mut();
            if j.open {
                j.promises = self.core.activity.promises();
                j.now = j.frozen.unwrap_or_else(now);
                j.log = j
                    .selected
                    .as_ref()
                    .map(|id| self.core.activity.log(id))
                    .unwrap_or_default();
                if j.promises.iter().any(|p| !p.state.is_settled()) {
                    ctx.request_repaint_after(std::time::Duration::from_millis(500));
                }
            }
            std::mem::take(&mut j.events)
        };
        for event in events {
            match event {
                Event::Ask(action, id) => self.core.activity.ask(action, &id),
                Event::ShowLibrary => {
                    let app = self.core.app.handle().clone();
                    app.raise_window(&WindowId::main());
                }
            }
        }
        self.follow_activity_window(ctx);
    }

    /// The focus's OS window, opened while the focus is open and closed
    /// with it; closed by itself, the focus closes and the main window
    /// stays. Where the backend has no windows of its own, a window of
    /// the main one instead.
    fn follow_activity_window(&mut self, ctx: &Egui) {
        if ctx.embed_viewports() {
            self.embedded_activity(ctx);
            return;
        }
        let app = self.core.app.handle().clone();
        let id = WindowId::new(WINDOW);
        let there = app.window_viewport(&id).is_some();
        let mut j = self.activity_focus.state.borrow_mut();
        match (j.open, there) {
            (true, false) if j.shown.is_some() => {
                j.open = false;
                j.shown = None;
            }
            (true, false) => {
                let title = crate::i18n::text(ctx, window_title());
                let spec = WindowSpec::new(WINDOW, title)
                    .size(default_size(ctx.content_rect().size()))
                    .min_size(MIN_SIZE)
                    .shell_bars(true);
                j.shown = Some(id.clone());
                drop(j);
                let state = self.activity_focus.state.clone();
                app.open_window(spec, move |ui, window| {
                    let Ok(mut j) = state.try_borrow_mut() else {
                        return;
                    };
                    let closing = !j.open || window.close_requested();
                    if !closing {
                        egui::CentralPanel::default().show(ui, |ui| j.window_ui(ui));
                        if !j.events.is_empty() {
                            ui.ctx().request_repaint_of(ViewportId::ROOT);
                        }
                        return;
                    }
                    j.open = false;
                    let app = window.app().clone();
                    if holds_main_window(&app) {
                        app.raise_window(&WindowId::main());
                    } else {
                        window.close();
                        super::import_window::hide_closing_window(ui.ctx());
                    }
                });
            }
            (false, true) if holds_main_window(&app) => {
                drop(j);
                app.raise_window(&WindowId::main());
            }
            (false, true) => {
                j.shown = None;
                drop(j);
                app.close_window(&id);
            }
            (false, false) => j.shown = None,
            (true, true) => {}
        }
    }

    /// The focus in a window of the main one (kittest, the web).
    fn embedded_activity(&mut self, ctx: &Egui) {
        let state = self.activity_focus.state.clone();
        let Ok(mut j) = state.try_borrow_mut() else {
            return;
        };
        if !j.open {
            return;
        }
        let title = crate::i18n::text(ctx, window_title());
        let main = ctx.content_rect().size();
        let max = (main - egui::vec2(32.0, 32.0)).max(Vec2::ZERO);
        let mut open = true;
        egui::Window::new(&title)
            .id(egui::Id::unique(EMBEDDED_ID))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(default_size(main))
            .min_size(MIN_SIZE.min(max))
            .max_size(max)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.set_min_size(default_size(main) - egui::vec2(16.0, 40.0));
                j.window_ui(ui);
            });
        if !open {
            j.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn promise(id: &str, state: State) -> Promise {
        Promise {
            id: id.into(),
            owner: format!("job-{id}"),
            kind: "import".into(),
            label: id.into(),
            request: json!({ "origin": "agent" }),
            state,
            cancel_asked: false,
            attempts: 1,
            created: 1,
            updated: 1,
        }
    }

    #[test]
    fn rows_put_running_jobs_first_and_filters_and_clear_keep_their_word() {
        let mut j = Jobs {
            promises: vec![
                promise("p3", State::Done { result: json!({}) }),
                promise(
                    "p2",
                    State::Running {
                        done: 1,
                        total: None,
                        note: None,
                    },
                ),
                promise("p1", State::Cancelled),
            ],
            ..Jobs::default()
        };
        let ids = |j: &Jobs| j.rows().iter().map(|p| p.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&j), ["p2", "p3", "p1"]);
        j.state = StateFilter::Failed;
        assert_eq!(ids(&j), ["p1"]);
        j.state = StateFilter::All;
        j.cleared.extend(["p3".to_string(), "p2".to_string()]);
        // A running job is never cleared.
        assert_eq!(ids(&j), ["p2", "p1"]);
        j.fix_selection();
        assert_eq!(j.selected.as_deref(), Some("p2"));
        assert_eq!(text_of(started_by(&j.promises[0])), "An agent");
    }

    fn text_of(m: Message) -> String {
        m.english
    }

    #[test]
    fn spans_read_short() {
        assert_eq!(span(42), "42 s");
        assert_eq!(span(185), "3 min 05 s");
        assert_eq!(span(3720), "1 h 02 min");
    }
}
