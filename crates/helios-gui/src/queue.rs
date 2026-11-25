//! The Queue panel: the work queued at the selected node, items rather
//! than files (see `docs/gui.md`, "Queue panel").
//!
//! - The panel stays in its place and follows the selected node
//!   (`view/graph/selection`, [`crate::params::SELECTION`]): a summary
//!   (waiting, running, done, failed, the oldest wait), then one row per
//!   item with its state, since when, its attempts and where it runs.
//! - What it shows comes from a [`QueueSource`]: the workflow level's
//!   queues ([`crate::workflow_queue::WorkflowQueue`]), a run's rows at
//!   the node as the connected node runs them, followed on the bus, and
//!   the long jobs that are not runs, as their promises.
//! - Its actions are bus commands: Retry, Move to the top or the bottom
//!   and Cancel run `helios.workflow.retry`, `helios.workflow.prioritize`
//!   and `helios.workflow.cancel` (the node's `workflow.*`), a promise's
//!   Cancel `job.cancel`. The rows change when the node says they did.
//! - Photos dragged out of the library and dropped on the panel join the
//!   run queued at the node: `helios.workflow.submit` extends it on its
//!   node with them, as library photos.
//! - Empty (no node selected, or nothing queued at it), the panel shows an
//!   empty state; with `interface.panels.queue_when_empty` off it closes
//!   instead, and opens again once the node has work.

use egui::Ui;
use egui_pop::api::AppHandle;
use egui_pop::core::tooltip::TooltipExt as _;
use egui_pop::core::ui::{Grid, Row, TextRole, Track, UiPop as _};
use egui_pop::model::ids::PanelId;
use helios_core::Message;

/// The panel's id.
pub const PANEL: &str = "helios.queue";

/// The setting that keeps the panel shown when the node has no queue.
pub const WHEN_EMPTY: &str = "interface.panels.queue_when_empty";

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
    /// Queue it again, failed or cancelled.
    Retry,
    /// Claim it before every other waiting item.
    Top,
    /// Claim it after every other waiting item.
    Bottom,
}

impl Action {
    /// Its button's label.
    fn message(self) -> Message {
        match self {
            Action::Cancel => Message::new("gui-queue-cancel", "Cancel"),
            Action::Retry => Message::new("gui-queue-retry", "Retry"),
            Action::Top => Message::new("gui-queue-top", "Top"),
            Action::Bottom => Message::new("gui-queue-bottom", "Bottom"),
        }
    }

    /// Its name in the panel's named places (`helios.queue.0.retry`).
    fn name(self) -> &'static str {
        match self {
            Action::Cancel => "cancel",
            Action::Retry => "retry",
            Action::Top => "top",
            Action::Bottom => "bottom",
        }
    }
}

/// One item of a node's queue.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueItem {
    /// What the source acts on (a run's item, a promise).
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
    /// Why it is held (a service's quota, a refused sign-in), and until
    /// when.
    pub paused: Option<helios_vault::promises::Paused>,
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

    /// Now, by the clock its items' [`QueueItem::since`] are on: the
    /// panel's waits move on as it is drawn, the queue read only when it
    /// changes. `None` keeps the time the queue was read at.
    fn now(&self, _app: &AppHandle) -> Option<f64> {
        None
    }

    /// Takes in what came since the last call (facts, reads); whether
    /// the queue should be read again. Asked each frame: cheap.
    fn due(&mut self, _app: &AppHandle) -> bool {
        false
    }

    /// The arguments of [`crate::workflow_queue::SUBMIT`] that add photos
    /// to the queue of `node` (the connected node, the run's export
    /// settings), all but `images`; `None` when no run queues there, and
    /// photos dropped on the panel are refused.
    fn submit_args(&self, _node: &str) -> Option<serde_json::Value> {
        None
    }
}

