//! An import's end, published once.
//!
//! Every import the desktop runs ends here, whichever path ran it: the
//! `library.import` command's thread, the photos chosen in a service's
//! page imported in-process, and the node's import promise (heard through
//! [`NodeWatch`]). Each calls [`Publisher::ended`] or
//! [`Publisher::failed`] on the import's thread; the shell then passes
//! the end through the hook point [`ImportEnded`] (or [`ImportFailed`])
//! on the interface's thread, its listeners in the order [`ORDER`] (or
//! [`FAILED_ORDER`]) declares, and says `import.ended` (or
//! `import.failed`) on the journal.
//!
//! The shell's own listeners hold the interface's state (`Rc`), not
//! `Send + Sync`: they are local listeners ([`listen_local`]), bound to
//! the interface's thread, where the shell passes the end: they run there
//! inline, in the declared order with the others.
//!
//! The two points are this crate's contract with the application's bus
//! ([`POINTS`]): the desktop's `main` builds the one bus and hands it to
//! the shell, which declares them there; the host's listeners
//! (`backups.on-import`, `library.refill`) and the shell's register on the
//! same bus, each with its owner and the listener it runs after.
//!
//! The cameras' default looks are put on the import's raws by
//! [`Publisher::ended`] itself, before the point, on every path: their
//! changes belong to the import's own undo step, and they write the
//! vault, which is not done on the interface's thread. The node's
//! promise puts them on before it says the end, so it passes their count.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use egui_pop::api::bus::bus_contract::PointId;
use egui_pop::api::bus::bus_contract::{
    self, Bus, HookError, ListenOpts, Point, PointDecl, Refused,
};
use helios_library::import::Report;
use helios_vault::undo::Changes;
use serde_json::{Value, json};

/// The hook point an import's end passes through, and the journal's
/// event once it did.
pub const ENDED: &str = "helios.import.ended";
/// The hook point a failed import passes through, and the journal's
/// event once it did.
pub const FAILED: &str = "helios.import.failed";
/// The owner of the shell's listeners of [`ImportEnded`].
pub const BUS_OWNER: &str = "helios.import";

/// The import's undo step recorded.
pub const HISTORY: &str = "helios.library.history";
/// The storage strategy's "after each import" asked of the node.
pub const BACKUPS: &str = "helios.backups.on-import";
/// The Library filled again.
pub const REFILL: &str = "helios.library.refill";
/// The cards the import read, short of their backups, wait for one.
pub const CARDS: &str = "helios.storage.cards-waiting";
/// The Duplicates panel holds every duplicate the import found.
pub const DUPLICATES: &str = "helios.duplicates.collect";
/// The import's outcome in the Log panel.
pub const LOG: &str = "helios.log.import";
/// The import's outcome in a toast.
pub const TOAST: &str = "helios.import.toast";
/// The Library's import sessions read again; one the user started shown.
pub const SESSION: &str = "helios.library.import-session";
/// The import window's History read again.
pub const WINDOW_HISTORY: &str = "helios.import-window.history";
/// The photos the import brought selected once the Library shows them.
pub const SELECT: &str = "helios.library.select-imported";

/// The listeners of [`ImportEnded`], each with the one it runs after:
/// the order they run in. The host registers [`BACKUPS`] and [`REFILL`]
/// (it holds the node's backups and the Library's fill); the shell the
/// others.
pub const ORDER: &[(&str, Option<&str>)] = &[
    (HISTORY, None),
    (BACKUPS, Some(HISTORY)),
    (REFILL, Some(BACKUPS)),
    (CARDS, Some(REFILL)),
    (DUPLICATES, Some(CARDS)),
    (LOG, Some(DUPLICATES)),
    (TOAST, Some(LOG)),
    (SESSION, Some(TOAST)),
    (WINDOW_HISTORY, Some(SESSION)),
    (SELECT, Some(WINDOW_HISTORY)),
];

/// The listeners of [`ImportFailed`], in order: no undo step, no backup,
/// nothing selected; what the import brought before it failed is shown
/// (the Library filled, the duplicates it found), and the failure said.
pub const FAILED_ORDER: &[(&str, Option<&str>)] = &[
    (REFILL, None),
    (DUPLICATES, Some(REFILL)),
    (LOG, Some(DUPLICATES)),
    (TOAST, Some(LOG)),
    (SESSION, Some(TOAST)),
    (WINDOW_HISTORY, Some(SESSION)),
];

/// The hook point an import's end passes through.
pub struct ImportEnded;

