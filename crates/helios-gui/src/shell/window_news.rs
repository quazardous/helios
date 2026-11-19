//! What comes in for the Library and calibration windows, said on the
//! bus.
//!
//! The Library's thumbnails and head renders come from threads of their
//! own; the calibration window's answers land from the threads its
//! commands run on. Each says so on its window's point ([`LibraryNews`],
//! `helios.library.window.news`; [`CalibrateNews`],
//! `helios.calibrate.window.news`), from any thread ([`say`]). The
//! window's listener is registered for the window ([`listen`], egui-pop's
//! `AppHandle::listen_local_in`), as the import window's is
//! ([`super::import_news`]): an emit from another thread queues it for
//! the interface's thread, the bus waker asks for the main window's pass,
//! which runs it and drains what came in, and its run asks for that
//! window's pass, and no other window's; the main window's while the
//! window is drawn in it, not open or going. No hand repaint.

use std::sync::Weak;

use egui_pop::api::app::AppHandle;
use egui_pop::api::bus::bus_contract::PointId;
use egui_pop::api::bus::bus_contract::{
    self, Bus, ListenOpts, Point, PointDecl, PointKind, Refused,
};
use egui_pop::api::windows::WindowId;

/// Something came in for the Library window: thumbnails, renders of
/// head versions, or what the main pass took for it.
pub struct LibraryNews;

impl Point for LibraryNews {
    const ID: PointId = PointId::new("helios.library.window.news");
    type Input = ();
    type Value = ();
    const KIND: PointKind = PointKind::Alter;
}

/// An answer the calibration window asked landed.
pub struct CalibrateNews;

impl Point for CalibrateNews {
    const ID: PointId = PointId::new("helios.calibrate.window.news");
    type Input = ();
    type Value = ();
    const KIND: PointKind = PointKind::Alter;
}

/// The points, their contract with the application's bus.
pub const POINTS: &[PointDecl] = &[
    PointDecl::of::<LibraryNews>(
        "Something came in for the Library window (thumbnails, renders of head versions, what the main pass took for it): its listener, registered for the window, wakes it",
    ),
    PointDecl::of::<CalibrateNews>(
        "An answer the calibration window asked landed: its listener, registered for the window, wakes it",
    ),
];

/// The owner of the windows' listeners.
const BUS_OWNER: &str = "helios.windows";

/// Declares [`POINTS`] on `bus` (once).
fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "window news: {refused}");
        }
    }
}

/// The listener of `P`, `heard_by`, registered for the window `window`:
/// hearing the news is the work, egui-pop wakes the window for it.
pub fn listen<P: Point<Input = (), Value = ()>>(app: &AppHandle, window: &str, heard_by: &str) {
    let bus = app.bus();
    declare(&*bus);
    bus.unlisten(&P::ID, heard_by);
    let listened = app.listen_local_in::<P>(
        &WindowId::new(window),
        heard_by,
        ListenOpts::new().owner(BUS_OWNER),
        |(), ()| Ok(()),
    );
    if let Err(e) = listened {
        log::error!(target: "helios", "{}: {e}", P::ID);
    }
}

/// Says `P` on `bus`, from any thread; nothing once the bus is gone.
pub fn say<P: Point<Input = (), Value = ()>>(bus: &Weak<dyn Bus>) {
    if let Some(bus) = bus.upgrade() {
        bus.alter::<P>((), ());
    }
}
