//! Background activity on the desktop: the node's promises (see
//! `docs/protocol.md`, "Promises"), followed off the interface's thread,
//! and the status bar's indicator of them.
//!
//! - **Followed.** A thread of its own reads `promises.list` from the
//!   desktop's node: often while one runs, seldom when none does. Every
//!   client of the node starts them (the import window, the CLI, an
//!   agent), so they all show here.
//! - **Every kind of job**: the node mirrors the desktop's jobs (exports,
//!   the trash, backups, snapshots…) as promises
//!   (`helios_node_core::job_promises`), so they show here beside the
//!   imports; work with no end (the image map, the thumbnail re-check)
//!   shows in the focus without counting as running ([`runs`]).
//! - **The indicator** ([`indicator`]): while promises run, a spinner,
//!   their count and the main one's progress ("Importing 120/412"); a
//!   promise that failed during this run, a warning badge until it is
//!   seen. A click runs [`COMMAND`] with the job clicked (the main one,
//!   or the newest failure): the failures are marked seen and the
//!   Activity focus opens with that job selected (`crate::shell`'s
//!   `activity_focus`: every kind of task in one place).
//! - **Each job's log**: the notes its progress gave (the file at hand)
//!   and the states it went through, as followed here ([`LogLine`]).
//! - **Started here.** [`Activity::start`] asks the node for an import by
//!   rule and answers with its promise at once: the window does not wait
//!   for the import, and may close while it runs.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use egui_pop::api::AppHandle;
use helios_core::Message;
use helios_inspect::client::Client;
use helios_vault::promises::{Promise, State};
use serde_json::{Value, json};

/// The command the indicator runs: the Activity focus shown (`id`: the
/// job selected), the failures marked seen.
pub const COMMAND: &str = "activity.show";
/// The indicator of the running tasks, for tests and agents.
pub const MARK: &str = "helios.status.activity";
/// The badge of the failed tasks.
pub const FAILED_MARK: &str = "helios.status.activity.failed";

/// How many of the newest promises are followed.
const FOLLOWED: usize = 50;
/// How often they are read while one runs.
const BUSY_EVERY: Duration = Duration::from_millis(150);
/// How often they are read when none runs.
const IDLE_EVERY: Duration = Duration::from_secs(1);
/// The step of the follower's sleep: a start asked reads at once.
const TICK: Duration = Duration::from_millis(50);
/// How many imports the import window lists.
const IMPORTS_SHOWN: usize = 4;

/// How many log lines a job keeps.
const LOG_LINES: usize = 200;

/// A line of a job's log, as followed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    /// When it was heard, in seconds since the epoch.
    pub at: u64,
    pub what: Logged,
}

/// What a log line says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Logged {
    /// The job went into this state (`State::name`), with its failure's
    /// reason.
    State(&'static str, Option<String>),
    /// Its progress's note: the file at hand.
    Note(String),
}

#[derive(Default)]
struct Shared {
    promises: Vec<Promise>,
    /// Each job's log, by promise id.
    logs: BTreeMap<String, Vec<LogLine>>,
    ctx: Option<egui::Context>,
}

impl Shared {
    /// The log lines `promises` add to what was heard, at `at`.
    fn log(&mut self, promises: &[Promise], at: u64) {
        for p in promises {
            let log = self.logs.entry(p.id.clone()).or_default();
            let name = p.state.name();
            let last_state = log.iter().rev().find_map(|l| match &l.what {
                Logged::State(s, _) => Some(*s),
                Logged::Note(_) => None,
            });
            if last_state != Some(name) {
                let reason = match &p.state {
                    State::Failed { reason, .. } => Some(reason.clone()),
                    _ => None,
                };
                log.push(LogLine {
                    at,
                    what: Logged::State(name, reason),
                });
            }
            if let State::Running {
                note: Some(note), ..
            } = &p.state
            {
                let last_note = log.iter().rev().find_map(|l| match &l.what {
                    Logged::Note(n) => Some(n.as_str()),
                    Logged::State(..) => None,
                });
                if last_note != Some(note.as_str()) {
                    log.push(LogLine {
                        at,
                        what: Logged::Note(note.clone()),
                    });
                }
            }
            if log.len() > LOG_LINES {
                log.drain(..log.len() - LOG_LINES);
            }
        }
        self.logs
            .retain(|id, _| promises.iter().any(|p| &p.id == id));
    }
}

/// The promises the desktop follows.
pub struct Activity {
    node: Option<Arc<Client>>,
    shared: Arc<Mutex<Shared>>,
    kick: Arc<AtomicBool>,
    /// When this run began: older failures are not this run's news.
    since: u64,
    /// The failures seen (the indicator clicked).
    seen: BTreeSet<String>,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            node: None,
            shared: Arc::default(),
            kick: Arc::default(),
            since: now(),
            seen: BTreeSet::new(),
        }
    }
}

