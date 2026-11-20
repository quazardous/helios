//! Quitting Helios as one request on the bus: `helios.app.quit`.
//!
//! Every close of the main window that would end Helios (Quit from any
//! window, egui-pop's `quit`, the last window closed where that quits)
//! asks [`AppQuit`] once, from the before-hook of the main window's
//! `window.close` step (`crate::shell`'s quit hooks). Its listeners are
//! the quit's conditions, named, in a declared order (`after` /
//! `before`), each with its deadline:
//!
//! - [`UNSAVED`]: an edit whose save failed and that nobody chose to
//!   discard: it refuses, and the hook asks "Retry saving", "Quit
//!   without saving" or Cancel;
//! - [`JOBS`]: jobs are running and nobody confirmed cancelling them:
//!   it refuses, and the hook asks "Quit Helios?";
//! - any other module holding something a quit would lose adds its own
//!   listener, and refuses with its reason.
//!
//! A refusal is the step's veto: the main window stays, the journal says
//! `window.close.vetoed`, and the refusing listener is named in the
//! veto's reason and in the quit's report ([`QuitReport`]).
//!
//! The point is **fail-open**, the one Helios request that is not
//! fail-closed: quitting must stay possible whatever a listener does. A
//! listener that fails, panics or misses its deadline is logged, marked
//! in the quit's trace (`crate::quit::mark`) and kept in the report, and
//! the quit goes on. A listener that may block registers `threaded`:
//! the request stops waiting for it at its deadline. A listener that
//! means to keep Helios open refuses, it does not fail.
//!
//! The system's end of session is not asked: it cannot be refused.
//!
//! Once the quit goes, the same request is asked again during the end
//! splash, one listener per frame ([`QuitStage::End`]): its turn names
//! the listener whose work runs now ([`NODE`]: the node told to quit,
//! [`CLIENT`]: the GUI client detached), the others pass. Each turn is
//! asked through bus-contract's `Defer` and heard at the host's drain,
//! so the splash draws between two listeners and names the one running
//! ([`end_steps`]). The two host steps (the changes saved, the interface
//! settings saved) need the window's state and run as the splash's own
//! steps, listed with the listeners, just before [`CLIENT`]. What the
//! end did is kept in a [`ShutdownReport`]: journaled, written for the
//! next start, which says what the last quit cut.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use helios_core::Message;
use serde::{Deserialize, Serialize};

use egui_pop::api::bus::bus_contract::PointId;
use egui_pop::api::bus::bus_contract::{
    self, Bus, HookError, ListenOpts, Point, PointDecl, PointKind, Refused,
};

/// The request a quit is asked on.
pub const QUIT: &str = "helios.app.quit";

/// The unsaved edit's listener: refuses while an edit whose save failed
/// is not discarded. Before [`JOBS`]: the edit is asked about first.
pub const UNSAVED: &str = "helios.quit.unsaved";

/// The running jobs' listener: refuses while jobs run unconfirmed.
pub const JOBS: &str = "helios.quit.jobs";

/// The node's listener: at its end turn, tells the local node to quit
/// (it closes the vault once its last client is gone).
pub const NODE: &str = "helios.quit.node";

/// The GUI client's listener: at its end turn, detaches the GUI from the
/// local node, its last client.
pub const CLIENT: &str = "helios.quit.client";

/// The owner of Helios's own quit listeners.
pub const OWNER: &str = "helios.quit";

/// The deadline of a listener that sets none: the request is asked on
/// the interface's thread, between two frames.
pub const DEADLINE: Duration = Duration::from_millis(500);

/// What asked the quit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitWhy {
    /// Quit (Ctrl+Q, File > Quit, egui-pop's `quit`), from any window.
    Quit,
    /// The last window closed, where that quits.
    LastWindow,
}

/// Which part of a quit is asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum QuitStage {
    /// Whether the quit may go: a listener may refuse.
    #[default]
    Ask,
    /// The quit goes, the end splash runs its listeners one per frame:
    /// this is the `turn`-th step, `listener`'s; the others pass.
    End { turn: usize, listener: String },
}

