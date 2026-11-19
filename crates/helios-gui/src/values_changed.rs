//! A change to the values, on the bus: `values.changed`.
//!
//! The value store ([`helios_values::Store`]) says each change in one go
//! through its change hook ([`helios_values::Store::on_change`]): a
//! write, an undo or a redo step, a gesture cancelled or ended, a
//! preview shown or left, the history cleared or left for another image.
//! [`hook`] turns each into a request, [`ValuesChanged`]
//! (`values.changed`), asked through bus-contract's `Defer`: the hook
//! runs under the store's lock, so a listener heard inline would take the
//! store again and wait for ever. Asked while the panels draw, it is
//! heard after the drawing, in the same pass (egui-pop drains the asks
//! once per pass); asked before the drawing (a command the shell runs
//! first) or from a window drawn apart since the last pass (an
//! application window, a deferred viewport), at the drain the shell makes
//! before the main window's drawing, which the bus wakes. A value written
//! on another thread (a camera look fitted off the frame) is heard at the
//! interface's next pass too, before the drawing.
//!
//! The change names the surface whose store changed ([`MAIN_SURFACE`]
//! for the main window's), so a listener of one surface leaves the
//! others' changes. Its listeners are named and ordered: the darkroom's
//! ([`listen_darkroom`], `helios.darkroom.sync`) brings the graph shown
//! up to the values and renders it again, once per drain however many
//! changes a slider's drag asked. The others run after it:
//! - the edit's marks ([`listen_edit_marks`], `helios.edit.marks`),
//!   read again and passed through `edit.changed` when they moved;
//! - the workspace's state ([`WORKSPACE`]) and the workshop session's
//!   record ([`SESSION`]), which only the shell can put in place: their
//!   listeners note them due ([`Due`]), and the shell does them once the
//!   drain returns, in the same pass;
//! - the Decisions panel's cards ([`listen_decisions`], [`DECISIONS`]),
//!   made again from the book and the graph the darkroom just brought up
//!   to the values;
//! - the Queue panel ([`listen_queue`], [`QUEUE`]), noted due when the
//!   selected node changed, as it is when a job event is said
//!   ([`QUEUE_JOBS`], after egui-pop's jobs model);
//! - the journal ([`listen_journal`], [`JOURNAL`]), which says
//!   [`PARAM_CHANGED`] once per step or gesture: a slider's drag asks a
//!   change per frame, written inside its gesture, gathered until the
//!   gesture's step (or its cancel, or its end) closes it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::app::AppHandle;
use egui_pop::api::bus::bus_contract::PointId;
use egui_pop::api::bus::bus_contract::{
    self, Bus, ListenOpts, Point, PointDecl, PointKind, Refused,
};
use egui_pop::api::bus::bus_contract_defer::BusDefer as _;
use helios_shell::commands::Origin;
use serde_json::json;

use crate::edit_marks::{self as em, Marks};
use crate::viewport::Viewport;

/// The request a change to the values is asked on.
pub const CHANGED: &str = "helios.values.changed";
/// The owner of the shell's listeners.
pub const BUS_OWNER: &str = "helios.shell";
/// The darkroom's listener: the graph shown follows the values.
pub const DARKROOM: &str = "helios.darkroom.sync";
/// The edit's marks' listener: read again after the graph follows.
pub const EDIT_MARKS: &str = "helios.edit.marks";
/// The workspace's state put in place (an edit, an undo, a redo of it).
pub const WORKSPACE: &str = "helios.workspace.sync";
/// The workshop session's record follows the value steps.
pub const SESSION: &str = "helios.workshop.session";
/// The Decisions panel's cards follow the graph shown, after the
/// darkroom brought it up to the values.
pub const DECISIONS: &str = "helios.decisions.follow";
/// The Queue panel follows the selected node.
pub const QUEUE: &str = "helios.queue.selection";
/// The Queue panel follows the jobs (after egui-pop's jobs model).
pub const QUEUE_JOBS: &str = "helios.queue.jobs";
/// The journal's listener: says [`PARAM_CHANGED`], once per gesture.
pub const JOURNAL: &str = "helios.param.journal";
/// The journal's event: values changed, once per step or gesture (a
/// drag's frames gathered), with the paths, the sequence numbers and who
/// wrote.
pub const PARAM_CHANGED: &str = "helios.param.changed";
/// The main window's surface: its store, its darkroom.
pub const MAIN_SURFACE: u64 = 0;

