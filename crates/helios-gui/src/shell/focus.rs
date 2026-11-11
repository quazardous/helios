//! A focus: what belongs to one window.
//!
//! A focus is a work intention given its own OS window (see
//! docs/design/windows.md and the [`Core`](super::core::Core), what every
//! window shares). It holds the window's workspace (its layout, its
//! places and modes, its sub-workspaces), the views and tools the window
//! shows (the library's grid and selection, the comparison of photos side
//! by side, the open image's viewport, its parameters, masks, network,
//! scopes and decisions, the Queue and Workshops panels), the import
//! window drawn in it, and what pictures of the window need (its named
//! rectangles, scale and screenshots).
//!
//! Today the process has one focus, the main window, which is the root
//! viewport: the root window is always the first focus. A second focus
//! (the Import window, through egui-pop's multi-window API) is made with
//! [`Focus::new`] on the same core: its viewport renders on the core's
//! engine, so no photo is loaded twice.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use egui::Rect;
use egui_pop::model::ids::PanelId;
use egui_pop::model::layout::Layout;
use helios_core::Engine;

use super::{Shot, import_window, workshop_edit, workspace_edit};
use crate::dialogs::MenuItem;
use crate::host::Workspace;
use crate::snapshot::Recorder;
use crate::workbench::Contents;

/// What belongs to one window (see the module's docs).
pub(super) struct Focus {
    /// The window this focus is drawn in: `ViewportId::ROOT` for the main
    /// window. Its display surfaces, title bar and pictures are keyed by
    /// it.
    pub(super) window: egui::ViewportId,
    /// What each of the window's panels draws.
    pub(super) contents: Contents,
    /// Why the tab dragged last frame may not go where it is, as the
    /// workbench's drop hint said it: said when the tab is let go there.
    pub(super) drag_refusal: Option<String>,
    pub(super) rects: BTreeMap<String, Rect>,
    /// The window of each named rectangle drawn outside this one (a
    /// detached panel's).
    pub(super) rect_windows: BTreeMap<String, egui::ViewportId>,
    pub(super) pixels_per_point: f32,
    /// The screen's own scale, as of the last frame (none before it).
    pub(super) native_scale: Option<f32>,
    pub(super) shots: VecDeque<Shot>,
    /// The screenshots asked for, as the window hands them back, oldest
    /// first: each answers the oldest of `shots`.
    pub(super) screenshots: Arc<Mutex<VecDeque<Arc<egui::ColorImage>>>>,
    pub(super) recording: Option<(String, Recorder)>,
    /// The workspace's name, which the window's title shows.
    pub(super) title: String,
    /// The scopes of the image shown.
    pub(super) scopes: Rc<RefCell<crate::scopes::Scopes>>,
    /// The library as this window shows it: its grid, filter and
    /// selection, over the catalogue the host gives.
    pub(super) library: Rc<RefCell<crate::library::Library>>,
    /// The comparison: its photos, the active one, its zoom and pan (the
    /// Compare place names its photos; the Compare panel draws it).
    pub(super) comparison: Rc<RefCell<crate::comparison::Comparison>>,
    /// The workspace opens in Compare (its template says so): the place
    /// is entered once the stack is read, when it shows the root.
    pub(super) open_in_compare: bool,
    /// The open image's decisions, and the Decisions panel's cards.
    pub(super) decisions: Rc<RefCell<crate::decisions::Decisions>>,
    /// Where the workspace imports from, and how (its template's).
    pub(super) import_source: Option<helios_template::ImportDesc>,
    /// The photos the imports running brought so far, and those the last
    /// one brought, to select once the Library shows them.
    pub(super) imported_ids: Vec<String>,
    pub(super) select_imported: Vec<String>,
    /// The import window, its sources and the cards watched. Drawn in this
    /// window today; the Import focus window of its own takes it over.
    pub(super) import_window: import_window::ImportWindow,
    /// A menu of commands asked for, open at a point.
    pub(super) menu: Option<(egui::Pos2, Vec<MenuItem>)>,
    pub(super) viewport: Rc<RefCell<crate::viewport::Viewport>>,
    /// The library's image being opened in the viewport, while its large
    /// thumbnail is awaited (the loupe's first frame).
    pub(super) opening: Option<String>,
    /// A small render of what the viewport shows, for the scopes and the
    /// navigator.
    pub(super) preview: Rc<crate::preview::Preview>,
    /// The image's own surface (Wayland), following the Wide-gamut
    /// preview setting.
    pub(super) image_surface: crate::display::ImageSurfaceSwitch,
    pub(super) params: Rc<RefCell<crate::params::Params>>,
    /// The Masks panel: the selected module's mask network as a tree.
    pub(super) mask_tree: Rc<RefCell<crate::mask_tree::MaskTree>>,
    /// The Graph panel: the workspace's network in egui-pop's editor.
    pub(super) graph: Rc<RefCell<crate::graph::GraphPanel>>,
    /// The workspace open, whose layout egui-pop keeps.
    pub(super) workspace: Workspace,
    /// The layout as last saved to the workspace's document: saved again
    /// once it changes, so a kill keeps it.
    pub(super) layout_saved: Option<Layout>,
    /// The workspace's state, as edited (`workspace_edit`).
    pub(super) edit: Option<workspace_edit::Edited>,
    /// The Queue panel: the selected node's queue.
    pub(super) queue_panel: Rc<RefCell<crate::queue::QueuePanel>>,
    /// The image's compare and mask tool, kept with their modes.
    pub(super) follow: crate::contexts::Follow,
    /// The workspace's sub-workspaces, for their place; whether they
    /// were read, and at which of the state's sequence numbers (none: no
    /// state).
    pub(super) subworkspaces: crate::contexts::Subworkspaces,
    pub(super) subworkspaces_seen: (bool, Option<u64>),
    /// Helios's status bar items, published when what they say changes.
    pub(super) status_items: crate::status_items::Items,
    /// The palettes a new layout gathered at the workbench's end edge
    /// (their group's first panel), moved off the end side bar once the
    /// areas are drawn ([`super::Shell::place_palettes`]).
    pub(super) palettes_to_place: Option<PanelId>,
    /// The workshops' aims this session (see `workshop_edit`).
    pub(super) workshop_aims: workshop_edit::Aims,
    /// The Workshops panel (experimental).
    pub(super) workshops: Rc<RefCell<crate::workshops::Panel>>,
}