impl QuitStage {
    /// Whether this is `id`'s end turn.
    pub fn turn_of(&self, id: &str) -> bool {
        matches!(self, QuitStage::End { listener, .. } if listener == id)
    }
}

/// A quit, as its listeners read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuitAsk {
    pub why: QuitWhy,
    /// What is asked: whether to go, or one listener's end turn.
    pub stage: QuitStage,
    /// The jobs a quit would cancel.
    pub running_jobs: usize,
    /// Cancelling them is confirmed: asked and answered Quit, or
    /// confirmations answered yes by policy.
    pub jobs_confirmed: bool,
    /// An edit whose save failed, in words (its reason): a quit would
    /// lose it.
    pub unsaved: Option<String>,
    /// Losing it is chosen: asked and answered "Quit without saving".
    pub unsaved_discarded: bool,
}

/// Helios quits (`helios.app.quit`): each listener says, in one line,
/// what it checked; one refuses to keep Helios open.
#[derive(Debug)]
pub struct AppQuit;

impl Point for AppQuit {
    const KIND: PointKind = PointKind::Request;
    const ID: PointId = PointId::new(QUIT);
    type Input = QuitAsk;
    type Value = Vec<String>;
}

/// The quit's point, its contract with the application's bus. Fail-open
/// by design (the module's doc): never `.fail_closed()`.
pub const POINTS: &[PointDecl] = &[PointDecl::of::<AppQuit>(
    "Helios quits (Quit, the last window closed): its listeners, in order, each within its deadline, may refuse with a reason (jobs running, an edit not saved); fail-open, a listener that fails or is late is reported and the quit goes on",
)
.deadline(DEADLINE)];

/// Declares [`POINTS`] on `bus` (once).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "quit: {refused}");
        }
    }
}

/// What the end's listeners act on, shared with the shell: the node to
/// tell, the GUI client to detach.
#[derive(Default)]
pub struct EndSlots {
    node: Mutex<Option<crate::node::Link>>,
    client: Mutex<Option<crate::node::Client>>,
    told: AtomicBool,
}

