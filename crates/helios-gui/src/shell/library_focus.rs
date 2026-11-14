//! The Library focus in a window of its own: Window › Library (and its
//! key) opens it, or brings it to the front when it is open.
//!
//! It shows the same Library as the main window's Library panel (the
//! browse sections, the filter and facets, the albums and smart albums,
//! the marks bar with its versions strip, the grid and its stacks), not a
//! copy: one [`crate::library::Library`], so the selection, the
//! collection shown and the marks are the same in both windows, and a
//! command (a key, a menu, an agent's) acts on that selection whichever
//! window it came from. The window is opened as the other foci's are
//! ([`super::focus_window`]); its size and place are kept by egui-pop and
//! it opens again at the next start when it was open at the last quit.

use std::cell::RefCell;
use std::rc::Rc;

use egui::{Context as Egui, Rect};
use egui_pop::api::windows::WindowId;
use helios_core::Message;
use serde_json::{Value, json};

use super::Shell;
use super::focus_window::{FocusWindow, WindowFocus};
use crate::library::Library;

/// The command opening the focus (Window › Library).
pub const COMMAND: &str = "library.window";

/// The focus's OS window: its id, under which egui-pop keeps its size
/// (`appWindow == 'library'`, `window: "library"` for agents).
pub const WINDOW: &str = "library";

/// The focus's window (`focus_window`).
pub(super) static FRAME: FocusWindow = FocusWindow {
    window: WINDOW,
    embedded: "helios.library.embedded",
    min_size: egui::vec2(560.0, 400.0),
    max_default_size: egui::vec2(1400.0, 900.0),
    share: 0.85,
};

/// The focus's state, shared by the shell and the window's content.
#[derive(Default)]
pub(crate) struct LibraryWindow {
    open: bool,
    /// The OS window opened for it, not seen closed since.
    shown: Option<WindowId>,
    /// The main window's Library, the one this window shows.
    library: Option<Rc<RefCell<Library>>>,
    /// Where the window drew the Library last, in its points.
    drawn: Option<Rect>,
    /// The window's last pass had input: the main window's pass takes
    /// what it asked of the Library.
    touched: bool,
}

/// The focus: its state shared with the window's content.
#[derive(Default)]
pub(super) struct LibraryFocus {
    pub(super) state: Rc<RefCell<LibraryWindow>>,
}

fn window_title() -> Message {
    Message::new("gui-library-window", "Library")
}

impl WindowFocus for LibraryWindow {
    fn is_open(&self) -> bool {
        self.open
    }

    fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    fn shown(&mut self) -> &mut Option<WindowId> {
        &mut self.shown
    }

    fn title(&self, ctx: &Egui) -> String {
        crate::i18n::text(ctx, window_title())
    }

    fn window_ui(&mut self, ui: &mut egui::Ui) {
        let Some(library) = &self.library else {
            return;
        };
        // Drawn while the main window draws its own (an embedded window):
        // the Library panel holds it, this pass shows nothing.
        let Ok(mut library) = library.try_borrow_mut() else {
            return;
        };
        self.drawn = Some(ui.available_rect_before_wrap());
        self.touched = ui
            .ctx()
            .input(|i| !i.events.is_empty() || i.pointer.any_down());
        ui.push_id("helios.library.window", |ui| library.grid(ui));
    }

    fn asks_main_pass(&self) -> bool {
        // The Library's asks (reads, writes, imports) are taken by the
        // main window's pass; a pass without input asks nothing, so the
        // two windows never wake each other in a loop.
        self.touched
    }
}

impl Shell {
    /// `library.window`: opens the Library focus in its own window, or
    /// brings it to the front.
    pub(super) fn open_library_window(&mut self) {
        self.library_focus.state.borrow_mut().library = Some(self.focus.library.clone());
        super::focus_window::open(self.core.app.handle(), &self.library_focus.state);
    }

    /// Closes the Library focus, as its window's close button does.
    pub fn close_library_window(&mut self) {
        self.library_focus.state.borrow_mut().open = false;
    }

    /// The focus as it is, for tests and agents: open, and where it drew
    /// the Library last (`[x0, y0, x1, y1]`, in its window's points).
    pub fn library_window_state(&self) -> Value {
        let w = self.library_focus.state.borrow();
        let drawn = w.drawn.map(|r| [r.min.x, r.min.y, r.max.x, r.max.y]);
        json!({ "open": w.open, "drawn": drawn })
    }

    /// Each frame of the main window: the focus's OS window opened or
    /// closed as the focus is.
    pub(super) fn library_window_frame(&mut self, ctx: &Egui) {
        let app = self.core.app.handle().clone();
        FRAME.follow(&app, ctx, &self.library_focus.state);
        // What the main window's pass changed in the Library (thumbnails
        // come, a read answered) shows in this window too.
        if self.library_focus.state.borrow().open
            && let Some(v) = app.window_viewport(&FRAME.id())
            && v != egui::ViewportId::ROOT
        {
            ctx.request_repaint_of(v);
        }
    }
}

/// Whether the Library window is drawn inside the main one (a backend
/// without OS windows of its own: kittest, the web). The main window's
/// Library panel then leaves the Library to it: egui keeps one widget per
/// id in a window, and the Library's widgets have ids of their own.
pub(super) fn in_main_window(state: &Rc<RefCell<LibraryWindow>>, ctx: &Egui) -> bool {
    ctx.embed_viewports() && state.try_borrow().is_ok_and(|w| w.open)
}

/// What the main window's Library panel shows meanwhile.
pub(super) fn elsewhere_ui(ui: &mut egui::Ui) {
    let text = crate::i18n::text(
        ui.ctx(),
        Message::new(
            "gui-library-window-elsewhere",
            "The Library is open in its own window",
        ),
    );
    ui.add(egui::Label::new(egui::RichText::new(text).weak()).wrap());
}
