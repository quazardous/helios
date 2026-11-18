//! A focus in an OS window of its own (ROADMAP, "One window is one
//! focus"): how the Activity, Calibration and Library foci open, come to
//! the front and close, written once.
//!
//! The focus's state is shared (`Rc<RefCell<…>>`) by the shell and the
//! window's content. The window opens while the focus is open and closes
//! with it; closed by itself, the focus closes and the main window stays.
//! Asked again while open, the window comes to the front. Where the
//! backend has no windows of its own (kittest, the web), the focus is
//! drawn in a window of the main one instead. egui-pop keeps each
//! window's size and place under its id, and gives them back when a
//! window of that id opens again.
//!
//! Which of these windows are open is kept too ([`OPEN_DOC`]), and they
//! open again at the next start ([`super::Shell::restore_focus_windows`]).

use std::cell::RefCell;
use std::rc::Rc;

use egui::{Context as Egui, Vec2, ViewportId};
use egui_pop::api::app::AppHandle;
use egui_pop::api::bus::bus_contract::{HookError, ListenOpts};
use egui_pop::api::bus::{PLACE, REGISTRY, Reason, WindowClose};
use egui_pop::api::windows::{WindowId, WindowSpec};

/// The document, under the interface's persistence directory, listing
/// the focus windows open (by window id), opened again at the next start.
pub(super) const OPEN_DOC: &str = "focus-windows.json";

/// What a focus drawn in a window of its own gives the window layer.
pub(super) trait WindowFocus: 'static {
    /// Whether the focus is open (its window shown or to be).
    fn is_open(&self) -> bool;
    fn set_open(&mut self, open: bool);
    /// The OS window opened for it, not seen closed since.
    fn shown(&mut self) -> &mut Option<WindowId>;
    /// The window's title, in the interface's language.
    fn title(&self, ctx: &Egui) -> String;
    /// The window's content.
    fn window_ui(&mut self, ui: &mut egui::Ui);
    /// Whether the content queued something for the main window's pass.
    fn asks_main_pass(&self) -> bool;
}

/// Opens a focus, its window brought to the front when open already.
pub(super) fn open<T: WindowFocus>(app: &AppHandle, state: &Rc<RefCell<T>>) {
    let mut s = state.borrow_mut();
    if let Some(id) = s.shown().as_ref() {
        app.raise_window(id);
    }
    s.set_open(true);
}

/// One focus's window: its id and sizes.
pub(super) struct FocusWindow {
    /// The OS window's id, under which egui-pop keeps its size.
    pub(super) window: &'static str,
    /// The id of the window of the main one where the backend has no
    /// windows of its own.
    pub(super) embedded: &'static str,
    pub(super) min_size: Vec2,
    pub(super) max_default_size: Vec2,
    /// The share of the main window a first opening takes.
    pub(super) share: f32,
}

impl FocusWindow {
    pub(super) fn id(&self) -> WindowId {
        WindowId::new(self.window)
    }

    /// The size a first opening takes, beside a main window of `main`.
    pub(super) fn default_size(&self, main: Vec2) -> Vec2 {
        let max = (main - egui::vec2(32.0, 32.0)).max(Vec2::ZERO);
        (main * self.share)
            .min(self.max_default_size)
            .max(self.min_size)
            .min(max)
    }

    /// Whether the focus's content holds the main window: moved into it
    /// when the main window was closed while it was open.
    pub(super) fn holds_main_window(&self, app: &AppHandle) -> bool {
        app.window_viewport(&self.id()) == Some(ViewportId::ROOT)
    }

    /// The owner of the focus's window on egui-pop's bus (ADR 0029):
    /// `helios.<window>`, who asks its opening and whose listener of
    /// `window.close` closes the focus.
    pub(super) fn owner(&self) -> String {
        format!("helios.{}", self.window)
    }

