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
//!   notes and states followed, [`crate::activity::LogLine`]); its
//!   console (the lines the job wrote, `helios.job.log`, heard in the journal);
//!   Cancel
//!   (unless the job says it cannot stop; a connected node's job is
//!   cancelled on that node, through its relay), Retry when the failure may be
//!   retried, and once done its result links: "Open folder" (an export, a
//!   backup, a snapshot), "Show in Library" (an import's photos, its
//!   session in the Library; a job naming its photos, a batch's or the
//!   trash's, exactly those: `library.show`) and "Import history" (the import window,
//!   whose History lists the record), and what the job offered (a batch's
//!   actions).
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

use egui::{Context as Egui, Rect, Sense};
use egui_pop::api::windows::WindowId;
use egui_pop::core::icon::{IconWidget, Source};
use egui_pop::core::ui::{Space, TextRole, Tone, UiPop as _};
use helios_core::Message;
use helios_ui::{Progress, Scroll};
use helios_vault::promises::{Promise, State};
use serde_json::{Value, json};

use super::Shell;
use super::focus_window::{FocusWindow, WindowFocus};
use crate::activity::Ask as Action;
use crate::activity::{LogLine, Logged, paused_message};

/// The focus's OS window: its id, under which egui-pop keeps its size.
pub const WINDOW: &str = "activity";

/// The focus's window (`focus_window`).
pub(super) static FRAME: FocusWindow = FocusWindow {
    window: WINDOW,
    embedded: "helios.activity.embedded",
    min_size: egui::vec2(640.0, 360.0),
    max_default_size: egui::vec2(1100.0, 720.0),
    share: 0.8,
};
/// A row's height in the Jobs list: two lines, the second holding a
/// compact row button (Cancel).
const ROW: f32 = 50.0;
/// How many failed items the detail lists.
const FAILED_SHOWN: usize = 20;
/// How many lines of its console a job keeps.
const CONSOLE_LINES: usize = 500;
/// How many jobs' consoles are kept (the oldest forgotten).
const CONSOLES: usize = 100;

/// What the window asked of the shell, run at the main window's pass.
#[derive(Clone, Debug, PartialEq)]
enum Event {
    Ask(Action, String),
    /// A result link: a command with its arguments (the folder opened, the
    /// import window shown), or [`SHOW_IMPORT`]: the main window to the
    /// front, its Library on the import's photos.
    Link(String, Value),
}

/// The pseudo-command of "Show in Library" on an import: its session
/// shown in the Library (`id`: the import history's record), the main
/// window brought forward.
const SHOW_IMPORT: &str = "activity.showImport";

/// The pseudo-command of "Show in Library" on a job naming its photos
/// (`ids`): exactly those shown in the Library ([`crate::library::SHOW_COMMAND`]),
/// the main window brought forward.
const SHOW_PHOTOS: &str = "activity.showPhotos";

/// A finished job's result links: what it says it made (its label, or
/// the words the job said it in; the command it runs and its arguments).
fn links(p: &Promise) -> Vec<(Message, Option<String>, String, Value)> {
    let State::Done { result } = &p.state else {
        return Vec::new();
    };
    let mut links = Vec::new();
    if let Some(history) = result["history"].as_str() {
        links.push((
            Message::new("gui-jobs-show-library", "Show in Library"),
            None,
            SHOW_IMPORT.to_string(),
            json!({ "id": history, "select": true }),
        ));
        links.push((
            Message::new("gui-jobs-import-record", "Import history"),
            None,
            super::import_window::COMMAND.to_string(),
            json!({}),
        ));
    }
    // The photos the job named (a batch's, the trash's): exactly those
    // in the Library.
    if result["history"].is_null()
        && let Some(images) = result["images"].as_array().filter(|i| !i.is_empty())
    {
        links.push((
            Message::new("gui-jobs-show-library", "Show in Library"),
            None,
            SHOW_PHOTOS.to_string(),
            json!({ "ids": images }),
        ));
    }
    if let Some(folder) = result["folder"].as_str() {
        links.push((
            Message::new("gui-jobs-open-folder", "Open folder"),
            None,
            super::export::OPEN_FOLDER.to_string(),
            json!({ "path": folder }),
        ));
    }
    // What the job finished offering (a batch's "Open folder"), in the
    // words it said them.
    for a in result["actions"].as_array().into_iter().flatten() {
        if let (Some(title), Some(command)) = (a["title"].as_str(), a["command"].as_str()) {
            links.push((
                Message::new("gui-jobs-open-folder", "Open folder"),
                Some(title.to_string()),
                command.to_string(),
                a["args"].clone(),
            ));
        }
    }
    links
}

