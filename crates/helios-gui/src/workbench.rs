//! Helios's areas: editor groups, side and bottom panels docked from the
//! workspace's layout (`egui_dock`), panels detached into auxiliary
//! windows, tabs dragged from one area to another under each panel's
//! rules. egui-pop draws the application around them (menu bar, overlays);
//! the areas are one block of its main zone until egui-pop's workbench
//! takes them over.

use std::collections::BTreeMap;

use egui::{Align2, Context as Egui, Id, Pos2, Rect, Ui, ViewportBuilder, ViewportId};
use egui_dock::{DockArea, DockState, TabViewer};
use helios_shell::layout::{Auxiliary, Layout, Node};
use helios_shell::panels::{Area, Panels};

use crate::dock::{from_dock, to_dock};
use crate::name_rect;

/// The screen auxiliary windows are said to be on, until egui names
/// monitors.
pub(crate) const SCREEN: &str = "primary";

/// How close to a window's edge a dragged tab offers a hidden area.
const EDGE: f32 = 48.0;

/// Draws a panel's content.
pub type Content = Box<dyn FnMut(&mut Ui)>;

pub(crate) struct Workbench {
    pub panels: Panels,
    docks: Vec<(Area, DockState<String>)>,
    pub auxiliary: Vec<Auxiliary>,
    /// Panels the user closed: a layout keeps them closed.
    pub hidden: Vec<String>,
    pub focused: Option<String>,
    pub contents: BTreeMap<String, Content>,
    /// Where each area was drawn last frame (the editors always are).
    area_rects: BTreeMap<Area, Rect>,
    /// The areas' rectangle, where hidden areas offer a drop zone.
    content_rect: Rect,
    /// The panel whose tab is being dragged, the area it comes from and
    /// where the pointer was last seen.
    dragging: Option<(String, Area, Pos2)>,
    /// What drawing has to say (a drop refused), for the shell's notices.
    pub warnings: Vec<String>,
}

impl Workbench {
    /// The panels in a saved layout (repaired), or the default one; the
    /// repairs are returned.
    pub fn new(panels: Panels, layout: Option<Layout>) -> (Workbench, Vec<String>) {
        let (layout, notes) = match layout {
            Some(l) => l.repair(&panels, &[SCREEN]),
            None => (Layout::default_for(&panels), Vec::new()),
        };
        let wb = Workbench {
            docks: layout
                .areas
                .iter()
                .map(|(area, node)| (*area, to_dock(node)))
                .collect(),
            auxiliary: layout.auxiliary,
            hidden: layout.hidden,
            panels,
            focused: None,
            contents: BTreeMap::new(),
            area_rects: BTreeMap::new(),
            content_rect: Rect::NOTHING,
            dragging: None,
            warnings: Vec::new(),
        };
        (wb, notes)
    }

    /// The layout as it stands, to be saved with the workspace.
    pub fn layout(&self) -> Layout {
        Layout {
            areas: self
                .docks
                .iter()
                .filter_map(|(area, dock)| from_dock(dock).map(|n| (*area, n)))
                .collect(),
            auxiliary: self.auxiliary.clone(),
            hidden: self.hidden.clone(),
        }
    }

    /// Arranges the panels as a layout says, repaired to today's panels;
    /// returns the repairs.
    pub fn set_layout(&mut self, layout: Layout) -> Vec<String> {
        let (l, notes) = layout.repair(&self.panels, &[SCREEN]);
        self.docks = l.areas.iter().map(|(a, n)| (*a, to_dock(n))).collect();
        self.auxiliary = l.auxiliary;
        self.hidden = l.hidden;
        notes
    }

    pub fn shown(&self, panel: &str) -> bool {
        self.docks
            .iter()
            .any(|(_, d)| d.find_tab(&panel.to_string()).is_some())
            || self.auxiliary.iter().any(|a| a.panel == panel)
    }

    fn undock(&mut self, panel: &str) {
        for (_, dock) in &mut self.docks {
            if let Some(path) = dock.find_tab(&panel.to_string()) {
                dock.remove_tab(path);
            }
        }
        self.docks.retain(|(_, d)| from_dock(d).is_some());
        if self.focused.as_deref() == Some(panel) {
            self.focused = None;
        }
    }