/// What a focus is made with: what the shell wired before egui-pop's
/// application was built, and what the core shares with it.
pub(super) struct Seed {
    pub(super) window: egui::ViewportId,
    pub(super) contents: Contents,
    pub(super) workspace: Workspace,
    pub(super) subworkspaces: crate::contexts::Subworkspaces,
    pub(super) library: Rc<RefCell<crate::library::Library>>,
    pub(super) viewport: Rc<RefCell<crate::viewport::Viewport>>,
    pub(super) controls: helios_controls::commands::SharedContexts,
}

impl Focus {
    /// A focus in `seed.window`, on its workspace, its viewport rendering
    /// on the core's engine (`seed.viewport`, made with
    /// [`viewport_on`]).
    pub(super) fn new(seed: Seed) -> Focus {
        let preview = Rc::new(crate::preview::Preview::new(
            seed.viewport.borrow().engine().clone(),
        ));
        Focus {
            window: seed.window,
            contents: seed.contents,
            drag_refusal: None,
            rects: BTreeMap::new(),
            rect_windows: BTreeMap::new(),
            pixels_per_point: 1.0,
            native_scale: None,
            shots: VecDeque::new(),
            screenshots: Arc::default(),
            recording: None,
            title: String::new(),
            scopes: Rc::default(),
            library: seed.library,
            comparison: Rc::default(),
            open_in_compare: false,
            decisions: Rc::default(),
            import_source: None,
            imported_ids: Vec::new(),
            select_imported: Vec::new(),
            import_window: import_window::ImportWindow::default(),
            menu: None,
            viewport: seed.viewport,
            opening: None,
            preview,
            image_surface: crate::display::ImageSurfaceSwitch::default(),
            params: Rc::new(RefCell::new(crate::params::Params::new(seed.controls))),
            mask_tree: Rc::default(),
            graph: Rc::default(),
            workspace: seed.workspace,
            layout_saved: None,
            edit: None,
            queue_panel: Rc::default(),
            follow: crate::contexts::Follow::default(),
            subworkspaces: seed.subworkspaces,
            subworkspaces_seen: (false, None),
            status_items: crate::status_items::Items::default(),
            palettes_to_place: None,
            workshop_aims: workshop_edit::Aims::new(),
            workshops: Rc::default(),
        }
    }
}

/// A window's viewport, rendering on the process's engine: every focus's
/// viewport shares the core's engine and render cache.
pub(super) fn viewport_on(engine: &Arc<Engine>) -> Rc<RefCell<crate::viewport::Viewport>> {
    Rc::new(RefCell::new(crate::viewport::Viewport::on_engine(
        engine.clone(),
    )))
}