/// What the status bar shows of the activity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    /// The promises running, the oldest (the main one) first.
    pub running: Vec<Promise>,
    /// The failures of this run not seen yet.
    pub failed: usize,
    /// The newest of them, which the badge's click selects.
    pub newest_failed: Option<String>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Activity {
    /// Follows the promises of `node` from now on, on a thread of its
    /// own (it ends with the activity).
    pub fn follow(&mut self, node: Arc<Client>) {
        self.node = Some(node.clone());
        let (shared, kick) = (Arc::downgrade(&self.shared), self.kick.clone());
        let followed = std::thread::Builder::new()
            .name("helios-activity".into())
            .spawn(move || follow(&node, &shared, &kick));
        if let Err(e) = followed {
            log::warn!(target: "helios", "the background tasks are not followed: {e}");
        }
    }

    /// Whether imports go through the node's promises.
    pub fn has_node(&self) -> bool {
        self.node.is_some()
    }

    /// The context repainted when the promises change.
    pub fn set_context(&self, ctx: &egui::Context) {
        let mut shared = lock(&self.shared);
        if shared.ctx.is_none() {
            shared.ctx = Some(ctx.clone());
        }
    }

    /// The promises last read, newest first.
    pub fn promises(&self) -> Vec<Promise> {
        lock(&self.shared).promises.clone()
    }

    /// Shows `promises` as if the node had answered them, their log
    /// lines heard at `at`: for pictures of the screens that show them,
    /// on promises of known times.
    pub fn show_promises(&self, promises: Vec<Promise>, at: u64) {
        let mut shared = lock(&self.shared);
        shared.log(&promises, at);
        shared.promises = promises;
    }

    /// The log of promise `id`, as followed, oldest line first.
    pub fn log(&self, id: &str) -> Vec<LogLine> {
        lock(&self.shared).logs.get(id).cloned().unwrap_or_default()
    }

    /// Asks the node for an import by rule (`library.import`'s params,
    /// `source` and `rule`): its promise, answered at once.
    ///
    /// # Errors
    /// No node, or the node refused the import (its choices, a source
    /// gone).
    pub fn start(&self, params: Value) -> Result<Promise, String> {
        let node = self
            .node
            .as_ref()
            .ok_or("no library open: imports run on its node")?;
        let mut answer = node
            .request("library.import", params)
            .map_err(|e| e.message)?;
        let promise: Promise =
            serde_json::from_value(answer["promise"].take()).map_err(|e| e.to_string())?;
        {
            let mut shared = lock(&self.shared);
            shared.promises.retain(|p| p.id != promise.id);
            shared.promises.insert(0, promise.clone());
            let promises = shared.promises.clone();
            shared.log(&promises, now());
        }
        self.kick.store(true, Ordering::SeqCst);
        Ok(promise)
    }

    /// Asks the node to cancel (`promises.cancel`) or retry
    /// (`promises.retry`) promise `id`, on a thread: the answer shows as
    /// the promise is read again.
    pub fn ask(&self, action: crate::promise_line::Action, id: &str) {
        let Some(node) = self.node.clone() else {
            return;
        };
        let method = match action {
            crate::promise_line::Action::Cancel => "promises.cancel",
            crate::promise_line::Action::Retry => "promises.retry",
        };
        let (id, kick) = (id.to_string(), self.kick.clone());
        let asked = std::thread::Builder::new()
            .name("helios-activity-ask".into())
            .spawn(move || {
                if let Err(e) = node.request(method, json!({ "id": id })) {
                    log::warn!(target: "helios", "{method} {id}: {}", e.message);
                }
                kick.store(true, Ordering::SeqCst);
            });
        if let Err(e) = asked {
            log::warn!(target: "helios", "{method}: {e}");
        }
    }

    fn unseen_failure(&self, p: &Promise) -> bool {
        matches!(p.state, State::Failed { .. })
            && p.updated >= self.since
            && !self.seen.contains(&p.id)
    }

    /// What the status bar shows; `None` when nothing runs and nothing
    /// failed unseen.
    pub fn summary(&self) -> Option<Summary> {
        let shared = lock(&self.shared);
        let mut running: Vec<Promise> = shared
            .promises
            .iter()
            .filter(|p| runs(p))
            .cloned()
            .collect();
        running.reverse();
        let mut unseen = shared.promises.iter().filter(|p| self.unseen_failure(p));
        let newest_failed = unseen.next().map(|p| p.id.clone());
        let failed = usize::from(newest_failed.is_some()) + unseen.count();
        (!running.is_empty() || failed > 0).then_some(Summary {
            running,
            failed,
            newest_failed,
        })
    }

    /// Marks the failures shown so far seen: the badge goes.
    pub fn acknowledge(&mut self) {
        let failed: Vec<String> = lock(&self.shared)
            .promises
            .iter()
            .filter(|p| matches!(p.state, State::Failed { .. }))
            .map(|p| p.id.clone())
            .collect();
        self.seen.extend(failed);
    }

    /// The imports the import window lists: those running, and those
    /// that settled during this run, newest first.
    pub fn imports(&self) -> Vec<Promise> {
        lock(&self.shared)
            .promises
            .iter()
            .filter(|p| p.kind == "import" && (!p.state.is_settled() || p.updated >= self.since))
            .take(IMPORTS_SHOWN)
            .cloned()
            .collect()
    }
}