/// A change to the values, in one go: what the store's change hook heard,
/// and the surface whose store it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    /// The surface whose store changed ([`MAIN_SURFACE`]).
    pub surface: u64,
    /// The paths changed, each once, in the order first changed.
    pub paths: Vec<String>,
    /// The sequence numbers the writes took, first and last; `None` for
    /// a preview shown or left (nothing written).
    pub seq: Option<(u64, u64)>,
    /// Who wrote; `None` for a preview shown or left, the history
    /// cleared or left.
    pub origin: Option<Origin>,
    /// Whether the undo history moved with it: a step made, undone or
    /// redone, a gesture ended, the history cleared or left for another
    /// image.
    pub history: bool,
    /// Whether it was written inside an open gesture or group (a drag's
    /// frame): its step, its cancel or its end is heard after it.
    pub gesture: bool,
}

/// A change to the values, asked after it is made: heard after the
/// drawing.
pub struct ValuesChanged;

impl Point for ValuesChanged {
    const ID: PointId = PointId::new(CHANGED);
    type Input = Change;
    type Value = ();
    const KIND: PointKind = PointKind::Request;
}

/// The point, its contract with the application's bus.
pub const POINTS: &[PointDecl] = &[PointDecl::of::<ValuesChanged>(
    "A change to a surface's values (a write, an undo or a redo step, a gesture cancelled, a preview shown or left), in one go: heard after the drawing, the darkroom first",
)
.fail_closed()];

/// Declares [`POINTS`] on `bus` (once).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "values: {refused}");
        }
    }
}

/// Gives `store` the hook that asks each change on `bus` as `surface`'s
/// ([`ValuesChanged`]). Written on the interface's thread (the one
/// calling this), the change is asked through `Defer`: heard after the
/// drawing. Written on another thread (a camera look fitted off the
/// frame), it is emitted there: the listeners, local to the interface's
/// thread, are queued for its next pass, which the bus wakes; never heard
/// on the writing thread, under the store's lock. Every listener of the
/// point is local for that reason. On a bus without `Defer` (none Helios
/// builds), a change written on the interface's thread is logged and not
/// heard: hearing it at once, under the store's lock, would wait for ever.
pub fn hook(store: &mut helios_values::Store, bus: Arc<dyn Bus>, surface: u64) {
    declare(&*bus);
    let interface = std::thread::current().id();
    store.on_change(move |c| {
        let change = Change {
            surface,
            paths: c.paths.clone(),
            seq: c.seq,
            origin: c.origin.clone(),
            history: c.history,
            gesture: c.gesture,
        };
        if std::thread::current().id() != interface {
            // The local listeners are queued for the interface's thread.
            let altered = bus.alter::<ValuesChanged>(change, ());
            debug_assert!(
                altered.refused.is_none() && altered.failures.is_empty(),
                "{CHANGED}: a listener heard on the writing thread"
            );
            return;
        }
        match bus.defer() {
            Some(defer) => {
                defer.ask::<ValuesChanged>(change, ());
            }
            None => {
                log::warn!(target: "helios", "{CHANGED}: the bus has no Defer, {change:?} unheard");
            }
        }
    });
}