/// Whether a job may be cancelled: its request says when it may not (a
/// snapshot, a job of another node, work with no end).
fn cancellable(p: &Promise) -> bool {
    !p.state.is_settled()
        && !p.cancel_asked
        && (p.request["cancellable"].as_bool() != Some(false) || relayed(p).is_some())
}

/// The job of a connected node a promise mirrors, when it can be
/// cancelled there (`relayed`): its id, `node:<name>/<id>`.
fn relayed(p: &Promise) -> Option<&str> {
    p.request[helios_node_core::job_promises::RELAYED]
        .as_bool()
        .filter(|r| *r)
        .and(p.request[helios_node_core::job_promises::JOB].as_str())
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
    /// Each job's console, by job id: the lines it wrote (`helios.job.log`),
    /// heard in the journal since the shell started.
    consoles: std::collections::BTreeMap<String, Vec<String>>,
    /// The jobs in the order their console started, oldest first.
    console_order: std::collections::VecDeque<String>,
    /// The journal read so far.
    console_seen: egui_pop::api::host::Seq,
    selected: Option<String>,
    /// The row under the pointer last frame.
    hovered: Option<String>,
    kind: Option<String>,
    state: StateFilter,
    /// The settled jobs "Clear finished" took out of the list.
    cleared: BTreeSet<String>,
    /// The rows drawn last frame, and whether each had a progress bar.
    drawn: Vec<(String, bool)>,
    /// The result links run, oldest first (for tests and agents).
    linked: Vec<(String, Value)>,
    /// Now, in seconds since the epoch.
    now: u64,
    /// The clock, frozen (pictures of the focus).
    frozen: Option<u64>,
    events: Vec<Event>,
}

/// The focus: its state shared with the window's content.
#[derive(Default)]
pub(super) struct ActivityFocus {
    pub(super) state: Rc<RefCell<Jobs>>,
}

/// The time zone the focus shows its times in: the machine's, or UTC
/// while its clock is frozen for pictures, so a picture comes out the same
/// on every machine whatever its zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    Local,
    Utc,
}

impl Zone {
    /// Instant `at` (seconds since the epoch) in this zone, as `pattern`
    /// (`chrono` format); empty when out of range.
    fn format(self, at: u64, pattern: &str) -> String {
        match self {
            Self::Local => in_zone(at, &chrono::Local, pattern),
            Self::Utc => in_zone(at, &chrono::Utc, pattern),
        }
    }
}

