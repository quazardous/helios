//! What comes in for the import window off the interface's thread, said
//! on the bus.
//!
//! The import window's work runs on threads of its own: a scan reading a
//! folder, the accounts read, a picking or an import of chosen photos
//! ending, a card plugged in or gone. Each says so with [`ImportNews`]
//! (`import.window.news`), emitted from its thread ([`say`]). The
//! window's listener is registered for the window ([`listen`], egui-pop's
//! `AppHandle::listen_local_in`): an emit from another thread queues it
//! for the interface's thread, the bus waker asks for the main window's
//! pass, which runs it (the main window's toasts and cards read there),
//! and its run asks for the import window's pass; the main window's while
//! the window is drawn in it, not open or going. No hand repaint.

use std::sync::Weak;

use egui_pop::api::app::AppHandle;
use egui_pop::api::bus::bus_contract::{
    self, Bus, ListenOpts, Point, PointDecl, PointKind, Refused,
};
use egui_pop::api::windows::WindowId;

/// The point the import window's news are said on.
pub const NEWS: &str = "import.window.news";
/// The owner of the window's listener.
const BUS_OWNER: &str = "helios.import";
/// The window's listener's id.
const HEARD_BY: &str = "helios.import.window";

/// Something came in for the import window off the interface's thread.
pub struct ImportNews;

impl Point for ImportNews {
    const ID: &'static str = NEWS;
    type Input = ();
    type Value = ();
    const KIND: PointKind = PointKind::Alter;
}

/// The point, its contract with the application's bus.
pub const POINTS: &[PointDecl] = &[PointDecl::of::<ImportNews>(
    "Something came in for the import window off the interface's thread (a scan, an account read, a card, an import of chosen photos ending): its listener, registered for the window, wakes it",
)];

/// Declares [`POINTS`] on `bus` (once).
fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "import news: {refused}");
        }
    }
}

/// The window's listener, registered for the window `window`: hearing
/// the news is the work, egui-pop wakes the window for it.
pub fn listen(app: &AppHandle, window: &str) {
    let bus = app.bus();
    declare(&*bus);
    bus.unlisten(NEWS, HEARD_BY);
    let listened = app.listen_local_in::<ImportNews>(
        &WindowId::new(window),
        HEARD_BY,
        ListenOpts::new().owner(BUS_OWNER),
        |(), ()| Ok(()),
    );
    if let Err(e) = listened {
        log::error!(target: "helios", "{NEWS}: {e}");
    }
}

/// Says the import window's news on `bus`, from any thread; nothing once
/// the bus is gone.
pub fn say(bus: &Weak<dyn Bus>) {
    if let Some(bus) = bus.upgrade() {
        bus.alter::<ImportNews>((), ());
    }
}