/// The darkroom's listener ([`DARKROOM`]) on `surface`'s changes: the
/// graph `viewport` shows follows `values`, and renders again when they
/// moved it. A drag asks once per frame; the first change of a drain
/// brings the graph up to every value written, the next find nothing to
/// do, so one render is asked per drain. The viewport busy (a drain made
/// while it is borrowed), the change is asked again for the next drain.
pub fn listen_darkroom(
    bus: &Arc<dyn Bus>,
    surface: u64,
    viewport: Rc<RefCell<Viewport>>,
    values: Arc<Mutex<helios_values::Store>>,
) {
    declare(&**bus);
    bus.unlisten(CHANGED, DARKROOM);
    let again = bus.clone();
    let listened = bus.listen_local::<ValuesChanged>(
        DARKROOM,
        ListenOpts::new().owner(BUS_OWNER),
        move |change: &Change, (): &mut ()| {
            if change.surface != surface {
                return Ok(());
            }
            let Ok(mut viewport) = viewport.try_borrow_mut() else {
                if let Some(defer) = again.defer() {
                    defer.ask::<ValuesChanged>(change.clone(), ());
                }
                return Ok(());
            };
            viewport.sync(&mut values.lock().unwrap_or_else(PoisonError::into_inner));
            Ok(())
        },
    );
    if let Err(e) = listened {
        log::error!(target: "helios", "{CHANGED}: {e}");
    }
}

/// The edit's marks' listener ([`EDIT_MARKS`], after the darkroom's) on
/// `surface`'s changes: the marks `values` hold, read again, and when
/// they moved, passed through `edit.changed` (each of its listeners'
/// parts, then said on the journal with those that ran). `marks` holds
/// them as last passed; busy (a drain made while a panel reads them),
/// the change is asked again for the next drain.
pub fn listen_edit_marks(
    app: &AppHandle,
    surface: u64,
    values: Arc<Mutex<helios_values::Store>>,
    marks: Rc<RefCell<Marks>>,
) {
    let bus = app.bus();
    declare(&*bus);
    for (id, _) in em::ORDER {
        if let Err(e) = em::listen(&*bus, id) {
            log::error!(target: "helios", "{}: {e}", em::CHANGED);
        }
    }
    bus.unlisten(CHANGED, EDIT_MARKS);
    let app = app.clone();
    let listened = bus.listen_local::<ValuesChanged>(
        EDIT_MARKS,
        ListenOpts::new().owner(BUS_OWNER).after(DARKROOM),
        move |change: &Change, (): &mut ()| {
            if change.surface != surface {
                return Ok(());
            }
            let read = Marks::read(&values.lock().unwrap_or_else(PoisonError::into_inner));
            let Ok(shown) = marks.try_borrow() else {
                if let Some(defer) = app.bus().defer() {
                    defer.ask::<ValuesChanged>(change.clone(), ());
                }
                return Ok(());
            };
            if *shown == read {
                return Ok(());
            }
            drop(shown);
            let mut data = read.to_json();
            let ran = em::pass(&*app.bus(), read.clone());
            if ran.iter().any(|id| id == em::MARKS) {
                *marks.borrow_mut() = read;
            }
            data["ran"] = json!(ran);
            app.publish("helios", None, em::CHANGED, data);
            Ok(())
        },
    );
    if let Err(e) = listened {
        log::error!(target: "helios", "{CHANGED}: {e}");
    }
}

/// What the values' listeners leave to the shell, which alone can do it
/// (it needs the whole shell): noted while the bus drains, done once the
/// drain returns, in the same pass.
#[derive(Default)]
pub struct Due {
    /// The workspace's state changed: put in place ([`WORKSPACE`]).
    pub workspace: Cell<bool>,
    /// The history moved: the workshop session's record follows it
    /// ([`SESSION`]).
    pub session: Cell<bool>,
    /// The selected node changed, or a job event was said: the Queue
    /// panel reads its node's queue again ([`QUEUE`], [`QUEUE_JOBS`]).
    pub queue: Cell<bool>,
}

