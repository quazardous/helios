//! The Navigator panel: egui-pop's zoom control and navigator
//! (`egui_pop::viewport::navigator`) over the viewport's own state, the
//! whole image drawn from the preview (`crate::preview`).
//!
//! Both move the viewport's view as its own gestures do: each change goes
//! through egui-pop's `zoom.change` step (once finished) and the
//! viewport's `before_view` hook, then into the value store
//! (`view/viewport/*`), where the viewport reads it. A drag of the frame
//! is one gesture, holding the view against other origins until it ends;
//! Escape puts the view back. None of it is an undo step: the view's
//! values are transient.

use std::cell::RefCell;
use std::sync::{Mutex, PoisonError};

use egui::{Color32, ColorImage, TextureHandle, Ui};
use egui_pop::api::AppHandle;
use egui_pop::viewport::navigator::{Navigator, ZoomControl};
use helios_values::{GestureId, Store};

use crate::preview::Preview;
use crate::viewport::{Viewport, view_of, write_view};

/// The panel's id.
pub const PANEL: &str = "helios.navigator";

/// The name the view's step gives the viewport.
const VIEWPORT: &str = "helios.viewport";

/// What the panel keeps between frames: the preview's texture, the drag
/// under way.
#[derive(Default)]
pub struct NavigatorPanel {
    texture: Option<(u64, TextureHandle)>,
    gesture: Option<GestureId>,
}

/// The preview, as the display shows it (sRGB, 8 bits).
pub fn picture(image: &helios_core::Image) -> ColorImage {
    let r = image.region();
    let n = image.channels() as usize;
    let pixels = image
        .data()
        .chunks_exact(n.max(1))
        .map(|p| {
            let at = |c: usize| p[c.min(n - 1)];
            let [red, green, blue] = crate::viewport::display([at(0), at(1), at(2)]);
            Color32::from_rgb(red, green, blue)
        })
        .collect();
    ColorImage::new([r.width as usize, r.height as usize], pixels)
}

impl NavigatorPanel {
    /// Draws the zoom control and the navigator; writes what they change
    /// into `store`.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        viewport: &RefCell<Viewport>,
        preview: &Preview,
        store: &Mutex<Store>,
        app: &AppHandle,
    ) {
        if let Some((n, image)) = preview.latest()
            && self.texture.as_ref().is_none_or(|(seen, _)| *seen != n)
        {
            let texture = ui.ctx().load_texture(
                "helios.navigator",
                picture(&image),
                egui::TextureOptions::LINEAR,
            );
            self.texture = Some((n, texture));
        }
        let mut vp = viewport.borrow_mut();
        let size = vp.image_size();
        let mut view = view_of(&store.lock().unwrap_or_else(PoisonError::into_inner));
        let zoom = ZoomControl::new(vp.state_mut(), &mut view)
            .on_change(egui_pop::imaging::zoom(app, VIEWPORT))
            .show(ui);
        let mut nav = Navigator::new(vp.state_mut(), &mut view)
            .on_change(egui_pop::imaging::zoom(app, VIEWPORT));
        if let Some((_, t)) = &self.texture {
            nav = nav.texture(t.id());
        }
        if let Some(s) = size {
            nav = nav.image_size(s);
        }
        let out = nav.show(ui);
        drop(vp);
        crate::name_rect(ui.ctx(), "helios.navigator.map", out.response.rect);
        crate::name_rect(ui.ctx(), "helios.navigator.zoom", zoom.response.rect);
        let mut s = store.lock().unwrap_or_else(PoisonError::into_inner);
        for c in zoom.changes.iter().chain(&out.changes) {
            if c.done {
                write_view(&mut s, &c.to, self.gesture);
                if let Some(g) = self.gesture.take() {
                    s.end(g);
                }
            } else {
                let g = *self.gesture.get_or_insert_with(|| {
                    s.begin_gesture(helios_shell::commands::Origin::User, "move the view")
                });
                write_view(&mut s, &c.to, Some(g));
            }
        }
        // Given up (Escape): the view goes back, no trace.
        if out.response.drag_stopped()
            && let Some(g) = self.gesture.take()
        {
            s.cancel(g);
        }
    }
}