fn in_zone<Tz>(at: u64, zone: &Tz, pattern: &str) -> String
where
    Tz: chrono::TimeZone,
    Tz::Offset: std::fmt::Display,
{
    chrono::DateTime::from_timestamp(at.cast_signed(), 0)
        .map(|t| t.with_timezone(zone).format(pattern).to_string())
        .unwrap_or_default()
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
        "export" | "batch" => "phosphor:export",
        "trash" => "phosphor:trash",
        "backup" => "phosphor:hard-drives",
        "snapshot" => "phosphor:floppy-disk",
        "render" => "phosphor:image",
        "calibration" => "phosphor:crosshair",
        "health" => "phosphor:heartbeat",
        "map" => "phosphor:map-trifold",
        "thumbs" => "phosphor:images",
        "capture" | "cull" | "workshop-run" => "phosphor:sparkle",
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

/// A job's state, held or not.
fn promise_state_message(p: &Promise) -> Message {
    match &p.paused {
        Some(paused) if !p.state.is_settled() => paused_message(paused),
        _ => state_message(&p.state),
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

impl Jobs {
    /// The zone times are shown in: UTC while the clock is frozen.
    fn zone(&self) -> Zone {
        if self.frozen.is_some() {
            Zone::Utc
        } else {
            Zone::Local
        }
    }

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
            helios_ui::scroll(ui, "helios.activity.detail", Scroll::Region, |ui| {
                self.detail_ui(ui);
            });
        });
        ui.allocate_rect(full, Sense::hover());
    }

    /// The Jobs panel: its filters, then one row per job.
    fn list_ui(&mut self, ui: &mut egui::Ui) {
        ui.text_role(
            TextRole::Title,
            text(ui, Message::new("gui-jobs-title", "Jobs")),
        );
        ui.horizontal(|ui| {
            let mut kinds: Vec<String> = self.promises.iter().map(|p| p.kind.clone()).collect();
            kinds.sort();
            kinds.dedup();
            let all = text(ui, Message::new("gui-jobs-kind-all", "Every kind"));
            // A kind filtered on stays offered once its jobs are gone.
            if let Some(k) = &self.kind
                && !kinds.contains(k)
            {
                kinds.push(k.clone());
            }
            let kinds: Vec<(Option<String>, String)> = std::iter::once((None, all))
                .chain(kinds.into_iter().map(|k| (Some(k.clone()), k)))
                .collect();
            let name = text(ui, Message::new("gui-jobs-filter-kind", "Kind shown"));
            helios_ui::choice(ui, "helios.activity.kind", &name, &mut self.kind, &kinds);
            let states = [
                StateFilter::All,
                StateFilter::Running,
                StateFilter::Done,
                StateFilter::Failed,
            ]
            .map(|s| (s, text(ui, s.message())));
            let name = text(ui, Message::new("gui-jobs-filter-state", "State shown"));
            helios_ui::choice(ui, "helios.activity.state", &name, &mut self.state, &states);
            let finished: Vec<String> = self
                .promises
                .iter()
                .filter(|p| p.state.is_settled() && !self.cleared.contains(&p.id))
                .map(|p| p.id.clone())
                .collect();
            let clear = text(ui, Message::new("gui-jobs-clear", "Clear finished"));
            let cleared = ui
                .add_enabled_ui(!finished.is_empty(), |ui| {
                    helios_ui::action(ui, helios_ui::Action::Secondary, &clear).clicked()
                })
                .inner;
            if cleared {
                self.cleared.extend(finished);
            }
        });
        ui.divider();
        self.fix_selection();
        let rows: Vec<Promise> = self.rows().into_iter().cloned().collect();
        self.drawn.clear();
        let mut hovered = None;
        helios_ui::scroll(ui, "helios.activity.jobs", Scroll::Region, |ui| {
            if rows.is_empty() {
                helios_ui::empty(ui, helios_ui::Empty::NoJobs);
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
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, selected, &p.label)
        });
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
                        ui.text_role(TextRole::Body, &p.label);
                        if let Some(s) = source(p) {
                            ui.muted(s);
                        }
                    });
                    ui.horizontal(|ui| {
                        self.progress_ui(ui, p, bar);
                        if actions && cancellable(p) {
                            let cancel = text(ui, Message::new("gui-promise-cancel", "Cancel"));
                            if helios_ui::row_action(ui, helios_ui::Action::Secondary, &cancel)
                                .clicked()
                            {
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
                helios_ui::progress(ui, Progress::Row, &p.label, Some(f));
                ui.text_role(
                    TextRole::Body,
                    text(
                        ui,
                        Message::new("gui-promise-counts", "{ $done } of { $total }")
                            .with("done", *done)
                            .with("total", total.unwrap_or(0)),
                    ),
                );
            }
            (s, _) if !s.is_settled() => {
                helios_ui::progress(ui, Progress::Row, &p.label, None);
                ui.muted(span(self.now.saturating_sub(p.created)));
                if p.cancel_asked {
                    ui.muted(text(
                        ui,
                        Message::new("gui-promise-cancelling", "Cancelling…"),
                    ));
                }
            }
            (s @ State::Failed { .. }, _) => {
                ui.text_tone(Tone::Danger, text(ui, state_message(s)));
            }
            (State::Cancelled, _) => {
                ui.muted(text(ui, state_message(&p.state)));
            }
            (s, _) => {
                ui.text_role(TextRole::Body, text(ui, state_message(s)));
            }
        }
        // Held: why, beside its progress.
        if let Some(paused) = &p.paused
            && !p.state.is_settled()
        {
            ui.text_tone(Tone::Warning, text(ui, paused_message(paused)));
        }
    }

    /// Folds the journal's `helios.job.log` lines since the last call into the
    /// jobs' consoles.
    fn hear_consoles(&mut self, app: &egui_pop::api::AppHandle) {
        for e in app.events_since(self.console_seen) {
            self.console_seen = e.seq;
            if e.name != helios_core::jobs::LOG {
                continue;
            }
            let (Some(id), Some(line)) = (e.data["id"].as_str(), e.data["text"].as_str()) else {
                continue;
            };
            self.console_line(id, line);
        }
    }

    /// Adds `line` to job `id`'s console.
    fn console_line(&mut self, id: &str, line: &str) {
        if !self.consoles.contains_key(id) {
            self.console_order.push_back(id.to_string());
            while self.console_order.len() > CONSOLES {
                if let Some(old) = self.console_order.pop_front() {
                    self.consoles.remove(&old);
                }
            }
        }
        let lines = self.consoles.entry(id.to_string()).or_default();
        lines.push(line.to_string());
        if lines.len() > CONSOLE_LINES {
            lines.remove(0);
        }
    }

    /// The console of the job promise `p` mirrors (`job` in its request).
    fn console(&self, p: &Promise) -> &[String] {
        p.request[helios_node_core::job_promises::JOB]
            .as_str()
            .and_then(|job| self.consoles.get(job))
            .map_or(&[], Vec::as_slice)
    }

    /// The Job detail panel, for the job selected.
    #[allow(
        clippy::too_many_lines,
        reason = "the detail's sections in their order"
    )]
    fn detail_ui(&mut self, ui: &mut egui::Ui) {
        ui.text_role(
            TextRole::Title,
            text(ui, Message::new("gui-jobs-detail", "Job detail")),
        );
        let Some(p) = self.selected().cloned() else {
            ui.muted(text(
                ui,
                Message::new("gui-jobs-none-selected", "No job selected"),
            ));
            return;
        };
        ui.divider();
        ui.horizontal(|ui| {
            ui.add(IconWidget::new(Source::named(
                kind_icon(&p.kind),
                "activity detail",
            )));
            ui.section_title(&p.label);
        });
        let mut fields: Vec<(String, String)> = Vec::new();
        let mut field = |name: Message, value: String| fields.push((text(ui, name), value));
        field(Message::new("gui-jobs-kind", "Kind"), p.kind.clone());
        if let Some(s) = source(&p) {
            field(Message::new("gui-jobs-source", "Source"), s);
        }
        let by = text(ui, started_by(&p));
        field(Message::new("gui-jobs-started-by", "Started by"), by);
        let state = text(ui, promise_state_message(&p));
        field(Message::new("gui-jobs-state", "State"), state);
        if let Some(until) = p.paused.as_ref().and_then(|w| w.until)
            && !p.state.is_settled()
        {
            let at = self.zone().format(until, "%H:%M:%S");
            field(Message::new("gui-jobs-goes-on", "Goes on at"), at);
        }
        let start = self.zone().format(p.created, "%Y-%m-%d %H:%M:%S");
        field(Message::new("gui-jobs-started", "Started"), start);
        let end = if p.state.is_settled() {
            p.updated
        } else {
            self.now
        };
        let elapsed = end.saturating_sub(p.created);
        field(Message::new("gui-jobs-elapsed", "Elapsed"), span(elapsed));
        if let State::Running {
            done,
            total: Some(total),
            ..
        } = &p.state
            && *done > 0
            && total > done
        {
            let left = elapsed.saturating_mul(total - done) / done;
            field(Message::new("gui-jobs-left", "Time left"), span(left));
        }
        if p.attempts > 1 {
            field(
                Message::new("gui-jobs-attempts", "Attempts"),
                p.attempts.to_string(),
            );
        }
        helios_ui::details(ui, "helios.activity.fields", &fields);
        ui.space_role(Space::Group);
        match &p.state {
            State::Running { done, total, note } => {
                helios_ui::progress(ui, Progress::Block, &p.label, fraction(&p.state));
                let counts = match total {
                    Some(total) => Message::new("gui-promise-counts", "{ $done } of { $total }")
                        .with("done", *done)
                        .with("total", *total),
                    None => {
                        Message::new("gui-promise-so-far", "{ $done } so far").with("done", *done)
                    }
                };
                ui.text_role(TextRole::Body, text(ui, counts));
                if let Some(note) = note {
                    ui.horizontal(|ui| {
                        ui.text_role(
                            TextRole::Body,
                            text(ui, Message::new("gui-jobs-step", "Now")),
                        );
                        ui.muted(note);
                    });
                }
            }
            State::Done { result } => outcome_ui(ui, result),
            // The state's line above says the rest.
            _ => {}
        }
        ui.space_role(Space::Group);
        ui.horizontal(|ui| {
            let secondary = helios_ui::Action::Secondary;
            if p.paused.is_some() && !p.state.is_settled() {
                let resume = text(ui, Message::new("gui-promise-resume", "Resume"));
                if helios_ui::action(ui, helios_ui::Action::Primary, &resume).clicked() {
                    self.events.push(Event::Ask(Action::Retry, p.id.clone()));
                }
            }
            if cancellable(&p) {
                let cancel = text(ui, Message::new("gui-promise-cancel", "Cancel"));
                if helios_ui::action(ui, secondary, &cancel).clicked() {
                    self.events.push(Event::Ask(Action::Cancel, p.id.clone()));
                }
            }
            if matches!(
                p.state,
                State::Failed {
                    retryable: true,
                    ..
                }
            ) {
                let retry = text(ui, Message::new("gui-promise-retry", "Retry"));
                if helios_ui::action(ui, secondary, &retry).clicked() {
                    self.events.push(Event::Ask(Action::Retry, p.id.clone()));
                }
            }
            for (label, said, command, args) in links(&p) {
                let label = said.unwrap_or_else(|| text(ui, label));
                if helios_ui::action(ui, secondary, &label).clicked() {
                    self.events.push(Event::Link(command, args));
                }
            }
        });
        ui.space_role(Space::Group);
        ui.text_role(
            TextRole::Body,
            text(ui, Message::new("gui-jobs-log", "Log")),
        );
        ui.card(Tone::Neutral, |ui| {
            ui.set_min_width(ui.available_width());
            if self.log.is_empty() {
                ui.muted("—");
            }
            for line in &self.log {
                let at = self.zone().format(line.at, "%H:%M:%S");
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
                    ui.meta(at);
                    ui.code_text(said);
                });
            }
        });
        // What the job wrote as it went (its `helios.job.log` lines): its
        // console, newest at the bottom.
        let console = self.console(&p);
        if !console.is_empty() {
            ui.space_role(Space::Group);
            ui.text_role(
                TextRole::Body,
                text(ui, Message::new("gui-jobs-console", "Console")),
            );
            ui.card(Tone::Neutral, |ui| {
                ui.set_min_width(ui.available_width());
                helios_ui::scroll(ui, "helios.activity.console", Scroll::Tail, |ui| {
                    for line in console {
                        ui.code_text(line);
                    }
                });
            });
        }
    }
}