/// The listeners that note the shell's parts due on `surface`'s changes,
/// after the darkroom's and the marks': the workspace's when `state`
/// changed ([`WORKSPACE`]), the session's when the history moved
/// ([`SESSION`]).
pub fn listen_due(bus: &Arc<dyn Bus>, surface: u64, state: &'static str, due: &Rc<Due>) {
    declare(&**bus);
    note_due(bus, surface, WORKSPACE, EDIT_MARKS, due, move |c, due| {
        if c.paths.iter().any(|p| p == state) {
            due.workspace.set(true);
        }
    });
    note_due(bus, surface, SESSION, WORKSPACE, due, |c, due| {
        if c.history {
            due.session.set(true);
        }
    });
}

/// Registers `id`, after `after`: `note` on each of `surface`'s changes.
fn note_due(
    bus: &Arc<dyn Bus>,
    surface: u64,
    id: &'static str,
    after: &'static str,
    due: &Rc<Due>,
    note: impl Fn(&Change, &Due) + 'static,
) {
    bus.unlisten(CHANGED, id);
    let due = due.clone();
    let listened = bus.listen_local::<ValuesChanged>(
        id,
        ListenOpts::new().owner(BUS_OWNER).after(after),
        move |change: &Change, (): &mut ()| {
            if change.surface == surface {
                note(change, &due);
            }
            Ok(())
        },
    );
    if let Err(e) = listened {
        log::error!(target: "helios", "{CHANGED}: {e}");
    }
}

/// The Decisions panel's listener ([`DECISIONS`], after the darkroom's)
/// on `surface`'s changes: its cards made again from the book and the
/// graph shown, which the darkroom just brought up to the values (a
/// question changed there makes its decision pending again). Busy (a
/// drain made while the viewport or the panel is borrowed), the change is
/// asked again for the next drain.
pub fn listen_decisions(
    bus: &Arc<dyn Bus>,
    surface: u64,
    viewport: Rc<RefCell<Viewport>>,
    decisions: Rc<RefCell<crate::decisions::Decisions>>,
) {
    declare(&**bus);
    bus.unlisten(CHANGED, DECISIONS);
    let again = bus.clone();
    let listened = bus.listen_local::<ValuesChanged>(
        DECISIONS,
        ListenOpts::new().owner(BUS_OWNER).after(DARKROOM),
        move |change: &Change, (): &mut ()| {
            if change.surface != surface {
                return Ok(());
            }
            let (Ok(viewport), Ok(mut decisions)) =
                (viewport.try_borrow(), decisions.try_borrow_mut())
            else {
                if let Some(defer) = again.defer() {
                    defer.ask::<ValuesChanged>(change.clone(), ());
                }
                return Ok(());
            };
            decisions.refresh(viewport.graph());
            Ok(())
        },
    );
    if let Err(e) = listened {
        log::error!(target: "helios", "{CHANGED}: {e}");
    }
}

/// The Queue panel's listeners: noted due ([`Due::queue`]) when
/// `surface`'s `selection` changed ([`QUEUE`], after the darkroom's), and
/// on each job event, once egui-pop's jobs model took it in
/// ([`QUEUE_JOBS`]). The shell reads the queue once the drain returns.
pub fn listen_queue(bus: &Arc<dyn Bus>, surface: u64, selection: &'static str, due: &Rc<Due>) {
    use egui_pop::api::bus::JobEvents;
    declare(&**bus);
    bus.unlisten(CHANGED, QUEUE);
    let noted = due.clone();
    let listened = bus.listen_local::<ValuesChanged>(
        QUEUE,
        ListenOpts::new().owner(BUS_OWNER).after(DARKROOM),
        move |change: &Change, (): &mut ()| {
            if change.surface == surface && change.paths.iter().any(|p| p == selection) {
                noted.queue.set(true);
            }
            Ok(())
        },
    );
    if let Err(e) = listened {
        log::error!(target: "helios", "{CHANGED}: {e}");
    }
    bus.unlisten(&JobEvents::ID, QUEUE_JOBS);
    let noted = due.clone();
    let listened = bus.listen_local::<JobEvents>(
        QUEUE_JOBS,
        ListenOpts::new()
            .owner(BUS_OWNER)
            .after(egui_pop::api::jobs::MODEL),
        move |_, (): &mut ()| {
            noted.queue.set(true);
            Ok(())
        },
    );
    if let Err(e) = listened {
        log::error!(target: "helios", "{}: {e}", JobEvents::ID);
    }
}

