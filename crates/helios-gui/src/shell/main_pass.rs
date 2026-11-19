//! A window drawn apart asks the main window's pass, on the bus.
//!
//! A focus in an OS window of its own (the Library, the import window) is
//! an egui deferred viewport: drawn in its own pass, after the main
//! window's. What it leaves for the shell (the Library's reads and
//! writes, the import window's events for the shell, its closing) is
//! taken by the main window's next pass. The window asks for that pass
//! with [`ask`]: a request ([`MainPass`], `window.main_pass`) asked
//! through bus-contract's `Defer`, which egui-pop's kernel wakes the main
//! window for and drains in its pass (egui-pop 0.13's guarantee: the bus
//! waker repaints the main window, so a deferred window's asks are
//! drained). No hand repaint of the main window is needed.
//!
//! The request carries the window asking; its listener
//! ([`listen`], the shell's) only traces it: the work is the main
//! window's pass itself.

use egui_pop::api::app::AppHandle;
use egui_pop::api::bus::bus_contract::{
    self, Bus, ListenOpts, Point, PointDecl, PointKind, Refused,
};

/// The request a window asks the main window's pass on.
pub const PASS: &str = "helios.window.main_pass";
/// The owner of the shell's listener.
pub const BUS_OWNER: &str = "helios.shell";
/// The shell's listener's id.
const HEARD_BY: &str = "helios.shell.main_pass";

/// A window drawn apart asks the main window's pass: the input names the
/// window (`library`, `import`).
pub struct MainPass;

impl Point for MainPass {
    const ID: &'static str = PASS;
    type Input = &'static str;
    type Value = ();
    const KIND: PointKind = PointKind::Request;
}

/// The point, its contract with the application's bus.
pub const POINTS: &[PointDecl] = &[PointDecl::of::<MainPass>(
    "A window drawn apart (the Library, the import window) asks the main window's pass, which takes what it left for the shell: heard after the drawing",
)
.fail_closed()];

/// Declares [`POINTS`] on `bus` (once).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "main pass: {refused}");
        }
    }
}

/// The shell's listener: the pass asked is the work, the listener traces
/// who asked.
pub fn listen(bus: &dyn Bus) {
    declare(bus);
    bus.unlisten(PASS, HEARD_BY);
    let listened = bus.listen_local::<MainPass>(
        HEARD_BY,
        ListenOpts::new().owner(BUS_OWNER),
        |window: &&'static str, (): &mut ()| {
            log::trace!(target: "helios", "{PASS}: asked by the {window} window");
            Ok(())
        },
    );
    if let Err(e) = listened {
        log::error!(target: "helios", "{PASS}: {e}");
    }
}

/// `window` asks the main window's pass: heard after the drawing, the
/// main window woken for it by the bus. On a bus without `Defer` (none
/// Helios builds), logged: the main window's next pass takes it.
pub fn ask(app: &AppHandle, window: &'static str) {
    declare(&*app.bus());
    if app.ask::<MainPass>(window, ()).is_none() {
        log::warn!(target: "helios", "{PASS}: the bus has no Defer, the {window} window waits");
    }
}