impl Point for ImportEnded {
    const ID: PointId = PointId::new(ENDED);
    type Input = ImportEnd;
    type Value = Reactions;
}

/// The hook point a failed import passes through.
pub struct ImportFailed;

impl Point for ImportFailed {
    const ID: PointId = PointId::new(FAILED);
    type Input = ImportEnd;
    type Value = Reactions;
}

/// The import's points, this crate's contract with the application's
/// bus: declared by [`declare`].
pub const POINTS: &[PointDecl] = &[
    PointDecl::of::<ImportEnded>("An import ended: what the desktop does after it"),
    PointDecl::of::<ImportFailed>("An import failed: what the desktop does after it"),
];

/// An import's end, as [`ImportEnded`]'s listeners read it.
#[derive(Clone, Debug)]
pub struct ImportEnd {
    /// The import's request (`library.import`'s arguments, or the node
    /// promise's), with `origin`.
    pub request: Value,
    /// Who asked for it (`origin` of the request; `user` when unsaid).
    pub origin: String,
    /// Its report (its changes taken into [`Reactions::changes`]), or
    /// why it failed.
    pub outcome: Result<Report, String>,
    /// How many of its raws took their camera's default look.
    pub camera_looks: usize,
}

impl ImportEnd {
    /// The end line the shell's listeners read: `{ "ended": report,
    /// "camera_looks": n }` or `{ "failed": error }`.
    pub fn line(&self) -> Value {
        match &self.outcome {
            Ok(report) => json!({ "ended": report, "camera_looks": self.camera_looks }),
            Err(e) => json!({ "failed": e }),
        }
    }

    /// The end of an import said by its end line alone (`line` as
    /// [`ImportEnd::line`] makes it): no changes to record.
    pub fn of_line(request: &Value, line: &Value) -> Option<ImportEnd> {
        let outcome = if let Some(e) = line.get("failed") {
            Err(e.as_str().map_or_else(|| e.to_string(), String::from))
        } else {
            Ok(report_of(line.get("ended")?))
        };
        Some(ImportEnd {
            request: request.clone(),
            origin: origin_of(request),
            outcome,
            camera_looks: line["camera_looks"].as_u64().unwrap_or(0) as usize,
        })
    }

    /// The point it passes through: [`ENDED`] or [`FAILED`].
    pub fn point(&self) -> &'static str {
        if self.outcome.is_ok() { ENDED } else { FAILED }
    }
}

/// What the import points' listeners did, in order, and what they leave.
#[derive(Clone, Debug, Default)]
pub struct Reactions {
    /// The listeners that ran, in their order.
    pub ran: Vec<String>,
    /// The import's changes, until [`HISTORY`] records them.
    pub changes: Option<Changes>,
    /// How many duplicates wait for an answer once [`DUPLICATES`] took
    /// those the import found.
    pub asked: usize,
    /// Whether [`SELECT`] selects the photos the import brought (the
    /// user's import, said by [`SESSION`]).
    pub select: bool,
}

/// Declares [`POINTS`] on `bus` (once; a second shell on the same bus
/// finds them there).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "import: {refused}");
        }
    }
}

/// A listener's place in one of the points: absent, or after the
/// listener named (none: first).
type Place = Option<Option<&'static str>>;