/// Values changed, as the journal says them ([`PARAM_CHANGED`]): a step
/// or a gesture whole.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Said {
    /// The paths written, each once, in the order first written.
    pub paths: Vec<String>,
    /// The sequence numbers the writes took, first and last.
    pub seq: (u64, u64),
    /// Who wrote.
    pub origin: Option<Origin>,
    /// Whether it made an undo step (a write, a gesture ended into one,
    /// an undo, a redo); not for a gesture cancelled, nor for transient
    /// values (the view).
    pub step: bool,
}

/// Gathers a surface's changes into what the journal says: one per change
/// outside a gesture, one per gesture whole (its frames, then its step,
/// its cancel or its end). A preview shown or left, and the history
/// moving with nothing written, say nothing; a change heard again (asked
/// again by a busy listener) is not counted twice.
#[derive(Debug, Default)]
pub struct Gather {
    pending: Option<Said>,
    /// The sequence numbers of the last changes taken in, to know one
    /// heard again (a thread's change may be heard after a later one).
    heard: std::collections::VecDeque<(u64, u64)>,
}

/// How many changes [`Gather`] remembers to know one heard again.
const HEARD: usize = 64;

impl Gather {
    /// Takes `change` in: what to say once it closes what was gathered.
    pub fn take(&mut self, change: &Change) -> Option<Said> {
        if let Some((first, last)) = change.seq {
            if self.heard.contains(&(first, last)) {
                return None;
            }
            if self.heard.len() == HEARD {
                self.heard.pop_front();
            }
            self.heard.push_back((first, last));
            let said = self.pending.get_or_insert_with(|| Said {
                seq: (first, last),
                ..Said::default()
            });
            for p in &change.paths {
                if !said.paths.contains(p) {
                    said.paths.push(p.clone());
                }
            }
            said.seq = (said.seq.0.min(first), said.seq.1.max(last));
            if change.origin.is_some() {
                said.origin.clone_from(&change.origin);
            }
        }
        if change.gesture {
            return None;
        }
        let mut said = self.pending.take()?;
        said.step = change.history;
        if said.origin.is_none() {
            said.origin.clone_from(&change.origin);
        }
        Some(said)
    }
}

/// The journal's listener ([`JOURNAL`], after the darkroom's) on
/// `surface`'s changes: [`PARAM_CHANGED`] said once per step or gesture
/// ([`Gather`]), with the surface, the paths, the sequence numbers, who
/// wrote and whether it made a step.
pub fn listen_journal(app: &AppHandle, surface: u64) {
    let bus = app.bus();
    declare(&*bus);
    bus.unlisten(CHANGED, JOURNAL);
    let app = app.clone();
    let gather = RefCell::new(Gather::default());
    let listened = bus.listen_local::<ValuesChanged>(
        JOURNAL,
        ListenOpts::new().owner(BUS_OWNER).after(DARKROOM),
        move |change: &Change, (): &mut ()| {
            if change.surface != surface {
                return Ok(());
            }
            let Some(said) = gather.borrow_mut().take(change) else {
                return Ok(());
            };
            app.publish(
                "helios",
                None,
                PARAM_CHANGED,
                json!({
                    "surface": surface,
                    "paths": said.paths,
                    "seq": [said.seq.0, said.seq.1],
                    "origin": said.origin,
                    "step": said.step,
                }),
            );
            Ok(())
        },
    );
    if let Err(e) = listened {
        log::error!(target: "helios", "{CHANGED}: {e}");
    }
}