impl std::fmt::Debug for EndSlots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EndSlots")
            .field("told", &self.told.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl EndSlots {
    /// The local node, told to quit at [`NODE`]'s turn.
    pub fn set_node(&self, link: crate::node::Link) {
        *self.node.lock().unwrap_or_else(PoisonError::into_inner) = Some(link);
    }

    /// The local node, when attached.
    pub fn node(&self) -> Option<crate::node::Link> {
        self.node
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The GUI's client of the node, dropped at [`CLIENT`]'s turn.
    pub fn set_client(&self, client: crate::node::Client) {
        *self.client.lock().unwrap_or_else(PoisonError::into_inner) = Some(client);
    }

    /// Tells the node to quit, once: cancelling its jobs when
    /// `cancel_jobs`. Whether a node was there to tell.
    pub fn tell(&self, cancel_jobs: bool) -> bool {
        let Some(node) = self.node() else {
            return false;
        };
        if !self.told.swap(true, Ordering::SeqCst) {
            if cancel_jobs {
                node.quit_cancelling_jobs();
            } else {
                node.quit();
            }
        }
        true
    }

    /// Whether the node was told to quit.
    pub fn told(&self) -> bool {
        self.told.load(Ordering::SeqCst)
    }

    /// Drops the GUI client; whether one was there.
    pub fn detach(&self) -> bool {
        let client = self
            .client
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        client.is_some()
    }
}

/// Registers Helios's own listeners on [`AppQuit`]: [`UNSAVED`],
/// [`JOBS`], then [`NODE`] and [`CLIENT`] over `slots`. Those there
/// already are replaced.
///
/// # Errors
/// The bus refused one.
pub fn listen(bus: &dyn Bus, slots: &std::sync::Arc<EndSlots>) -> Result<(), String> {
    declare(bus);
    for id in [UNSAVED, JOBS, NODE, CLIENT] {
        bus.unlisten(QUIT, id);
    }
    bus.listen::<AppQuit>(
        UNSAVED,
        ListenOpts::new().before(JOBS).owner(OWNER),
        |ask, said| {
            if ask.stage != QuitStage::Ask {
                return Ok(());
            }
            match &ask.unsaved {
                None => {
                    said.push(format!("{UNSAVED}: every edit saved"));
                    Ok(())
                }
                Some(_) if ask.unsaved_discarded => {
                    said.push(format!(
                        "{UNSAVED}: an edit not saved, discarding it chosen"
                    ));
                    Ok(())
                }
                Some(why) => Err(HookError::refuse(format!(
                    "an edit could not be saved: {why}"
                ))),
            }
        },
    )
    .map_err(|e| e.to_string())?;
    bus.listen::<AppQuit>(JOBS, ListenOpts::new().owner(OWNER), |ask, said| {
        if ask.stage != QuitStage::Ask {
            Ok(())
        } else if ask.running_jobs == 0 {
            said.push(format!("{JOBS}: no job running"));
            Ok(())
        } else if ask.jobs_confirmed {
            said.push(format!(
                "{JOBS}: {} jobs running, cancelling them confirmed",
                ask.running_jobs
            ));
            Ok(())
        } else {
            Err(HookError::refuse(format!(
                "{} jobs are running: asked first",
                ask.running_jobs
            )))
        }
    })
    .map_err(|e| e.to_string())?;
    let node = slots.clone();
    bus.listen::<AppQuit>(
        NODE,
        ListenOpts::new().after(JOBS).owner(OWNER),
        move |ask, said| {
            if ask.stage.turn_of(NODE) {
                let told = node.tell(ask.jobs_confirmed);
                said.push(format!(
                    "{NODE}: {}",
                    if told {
                        "the node told to quit"
                    } else {
                        "no node"
                    }
                ));
            }
            Ok(())
        },
    )
    .map_err(|e| e.to_string())?;
    let client = slots.clone();
    bus.listen::<AppQuit>(
        CLIENT,
        ListenOpts::new().after(NODE).owner(OWNER),
        move |ask, said| {
            if ask.stage.turn_of(CLIENT) {
                let detached = client.detach();
                if detached {
                    crate::quit::mark("GUI client disconnected from the node");
                }
                said.push(format!(
                    "{CLIENT}: {}",
                    if detached {
                        "the GUI client detached"
                    } else {
                        "no client"
                    }
                ));
            }
            Ok(())
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// What a quit's request did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuitReport {
    /// Each listener's line, in the order they ran.
    pub said: Vec<String>,
    /// Each listener that failed or missed its deadline, as text: its
    /// part dropped, the quit gone on.
    pub failures: Vec<String>,
    /// The listener that refused, and its reason: Helios stays open.
    pub refused: Option<(String, String)>,
}

impl QuitReport {
    /// Whether the quit goes on.
    pub fn goes(&self) -> bool {
        self.refused.is_none()
    }
}

/// Asks the quit `ask` on `bus`: what its listeners did. Every failure
/// is logged and marked in the quit's trace.
pub fn ask(bus: &dyn Bus, ask: QuitAsk) -> QuitReport {
    declare(bus);
    let out = bus.alter::<AppQuit>(ask, Vec::new());
    let mut report = QuitReport {
        said: out.value,
        ..QuitReport::default()
    };
    for failure in out.failures.iter().filter(|f| f.dropped()) {
        log::warn!(target: "helios::quit", "{failure}: the quit goes on");
        crate::quit::mark(&format!("{QUIT}: {} failed or late", failure.listener));
        report.failures.push(failure.to_string());
    }
    for listener in &out.deferred {
        log::warn!(
            target: "helios::quit",
            "{QUIT}: `{listener}` listens on another thread: not heard before the quit"
        );
    }
    if let Some(r) = out.refused {
        log::info!(target: "helios::quit", "{r}");
        report.refused = Some((r.listener, r.reason));
    }
    report
}

/// The order [`AppQuit`]'s listeners run in on `bus`.
pub fn order(bus: &dyn Bus) -> String {
    bus_contract::order_dump(bus, &[QUIT])
}

/// A step of the end splash, in the order it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndStep {
    /// The listener's id, or the host step's name.
    pub id: String,
    /// Run by the host with the window's state, not by a listener of the
    /// request.
    pub host: bool,
    /// A listener's deadline, as the bus gives it.
    pub deadline: Option<Duration>,
}

/// The end splash's steps: [`AppQuit`]'s listeners in the order they
/// run, and the host's steps `host` just before [`CLIENT`] (they need
/// the GUI client), at the end when it is not there.
pub fn end_steps(bus: &dyn Bus, host: &[&str]) -> Vec<EndStep> {
    declare(bus);
    let mut steps: Vec<EndStep> = bus
        .listeners(QUIT)
        .unwrap_or_default()
        .into_iter()
        .map(|l| EndStep {
            id: l.id,
            host: false,
            deadline: Some(l.deadline),
        })
        .collect();
    let at = steps
        .iter()
        .position(|s| s.id == CLIENT)
        .unwrap_or(steps.len());
    steps.splice(
        at..at,
        host.iter().map(|id| EndStep {
            id: (*id).to_owned(),
            host: true,
            deadline: None,
        }),
    );
    steps
}

/// What is added to the end's declared deadlines: the splash's own
/// frames between two steps.
pub const END_MARGIN: Duration = Duration::from_secs(2);

/// The hard ceiling of [`end_limit`]: a broken declaration (a listener
/// declaring an hour) never holds the window longer.
pub const END_CEILING: Duration = Duration::from_mins(1);

/// How long the end splash may last before it goes on as if "Quit now"
/// was chosen: its steps one after the other, each listener within its
/// declared deadline, each host step within `host_step`, then the node's
/// stop within `node` (its own listeners' deadlines,
/// `Link::stop_deadline`), plus [`END_MARGIN`]; never past
/// [`END_CEILING`].
pub fn end_limit(steps: &[EndStep], host_step: Duration, node: Duration) -> Duration {
    steps
        .iter()
        .map(|s| s.deadline.unwrap_or(host_step))
        .fold(Duration::ZERO, Duration::saturating_add)
        .saturating_add(node)
        .saturating_add(END_MARGIN)
        .min(END_CEILING)
}

/// A listener's row on the end splash: Helios's own named, another
/// module's by its id.
pub fn label(id: &str) -> Message {
    match id {
        UNSAVED => Message::new("gui-end-unsaved", "Checking the unsaved edits"),
        JOBS => Message::new("gui-end-jobs", "Checking the running jobs"),
        NODE => Message::new("gui-end-node", "Asking the library's node to stop"),
        CLIENT => Message::new("gui-end-client", "Disconnecting from the library's node"),
        other => Message::plain(other),
    }
}

/// What one end turn did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnOutcome {
    /// The listener did its part.
    Finished,
    /// The listener missed its deadline: `cut` when the request stopped
    /// waiting for it (its work may still run, or be lost).
    Late { deadline: Duration, cut: bool },
    /// The listener failed: its part dropped.
    Failed(String),
}

/// Asks `ask`'s end turn through the bus's `Defer`, heard at `drain`
/// (the host's drain of its asks, between two frames); at once on a bus
/// without `Defer`. What the turn's listener did. Fail-open: whatever it
/// did, the quit goes on.
pub fn turn(
    bus: &dyn Bus,
    drain: impl FnOnce() -> Vec<bus_contract::PendingRun>,
    ask: QuitAsk,
) -> TurnOutcome {
    use egui_pop::api::bus::bus_contract_defer::BusDefer as _;
    declare(bus);
    let QuitStage::End { listener, .. } = ask.stage.clone() else {
        return TurnOutcome::Failed("not an end turn".into());
    };
    let failures = if let Some(defer) = bus.defer() {
        let number = defer.ask::<AppQuit>(ask, Vec::new());
        let Some(run) = drain().into_iter().find(|r| r.ask == Some(number)) else {
            return TurnOutcome::Failed("not heard at the drain".into());
        };
        run.failures
    } else {
        bus.alter::<AppQuit>(ask, Vec::new()).failures
    };
    let mut outcome = TurnOutcome::Finished;
    for failure in failures.iter().filter(|f| f.dropped()) {
        log::warn!(target: "helios::quit", "{failure}: the quit goes on");
        crate::quit::mark(&format!("{QUIT}: {} failed or late", failure.listener));
        if failure.listener != listener {
            continue;
        }
        outcome = match &failure.kind {
            bus_contract::FailureKind::Late { deadline, took } => TurnOutcome::Late {
                deadline: *deadline,
                cut: took.is_none(),
            },
            other => TurnOutcome::Failed(other.to_string()),
        };
    }
    outcome
}

/// The file the end's report is written to for the next start, in the
/// configuration directory.
pub const REPORT_FILE: &str = "last-quit.json";

/// The journal's event the end's report is said under.
pub const REPORT_EVENT: &str = "helios.app.quit.ended";

/// A listener past its deadline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Late {
    pub listener: String,
    /// Its deadline, in milliseconds.
    pub deadline_ms: u64,
    /// The request stopped waiting for it: its work may be lost.
    pub cut: bool,
}

/// What the end of a quit did: who finished, who went past its
/// deadline, who failed, who was abandoned (the end left before their
/// turn, "Quit now" or the end's limit), and whether work may have been
/// lost without being known. Said on the journal ([`REPORT_EVENT`]) and
/// written for the next start ([`ShutdownReport::write`]), which says
/// what the last quit cut.
///
/// Its shape follows the `ShutdownReport` of qubit-event-bus
/// (<https://github.com/qubit-ltd/rs-event-bus>): a graceful shutdown
/// with a deadline ends with a report of its outcome, the deliveries
/// known abandoned, and whether some may have been abandoned unseen.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownReport {
    /// The steps that did their part, in order.
    pub finished: Vec<String>,
    /// The listeners past their deadline.
    pub late: Vec<Late>,
    /// The listeners that failed, and how.
    pub failed: Vec<(String, String)>,
    /// The steps the end left before their turn.
    pub abandoned: Vec<String>,
    /// Some work may have been lost without being known: a listener cut
    /// at its deadline, one failed, or a step abandoned.
    pub may_have_lost_work: bool,
}