/// A finished job's per-item outcome: an import's counts and the files
/// that failed with their reasons.
fn outcome_ui(ui: &mut egui::Ui, result: &Value) {
    let report = &result["report"];
    let mut counts: Vec<(String, String)> = Vec::new();
    if report.is_object() {
        let count = |k: &str| report[k].as_u64().unwrap_or(0);
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
                counts.push((text(ui, m), count(key).to_string()));
            }
        }
    } else if let Some(done) = result["done"].as_u64() {
        // A job of the desktop's, mirrored: how many it did, skipped and
        // failed.
        counts.push((
            text(ui, Message::new("gui-jobs-items-done", "Done")),
            match result["total"].as_u64() {
                Some(total) => format!("{done}/{total}"),
                None => done.to_string(),
            },
        ));
        let skipped = result["skipped"].as_u64().unwrap_or(0);
        if skipped > 0 {
            counts.push((
                text(ui, Message::new("gui-jobs-skipped", "Skipped")),
                skipped.to_string(),
            ));
        }
        let failed = result["failed"].as_array().map_or(0, Vec::len);
        if failed > 0 {
            counts.push((
                text(ui, Message::new("gui-jobs-items-failed", "Failed")),
                failed.to_string(),
            ));
        }
    } else {
        ui.text_role(
            TextRole::Body,
            text(ui, Message::new("gui-promise-done", "Done")),
        );
    }
    if !counts.is_empty() {
        helios_ui::details(ui, "helios.activity.outcome", &counts);
    }
    if let Some(folder) = result["folder"].as_str() {
        ui.horizontal(|ui| {
            ui.text_role(
                TextRole::Body,
                text(ui, Message::new("gui-jobs-folder", "Folder")),
            );
            ui.muted(folder);
        });
    }
    let failed = result["failed"].as_array().map_or(&[][..], Vec::as_slice);
    for f in failed.iter().take(FAILED_SHOWN) {
        let path = f["path"].as_str().unwrap_or_default();
        let name = std::path::Path::new(path)
            .file_name()
            .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
        ui.horizontal(|ui| {
            ui.text_role(TextRole::Body, name);
            ui.text_tone(Tone::Danger, f["error"].as_str().unwrap_or_default());
        });
    }
}

