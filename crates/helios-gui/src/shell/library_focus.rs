//! The Library focus in a window of its own: Window › Library (and its
//! key) opens it, or brings it to the front when it is open.
//!
//! It shows the same Library as the main window's Library panel (the
//! browse sections, the filter and facets, the albums and smart albums,
//! the marks bar with its versions strip, the grid and its stacks), not a
//! copy: one [`crate::library::Library`], so the photos and their marks
//! are the same in both windows. Each window draws its own gallery
//! ([`crate::library::WINDOW_GALLERY`]): its search, filter, album or
//! folder and selection are its own, and a command (a key, a menu, an
//! agent's) acts on the gallery of the window with the input. The window is opened as the other foci's are
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
    /// The Library's stamp ([`Library::stamp`]) when the window last drew
    /// it: the main window's pass wakes the window only when it moved.
    drawn_stamp: Option<u64>,
}

/// The focus: its state shared with the window's content.
#[derive(Default)]
pub(super) struct LibraryFocus {
    pub(super) state: Rc<RefCell<LibraryWindow>>,
}

fn window_title() -> Message {
    Message::new("gui-library-window", "Library")
}

impl LibraryWindow {
    /// Whether the window, open, has something new to draw: the Library
    /// moved since it drew it, thumbnails or renders wait to be taken, or
    /// its own input asked the main window's pass something.
    fn has_news(&self) -> bool {
        if !self.open {
            return false;
        }
        if self.touched {
            return true;
        }
        let Some(library) = &self.library else {
            return false;
        };
        library
            .try_borrow()
            .is_ok_and(|l| self.drawn_stamp != Some(l.stamp()) || l.arrivals_waiting())
    }
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
        ui.push_id("helios.library.window", |ui| {
            library.show(ui, &crate::library::GalleryOptions::LIBRARY_WINDOW);
        });
        // What this pass drew, its own changes included.
        self.drawn_stamp = Some(library.stamp());
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
        // Something came in for the window (thumbnails, renders, a read
        // answered, a mark, what its own input asked): egui-pop wakes it
        // (the main window, where it is drawn, needs no other pass). A
        // main pass that brought nothing (a hover, a drag elsewhere)
        // leaves it idle.
        if self.library_focus.state.borrow().has_news()
            && app
                .window_viewport(&FRAME.id())
                .is_some_and(|v| app.window_group(v) != egui::ViewportId::ROOT)
        {
            app.wake_window(&FRAME.id());
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