impl ShutdownReport {
    /// Keeps what `id`'s turn did.
    pub fn record(&mut self, id: &str, outcome: &TurnOutcome) {
        match outcome {
            TurnOutcome::Finished => self.finished.push(id.to_owned()),
            TurnOutcome::Late { deadline, cut } => {
                self.may_have_lost_work |= *cut;
                self.late.push(Late {
                    listener: id.to_owned(),
                    deadline_ms: u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX),
                    cut: *cut,
                });
            }
            TurnOutcome::Failed(why) => {
                self.may_have_lost_work = true;
                self.failed.push((id.to_owned(), why.clone()));
            }
        }
    }

    /// The steps `ids` were left before their turn.
    pub fn abandon<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) {
        for id in ids {
            self.may_have_lost_work = true;
            self.abandoned.push(id.to_owned());
        }
    }

    /// Whether every step did its part in time.
    pub fn clean(&self) -> bool {
        self.late.is_empty() && self.failed.is_empty() && self.abandoned.is_empty()
    }

    /// What the next start says of it, one line per step cut: empty when
    /// the quit was clean.
    pub fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .late
            .iter()
            .map(|l| {
                let deadline = Duration::from_millis(l.deadline_ms).as_secs_f64();
                if l.cut {
                    format!(
                        "the last quit stopped `{}` after its {deadline} s deadline",
                        l.listener
                    )
                } else {
                    format!(
                        "at the last quit, `{}` went past its {deadline} s deadline",
                        l.listener
                    )
                }
            })
            .collect();
        lines.extend(
            self.failed
                .iter()
                .map(|(id, why)| format!("at the last quit, `{id}` {why}")),
        );
        lines.extend(
            self.abandoned
                .iter()
                .map(|id| format!("the last quit left `{id}` before it ran")),
        );
        if self.may_have_lost_work {
            lines.push("some work may have been lost".into());
        }
        lines
    }

    /// Writes it to [`REPORT_FILE`] in `dir`, for the next start.
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(dir.join(REPORT_FILE), text)
    }

    /// The last quit's report in `dir`, taken (the file removed): `None`
    /// when there is none or it cannot be read.
    pub fn take(dir: &Path) -> Option<ShutdownReport> {
        let path = dir.join(REPORT_FILE);
        let text = std::fs::read_to_string(&path).ok()?;
        let _ = std::fs::remove_file(&path);
        serde_json::from_str(&text)
            .map_err(|e| log::warn!(target: "helios::quit", "{}: {e}", path.display()))
            .ok()
    }
}

