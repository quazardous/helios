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
//!   is the one place in Helios that reads the monitors. winit has no
//!   hot-plug event, so the loop also re-reads them slowly
//!   ([`ScreenWatch`], every [`ScreenWatch::PERIOD`]) while a Helios window
//!   is focused or more than one screen is known: a monitor unplugged with
//!   no event on a Helios window is heard all the same. The re-read wakes
//!   the loop by its own timer (`ControlFlow::WaitUntil`, never a thread)
//!   and asks for a repaint only when the screens changed.
//!
//! - **The main window's frames owed** (`egui_pop::api::main_wake`). A
//!   compositor sends no frame callback to a window it does not show
//!   (GNOME under Wayland, the main window covered by Settings or by
//!   detached panels), so the system sends it no redraw, and eframe runs
//!   no pass of it; that pass is what lets a closed window go and draws
//!   the detached panels. A frame of the main window owed past the grace
//!   is painted by the loop itself.
//!
//! Everything else goes to eframe as it comes.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eframe::UserEvent;
use egui_pop::api::main_wake::{MainWake, PassCounter};
use egui_pop::api::window::{self, Screen};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
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
    let mut wake = MainWake::new();
    wake.set_enabled(egui_pop::api::main_wake::paint_safe(
        options.wgpu_options.surface.present_mode,
    ));
    // The main window and the context, known once eframe made them.
    let main: Rc<RefCell<Option<Main>>> = Rc::default();
    let main_in = main.clone();
    // eframe keeps a creation error inside the handler `create_native`
    // returns: kept here for the caller.
    let made = Rc::new(RefCell::new(Creation::Pending));
    let made_in = made.clone();
    // The context the screen watch asks its repaints of, once made.
    let context: Rc<RefCell<Option<egui::Context>>> = Rc::default();
    let context_in = context.clone();
    let creator: eframe::AppCreator<'_> = Box::new(move |cc| {
        if let Some(w) = cc.winit_window() {
            *main_in.borrow_mut() = Some(Main {
                window: w.id(),
                ctx: cc.egui_ctx.clone(),
                counter: PassCounter::install(&cc.egui_ctx),
            });
        }
        match creator(cc) {
            Ok(app) => {
                *context_in.borrow_mut() = Some(cc.egui_ctx.clone());
                *made_in.borrow_mut() = Creation::Done;
                Ok(app)
            }
            Err(e) => {
                let told: Box<dyn std::error::Error + Send + Sync> =
                    Box::new(Reported(e.to_string()));
                *made_in.borrow_mut() = Creation::Failed(eframe::Error::AppCreation(e));
                Err(told)
            }
        }
    });
    let mut runner = Runner {
        eframe: eframe::create_native(title, options, creator, &event_loop),
        wake,
        main,
        context,
        watch: ScreenWatch::new(None),
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

/// The monitors as winit sees them.
fn read_screens(event_loop: &ActiveEventLoop) -> Vec<Screen> {
    event_loop
        .available_monitors()
        .map(|m| {
            let p = m.position();
            let s = m.size();
            Screen::from_physical(m.name(), (p.x, p.y), (s.width, s.height), m.scale_factor())
        })
        .collect()
}

/// The loop's slow re-read of the monitors, for the unplug winit says no
/// event of. It runs while a Helios window is focused (the user is there
/// to see a window left off-screen) or while more than one screen is
/// known (a window may sit on one that can go, focused or not); with one
/// screen and no focus nothing can be stranded, and the next focus reads
/// the monitors at once. Testing "a window on a non-primary screen"
/// itself would need every window's place, which eframe keeps, and a
/// primary monitor Wayland does not name: more screens than one is the
/// cheap superset.
pub struct ScreenWatch {
    ctx: Option<egui::Context>,
    focused: Option<WindowId>,
    next: Option<Instant>,
}

impl ScreenWatch {
    /// How often the monitors are re-read, while watched.
    pub const PERIOD: Duration = Duration::from_secs(3);

    /// A watch asking its repaints of `ctx` (none: no repaint asked).
    pub fn new(ctx: Option<egui::Context>) -> ScreenWatch {
        ScreenWatch {
            ctx,
            focused: None,
            next: None,
        }
    }

    /// Window `id` gained or lost the focus.
    pub fn focused(&mut self, id: WindowId, focused: bool) {
        if focused {
            self.focused = Some(id);
        } else if self.focused == Some(id) {
            self.focused = None;
        }
    }

    fn watched(&self) -> bool {
        self.focused.is_some() || window::screen_list().len() > 1
    }

    /// When the loop must wake to re-read the monitors, from `now`; none
    /// while not watched.
    pub fn next_wake(&mut self, now: Instant) -> Option<Instant> {
        if !self.watched() {
            self.next = None;
            return None;
        }
        Some(*self.next.get_or_insert(now + Self::PERIOD))
    }

    /// At `now`, past the wake: the monitors re-read by `read` and told.
    /// True when they changed.
    pub fn wake(&mut self, now: Instant, read: impl FnOnce() -> Vec<Screen>) -> bool {
        match self.next {
            Some(at) if now >= at => {
                self.next = None;
                self.tell(read())
            }
            _ => false,
        }
    }

    /// Tells egui-pop `screens` when they changed, with a repaint of the
    /// main window, whose pass says the change on the bus. True when
    /// they changed.
    pub fn tell(&self, screens: Vec<Screen>) -> bool {
        if window::screen_list() == screens {
            return false;
        }
        log::debug!(target: "helios::event_loop", "screens: {screens:?}");
        window::set_screens(screens);
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        true
    }
}

/// The main window eframe made, and the application's context.
struct Main {
    window: WindowId,
    ctx: egui::Context,
    counter: PassCounter,
}

/// The loop's handler: the screens first, eframe for everything.
struct Runner<'a> {
    eframe: eframe::EframeWinitApplication<'a>,
    /// The main window's frames owed, painted when the system withholds
    /// its redraw.
    wake: MainWake<WindowId>,
    main: Rc<RefCell<Option<Main>>>,
    context: Rc<RefCell<Option<egui::Context>>>,
    watch: ScreenWatch,
}

