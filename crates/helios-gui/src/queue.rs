//! The Queue panel: the work queued at the selected node, items rather
//! than files (see `docs/gui.md`, "Queue panel").
//!
//! - The panel stays in its place and follows the selected node
//!   (`view/graph/selection`, [`crate::params::SELECTION`]): a summary
//!   (waiting, running, done, failed, the oldest wait), then one row per
//!   item with its state, since when, its attempts and where it runs.
//! - What it shows comes from a [`QueueSource`]. Until the workflow level
//!   runs, the source is [`JobsQueue`]: the jobs that name the node
//!   (`node` in `job.started`, `helios_core::jobs::Start::node`), as
//!   egui-pop's jobs model holds them, a connected node's included. The
//!   workflow's own queue (claims, leases, retries) replaces it without
//!   touching the panel.
//! - Its actions go through the jobs' commands, as the jobs centre's do:
//!   Cancel is `job.cancel`, run in the desktop or sent to the node that
//!   runs the job. Retry and reprioritise are not offered: no Helios job
//!   can be retried or moved in its queue yet.
//! - Empty (no node selected, or nothing queued at it), the panel shows an
//!   empty state; with `interface.panels.queue_when_empty` off it closes
//!   instead, and opens again once the node has work.

use std::collections::BTreeMap;

use egui::Ui;
use egui_pop::api::AppHandle;
use egui_pop::api::host::Seq;
use egui_pop::api::jobs::JOBS;
use egui_pop::model::ids::PanelId;
use egui_pop::model::jobs::{Job, JobState, Request};
use helios_core::Message;

/// The panel's id.
pub const PANEL: &str = "helios.queue";

/// The setting that keeps the panel shown when the node has no queue.
pub const WHEN_EMPTY: &str = "interface.panels.queue_when_empty";

/// How many jobs' nodes [`JobsQueue`] remembers before it forgets the
/// ones the jobs model forgot.
const REMEMBERED: usize = 512;

/// Where an item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemState {
    /// Queued, not started yet.
    Waiting,
    Running,
    /// Held: its node is out of reach, or it was paused.
    Paused,
    /// Cancelling was accepted: it winds down.
    Cancelling,
    Done,
    Failed,
    Cancelled,
}

impl ItemState {
    /// Whether it still goes on (waiting included).
    pub fn active(self) -> bool {
        matches!(
            self,
            ItemState::Waiting | ItemState::Running | ItemState::Paused | ItemState::Cancelling
        )
    }

    fn message(self) -> Message {
        match self {
            ItemState::Waiting => Message::new("gui-queue-state-waiting", "Waiting"),
            ItemState::Running => Message::new("gui-queue-state-running", "Running"),
            ItemState::Paused => Message::new("gui-queue-state-paused", "Paused"),
            ItemState::Cancelling => Message::new("gui-queue-state-cancelling", "Cancelling"),
            ItemState::Done => Message::new("gui-queue-state-done", "Done"),
            ItemState::Failed => Message::new("gui-queue-state-failed", "Failed"),
            ItemState::Cancelled => Message::new("gui-queue-state-cancelled", "Cancelled"),
        }
    }
}

/// What can be asked of an item, through the control API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Stop it, waiting or running.
    Cancel,
}

/// One item of a node's queue.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueItem {
    /// What the source acts on (a job's id).
    pub id: String,
    /// What it is, for people.
    pub title: String,
    pub state: ItemState,
    /// Since when it is in its state, by [`NodeQueue::now`]'s clock.
    pub since: f64,
    /// How many times it started.
    pub attempts: u32,
    /// Who or where: this desktop, or the connected node running it.
    pub place: Place,
    /// Why it failed.
    pub error: Option<String>,
    /// What may be asked of it now.
    pub actions: Vec<Action>,
}

/// Where an item runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// In the desktop's own process.
    Here,
    /// On a connected node, by its name.
    Node(String),
}

/// A node's queue, as its source sees it now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeQueue {
    /// Its items, in the order they were queued.
    pub items: Vec<QueueItem>,
    /// Now, by the clock [`QueueItem::since`] is on.
    pub now: f64,
}

/// A queue at a glance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub waiting: usize,
    /// Running, paused or cancelling.
    pub running: usize,
    pub done: usize,
    pub failed: usize,
    pub cancelled: usize,
    /// How long the oldest waiting item has waited, in seconds.
    pub oldest_wait: Option<f64>,
}