/// The window's title.
fn window_title() -> Message {
    Message::new("gui-jobs-window", "Activity")
}

impl WindowFocus for Jobs {
    fn is_open(&self) -> bool {
        self.open
    }

    fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    fn shown(&mut self) -> &mut Option<WindowId> {
        &mut self.shown
    }

    fn title(&self, ctx: &Egui) -> String {
        crate::i18n::text(ctx, window_title())
    }

    fn window_ui(&mut self, ui: &mut egui::Ui) {
        Jobs::window_ui(self, ui);
    }

    fn asks_main_pass(&self) -> bool {
        !self.events.is_empty()
    }
}

impl Shell {
    /// `activity.show`: opens the Activity focus, job `id` selected when
    /// given. Open already, its window comes forward.
    pub(super) fn open_activity(&mut self, args: &Value) {
        self.core.activity.acknowledge();
        super::focus_window::open(self.core.app.handle(), &self.activity_focus.state);
        let mut j = self.activity_focus.state.borrow_mut();
        if let Some(id) = args["id"].as_str() {
            j.selected = Some(id.to_string());
            // The job asked for shows, whatever the filters said.
            j.kind = None;
            j.state = StateFilter::All;
            j.cleared.remove(id);
        }
    }

    /// `activity.resume`: held promise `id` goes on now (`promises.retry`
    /// on the node).
    pub(super) fn resume_activity(&mut self, args: &Value) -> Result<Value, String> {
        let id = args["id"]
            .as_str()
            .ok_or("`id`: the held promise to resume")?;
        if !self.core.activity.has_node() {
            return Err("no library open: its tasks run on its node".into());
        }
        self.core.activity.ask(Action::Retry, id);
        Ok(Value::Null)
    }

