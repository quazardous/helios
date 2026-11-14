//! Background activity on the desktop: the node's promises (see
//! `docs/protocol.md`, "Promises"), followed off the interface's thread,
//! and the status bar's indicator of them.
//!
//! - **Followed.** A thread of its own reads `promises.list` from the
//!   desktop's node: often while one runs, seldom when none does. Every
//!   client of the node starts them (the import window, the CLI, an
//!   agent), so they all show here.
//! - **The indicator** ([`indicator`]): while promises run, a spinner,
//!   their count and the main one's progress ("Importing 120/412"); a
//!   promise that failed during this run, a warning badge until it is
//!   seen. A click runs [`COMMAND`], which marks the failures seen and
//!   opens the import window, whose footer lists the imports with their
//!   progress, outcome, Cancel and Retry. The Activity window, every kind
//!   of task in one place, will open from it instead.
//! - **Started here.** [`Activity::start`] asks the node for an import by
//!   rule and answers with its promise at once: the window does not wait
//!   for the import, and may close while it runs.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use egui_pop::api::AppHandle;
use helios_core::Message;
use helios_inspect::client::Client;
use helios_vault::promises::{Promise, State};
use serde_json::{Value, json};

/// The command the indicator runs: the background tasks shown, their
/// failures marked seen.
pub const COMMAND: &str = "activity.show";
/// The indicator of the running tasks, for tests and agents.
pub const MARK: &str = "helios.status.activity";
/// The badge of the failed tasks.
pub const FAILED_MARK: &str = "helios.status.activity.failed";

/// How many of the newest promises are followed.
const FOLLOWED: usize = 20;
/// How often they are read while one runs.
const BUSY_EVERY: Duration = Duration::from_millis(150);
/// How often they are read when none runs.
const IDLE_EVERY: Duration = Duration::from_secs(1);
/// The step of the follower's sleep: a start asked reads at once.
const TICK: Duration = Duration::from_millis(50);
/// How many imports the import window lists.
const IMPORTS_SHOWN: usize = 4;

#[derive(Default)]
struct Shared {
    promises: Vec<Promise>,
    ctx: Option<egui::Context>,
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
            .filter(|p| !p.state.is_settled())
            .cloned()
            .collect();
        running.reverse();
        let failed = shared
            .promises
            .iter()
            .filter(|p| self.unseen_failure(p))
            .count();
        (!running.is_empty() || failed > 0).then_some(Summary { running, failed })
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
                let busy = promises.iter().any(|p| !p.state.is_settled());
                if shared.promises != promises {
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
/// click on either runs [`COMMAND`].
pub fn indicator(ui: &mut egui::Ui, app: &AppHandle, summary: &Summary) {
    use egui_pop::core::indicator::{Indicator, Tone};
    let ctx = ui.ctx().clone();
    let text = |m: Message| crate::i18n::text(&ctx, m);
    let action = text(Message::new(
        "gui-activity-show",
        "Show the background tasks",
    ));
    let mut clicked = false;
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
        clicked |= r.clicked();
        let r = ui
            .add(egui::Label::new(progress_text(&ctx, main)).sense(egui::Sense::click()))
            .on_hover_text(list);
        clicked |= r.clicked();
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
        clicked |= r.clicked();
    }
    if clicked {
        app.run(
            egui_pop::model::ids::CommandId::from_static(COMMAND),
            Value::Null,
        );
    }
}