/// Whether `p` runs, as the status bar counts it: work with no end (the
/// image map, the thumbnail re-check) shows in the Activity focus, but is
/// not a task under way.
pub fn runs(p: &Promise) -> bool {
    !p.state.is_settled() && !helios_node_core::job_promises::is_standing(&p.request)
}

/// Reads the promises of `node` until the activity is gone.
fn follow(node: &Client, shared: &Weak<Mutex<Shared>>, kick: &AtomicBool) {
    loop {
        let read = node
            .request("promises.list", json!({ "limit": FOLLOWED }))
            .map_err(|e| e.message)
            .and_then(|mut answer| {
                serde_json::from_value::<Vec<Promise>>(answer["promises"].take())
                    .map_err(|e| e.to_string())
            });
        let Some(held) = shared.upgrade() else {
            return;
        };
        let busy = match read {
            Ok(promises) => {
                let mut shared = lock(&held);
                let busy = promises.iter().any(runs);
                if shared.promises != promises {
                    shared.log(&promises, now());
                    shared.promises = promises;
                    if let Some(ctx) = &shared.ctx {
                        ctx.request_repaint();
                    }
                }
                busy
            }
            Err(e) => {
                log::debug!(target: "helios", "promises.list: {e}");
                false
            }
        };
        drop(held);
        let every = if busy { BUSY_EVERY } else { IDLE_EVERY };
        let mut slept = Duration::ZERO;
        while slept < every {
            std::thread::sleep(TICK);
            slept += TICK;
            if kick.swap(false, Ordering::SeqCst) {
                break;
            }
        }
    }
}

/// How far a promise is, in words: "Importing 120/412" for an import,
/// else its label and percent.
pub fn progress_text(ctx: &egui::Context, p: &Promise) -> String {
    let text = |m: Message| crate::i18n::text(ctx, m);
    let (done, total) = match &p.state {
        State::Running { done, total, .. } => (*done, *total),
        _ => (0, None),
    };
    match (p.kind.as_str(), total) {
        ("import", Some(total)) => text(
            Message::new("gui-activity-importing", "Importing { $done }/{ $total }")
                .with("done", done)
                .with("total", total),
        ),
        ("import", None) => text(Message::new("gui-activity-importing-start", "Importing…")),
        (_, Some(total)) if total > 0 => text(
            Message::new("gui-activity-percent", "{ $label } { $percent } %")
                .with("label", p.label.as_str())
                .with("percent", done.saturating_mul(100) / total),
        ),
        _ => p.label.clone(),
    }
}

/// The status bar's indicator of `summary`: a spinner, the count of the
/// tasks running and the main one's progress; the failures' badge. A
/// click runs [`COMMAND`] on the job clicked: the main one, or the
/// newest failure for the badge.
pub fn indicator(ui: &mut egui::Ui, app: &AppHandle, summary: &Summary) {
    use egui_pop::core::indicator::{Indicator, Tone};
    let ctx = ui.ctx().clone();
    let text = |m: Message| crate::i18n::text(&ctx, m);
    let action = text(Message::new(
        "gui-activity-show",
        "Show the background tasks",
    ));
    let mut clicked = None;
    if let Some(main) = summary.running.first() {
        ui.spinner();
        let list = summary
            .running
            .iter()
            .map(|p| format!("{}: {}", p.label, progress_text(&ctx, p)))
            .collect::<Vec<_>>()
            .join("\n");
        let meaning = text(
            Message::new(
                "gui-activity-running",
                "{ $count } background tasks running",
            )
            .with("count", summary.running.len()),
        );
        let r = ui.add(
            Indicator::count(summary.running.len() as u64)
                .tone(Tone::Accent)
                .id(MARK)
                .name(meaning.clone())
                .tooltip(format!("{meaning}\n{list}"))
                .interactive(action.clone()),
        );
        let label = ui
            .add(egui::Label::new(progress_text(&ctx, main)).sense(egui::Sense::click()))
            .on_hover_text(list);
        if r.clicked() || label.clicked() {
            clicked = Some(main.id.clone());
        }
    }
    if summary.failed > 0 {
        if !summary.running.is_empty() {
            ui.separator();
        }
        let meaning = text(
            Message::new("gui-activity-failed", "{ $count } background tasks failed")
                .with("count", summary.failed),
        );
        ui.colored_label(ui.visuals().warn_fg_color, "⚠");
        let r = ui.add(
            Indicator::count(summary.failed as u64)
                .tone(Tone::Warning)
                .id(FAILED_MARK)
                .name(meaning.clone())
                .tooltip(meaning)
                .interactive(action),
        );
        if r.clicked() {
            clicked.clone_from(&summary.newest_failed);
        }
    }
    if let Some(id) = clicked {
        app.run(
            egui_pop::model::ids::CommandId::from_static(COMMAND),
            json!({ "id": id }),
        );
    }
}