impl Runner<'_> {
    /// The monitors read and told egui-pop, when they changed.
    fn tell_screens(&mut self, event_loop: &ActiveEventLoop) {
        self.adopt_context();
        self.watch.tell(read_screens(event_loop));
    }

    /// The watch asks its repaints of eframe's context, once made.
    fn adopt_context(&mut self) {
        if self.watch.ctx.is_none() {
            self.watch.ctx.clone_from(&self.context.borrow());
        }
    }

    /// The main window known: the wake counts its passes.
    fn know_main(&mut self) {
        if self.wake.main().is_some() {
            return;
        }
        if let Some(m) = self.main.borrow().as_ref() {
            self.wake.set_main(m.window);
            self.wake.set_counter(m.counter.clone());
        }
    }

    /// Hands a window's redraw to eframe, through the wake.
    fn redraw(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        self.know_main();
        let eframe = &mut self.eframe;
        self.wake.redraw(id, || {
            eframe.window_event(event_loop, id, WindowEvent::RedrawRequested);
        });
    }

    /// Paints the main window when its frame is owed past the grace.
    fn paint_owed(&mut self, event_loop: &ActiveEventLoop) {
        self.know_main();
        if let Some(main) = self.wake.due(Instant::now()) {
            log::debug!(target: "helios::event_loop", "the main window's redraw is withheld: painted");
            self.redraw(event_loop, main);
        }
    }
}

impl ApplicationHandler<UserEvent> for Runner<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.tell_screens(event_loop);
        self.eframe.resumed(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Moved(_) => {
                self.tell_screens(event_loop);
            }
            WindowEvent::Focused(focused) => {
                self.watch.focused(id, focused);
                if focused {
                    self.tell_screens(event_loop);
                }
            }
            WindowEvent::Destroyed => self.watch.focused(id, false),
            _ => {}
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            self.redraw(event_loop, id);
            return;
        }
        self.eframe.window_event(event_loop, id, event);
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        self.adopt_context();
        self.watch.wake(Instant::now(), || read_screens(event_loop));
        self.eframe.new_events(event_loop, cause);
        self.paint_owed(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        self.know_main();
        if let Some(m) = self.main.borrow().as_ref() {
            self.wake.user_event(&m.ctx, &event);
        }
        self.eframe.user_event(event_loop, event);
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, device: DeviceId, event: DeviceEvent) {
        self.eframe.device_event(event_loop, device, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.paint_owed(event_loop);
        self.eframe.about_to_wait(event_loop);
        // eframe set the wake its repaints need; the re-read's and the
        // owed frame's, if sooner. Waking asks no repaint (eframe repaints
        // only a window whose time came), so idle stays idle.
        if event_loop.exiting() {
            return;
        }
        let next = [self.watch.next_wake(Instant::now()), self.wake.next()]
            .into_iter()
            .flatten()
            .min();
        if let Some(at) = next {
            let flow = match event_loop.control_flow() {
                ControlFlow::Poll => ControlFlow::Poll,
                ControlFlow::Wait => ControlFlow::WaitUntil(at),
                ControlFlow::WaitUntil(t) => ControlFlow::WaitUntil(t.min(at)),
            };
            event_loop.set_control_flow(flow);
        }
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
