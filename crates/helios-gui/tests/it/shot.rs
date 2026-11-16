//! Snapshots cut to what they test.
//!
//! The shell draws every screen inside its chrome: the title bar, the
//! menu bar, the status bar and the panels around. A picture of the whole
//! window changes with any of them, so an item added to the status bar
//! would rewrite the picture of every dialog drawn over the main window.
//! A suite's picture is cut instead:
//!
//! - [`Shots::try_window_shot`]: the window titled so (the import window,
//!   the Activity, the Library window: an `egui::Window` over the main
//!   one), frame and title bar included, and what floats over it;
//! - [`Shots::try_body_shot`]: everything between the top bars and the
//!   status bar, for a focus filling the workbench (Compare);
//! - [`Shots::try_whole_shot`]: the whole window, for the chrome's own
//!   suite (`chrome_snapshots`) alone;
//! - [`Shots::try_shot_of`]: a rectangle the test names.
//!
//! The cut is rounded to whole pixels and depends on the layout alone, so
//! a picture has the same size on every run. No suite of the shell calls
//! kittest's `snapshot`, which takes the whole window.

use egui::{Id, Order, Rect, accesskit::Role};
use egui_kittest::kittest::Queryable as _;
use egui_kittest::{Harness, SnapshotError, SnapshotResult};
use helios_gui::Shell;

/// The ids of the panels egui-pop draws the shell's bars in.
const TITLE_BAR: &str = "egui_pop.title";
const TOP_BAR: &str = "egui_pop.top";
const STATUS_BAR: &str = "egui_pop.status";

/// The cuts of the shell's picture.
pub trait Shots {
    /// The window titled `title`, and what floats over it, compared with
    /// the snapshot `name`.
    fn try_window_shot(&mut self, name: impl Into<String>, title: &str) -> SnapshotResult;

    /// Everything between the top bars and the status bar, compared with
    /// the snapshot `name`.
    fn try_body_shot(&mut self, name: impl Into<String>) -> SnapshotResult;

    /// The whole window, compared with the snapshot `name`: for the
    /// chrome's own suite.
    fn try_whole_shot(&mut self, name: impl Into<String>) -> SnapshotResult;

    /// `rect` of the window, in points, compared with the snapshot `name`;
    /// nothing widens it.
    fn try_shot_of(&mut self, name: impl Into<String>, rect: Rect) -> SnapshotResult;
}

impl Shots for Harness<'_, Shell> {
    fn try_window_shot(&mut self, name: impl Into<String>, title: &str) -> SnapshotResult {
        let window = self
            .query_by_role_and_label(Role::Window, title)
            .map(|n| n.rect())
            .unwrap_or_else(|| panic!("no window titled {title:?} is shown"));
        let cut = above(&self.ctx, window);
        self.try_shot_of(name, cut)
    }

    fn try_body_shot(&mut self, name: impl Into<String>) -> SnapshotResult {
        let cut = bars(self).body;
        self.try_shot_of(name, cut)
    }

    fn try_whole_shot(&mut self, name: impl Into<String>) -> SnapshotResult {
        let window = self.ctx.content_rect();
        self.try_shot_of(name, window)
    }

    fn try_shot_of(&mut self, name: impl Into<String>, rect: Rect) -> SnapshotResult {
        // As kittest's own `try_snapshot`: every widget named.
        self.check_accessibility();
        let image = self
            .render()
            .map_err(|err| SnapshotError::RenderError { err })?;
        let rect = rect.intersect(self.ctx.content_rect());
        assert!(rect.is_positive(), "nothing to show in {rect:?}");
        let cut = helios_gui::snapshot::crop(&image, rect, self.ctx.pixels_per_point());
        egui_kittest::try_image_snapshot_options(&cut, name.into(), self.options())
    }
}

/// The shell's bars, where egui-pop drew them last pass.
#[derive(Clone, Copy, Debug)]
pub struct Bars {
    /// The title bar and the menu bar, all of the window's width (empty
    /// when the window has neither).
    pub top: Rect,
    /// The status bar, all of the window's width.
    pub status: Rect,
    /// Between the two: the workbench.
    pub body: Rect,
}

/// The outer rect of a panel egui-pop draws a bar in.
fn bar(ctx: &egui::Context, id: &str) -> Option<Rect> {
    egui::PanelState::load(ctx, Id::unique(id)).map(|s| s.outer_rect)
}

/// The shell's bars, from the panels they are drawn in.
pub fn bars(h: &Harness<'_, Shell>) -> Bars {
    let window = h.ctx.content_rect();
    let bottom_of_top = [TITLE_BAR, TOP_BAR]
        .into_iter()
        .filter_map(|id| bar(&h.ctx, id))
        .map(|r| r.bottom())
        .fold(window.top(), f32::max);
    let top_of_status = bar(&h.ctx, STATUS_BAR).map_or(window.bottom(), |r| r.top());
    Bars {
        top: Rect::from_min_max(window.min, egui::pos2(window.right(), bottom_of_top)),
        status: Rect::from_min_max(egui::pos2(window.left(), top_of_status), window.max),
        body: Rect::from_min_max(
            egui::pos2(window.left(), bottom_of_top),
            egui::pos2(window.right(), top_of_status),
        ),
    }
}

/// `window`, widened to hold what is drawn over it this frame: the areas
/// painted after its own (a menu, a combo box's list, a tip), but one
/// covering the whole window. The palettes and panels drawn under it stay
/// out.
fn above(ctx: &egui::Context, window: Rect) -> Rect {
    let screen = ctx.content_rect();
    let floating = ctx.memory(|m| {
        let layers: Vec<_> = m
            .layer_ids()
            .filter(|layer| layer.order != Order::Background && m.areas().is_visible(layer))
            .filter_map(|layer| Some((layer, m.area_rect(layer.id)?)))
            .collect();
        // The window's own area: the one holding its rect, the smallest
        // (egui keeps its frame's shadow out of the area's rect).
        let own = layers
            .iter()
            .enumerate()
            .filter(|(_, (_, rect))| rect.expand(1.0).contains_rect(window))
            .min_by(|a, b| a.1.1.area().total_cmp(&b.1.1.area()))
            .map(|(i, _)| i)?;
        layers[own + 1..]
            .iter()
            .map(|(_, rect)| *rect)
            .filter(|rect| rect.is_positive() && !rect.contains_rect(screen.shrink(1.0)))
            .reduce(Rect::union)
    });
    floating
        .map_or(window, |f| window.union(f))
        .intersect(screen)
}