/// The node's quit (`helios.node.quit`) followed by the end splash, from
/// the facts the node says over its link: each listener's row named as
/// it begins ("Stopping the exports", waiting for `exports`), and what
/// the node's quit did, kept for the end's report.
pub struct NodeEnd {
    facts: std::sync::mpsc::Receiver<helios_core::Envelope>,
    phase: helios_node_core::phases::Phase,
    /// The row of the node's listener running now.
    step: Option<helios_node_core::phases::Running>,
    ended: Option<helios_node_core::node_quit::QuitEnded>,
}

impl std::fmt::Debug for NodeEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeEnd")
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

impl NodeEnd {
    /// Follows the node's quit through `facts` (`Link::follow_quit`), its
    /// rows on `phase` (the end splash's).
    pub fn new(
        facts: std::sync::mpsc::Receiver<helios_core::Envelope>,
        phase: helios_node_core::phases::Phase,
    ) -> Self {
        Self {
            facts,
            phase,
            step: None,
            ended: None,
        }
    }

    /// Reads the node's facts said since the last pass: a listener begun
    /// gets its row (the one before is done), the end closes the last.
    pub fn pump(&mut self) {
        use helios_core::Event as _;
        use helios_node_core::node_quit::{ListenerBegan, QuitEnded};
        while let Ok(fact) = self.facts.try_recv() {
            if fact.kind == ListenerBegan::KIND {
                let Ok(began) = serde_json::from_value::<ListenerBegan>(fact.payload) else {
                    continue;
                };
                self.step = None;
                let step = self.phase.begin(began.label);
                step.why(waiting_for(&began.listener));
                self.step = Some(step);
            } else if fact.kind == QuitEnded::KIND {
                self.step = None;
                self.ended = serde_json::from_value(fact.payload).ok();
            }
        }
    }

    /// What the node's quit did, once it ended.
    pub fn ended(&self) -> Option<&helios_node_core::node_quit::QuitEnded> {
        self.ended.as_ref()
    }
}

/// What the end splash says a node listener's row waits for: "Waiting
/// for: exports".
pub fn waiting_for(listener: &str) -> Message {
    let what = listener
        .strip_prefix(helios_node_core::node_quit::QUIT)
        .and_then(|l| l.strip_prefix('.'))
        .unwrap_or(listener);
    Message::new("gui-end-node-waiting", "Waiting for: { $what }").with("what", what.to_owned())
}

impl ShutdownReport {
    /// Keeps what the node's quit did, its listeners by their id, the cut
    /// ones named.
    pub fn record_node(&mut self, ended: &helios_node_core::node_quit::QuitEnded) {
        for id in &ended.finished {
            self.record(id, &TurnOutcome::Finished);
        }
        for late in &ended.late {
            let outcome = TurnOutcome::Late {
                deadline: Duration::from_millis(late.deadline_ms),
                cut: late.cut,
            };
            self.record(&late.listener, &outcome);
        }
        for (id, why) in &ended.failed {
            self.record(id, &TurnOutcome::Failed(why.clone()));
        }
    }
}