/// Where listener `id` runs in the import's points: after the listener
/// [`ORDER`] names (in [`ImportEnded`]) and after the one
/// [`FAILED_ORDER`] names (in [`ImportFailed`]), when they name it.
///
/// # Errors
/// `id` is in neither order.
fn places(id: &str) -> Result<(Place, Place), String> {
    let place = |order: &[(&str, Option<&'static str>)]| {
        order
            .iter()
            .find(|(listener, _)| *listener == id)
            .map(|(_, after)| *after)
    };
    let (ended, failed) = (place(ORDER), place(FAILED_ORDER));
    if ended.is_none() && failed.is_none() {
        return Err(format!("`{id}` is not a listener of {ENDED} nor {FAILED}"));
    }
    Ok((ended, failed))
}

fn opts(after: Option<&str>) -> ListenOpts {
    let opts = ListenOpts::new().owner(BUS_OWNER);
    match after {
        Some(after) => opts.after(after),
        None => opts,
    }
}

/// Registers `run` as the listener `id` of the import's points on `bus`:
/// of [`ImportEnded`] and of [`ImportFailed`] when their order
/// ([`ORDER`], [`FAILED_ORDER`]) names it, after the listener it names
/// there; one there already under `id` is replaced.
///
/// # Errors
/// `id` is in neither order, or the bus refused it.
pub fn listen(
    bus: &dyn Bus,
    id: &'static str,
    run: impl Fn(&ImportEnd, &mut Reactions) + Send + Sync + 'static,
) -> Result<(), String> {
    let (ended, failed) = places(id)?;
    declare(bus);
    let run = Arc::new(run);
    if let Some(after) = ended {
        bus.unlisten(ENDED, id);
        let run = run.clone();
        bus.listen::<ImportEnded>(id, opts(after), move |end, reactions| {
            run(end, reactions);
            reactions.ran.push(id.to_owned());
            Ok::<(), HookError>(())
        })
        .map_err(|e| e.to_string())?;
    }
    if let Some(after) = failed {
        bus.unlisten(FAILED, id);
        bus.listen::<ImportFailed>(id, opts(after), move |end, reactions| {
            run(end, reactions);
            reactions.ran.push(id.to_owned());
            Ok::<(), HookError>(())
        })
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// [`listen`] for a listener that holds the interface's state (not
/// `Send + Sync`): a local listener, bound to the calling thread (the
/// interface's), in the same declared place. An end passed on that
/// thread runs it inline, in order with the others; passed from another
/// thread, it runs at that thread's next pass (`Altered::deferred`).
///
/// # Errors
/// `id` is in neither order, or the bus refused it.
pub fn listen_local(
    bus: &dyn Bus,
    id: &'static str,
    run: impl Fn(&ImportEnd, &mut Reactions) + 'static,
) -> Result<(), String> {
    let (ended, failed) = places(id)?;
    declare(bus);
    let run = Rc::new(run);
    if let Some(after) = ended {
        bus.unlisten(ENDED, id);
        let run = run.clone();
        bus.listen_local::<ImportEnded>(id, opts(after), move |end, reactions| {
            run(end, reactions);
            reactions.ran.push(id.to_owned());
            Ok::<(), HookError>(())
        })
        .map_err(|e| e.to_string())?;
    }
    if let Some(after) = failed {
        bus.unlisten(FAILED, id);
        bus.listen_local::<ImportFailed>(id, opts(after), move |end, reactions| {
            run(end, reactions);
            reactions.ran.push(id.to_owned());
            Ok::<(), HookError>(())
        })
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Passes `end` through its point ([`ImportEnded`], or [`ImportFailed`]
/// when it failed) on `bus`: the listeners that ran, in order, and what
/// they left; the failures of those that failed.
pub fn pass(
    bus: &dyn Bus,
    end: ImportEnd,
    reactions: Reactions,
) -> bus_contract::Altered<Reactions> {
    if end.outcome.is_ok() {
        bus.alter::<ImportEnded>(end, reactions)
    } else {
        bus.alter::<ImportFailed>(end, reactions)
    }
}

/// The order the import points' listeners run in on `bus`.
pub fn order(bus: &dyn Bus) -> String {
    bus_contract::order_dump(bus, &[ENDED, FAILED])
}

/// Puts the cameras' default looks on the raws of an import (its
/// changes, to which theirs are added); how many took one.
pub type CameraLooks = Arc<dyn Fn(&mut Changes) -> Result<usize, String> + Send + Sync>;

/// Where an import's end is said, from the import's thread.
#[derive(Clone)]
pub struct Publisher {
    ends: Sender<ImportEnd>,
    looks: Option<CameraLooks>,
    wake: Option<egui::Context>,
}

impl Publisher {
    /// A publisher sending to `ends`.
    pub fn new(ends: Sender<ImportEnd>) -> Publisher {
        Publisher {
            ends,
            looks: None,
            wake: None,
        }
    }

    /// The cameras' looks put on the raws of the imports it says ended.
    #[must_use]
    pub fn camera_looks(mut self, looks: CameraLooks) -> Publisher {
        self.looks = Some(looks);
        self
    }

    /// `ctx` woken once an end is said.
    #[must_use]
    pub fn waking(mut self, ctx: egui::Context) -> Publisher {
        self.wake = Some(ctx);
        self
    }

    /// The import asked by `request` ended with `report`; `camera_looks`,
    /// the looks put on its raws already (the node's promise), else they
    /// are put on now unless the request says `camera_look: false`.
    pub fn ended(&self, request: &Value, mut report: Report, camera_looks: Option<usize>) {
        let camera_looks = match (camera_looks, &self.looks) {
            (Some(n), _) => n,
            (None, Some(looks)) if request.get("camera_look") != Some(&json!(false)) => {
                looks(&mut report.changes).unwrap_or_else(|e| {
                    log::warn!(target: "helios", "import: camera look: {e}");
                    0
                })
            }
            (None, _) => 0,
        };
        self.send(ImportEnd {
            request: request.clone(),
            origin: origin_of(request),
            outcome: Ok(report),
            camera_looks,
        });
    }

    /// The import asked by `request` failed with `error`.
    pub fn failed(&self, request: &Value, error: &str) {
        log::warn!(target: "helios", "import: {error}");
        self.send(ImportEnd {
            request: request.clone(),
            origin: origin_of(request),
            outcome: Err(error.to_owned()),
            camera_looks: 0,
        });
    }

    fn send(&self, end: ImportEnd) {
        let _ = self.ends.send(end);
        if let Some(ctx) = &self.wake {
            ctx.request_repaint();
        }
    }
}

/// What the desktop hears of an import its node runs as a promise: each
/// file's line for its Duplicates and Log panels, then the end, published
/// as every import's is.
pub struct NodeWatch {
    lines: Sender<(Value, Value)>,
    ends: Publisher,
}

impl NodeWatch {
    /// Lines to `lines` (the shell's `import_sink`), the end to `ends`.
    pub fn new(lines: Sender<(Value, Value)>, ends: Publisher) -> NodeWatch {
        NodeWatch { lines, ends }
    }
}

impl helios_node_core::import_promise::ImportWatch for NodeWatch {
    fn line(&self, request: &Value, line: &Value) {
        let _ = self.lines.send((request.clone(), line.clone()));
    }

    fn ended(&self, request: &Value, report: &mut Report, camera_looks: usize) {
        let mut ended = report.clone();
        ended.changes = std::mem::take(&mut report.changes);
        self.ends.ended(request, ended, Some(camera_looks));
    }

    fn failed(&self, request: &Value, error: &str) {
        self.ends.failed(request, error);
    }
}

/// Several imports' reports as one (the batches of photos chosen in a
/// service's page): their counts added up, their changes and cards kept.
pub fn merged(reports: Vec<Report>) -> Report {
    let mut all = reports.into_iter();
    let Some(mut into) = all.next() else {
        return Report::default();
    };
    for r in all {
        into.files += r.files;
        into.added += r.added;
        into.located += r.located;
        into.unchanged += r.unchanged;
        into.skipped += r.skipped;
        into.asked += r.asked;
        into.failed += r.failed;
        into.cancelled |= r.cancelled;
        into.copied += r.copied;
        into.fetched += r.fetched;
        into.unfetched += r.unfetched;
        into.retried += r.retried;
        into.sidecars += r.sidecars;
        into.edits += r.edits;
        into.edits_reported += r.edits_reported;
        into.edits_failed += r.edits_failed;
        into.metadata += r.metadata;
        into.paired += r.paired;
        into.read += r.read;
        into.cards.extend(r.cards);
        into.changes.images.extend(r.changes.images);
        into.changes.documents.extend(r.changes.documents);
        into.copies.extend(r.copies);
        if into.history.is_none() {
            into.history = r.history;
        }
    }
    into
}

pub use helios_storage_ui::card_of;

/// Who asked for an import, by its request.
fn origin_of(request: &Value) -> String {
    request["origin"].as_str().unwrap_or("user").to_owned()
}

/// A report read back from its JSON (an end said as a line): the counts
/// and the cards the listeners read; no changes.
fn report_of(v: &Value) -> Report {
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    Report {
        files: n("files"),
        added: n("added"),
        located: n("located"),
        unchanged: n("unchanged"),
        skipped: n("skipped"),
        asked: n("asked"),
        failed: n("failed"),
        cancelled: v["cancelled"] == true,
        copied: n("copied"),
        fetched: n("fetched"),
        unfetched: n("unfetched"),
        retried: n("retried"),
        sidecars: n("sidecars"),
        edits: n("edits"),
        edits_reported: n("edits_reported"),
        edits_failed: n("edits_failed"),
        metadata: n("metadata"),
        paired: n("paired"),
        read: n("read"),
        history: v["history"].as_str().map(String::from),
        cards: v["cards"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(card_of)
            .collect(),
        ..Report::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_library::import::Card;

    #[test]
    fn listeners_run_in_the_declared_order_whatever_order_they_register_in() {
        let bus: Arc<dyn Bus> = Arc::new(egui_pop::api::bus::egui_pop_hooks::Kernel::new());
        for (id, _) in ORDER.iter().rev() {
            listen(&*bus, id, |_, _| {}).unwrap();
        }
        let end = |outcome| ImportEnd {
            request: json!({}),
            origin: "user".into(),
            outcome,
            camera_looks: 0,
        };
        let ran = pass(&*bus, end(Ok(Report::default())), Reactions::default())
            .value
            .ran;
        let declared: Vec<&str> = ORDER.iter().map(|(id, _)| *id).collect();
        assert_eq!(ran, declared);
        let ran = pass(&*bus, end(Err("gone".into())), Reactions::default())
            .value
            .ran;
        let declared: Vec<&str> = FAILED_ORDER.iter().map(|(id, _)| *id).collect();
        assert_eq!(ran, declared);
    }

    #[test]
    fn a_listener_not_declared_is_refused() {
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::new();
        assert!(listen(&bus, "nobody.knows", |_, _| {}).is_err());
        assert!(listen_local(&bus, "nobody.knows", |_, _| {}).is_err());
    }

    /// Local listeners (the interface's state, `Rc`) and shared ones
    /// run in one declared order; passed on their thread, a local
    /// listener's work is done inside the pass, nothing deferred.
    #[test]
    fn local_listeners_run_inside_the_pass_in_the_declared_order() {
        let bus: Arc<dyn Bus> = Arc::new(egui_pop::api::bus::egui_pop_hooks::Kernel::new());
        let seen = Rc::new(std::cell::RefCell::new(Vec::new()));
        for (id, _) in ORDER.iter().rev() {
            if [HISTORY, BACKUPS, REFILL].contains(id) {
                listen(&*bus, id, |_, _| {}).unwrap();
            } else {
                let seen = seen.clone();
                listen_local(&*bus, id, move |_, r| {
                    seen.borrow_mut().push(*id);
                    if *id == DUPLICATES {
                        r.asked = 2;
                    }
                })
                .unwrap();
            }
        }
        let end = ImportEnd {
            request: json!({}),
            origin: "user".into(),
            outcome: Ok(Report::default()),
            camera_looks: 0,
        };
        let altered = pass(&*bus, end, Reactions::default());
        assert!(altered.deferred.is_empty(), "{:?}", altered.deferred);
        let declared: Vec<&str> = ORDER.iter().map(|(id, _)| *id).collect();
        assert_eq!(altered.value.ran, declared);
        assert_eq!(*seen.borrow(), declared[3..]);
        assert_eq!(altered.value.asked, 2);
        assert!(order(&*bus).contains("local"), "{}", order(&*bus));
    }

    #[test]
    fn merged_reports_add_their_counts_and_keep_the_first_session() {
        let a = Report {
            added: 2,
            files: 2,
            history: Some("h1".into()),
            ..Report::default()
        };
        let b = Report {
            added: 1,
            files: 3,
            skipped: 2,
            history: Some("h2".into()),
            ..Report::default()
        };
        let m = merged(vec![a, b]);
        assert_eq!((m.added, m.files, m.skipped), (3, 5, 2));
        assert_eq!(m.history.as_deref(), Some("h1"));
    }

    #[test]
    fn an_end_line_reads_back_as_the_end_it_said() {
        let request = json!({ "origin": "agent" });
        let end = ImportEnd::of_line(
            &request,
            &json!({ "ended": { "added": 3, "history": "h" }, "camera_looks": 2 }),
        )
        .unwrap();
        assert_eq!(end.origin, "agent");
        assert_eq!(end.camera_looks, 2);
        assert_eq!(end.outcome.unwrap().added, 3);
        let failed = ImportEnd::of_line(&request, &json!({ "failed": "gone" })).unwrap();
        assert_eq!(failed.outcome.unwrap_err(), "gone");
        assert!(ImportEnd::of_line(&request, &json!({ "status": "added" })).is_none());
    }

    #[test]
    fn an_end_line_keeps_the_cards_the_import_read() {
        let card = Card {
            path: "/media/EOS_DIGITAL".into(),
            label: "EOS_DIGITAL".into(),
            photos: 1,
            want: 1,
            short: 1,
            root: "/library".into(),
            files: vec![("/media/EOS_DIGITAL/DCIM/1.JPG".into(), None)],
            ..Card::default()
        };
        let report = Report {
            added: 1,
            cards: vec![card.clone()],
            ..Report::default()
        };
        let end = ImportEnd::of_line(&json!({}), &json!({ "ended": report })).unwrap();
        assert_eq!(end.outcome.unwrap().cards, [card]);
    }
}