    /// Puts a panel back in its default area.
    pub fn dock(&mut self, panel: &str) {
        let Some(area) = self.panels.get(panel).map(|p| p.area) else {
            return;
        };
        self.auxiliary.retain(|a| a.panel != panel);
        self.hidden.retain(|h| h != panel);
        if let Some((_, dock)) = self.docks.iter_mut().find(|(a, _)| *a == area) {
            dock.push_to_first_leaf(panel.to_string());
        } else {
            self.docks.push((
                area,
                to_dock(&Node::Tabs {
                    panels: vec![panel.to_string()],
                    active: 0,
                }),
            ));
            self.docks.sort_by_key(|(a, _)| *a);
        }
    }

    /// Shows a panel, or hides it (closed: a layout keeps it closed).
    pub fn toggle(&mut self, panel: &str) {
        if self.shown(panel) {
            self.undock(panel);
            self.auxiliary.retain(|a| a.panel != panel);
            self.hidden.push(panel.to_string());
        } else {
            self.dock(panel);
        }
    }

    /// Moves a panel to a window of its own, where it was drawn.
    pub fn detach(&mut self, panel: &str, was: Option<Rect>) {
        if self.auxiliary.iter().any(|a| a.panel == panel) {
            return;
        }
        let r = was.unwrap_or(Rect::from_min_size(
            egui::pos2(80.0, 80.0),
            egui::vec2(480.0, 360.0),
        ));
        self.undock(panel);
        self.auxiliary.push(Auxiliary {
            panel: panel.to_string(),
            screen: SCREEN.into(),
            rect: [r.min.x, r.min.y, r.width(), r.height()],
        });
    }

    pub fn focus(&mut self, panel: &str) {
        if !self.shown(panel) {
            self.dock(panel);
        }
        for (_, dock) in &mut self.docks {
            if let Some(path) = dock.find_tab(&panel.to_string()) {
                let _ = dock.set_active_tab(path);
                dock.set_focused_node_and_surface(path.node_path());
            }
        }
        self.focused = Some(panel.to_string());
    }

    /// Moves a panel to an area, into the tabs under `at` when given,
    /// else the area's first tabs; refused when the panel may not be there.
    pub fn move_to(&mut self, panel: &str, area: Area, at: Option<Pos2>) -> Result<(), String> {
        let spec = self
            .panels
            .get(panel)
            .ok_or_else(|| format!("no panel `{panel}`"))?;
        if !spec.allows(area) {
            return Err(format!("{} stays out of {}", spec.title, area.name()));
        }
        self.undock(panel);
        self.auxiliary.retain(|a| a.panel != panel);
        self.hidden.retain(|h| h != panel);
        let tab = panel.to_string();
        if let Some((_, dock)) = self.docks.iter_mut().find(|(a, _)| *a == area) {
            let leaf = at.and_then(|at| {
                dock.main_surface()
                    .iter()
                    .position(|n| matches!(n, egui_dock::Node::Leaf(l) if l.rect.contains(at)))
            });
            match leaf {
                Some(i) => {
                    let tree = dock.main_surface_mut();
                    let node = egui_dock::NodeIndex(i);
                    tree.set_focused_node(node);
                    tree.push_to_focused_leaf(tab);
                }
                None => dock.push_to_first_leaf(tab),
            }
        } else {
            self.docks.push((
                area,
                to_dock(&Node::Tabs {
                    panels: vec![tab],
                    active: 0,
                }),
            ));
            self.docks.sort_by_key(|(a, _)| *a);
        }
        self.focused = Some(panel.to_string());
        Ok(())
    }

    /// The area a drop at `at` goes to: the one drawn there, else the
    /// zone of a hidden side or bottom area along the areas' edges.
    fn area_at(&self, at: Pos2) -> Option<Area> {
        let shown = |a: Area| self.docks.iter().any(|(d, _)| *d == a);
        let c = self.content_rect;
        if c.is_positive() {
            if !shown(Area::PrimarySideBar) && at.x < c.left() + EDGE {
                return Some(Area::PrimarySideBar);
            }
            if !shown(Area::SecondarySideBar) && at.x > c.right() - EDGE {
                return Some(Area::SecondarySideBar);
            }
            if !shown(Area::BottomPanel) && at.y > c.bottom() - EDGE {
                return Some(Area::BottomPanel);
            }
        }
        self.area_rects
            .iter()
            .filter(|(a, _)| **a != Area::Editors)
            .find(|(_, r)| r.contains(at))
            .map(|(a, _)| *a)
            .or_else(|| {
                self.area_rects
                    .get(&Area::Editors)
                    .filter(|r| r.contains(at))
                    .map(|_| Area::Editors)
            })
    }

