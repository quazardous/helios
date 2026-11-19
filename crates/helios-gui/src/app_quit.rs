//! Quitting Helios as one request on the bus: `helios.app.quit`.
//!
//! Every close of the main window that would end Helios (Quit from any
//! window, egui-pop's `quit`, the last window closed where that quits)
//! asks [`AppQuit`] once, from the before-hook of the main window's
//! `window.close` step (`crate::shell`'s quit hooks). Its listeners are
//! the quit's conditions, named, in a declared order (`after` /
//! `before`), each with its deadline:
//!
//! - [`JOBS`]: jobs are running and nobody confirmed cancelling them:
//!   it refuses, and the hook asks "Quit Helios?";
//! - any module holding something a quit would lose (an edit not saved)
//!   adds its own listener, and refuses with its reason.
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

use std::time::Duration;

use egui_pop::api::bus::bus_contract::{
    self, Bus, HookError, ListenOpts, Point, PointDecl, PointKind, Refused,
};

/// The request a quit is asked on.
pub const QUIT: &str = "helios.app.quit";

/// The running jobs' listener: refuses while jobs run unconfirmed.
pub const JOBS: &str = "helios.quit.jobs";

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

/// A quit, as its listeners read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuitAsk {
    pub why: QuitWhy,
    /// The jobs a quit would cancel.
    pub running_jobs: usize,
    /// Cancelling them is confirmed: asked and answered Quit, or
    /// confirmations answered yes by policy.
    pub jobs_confirmed: bool,
}

/// Helios quits (`helios.app.quit`): each listener says, in one line,
/// what it checked; one refuses to keep Helios open.
#[derive(Debug)]
pub struct AppQuit;

impl Point for AppQuit {
    const KIND: PointKind = PointKind::Request;
    const ID: &'static str = QUIT;
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

/// Registers Helios's own listeners on [`AppQuit`]: [`JOBS`]. One there
/// already is replaced.
///
/// # Errors
/// The bus refused it.
pub fn listen(bus: &dyn Bus) -> Result<(), String> {
    declare(bus);
    bus.unlisten(QUIT, JOBS);
    bus.listen::<AppQuit>(JOBS, ListenOpts::new().owner(OWNER), |ask, said| {
        if ask.running_jobs == 0 {
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
    .map_err(|e| e.to_string())
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