/// Whether the panel shows when empty, from the resolved settings
/// ([`crate::config`]).
pub fn when_empty(config: &helios_config::Config) -> bool {
    config.toggle(WHEN_EMPTY)
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
            source: Box::new(crate::workflow_queue::WorkflowQueue::default()),
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

    /// Whether the source took in something that changes the queue: the
    /// caller reads it again ([`QueuePanel::update`]).
    pub fn source_due(&mut self, app: &AppHandle) -> bool {
        self.source.due(app)
    }

    /// Asks for `action` on the item `id`, through the source.
    pub fn act(&mut self, app: &AppHandle, id: &str, action: Action) {
        self.source.act(app, id, action);
    }

    /// Follows `node` (the selection), reads its queue, and closes or
    /// opens the panel as the setting says: closed while empty when it is
    /// off, opened again once there is work if it closed itself. Called
    /// when the selection changed or a job event was said, not each
    /// frame.
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

    /// Draws the panel; photos dragged out of the library and dropped on
    /// it join the queue shown ([`QueuePanel::take_drop`]).
    pub fn ui(&mut self, ui: &mut Ui, app: &AppHandle) {
        let area = ui.max_rect();
        self.draw(ui, app);
        self.take_drop(ui.ctx(), area);
    }

    /// Photos dropped on the panel (`area`), from any window (egui-pop's
    /// drag session): they join the run queued at the followed node,
    /// through [`crate::workflow_queue::SUBMIT`], the command an agent
    /// runs, which extends the run on its node with them as library
    /// photos (`image:` items). With no run there the drop is refused,
    /// saying why. Never an undo step: the node's queue is work, not the
    /// library's state.
    fn take_drop(&mut self, ctx: &egui::Context, area: egui::Rect) {
        use egui_pop::api::bus::drag::{self, Seen};
        let Some(node) = self.node.clone() else {
            return;
        };
        let args = self.source.submit_args(&node);
        let refusal = args.is_none().then(|| {
            Message::new(
                "gui-queue-drop-no-run",
                "Not here: no run queues at { $node } yet",
            )
            .with("node", node.as_str())
        });
        let seen = drag::target::<crate::library::DraggedPhotos>(ctx, PANEL, area, |_| {
            refusal.clone().map(|m| crate::i18n::text(ctx, m))
        });
        match (seen, args) {
            (Seen::Dropped(photos), Some(mut args)) => {
                args["images"] = serde_json::json!(photos.0);
                crate::dialogs::request(
                    ctx,
                    crate::dialogs::Request::Run(
                        crate::workflow_queue::SUBMIT.into(),
                        args,
                        helios_shell::commands::Origin::User,
                    ),
                );
            }
            (Seen::Refused { .. }, _) => {
                if let Some(m) = refusal {
                    crate::dialogs::request(
                        ctx,
                        crate::dialogs::Request::Notify(crate::dialogs::Level::Warning, m),
                    );
                }
            }
            _ => {}
        }
    }

    /// Draws the queue.
    fn draw(&mut self, ui: &mut Ui, app: &AppHandle) {
        let ctx = ui.ctx().clone();
        // Read when it changes; its waits counted to now.
        if let (Some(queue), Some(now)) = (&mut self.queue, self.source.now(app)) {
            queue.now = now;
        }
        let text = |m: Message| crate::i18n::text(&ctx, m);
        let Some(node) = self.node.clone() else {
            ui.muted(text(Message::new(
                "gui-queue-no-selection",
                "Select a node to see its queue",
            )));
            return;
        };
        let queue = match &self.queue {
            Some(q) if !q.is_empty() => q,
            _ => {
                ui.muted(text(
                    Message::new("gui-queue-empty", "Nothing queued at { $node }")
                        .with("node", node.as_str()),
                ));
                return;
            }
        };
        let locale = crate::i18n::locale(&ctx);
        let summary = queue.summary();
        ui.strong_text(text(
            Message::new("gui-queue-title", "Queue of { $node }").with("node", node.as_str()),
        ));
        ui.text_role(
            TextRole::Body,
            text(
                Message::new(
                    "gui-queue-summary",
                    "{ $waiting } waiting, { $running } running, { $done } done, { $failed } failed",
                )
                .with("waiting", summary.waiting)
                .with("running", summary.running)
                .with("done", summary.done)
                .with("failed", summary.failed + summary.cancelled),
            ),
        );
        if let Some(wait) = summary.oldest_wait {
            ui.muted(text(
                Message::new("gui-queue-oldest-wait", "Oldest wait: { $time }")
                    .with("time", duration(&ctx, &locale, wait)),
            ));
        }
        ui.divider();
        let mut asked = Vec::new();
        // Each column as wide as its content, as a table's.
        let tracks = [Track::CONTENT; 6];
        helios_ui::scroll(ui, "helios.queue.scroll", helios_ui::Scroll::Region, |ui| {
            ui.grid(Grid::new("helios.queue.items", tracks), |cells| {
                // Each key written out: the extractor finds them.
                for header in [
                    Message::new("gui-queue-column-state", "State"),
                    Message::new("gui-queue-column-item", "Item"),
                    Message::new("gui-queue-column-since", "Since"),
                    Message::new("gui-queue-column-attempts", "Attempts"),
                    Message::new("gui-queue-column-where", "Where"),
                ] {
                    cells.header(text(header));
                }
                cells.cell(|_| ());
                for (i, item) in queue.items.iter().enumerate() {
                    cells.cell(|ui| {
                        let state = ui.text_role(TextRole::Body, text(item.state.message()));
                        if let Some(paused) = &item.paused {
                            state.tip(text(paused_reason(&ctx, &locale, paused, queue.now)));
                        } else if let Some(error) = &item.error {
                            state.tip(error.as_str());
                        }
                    });
                    cells.cell(|ui| ui.text_role(TextRole::Body, &item.title));
                    cells.cell(|ui| {
                        ui.text_role(
                            TextRole::Body,
                            duration(&ctx, &locale, (queue.now - item.since).max(0.0)),
                        )
                    });
                    cells.cell(|ui| ui.text_role(TextRole::Body, item.attempts.to_string()));
                    cells.cell(|ui| {
                        ui.text_role(
                            TextRole::Body,
                            match &item.place {
                                Place::Here => text(Message::new("gui-queue-here", "This desktop")),
                                Place::Node(name) => text(
                                    Message::new("gui-queue-on-node", "Node { $name }")
                                        .with("name", name.as_str()),
                                ),
                            },
                        )
                    });
                    cells.cell(|ui| {
                        if item.actions.is_empty() {
                            return;
                        }
                        ui.row(
                            Row::new(item.actions.iter().map(|_| Track::CONTENT)),
                            |actions| {
                                for &action in &item.actions {
                                    actions.cell(|ui| {
                                        let label = text(action.message());
                                        let kind = match action {
                                            Action::Cancel => helios_ui::Action::Danger,
                                            _ => helios_ui::Action::Secondary,
                                        };
                                        let r = helios_ui::row_action(ui, kind, &label);
                                        let described = format!("{label} {}", item.title);
                                        r.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::Role::Button,
                                                true,
                                                &described,
                                            )
                                        });
                                        crate::name_rect(
                                            ui.ctx(),
                                            &format!("{PANEL}.{i}.{}", action.name()),
                                            r.rect,
                                        );
                                        if r.clicked() {
                                            asked.push((item.id.clone(), action));
                                        }
                                    });
                                }
                            },
                        );
                    });
                }
            });
        });
        self.asked.extend(asked);
        self.run_asked(app);
    }
}

