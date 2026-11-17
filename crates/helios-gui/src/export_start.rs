//! An export asked for, on the bus: a request heard in the pass it is
//! asked in.
//!
//! `library.export` (the command, or the export dialog's answer) passes
//! the export once through a request point ([`ExportStart`],
//! `export.start`) with the workspace's network it renders through. Its
//! listener, `export.run`, is the host's: it hands the export to the
//! exporter, which plans it over the library through the node on a thread
//! of its own and runs it as jobs. With no listener (no library open: the
//! exports take their images from one), or one that refuses, the request
//! is refused with its reason. The pass is said on the journal:
//! `export.started` with the images counted, the folder and the listener
//! that ran, or `export.refused` with who refused and why. A plan that
//! fails later, on its thread, comes back to the host's notices.
//!
//! The listener holds the host's exporter: a local listener
//! ([`listen_local`]), bound to the interface's thread, where the command
//! runs.

use egui_pop::api::bus::bus_contract::{
    self, Bus, ListenOpts, Point, PointDecl, PointKind, Refused,
};
use serde_json::{Value, json};

use crate::shell::ExportAsk;

/// The request to start an export.
pub const START: &str = "export.start";
/// The journal's event once an export started.
pub const STARTED: &str = "export.started";
/// The journal's event when the request was refused.
pub const REFUSED: &str = "export.refused";
/// The listener that runs an export: the host's.
pub const RUN: &str = "export.run";
/// The owner of the listener.
pub const BUS_OWNER: &str = "helios.export";

/// Why an export is refused when no listener ran it.
pub const NO_LIBRARY: &str = "no library open: exports take their images from one";

/// An export asked for, as the listener reads it.
#[derive(Clone, Debug)]
pub struct Asked {
    /// The images, the settings, the words of the finished batch.
    pub ask: ExportAsk,
    /// The workspace's network the images render through.
    pub bundle: helios_template::Bundle,
}

/// A refusal: the listener that refused (or the point, when none ran),
/// and why.
pub use crate::request::Refusal;

/// What the listeners did.
#[derive(Clone, Debug, Default)]
pub struct Started {
    /// The listeners that ran, in their order.
    pub ran: Vec<String>,
    /// The request was refused, by whom and why.
    pub refused: Option<Refusal>,
}

impl crate::request::Refusable for Started {
    fn ran(&mut self) -> &mut Vec<String> {
        &mut self.ran
    }
    fn refusal(&mut self) -> &mut Option<Refusal> {
        &mut self.refused
    }
}

/// The request to start an export.
pub struct ExportStart;

impl Point for ExportStart {
    const ID: &'static str = START;
    type Input = Asked;
    type Value = Started;
    const KIND: PointKind = PointKind::Request;
}

/// The export point, this crate's contract with the application's bus:
/// declared by [`declare`].
pub const POINTS: &[PointDecl] = &[PointDecl::of::<ExportStart>(
    "Start an export: the host's exporter plans and runs it; refused with its reason when no library is open",
)];

/// Declares [`POINTS`] on `bus` (once; a second desktop on the same bus
/// finds them there).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "export: {refused}");
        }
    }
}

/// Registers `run` as the export's listener ([`RUN`]) on `bus`: a local
/// listener bound to the calling thread (the interface's). `run` refuses
/// the export with `Err(reason)`. One there already is replaced.
///
/// # Errors
/// The bus refused it.
pub fn listen_local(
    bus: &dyn Bus,
    run: impl Fn(&Asked) -> Result<(), String> + 'static,
) -> Result<(), String> {
    declare(bus);
    bus.unlisten(START, RUN);
    bus.listen_local::<ExportStart>(
        RUN,
        ListenOpts::new().owner(BUS_OWNER),
        crate::request::listener(RUN, move |asked, _: &mut Started| run(asked)),
    )
    .map_err(|e| e.to_string())
}

/// Passes `asked` through `export.start` on `bus`: what the listeners
/// did; refused by the point itself when none ran ([`NO_LIBRARY`]).
/// Fail-closed ([`crate::request::pass`]): a listener that failed (an
/// error, a panic, past its deadline) refuses the export.
pub fn pass(bus: &dyn Bus, asked: Asked) -> Started {
    declare(bus);
    let mut started = crate::request::pass::<ExportStart>(bus, asked, Started::default());
    if started.ran.is_empty() && started.refused.is_none() {
        started.refused = Some(Refusal {
            by: START.to_owned(),
            reason: NO_LIBRARY.to_owned(),
        });
    }
    started
}

/// The journal's words for a pass: the images counted, the folder, the
/// listeners that ran, and the refusal if any.
pub fn said(asked: &Asked, started: &Started) -> Value {
    let mut data = json!({
        "images": asked.ask.images.len(),
        "folder": asked.ask.settings.folder,
        "ran": started.ran,
    });
    if let Some(r) = &started.refused {
        data["refused"] = json!({ "by": r.by, "reason": r.reason });
    }
    data
}

/// A listener ([`RUN`]) that keeps the exports asked, for a host that
/// runs them later itself (a headless harness, a test): what it kept.
///
/// # Errors
/// The bus refused it.
pub fn keep(bus: &dyn Bus) -> Result<std::rc::Rc<std::cell::RefCell<Vec<Asked>>>, String> {
    let kept = std::rc::Rc::<std::cell::RefCell<Vec<Asked>>>::default();
    let into = kept.clone();
    listen_local(bus, move |asked| {
        into.borrow_mut().push(asked.clone());
        Ok(())
    })?;
    Ok(kept)
}