impl NodeQueue {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn summary(&self) -> Summary {
        let mut s = Summary::default();
        for item in &self.items {
            match item.state {
                ItemState::Waiting => {
                    s.waiting += 1;
                    let waited = (self.now - item.since).max(0.0);
                    s.oldest_wait = Some(s.oldest_wait.map_or(waited, |w: f64| w.max(waited)));
                }
                ItemState::Running | ItemState::Paused | ItemState::Cancelling => s.running += 1,
                ItemState::Done => s.done += 1,
                ItemState::Failed => s.failed += 1,
                ItemState::Cancelled => s.cancelled += 1,
            }
        }
        s
    }
}

/// Where the panel reads a node's queue, and how it acts on its items.
pub trait QueueSource {
    /// The queue of `node` (a node of the graph, by name); `None` when the
    /// node has no queue.
    fn queue(&mut self, app: &AppHandle, node: &str) -> Option<NodeQueue>;

    /// Asks for `action` on the item `id` of a queue it gave.
    fn act(&mut self, app: &AppHandle, id: &str, action: Action);
}

/// The interim source: the jobs that name a node of the graph, from
/// egui-pop's jobs model (the desktop's jobs and the connected nodes').
#[derive(Debug, Default)]
pub struct JobsQueue {
    /// The journal read up to here.
    seen: Seq,
    /// Each job's node, as its `job.started` named it.
    nodes: BTreeMap<String, String>,
}

impl JobsQueue {
    /// Reads the jobs started since the last call.
    fn follow(&mut self, app: &AppHandle) {
        for e in app.events_since(self.seen) {
            self.seen = e.seq;
            if e.name != helios_core::jobs::STARTED {
                continue;
            }
            if let (Some(id), Some(node)) = (e.data["id"].as_str(), e.data["node"].as_str()) {
                self.nodes.insert(id.to_string(), node.to_string());
            }
        }
    }
}

/// An item's state from its job's: started but without any progress yet,
/// a job is still waiting in its queue.
fn state_of(job: &Job) -> ItemState {
    match job.state {
        JobState::Running
            if job.step.is_none() && job.done == 0 && job.item.is_none() && job.phase.is_none() =>
        {
            ItemState::Waiting
        }
        JobState::Running => ItemState::Running,
        JobState::Paused => ItemState::Paused,
        JobState::Cancelling => ItemState::Cancelling,
        JobState::Done => ItemState::Done,
        JobState::Failed => ItemState::Failed,
        JobState::Cancelled => ItemState::Cancelled,
    }
}

/// An item from its job.
fn item_of(job: &Job) -> QueueItem {
    let state = state_of(job);
    let place = match crate::relay::split_node_job(&job.id) {
        Some((node, _)) => Place::Node(node.to_string()),
        None => Place::Here,
    };
    let mut actions = Vec::new();
    if state.active() && state != ItemState::Cancelling && job.cancellable {
        actions.push(Action::Cancel);
    }
    QueueItem {
        id: job.id.clone(),
        title: job.title.clone(),
        state,
        since: job.ended_at.unwrap_or(job.started_at),
        attempts: job.attempts,
        place,
        error: job.error.clone(),
        actions,
    }
}

impl QueueSource for JobsQueue {
    fn queue(&mut self, app: &AppHandle, node: &str) -> Option<NodeQueue> {
        self.follow(app);
        let centre = app.service(&JOBS)?;
        if self.nodes.len() > REMEMBERED {
            self.nodes.retain(|id, _| centre.job(id).is_some());
        }
        let items: Vec<QueueItem> = centre
            .all()
            .iter()
            .filter(|j| self.nodes.get(&j.id).is_some_and(|n| n == node))
            .map(item_of)
            .collect();
        (!items.is_empty()).then(|| NodeQueue {
            items,
            now: centre.now(),
        })
    }

    fn act(&mut self, app: &AppHandle, id: &str, action: Action) {
        match action {
            // The jobs centre's way: `job.cancel`, run in the desktop or
            // on the job's node.
            Action::Cancel => egui_pop::api::jobs::ask(app, id, Request::Cancel),
        }
    }
}

/// Whether the panel shows when empty, from the user's settings layer
/// (its TOML text; none: the default).
pub fn when_empty(user_layer: Option<&str>) -> Result<bool, String> {
    let mut config = helios_config::Config::new(helios_config::Schema::core());
    if let Some(text) = user_layer {
        config
            .load_str(helios_config::Layer::User, text)
            .map_err(|problems| {
                problems
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            })?;
    }
    Ok(config.toggle(WHEN_EMPTY))
}