    /// The focus's listener of egui-pop's `window.close`, local to the
    /// interface's thread: whatever closes its window (its title bar, a
    /// command, Escape, the compositor, the main window's close), the
    /// focus closes in the same pass, after the registry let the window
    /// go and before its place is kept.
    ///
    /// # Errors
    /// The bus refused the listener.
    pub(super) fn listen_close<T: WindowFocus>(
        &'static self,
        app: &AppHandle,
        state: &Rc<RefCell<T>>,
    ) -> Result<(), String> {
        let state = state.clone();
        let owner = self.owner();
        listen_window_close(app, &owner, self.window, move |did| {
            let mut s = state
                .try_borrow_mut()
                .map_err(|_| HookError::failed("the focus is being drawn"))?;
            s.set_open(false);
            *s.shown() = None;
            did.push(format!("{}: closed", self.window));
            Ok(())
        })
    }

    /// Each frame of the main window: the focus's OS window opened when
    /// the focus opened, as its owner's `window.open` request, and asked
    /// closed when the focus closed (a command); closed by itself, its
    /// listener of `window.close` ([`FocusWindow::listen_close`]) closed
    /// the focus already.
    pub(super) fn follow<T: WindowFocus>(
        &'static self,
        app: &AppHandle,
        ctx: &Egui,
        state: &Rc<RefCell<T>>,
    ) {
        if ctx.embed_viewports() {
            self.embedded_ui(ctx, state);
            return;
        }
        let id = self.id();
        let there = app.window_viewport(&id).is_some();
        let mut s = state.borrow_mut();
        if s.is_open() && !there {
            let title = s.title(ctx);
            let spec = WindowSpec::new(self.window, title)
                .size(self.default_size(ctx.content_rect().size()))
                .min_size(self.min_size)
                .shell_bars(true);
            *s.shown() = Some(id.clone());
            drop(s);
            let state = state.clone();
            app.request_window_open(spec, &self.owner(), Reason::Command, move |ui, window| {
                let Ok(mut s) = state.try_borrow_mut() else {
                    return;
                };
                if s.is_open() && !window.close_requested() {
                    egui::CentralPanel::default().show(ui, |ui| s.window_ui(ui));
                    if s.asks_main_pass() {
                        super::main_pass::ask(window.app(), self.window);
                    }
                    return;
                }
                let app = window.app().clone();
                if self.holds_main_window(&app) {
                    // Moved into the main window (closed while it was
                    // open): the shell comes back there rather than the
                    // last window closing.
                    window.keep_open();
                    s.set_open(false);
                    drop(s);
                    app.raise_window(&WindowId::main());
                } else {
                    // Closed from the window itself (Escape, its close
                    // button): its `window.close` at the end of this pass.
                    window.close();
                }
            });
        } else if !s.is_open() && there {
            drop(s);
            if self.holds_main_window(app) {
                app.raise_window(&WindowId::main());
            } else {
                app.close_window(&id);
            }
        }
    }

    /// The focus in a window of the main one (kittest, the web).
    fn embedded_ui<T: WindowFocus>(&self, ctx: &Egui, state: &Rc<RefCell<T>>) {
        let Ok(mut s) = state.try_borrow_mut() else {
            return;
        };
        if !s.is_open() {
            return;
        }
        let title = s.title(ctx);
        let main = ctx.content_rect().size();
        let max = (main - egui::vec2(32.0, 32.0)).max(Vec2::ZERO);
        let size = self.default_size(main);
        let mut open = true;
        egui::Window::new(&title)
            .id(egui::Id::unique(self.embedded))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(size)
            .min_size(self.min_size.min(max))
            .max_size(max)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.set_min_size(size - egui::vec2(16.0, 40.0));
                s.window_ui(ui);
            });
        if !open {
            s.set_open(false);
        }
    }
}

