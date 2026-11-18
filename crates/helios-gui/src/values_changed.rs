//! A change to the values, on the bus: `values.changed`.
//!
//! The value store ([`helios_values::Store`]) says each change in one go
//! through its change hook ([`helios_values::Store::on_change`]): a
//! write, an undo or a redo step, a gesture cancelled, a preview shown or
//! left. [`hook`] turns each into a request, [`ValuesChanged`]
//! (`values.changed`), asked through bus-contract's `Defer`: the hook
//! runs under the store's lock, so a listener heard inline would take the
//! store again and wait for ever. Asked while the panels draw, it is
//! heard after the drawing, in the same pass (egui-pop drains the asks
//! once per pass); asked from a window drawn apart (an application
//! window, a deferred viewport), at the main window's next pass, which
//! the bus wakes.
//!
//! The change names the surface whose store changed ([`MAIN_SURFACE`]
//! for the main window's), so a listener of one surface leaves the
//! others' changes. Its listeners are named and ordered: the darkroom's
//! ([`listen_darkroom`], `helios.darkroom.sync`) brings the graph shown
//! up to the values and renders it again, once per drain however many
//! changes a slider's drag asked.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::bus::bus_contract::{
    self, Bus, ListenOpts, Point, PointDecl, PointKind, Refused,
};
use helios_shell::commands::Origin;

use crate::viewport::Viewport;

/// The request a change to the values is asked on.
pub const CHANGED: &str = "values.changed";
/// The owner of the shell's listeners.
pub const BUS_OWNER: &str = "helios.shell";
/// The darkroom's listener: the graph shown follows the values.
pub const DARKROOM: &str = "helios.darkroom.sync";
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
    /// Who wrote; `None` for a preview shown or left.
    pub origin: Option<Origin>,
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
/// ([`ValuesChanged`]), heard after the drawing. Values are written on
/// the interface's thread, the one calling this: a debug build checks
/// it (an ask from another thread would wait for that thread's drain,
/// never made). On a bus without `Defer` (none Helios builds), a change
/// is logged and not heard: hearing it at once, under the store's lock,
/// would wait for ever.
pub fn hook(store: &mut helios_values::Store, bus: Arc<dyn Bus>, surface: u64) {
    declare(&*bus);
    let interface = std::thread::current().id();
    store.on_change(move |c| {
        debug_assert_eq!(
            std::thread::current().id(),
            interface,
            "{CHANGED}: values written off the interface's thread ({:?})",
            c.paths
        );
        let change = Change {
            surface,
            paths: c.paths.clone(),
            seq: c.seq,
            origin: c.origin.clone(),
        };
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
