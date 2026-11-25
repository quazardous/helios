//! The menus of commands panels ask for (`dialogs::Request::Menu`: a
//! photo's, an album's, a folder's, a keyword's), drawn in the window
//! they were asked from.
//!
//! A menu is asked while a window draws, at a point in that window's
//! points; the shell answers the asks in the main window's pass. The menu
//! is not drawn there: it is a block of egui-pop's `WINDOW_OVERLAYS` zone,
//! which every window draws in its own pass (the main window, a detached
//! panel's, a focus window such as the Library's), and the block draws
//! the open menu only in the pass of the window it belongs to. A menu
//! whose window closed before it showed goes to the main window, said in
//! the log. The item chosen runs in the main window's pass, as any
//! command a panel asks (`dialogs::Request::Run`).

use std::cell::RefCell;
use std::rc::Rc;

use egui::{Id, Pos2, Rect, Ui, ViewportId};
use egui_pop::api::app::AppHandle;
use egui_pop::api::zones::Block;
use egui_pop::model::ids::{BlockId, CommandId};
use egui_pop::model::when::WhenContext;
use helios_shell::commands::Origin;
use serde_json::Value;

use crate::dialogs::MenuItem;

/// The block drawing the open menu in each window's pass.
pub(crate) const BLOCK: BlockId = BlockId::from_static("helios.menus");

/// The menu's name for pictures and agents (`target_rect`).
pub(crate) const RECT: &str = "helios.menu";

/// A menu open.
#[derive(Clone, Debug)]
pub(crate) struct Open {
    /// Where it opens, in `window`'s points.
    pub(crate) at: Pos2,
    pub(crate) items: Vec<MenuItem>,
    /// The window it was asked from, the one it is drawn in.
    pub(crate) window: ViewportId,
    /// Where it was drawn last, in `window`'s points.
    pub(crate) rect: Option<Rect>,
}

/// The menu open, if one is: shared by the shell, which opens and closes
/// it, and the block drawing it.
#[derive(Clone, Default)]
pub(crate) struct Menus(Rc<RefCell<Option<Open>>>);

impl Menus {
    /// Opens a menu in `window`, at `at` of its points; the one open
    /// closes.
    pub(crate) fn open(&self, window: ViewportId, at: Pos2, items: Vec<MenuItem>) {
        log::debug!(target: "helios", "menu: opened in window {window:?} at {at:?}");
        *self.0.borrow_mut() = Some(Open {
            at,
            items,
            window,
            rect: None,
        });
    }

    pub(crate) fn close(&self) {
        *self.0.borrow_mut() = None;
    }

    /// Where the open menu was drawn last, and in which window.
    pub(crate) fn shown(&self) -> Option<(Rect, ViewportId)> {
        let open = self.0.try_borrow().ok()?;
        let open = open.as_ref()?;
        Some((open.rect?, open.window))
    }

    /// The window the open menu belongs to, if one is open.
    pub(crate) fn window(&self) -> Option<ViewportId> {
        self.0.try_borrow().ok()?.as_ref().map(|o| o.window)
    }

    /// The block of `WINDOW_OVERLAYS` drawing it.
    pub(crate) fn block(&self) -> Block {
        let menus = self.clone();
        Block::add(BLOCK, 20, move |ui, app| menus.draw(ui, app))
    }

    /// Draws the open menu if it belongs to the window drawing.
    fn draw(&self, ui: &mut Ui, app: &AppHandle) {
        let here = ui.ctx().viewport_id();
        let Ok(mut slot) = self.0.try_borrow_mut() else {
            return;
        };
        let Some(open) = slot.as_mut() else { return };
        if open.window != here {
            // Its window is gone: the menu shows in the main window
            // rather than nowhere.
            if here == ViewportId::ROOT && !still_open(app, open.window) {
                log::warn!(
                    target: "helios",
                    "menu: its window {:?} closed, shown in the main window",
                    open.window
                );
                open.window = ViewportId::ROOT;
            } else {
                return;
            }
        }
        let mut shown = true;
        let mut chosen = None;
        let cx = app.when_context();
        let r = egui::Popup::new(
            Id::unique(RECT),
            ui.ctx().clone(),
            open.at,
            egui::LayerId::background(),
        )
        .kind(egui::PopupKind::Menu)
        .open_bool(&mut shown)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(200.0);
            egui::containers::menu::menu_style(ui.style_mut());
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                rows(ui, app, &cx, &open.items, &mut chosen);
            });
        });
        if let Some(r) = r {
            open.rect = Some(r.response.rect);
        }
        if let Some((id, args)) = chosen {
            *slot = None;
            // Run by the shell, in the main window's pass.
            crate::dialogs::request(
                ui.ctx(),
                crate::dialogs::Request::Run(id, args, Origin::User),
            );
            if here != ViewportId::ROOT {
                super::main_pass::ask(app, "menu");
            }
        } else if !shown {
            *slot = None;
        }
    }
}

/// Whether `window` is still open: the main window, a detached panel's
/// or an application window's.
fn still_open(app: &AppHandle, window: ViewportId) -> bool {
    window == ViewportId::ROOT
        || app.window_panel(window).is_some()
        || app.window_at(window).is_some()
}

/// A menu's rows, the item chosen kept in `chosen`.
fn rows(
    ui: &mut Ui,
    h: &AppHandle,
    cx: &WhenContext,
    items: &[MenuItem],
    chosen: &mut Option<(String, Value)>,
) {
    use egui_pop::core::ui::UiPop as _;
    for item in items {
        match item {
            MenuItem::Separator => {
                ui.divider();
            }
            MenuItem::Submenu { label, items } => {
                egui_pop::api::menus::submenu(ui, label, |ui| rows(ui, h, cx, items, chosen));
            }
            MenuItem::Command {
                id,
                args,
                label,
                enabled,
            } => {
                let Ok(cid) = CommandId::new(id.as_str()) else {
                    continue;
                };
                let Some(spec) = h.command(&cid) else {
                    continue;
                };
                let title = label.clone().unwrap_or_else(|| h.command_menu_title(&spec));
                let keys = h.keys_for(&cid).first().cloned();
                // A row whose command does not apply says why; one its
                // owner greyed (an offline folder's) says nothing more.
                let why = (*enabled && !h.applies(&cid, cx))
                    .then(|| format!("Does not apply here ({})", spec.when));
                // egui-pop's menu row: the kit's look, a menu item named by
                // its title for screen readers and accesskit tests.
                let r = ui
                    .add_enabled_ui(*enabled, |ui| {
                        egui_pop::api::menus::command_row(
                            ui,
                            &title,
                            keys.as_deref(),
                            why.as_deref(),
                        )
                    })
                    .inner;
                if r.clicked() {
                    *chosen = Some((id.clone(), args.clone()));
                    ui.close();
                }
            }
        }
    }
}