/// The panel: the selected node's queue, as its source last gave it.
pub struct QueuePanel {
    source: Box<dyn QueueSource>,
    /// The node followed (the selection), if any.
    node: Option<String>,
    queue: Option<NodeQueue>,
    /// Shown when empty (`interface.panels.queue_when_empty`).
    when_empty: bool,
    /// Closed by the panel itself for being empty: opened again once
    /// there is work.
    closed_empty: bool,
    /// Actions clicked, run after drawing.
    asked: Vec<(String, Action)>,
}

impl Default for QueuePanel {
    fn default() -> Self {
        Self {
            source: Box::new(JobsQueue::default()),
            node: None,
            queue: None,
            when_empty: true,
            closed_empty: false,
            asked: Vec::new(),
        }
    }
}

impl QueuePanel {
    /// Reads queues from `source` from now on.
    pub fn set_source(&mut self, source: Box<dyn QueueSource>) {
        self.source = source;
        self.queue = None;
    }

    /// Whether the panel shows when empty.
    pub fn set_when_empty(&mut self, on: bool) {
        self.when_empty = on;
    }

    pub fn when_empty_shown(&self) -> bool {
        self.when_empty
    }

    /// The node followed.
    pub fn node(&self) -> Option<&str> {
        self.node.as_deref()
    }

    /// The followed node's queue, as last read.
    pub fn queue(&self) -> Option<&NodeQueue> {
        self.queue.as_ref()
    }

    /// Whether there is nothing to show.
    pub fn is_empty(&self) -> bool {
        self.queue.as_ref().is_none_or(NodeQueue::is_empty)
    }

    /// Asks for `action` on the item `id`, through the source.
    pub fn act(&mut self, app: &AppHandle, id: &str, action: Action) {
        self.source.act(app, id, action);
    }

    /// Follows `node` (the selection), reads its queue, and closes or
    /// opens the panel as the setting says: closed while empty when it is
    /// off, opened again once there is work if it closed itself.
    ///
    /// Whether what the panel shows changed (the node or its items): it
    /// was drawn already this frame, so the caller asks for another.
    pub fn update(&mut self, app: &AppHandle, node: Option<&str>) -> bool {
        let node = node.filter(|n| !n.is_empty()).map(str::to_string);
        let queue = match &node {
            Some(n) => self.source.queue(app, n),
            None => None,
        };
        // `now` moves every frame: only the node and the items count.
        let changed = node != self.node
            || queue.as_ref().map(|q| &q.items) != self.queue.as_ref().map(|q| &q.items);
        self.node = node;
        self.queue = queue;
        let Ok(id) = PanelId::new(PANEL) else {
            return changed;
        };
        let empty = self.is_empty();
        if empty && !self.when_empty && app.panel_visible(&id) {
            app.set_panel_visible(&id, false);
            self.closed_empty = true;
        } else if !empty && self.closed_empty {
            self.closed_empty = false;
            if !app.panel_visible(&id) {
                app.set_panel_visible(&id, true);
            }
        }
        changed
    }

    /// Runs the actions clicked in the last drawing.
    fn run_asked(&mut self, app: &AppHandle) {
        for (id, action) in std::mem::take(&mut self.asked) {
            self.source.act(app, &id, action);
        }
    }