/// Registers `close` as `owner`'s listener of egui-pop's `window.close`
/// for the window `window`, local to the interface's thread (the state
/// it closes is `Rc`), after the registry let the window go and before
/// its place is kept: run for that window's closes alone. Closed by the
/// person from the window itself, the main window comes forward.
///
/// # Errors
/// The bus refused the listener.
pub(super) fn listen_window_close(
    app: &AppHandle,
    owner: &str,
    window: &'static str,
    close: impl Fn(&mut Vec<String>) -> Result<(), HookError> + 'static,
) -> Result<(), String> {
    let bus = app.bus();
    let id = format!("{owner}.close");
    bus.unlisten(
        <WindowClose as egui_pop::api::bus::bus_contract::Point>::ID,
        &id,
    );
    bus.listen_local::<WindowClose>(
        id,
        ListenOpts::new().owner(owner).after(REGISTRY).before(PLACE),
        move |request, did| {
            if request.window != window {
                return Ok(());
            }
            close(did)?;
            // Closed by the person from the window itself (its close
            // button, Escape, the system's shortcut): the main window,
            // behind it, comes forward.
            if request.reason == Reason::User
                && let Some(app) = egui_pop::api::bus::asking()
            {
                super::raise_main_window(&app);
                did.push(format!("{window}: the main window brought forward"));
            }
            Ok(())
        },
    )
    .map_err(|e| e.to_string())
}

impl super::Shell {
    /// The listeners of egui-pop's `window.close` closing the state of
    /// Helios's own windows: the Activity, Calibration and Library foci
    /// and the import window.
    pub(super) fn listen_window_closes(&mut self) {
        let app = self.core.app.handle().clone();
        let listened = [
            super::activity_focus::FRAME.listen_close(&app, &self.activity_focus.state),
            super::calibrate_focus::FRAME.listen_close(&app, &self.calibration.state),
            super::library_focus::FRAME.listen_close(&app, &self.library_focus.state),
            self.listen_import_window_close(),
        ];
        for e in listened.into_iter().filter_map(Result::err) {
            self.notes.push(format!("window.close: {e}"));
        }
    }

    /// The order the listeners of `window.close` run in (egui-pop's and
    /// Helios's windows').
    pub fn window_close_order(&self) -> String {
        egui_pop::api::bus::order_dump(
            &*self.core.app.handle().bus(),
            &[<WindowClose as egui_pop::api::bus::bus_contract::Point>::ID],
        )
    }

    /// The focus windows open, by window id, in a fixed order.
    fn open_focus_windows(&self) -> Vec<&'static str> {
        let mut open = Vec::new();
        if self.activity_focus.state.borrow().is_open() {
            open.push(super::activity_focus::WINDOW);
        }
        if self.library_focus.state.borrow().is_open() {
            open.push(super::library_focus::WINDOW);
        }
        open
    }

    /// Each frame: which focus windows are open, kept for the next start
    /// when it changed. Not while quitting: the windows closing then
    /// are the ones to open again.
    pub(super) fn keep_focus_windows(&mut self) {
        let app = self.core.app.handle().clone();
        if app.quitting() || app.persist().is_none() {
            return;
        }
        let text = serde_json::to_string(&self.open_focus_windows()).unwrap_or_default();
        if self.focus_windows_kept.as_deref() == Some(text.as_str()) {
            return;
        }
        app.save_later(OPEN_DOC, text.clone());
        self.focus_windows_kept = Some(text);
    }

    /// At the start: the focus windows open at the last quit open again,
    /// each where egui-pop kept it.
    pub(super) fn restore_focus_windows(&mut self) {
        let Some(persist) = self.core.app.handle().persist() else {
            return;
        };
        let Some(open) = std::fs::read_to_string(persist.path(OPEN_DOC))
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        else {
            return;
        };
        for window in open {
            match window.as_str() {
                super::activity_focus::WINDOW => self.open_activity(&serde_json::Value::Null),
                super::library_focus::WINDOW => self.open_library_window(),
                _ => {}
            }
        }
    }
}