    /// Cancel or Retry asked on promise `id`: of the node's promises; a job
    /// relayed from a connected node is cancelled there, through its relay
    /// (`job.cancel` to its control endpoint).
    fn ask_activity(&mut self, action: Action, id: &str) {
        let relayed = {
            let j = self.activity_focus.state.borrow();
            j.promises
                .iter()
                .find(|p| p.id == id)
                .and_then(relayed)
                .map(String::from)
        };
        match (action, relayed) {
            (Action::Cancel, Some(job)) => {
                let cancelled = crate::relay::split_node_job(&job)
                    .ok_or_else(|| format!("`{job}` is not a node's job"))
                    .and_then(|(node, own)| {
                        self.core
                            .relays
                            .borrow()
                            .get(node)
                            .ok_or_else(|| format!("node `{node}` is not followed any more"))?
                            .cancel(own)
                    });
                if let Err(e) = cancelled {
                    self.warn(format!("{job}: {e}"));
                }
            }
            _ => self.core.activity.ask(action, id),
        }
    }

    /// Runs a result link of the detail: the import's session shown in
    /// the Library, the main window brought forward; or the link's
    /// command.
    fn run_link(&mut self, command: &str, args: &Value) {
        let shown = match command {
            SHOW_IMPORT => Some(crate::library::IMPORT_SESSION_COMMAND),
            SHOW_PHOTOS => Some(crate::library::SHOW_COMMAND),
            _ => None,
        };
        let ran = if let Some(shown) = shown {
            let ran = self.run(shown, args.clone(), helios_shell::commands::Origin::User);
            self.core.app.handle().raise_window(&WindowId::main());
            ran
        } else {
            self.run(command, args.clone(), helios_shell::commands::Origin::User)
        };
        if let Err(e) = ran {
            self.warn(format!("{command}: {e}"));
        }
        self.activity_focus
            .state
            .borrow_mut()
            .linked
            .push((command.to_string(), args.clone()));
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
        let links: Vec<Value> = j
            .selected()
            .map(links)
            .unwrap_or_default()
            .into_iter()
            .map(|(_, _, command, args)| json!({ "command": command, "args": args }))
            .collect();
        let console = j
            .selected()
            .map(|p| j.console(p).to_vec())
            .unwrap_or_default();
        json!({ "open": j.open, "selected": j.selected, "rows": rows, "links": links,
                "console": console,
                "linked": j.linked.iter().map(|(c, a)| json!({ "command": c, "args": a }))
                    .collect::<Vec<_>>() })
    }

