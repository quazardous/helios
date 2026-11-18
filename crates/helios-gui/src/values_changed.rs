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
//!   drain returns, in the same pass.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::app::AppHandle;
use egui_pop::api::bus::bus_contract::{
    self, Bus, ListenOpts, Point, PointDecl, PointKind, Refused,
};
use helios_shell::commands::Origin;
use serde_json::json;

use crate::edit_marks::{self as em, Marks};
use crate::viewport::Viewport;

/// The request a change to the values is asked on.
pub const CHANGED: &str = "values.changed";
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
}

/// A change to the values, asked after it is made: heard after the
/// drawing.
pub struct ValuesChanged;

impl Point for ValuesChanged {
    const ID: &'static str = CHANGED;
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