    /// The tab egui is dragging, if it is one of the docks' tabs.
    fn dragged_tab(&self, ctx: &Egui) -> Option<(String, Area)> {
        let id = ctx.dragged_id()?;
        self.docks.iter().find_map(|(area, dock)| {
            let base = Id::new(format!("helios.area.{area:?}"));
            dock.iter_all_tabs().find_map(|(path, tab)| {
                // How egui_dock names a tab's drag (`show/leaf.rs`).
                let tab_id = base
                    .with((path.surface, "surface"))
                    .with((path.node_path().node, "node"))
                    .with((path.tab.0, "tab"));
                (tab_id == id).then(|| (tab.clone(), *area))
            })
        })
    }

    /// Follows a tab drag; when it is dropped over another area, `egui_dock`
    /// (which only sees its own area) leaves it where it was, and the shell
    /// moves it, or says why it may not go there.
    fn follow_drag(&mut self, ctx: &Egui) {
        let pos = ctx.input(|i| i.pointer.latest_pos());
        if let Some((panel, from)) = self.dragged_tab(ctx) {
            let at = pos.or(self.dragging.as_ref().map(|d| d.2));
            if let Some(at) = at {
                self.dragging = Some((panel, from, at));
            }
            return;
        }
        let Some((panel, from, last)) = self.dragging.take() else {
            return;
        };
        let at = pos.unwrap_or(last);
        let Some(to) = self.area_at(at).filter(|a| *a != from) else {
            return;
        };
        if let Err(e) = self.move_to(&panel, to, Some(at)) {
            self.warnings.push(e);
        }
    }

    /// While a tab is dragged over another area, shows whether it may go
    /// there.
    fn drop_hint(&mut self, ctx: &Egui) {
        let Some((panel, from, at)) = self.dragging.clone() else {
            return;
        };
        let Some(to) = self.area_at(at).filter(|a| *a != from) else {
            return;
        };
        let Some(spec) = self.panels.get(&panel) else {
            return;
        };
        let c = self.content_rect;
        let rect = self.area_rects.get(&to).copied().unwrap_or(match to {
            Area::PrimarySideBar => {
                Rect::from_min_max(c.min, egui::pos2(c.left() + EDGE, c.bottom()))
            }
            Area::SecondarySideBar => {
                Rect::from_min_max(egui::pos2(c.right() - EDGE, c.top()), c.max)
            }
            Area::BottomPanel => Rect::from_min_max(egui::pos2(c.left(), c.bottom() - EDGE), c.max),
            Area::Editors => c,
        });
        let visuals = ctx.global_style().visuals.clone();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            Id::new("helios.drop"),
        ));
        if spec.allows(to) {
            let fill = visuals.selection.bg_fill.gamma_multiply(0.25);
            painter.rect_filled(rect, 4.0, fill);
            painter.rect_stroke(
                rect,
                4.0,
                visuals.selection.stroke,
                egui::StrokeKind::Inside,
            );
        } else {
            let fill = visuals.error_fg_color.gamma_multiply(0.15);
            painter.rect_filled(rect, 4.0, fill);
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!("{} stays out of {}", spec.title, to.name()),
                egui::FontId::proportional(13.0),
                visuals.error_fg_color,
            );
        }
        name_rect(ctx, "helios.drop", rect);
    }

    /// Draws the docked areas in `ui`: side areas first, the editors take
    /// what is left.
    pub fn ui(&mut self, ui: &mut Ui) {
        let Workbench {
            docks,
            panels,
            contents,
            focused,
            area_rects,
            content_rect,
            ..
        } = self;
        *content_rect = ui.max_rect();
        area_rects.clear();
        let mut viewer = Viewer {
            ctx: ui.ctx().clone(),
            panels,
            contents,
            focused,
        };
        let mut editors = None;
        for (area, dock) in docks.iter_mut() {
            let id = format!("helios.area.{area:?}");
            let panel = match area {
                Area::Editors => {
                    editors = Some(dock);
                    continue;
                }
                Area::PrimarySideBar => egui::Panel::left(id.clone()).default_size(260.0),
                Area::SecondarySideBar => egui::Panel::right(id.clone()).default_size(260.0),
                Area::BottomPanel => egui::Panel::bottom(id.clone()).default_size(200.0),
            };
            let shown = panel.resizable(true).show(ui, |ui| {
                show_dock(ui, &id, dock, &mut viewer);
            });
            area_rects.insert(*area, shown.response.rect);
        }
        let shown = egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                if let Some(dock) = editors {
                    show_dock(ui, "helios.area.Editors", dock, &mut viewer);
                }
            });
        area_rects.insert(Area::Editors, shown.response.rect);
        let ctx = ui.ctx().clone();
        self.follow_drag(&ctx);
        self.drop_hint(&ctx);
    }

    /// Draws the detached panels in windows of their own; a window
    /// closed docks its panel back.
    pub fn windows(&mut self, ctx: &Egui, framed: bool) {
        let mut back = Vec::new();
        for aux in &mut self.auxiliary {
            let title = panel_title(ctx, &self.panels, &aux.panel);
            // The same title and, when Helios draws the main window's
            // frame, the same frame.
            let window_title = format!("{title} — Helios");
            let builder = ViewportBuilder::default()
                .with_title(window_title.clone())
                .with_decorations(!framed)
                .with_inner_size([aux.rect[2], aux.rect[3]]);
            let mut content = self.contents.get_mut(&aux.panel);
            let panel = aux.panel.clone();
            ctx.show_viewport_immediate(ViewportId::from_hash_of(&panel), builder, |ui, _| {
                if framed {
                    let vctx = ui.ctx().clone();
                    crate::frame::resize_handles(&vctx);
                    let request = egui::Panel::top("helios.titlebar")
                        .exact_size(crate::frame::TITLE_HEIGHT)
                        .frame(egui::Frame::NONE.fill(ui.visuals().panel_fill))
                        .show(ui, |ui| crate::frame::title_bar(ui, &window_title))
                        .inner;
                    match request {
                        Some(crate::frame::Request::Minimize) => {
                            vctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                        Some(crate::frame::Request::ToggleMaximize) => {
                            let on = vctx.input(|i| i.viewport().maximized.unwrap_or(false));
                            vctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!on));
                        }
                        Some(crate::frame::Request::Close) => back.push(panel.clone()),
                        Some(crate::frame::Request::SystemMenu(_)) | None => {}
                    }
                }
                egui::CentralPanel::default().show(ui, |ui| {
                    name_rect(ui.ctx(), &panel, ui.max_rect());
                    match content.as_deref_mut() {
                        Some(c) => c(ui),
                        None => placeholder(ui, &title),
                    }
                });
                if ui.input(|i| i.viewport().close_requested()) {
                    back.push(panel.clone());
                }
            });
        }
        for p in back {
            self.dock(&p);
        }
    }
}

