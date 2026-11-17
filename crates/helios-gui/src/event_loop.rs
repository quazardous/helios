//! Helios's event loop: eframe on a winit loop Helios runs
//! (`eframe::create_native`), so that what egui-pop needs from the system
//! and only the loop sees can be told to it. egui-pop is built without its
//! own runner (`create-native`): Helios keeps the loop.
//!
//! What the loop tells egui-pop:
//!
//! - **The screens** (`egui_pop::api::window::set_screens`), each monitor
//!   named as winit names it (a screen plugged in again is recognised),
//!   its rectangle in points and its scale factor. Read in `resumed`,
//!   before eframe's own `resumed` creates the main window, so the place
//!   kept from the last run is repaired against them
//!   (`shell::place_main_window`); read again when a window's scale factor
//!   changes or a window moves (onto another monitor, maybe), so a window
//!   opened later (a detached panel, the import window) is repaired against
//!   the screens of the moment. egui-pop says a change on its bus
//!   (`screen.removed`, `screen.added`, by name) and moves the windows on
//!   a screen gone onto one left, a detached panel docking back. This loop
//!   is the one place in Helios that reads the monitors.
//!
//! Everything else goes to eframe as it comes.

use std::cell::RefCell;
use std::rc::Rc;

use eframe::UserEvent;
use egui_pop::api::window::{self, Screen};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::run_on_demand::EventLoopExtRunOnDemand;
use winit::window::WindowId;

/// Runs eframe on Helios's loop until its last window closes, as
/// `eframe::run_native` does (the call returns, and what follows the
/// window runs).
pub fn run(
    title: &str,
    mut options: eframe::NativeOptions,
    creator: eframe::AppCreator<'_>,
) -> eframe::Result {
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    if let Some(hook) = options.event_loop_builder.take() {
        hook(&mut builder);
    }
    let mut event_loop = builder.build()?;
    // eframe keeps a creation error inside the handler `create_native`
    // returns: kept here for the caller.
    let made = Rc::new(RefCell::new(Creation::Pending));
    let made_in = made.clone();
    let creator: eframe::AppCreator<'_> = Box::new(move |cc| match creator(cc) {
        Ok(app) => {
            *made_in.borrow_mut() = Creation::Done;
            Ok(app)
        }
        Err(e) => {
            let told: Box<dyn std::error::Error + Send + Sync> = Box::new(Reported(e.to_string()));
            *made_in.borrow_mut() = Creation::Failed(eframe::Error::AppCreation(e));
            Err(told)
        }
    });
    let mut runner = Runner {
        eframe: eframe::create_native(title, options, creator, &event_loop),
    };
    event_loop.run_app_on_demand(&mut runner)?;
    drop(runner);
    match std::mem::replace(&mut *made.borrow_mut(), Creation::Pending) {
        Creation::Done => Ok(()),
        Creation::Failed(e) => Err(e),
        Creation::Pending => Err(eframe::Error::AppCreation(Box::new(Reported(
            "eframe stopped before the application was made (its window or renderer could not start; see the log)".into(),
        )))),
    }
}

/// What became of the application's creation.
enum Creation {
    Pending,
    Done,
    Failed(eframe::Error),
}

/// An error eframe gets in place of the one kept for the caller.
#[derive(Debug)]
struct Reported(String);

impl std::fmt::Display for Reported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Reported {}

/// Tells egui-pop the monitors, when they changed.
fn tell_screens(event_loop: &ActiveEventLoop) {
    let screens: Vec<Screen> = event_loop
        .available_monitors()
        .map(|m| {
            let p = m.position();
            let s = m.size();
            Screen::from_physical(m.name(), (p.x, p.y), (s.width, s.height), m.scale_factor())
        })
        .collect();
    if window::screen_list() != screens {
        log::debug!(target: "helios::event_loop", "screens: {screens:?}");
        window::set_screens(screens);
    }
}

/// The loop's handler: the screens first, eframe for everything.
struct Runner<'a> {
    eframe: eframe::EframeWinitApplication<'a>,
}

impl ApplicationHandler<UserEvent> for Runner<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        tell_screens(event_loop);
        self.eframe.resumed(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if matches!(
            event,
            WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Moved(_)
        ) {
            tell_screens(event_loop);
        }
        self.eframe.window_event(event_loop, id, event);
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        self.eframe.new_events(event_loop, cause);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        self.eframe.user_event(event_loop, event);
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, device: DeviceId, event: DeviceEvent) {
        self.eframe.device_event(event_loop, device, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.eframe.about_to_wait(event_loop);
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.eframe.suspended(event_loop);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        self.eframe.exiting(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        self.eframe.memory_warning(event_loop);
    }
}