    /// Draws the panel.
    pub fn ui(&mut self, ui: &mut Ui, app: &AppHandle) {
        let ctx = ui.ctx().clone();
        let text = |m: Message| crate::i18n::text(&ctx, m);
        let Some(node) = self.node.clone() else {
            ui.weak(text(Message::new(
                "gui-queue-no-selection",
                "Select a node to see its queue",
            )));
            return;
        };
        let queue = match &self.queue {
            Some(q) if !q.is_empty() => q,
            _ => {
                ui.weak(text(
                    Message::new("gui-queue-empty", "Nothing queued at { $node }")
                        .with("node", node.as_str()),
                ));
                return;
            }
        };
        let locale = crate::i18n::locale(&ctx);
        let summary = queue.summary();
        ui.strong(text(
            Message::new("gui-queue-title", "Queue of { $node }").with("node", node.as_str()),
        ));
        ui.label(text(
            Message::new(
                "gui-queue-summary",
                "{ $waiting } waiting, { $running } running, { $done } done, { $failed } failed",
            )
            .with("waiting", summary.waiting)
            .with("running", summary.running)
            .with("done", summary.done)
            .with("failed", summary.failed + summary.cancelled),
        ));
        if let Some(wait) = summary.oldest_wait {
            ui.weak(text(
                Message::new("gui-queue-oldest-wait", "Oldest wait: { $time }")
                    .with("time", duration(&ctx, &locale, wait)),
            ));
        }
        ui.separator();
        let mut asked = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt("helios.queue.scroll")
            .show(ui, |ui| {
                egui::Grid::new("helios.queue.items")
                    .striped(true)
                    .num_columns(6)
                    .show(ui, |ui| {
                        // Each key written out: the extractor finds them.
                        for header in [
                            Message::new("gui-queue-column-state", "State"),
                            Message::new("gui-queue-column-item", "Item"),
                            Message::new("gui-queue-column-since", "Since"),
                            Message::new("gui-queue-column-attempts", "Attempts"),
                            Message::new("gui-queue-column-where", "Where"),
                        ] {
                            ui.strong(text(header));
                        }
                        ui.label("");
                        ui.end_row();
                        for (i, item) in queue.items.iter().enumerate() {
                            let state = ui.label(text(item.state.message()));
                            if let Some(error) = &item.error {
                                state.on_hover_text(error);
                            }
                            ui.label(&item.title);
                            ui.label(duration(&ctx, &locale, (queue.now - item.since).max(0.0)));
                            ui.label(item.attempts.to_string());
                            ui.label(match &item.place {
                                Place::Here => text(Message::new("gui-queue-here", "This desktop")),
                                Place::Node(name) => text(
                                    Message::new("gui-queue-on-node", "Node { $name }")
                                        .with("name", name.as_str()),
                                ),
                            });
                            if item.actions.contains(&Action::Cancel) {
                                let label = text(Message::new("gui-queue-cancel", "Cancel"));
                                let r = ui.button(&label);
                                let described = format!("{label} {}", item.title);
                                r.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::Button,
                                        true,
                                        &described,
                                    )
                                });
                                crate::name_rect(ui.ctx(), &format!("{PANEL}.{i}.cancel"), r.rect);
                                if r.clicked() {
                                    asked.push((item.id.clone(), Action::Cancel));
                                }
                            } else {
                                ui.label("");
                            }
                            ui.end_row();
                        }
                    });
            });
        self.asked.extend(asked);
        self.run_asked(app);
    }
}

/// A duration as people read it: seconds, minutes or hours.
fn duration(ctx: &egui::Context, locale: &str, seconds: f64) -> String {
    let m = if seconds < 60.0 {
        Message::new("gui-queue-seconds", "{ $n } s").with("n", seconds.floor() as u64)
    } else if seconds < 3600.0 {
        Message::new("gui-queue-minutes", "{ $n } min").with("n", (seconds / 60.0).floor() as u64)
    } else {
        Message::new("gui-queue-hours", "{ $n } h")
            .with("n", crate::i18n::number(locale, seconds / 3600.0, 1))
    };
    crate::i18n::text(ctx, m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(state: ItemState, since: f64) -> QueueItem {
        QueueItem {
            id: "j".into(),
            title: "Export a.png".into(),
            state,
            since,
            attempts: 1,
            place: Place::Here,
            error: None,
            actions: Vec::new(),
        }
    }

    #[test]
    fn a_summary_counts_the_states_and_the_oldest_wait() {
        let q = NodeQueue {
            items: vec![
                item(ItemState::Waiting, 4.0),
                item(ItemState::Waiting, 1.0),
                item(ItemState::Running, 0.0),
                item(ItemState::Cancelling, 0.0),
                item(ItemState::Done, 0.0),
                item(ItemState::Failed, 0.0),
            ],
            now: 10.0,
        };
        let s = q.summary();
        assert_eq!((s.waiting, s.running, s.done, s.failed), (2, 2, 1, 1));
        assert_eq!(s.oldest_wait, Some(9.0));
        assert!(NodeQueue::default().summary().oldest_wait.is_none());
    }

    #[test]
    fn the_setting_comes_from_the_user_layer() {
        assert_eq!(when_empty(None), Ok(true));
        let off = "[interface.panels]\nqueue_when_empty = false\n";
        assert_eq!(when_empty(Some(off)), Ok(false));
        assert!(when_empty(Some("[interface.panels]\nqueue_when_empty = 3\n")).is_err());
    }
}