/// A panel's title in the interface's language (egui-pop's
/// `panel-<id>.title`), else its declared English, else its id.
fn panel_title(ctx: &Egui, panels: &Panels, id: &str) -> String {
    panels.get(id).map_or_else(
        || id.to_string(),
        |p| egui_pop::core::strings::panel_title(ctx, id, &p.title),
    )
}

fn placeholder(ui: &mut Ui, title: &str) {
    ui.centered_and_justified(|ui| {
        ui.weak(title);
    });
}

fn show_dock(ui: &mut Ui, id: &str, dock: &mut DockState<String>, viewer: &mut Viewer) {
    DockArea::new(dock)
        .id(Id::new(id))
        .show_close_buttons(false)
        .show_add_buttons(false)
        .show_leaf_close_all_buttons(false)
        .show_leaf_collapse_buttons(false)
        .show_inside(ui, viewer);
}

struct Viewer<'a> {
    /// Where titles are translated.
    ctx: Egui,
    panels: &'a Panels,
    contents: &'a mut BTreeMap<String, Content>,
    focused: &'a mut Option<String>,
}

impl TabViewer for Viewer<'_> {
    type Tab = String;

    fn id(&mut self, tab: &mut String) -> Id {
        Id::new(("helios.panel", tab.as_str()))
    }

    fn title(&mut self, tab: &mut String) -> egui::WidgetText {
        panel_title(&self.ctx, self.panels, tab).into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut String) {
        let rect = ui.max_rect();
        name_rect(ui.ctx(), tab, rect);
        if ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(rect) {
            *self.focused = Some(tab.clone());
        }
        if let Some(c) = self.contents.get_mut(tab) {
            c(ui);
        } else {
            let title = panel_title(ui.ctx(), self.panels, tab);
            placeholder(ui, &title);
        }
    }

    fn allowed_in_windows(&self, _tab: &mut String) -> bool {
        // Detached panels get real windows, through `<panel>.detach`; a tab
        // dropped over another area is moved there by the shell.
        false
    }
}