    /// Each frame of the main window: the promises read, what the window
    /// asked run, and its OS window opened or closed as the focus is.
    pub(super) fn activity_frame(&mut self, ctx: &Egui) {
        let events = {
            let mut j = self.activity_focus.state.borrow_mut();
            j.hear_consoles(self.core.app.handle());
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
                Event::Ask(action, id) => self.ask_activity(action, &id),
                Event::Link(command, args) => self.run_link(&command, &args),
            }
        }
        let app = self.core.app.handle().clone();
        FRAME.follow(&app, ctx, &self.activity_focus.state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_instant_reads_in_each_zone_and_a_frozen_clock_reads_in_utc() {
        // 2025-11-14 22:33:20 UTC.
        let at = 1_763_159_600;
        let paris = chrono::FixedOffset::east_opt(3600).unwrap();
        assert_eq!(in_zone(at, &chrono::Utc, "%H:%M:%S"), "22:33:20");
        assert_eq!(
            in_zone(at, &paris, "%Y-%m-%d %H:%M:%S"),
            "2025-11-14 23:33:20"
        );
        assert_eq!(Zone::Utc.format(at, "%H:%M:%S"), "22:33:20");
        let mut jobs = Jobs::default();
        assert_eq!(jobs.zone(), Zone::Local);
        jobs.frozen = Some(at);
        assert_eq!(jobs.zone(), Zone::Utc);
    }

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
            paused: None,
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