/// Why a held item waits, and when it goes on by itself (`now` on the
/// wall clock its `until` is on).
fn paused_reason(
    ctx: &egui::Context,
    locale: &str,
    paused: &helios_vault::promises::Paused,
    now: f64,
) -> Message {
    use helios_vault::promises::Wait;
    match (paused.wait, paused.until) {
        (Wait::Quota, Some(until)) => Message::new(
            "gui-queue-paused-quota-until",
            "The service asks to wait: going on in { $time }",
        )
        .with("time", duration(ctx, locale, (until as f64 - now).max(0.0))),
        (Wait::Quota, None) => Message::new(
            "gui-queue-paused-quota",
            "The service asks to wait: going on by itself",
        ),
        (Wait::SignIn, _) => Message::new(
            "gui-queue-paused-sign-in",
            "The service refused the sign-in: sign in again to go on",
        ),
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
            paused: None,
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
        use crate::config::{Settings, Sources};
        let user = |text: &str| {
            Settings::resolve(&Sources {
                user: Some(text.into()),
                ..Sources::default()
            })
        };
        assert!(when_empty(Settings::default().config()));
        let off = user("[interface.panels]\nqueue_when_empty = false\n");
        assert!(!when_empty(off.config()));
        // A value that does not check is left out, and said.
        let bad = user("[interface.panels]\nqueue_when_empty = 3\n");
        assert!(when_empty(bad.config()));
        assert_eq!(bad.problems().len(), 1);
    }
}
