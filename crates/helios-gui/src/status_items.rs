//! Helios's items in egui-pop's status bar, each shown only where it
//! means something (a `when` clause over the context stack's keys): the
//! number of images in the library, the zoom and the history's length in
//! the darkroom, the picks and rejects while culling. egui-pop's own
//! breadcrumb and mode chips start the bar.
//!
//! Each frame [`Items::frame`] measures what the items say and publishes
//! an event (`helios.status.<item>`) when it changed; each item's reducer
//! takes the text and the count from it. A hidden item keeps following
//! its events, so it is current when its place comes back.

use std::collections::BTreeMap;

use egui_pop::api::AppHandle;
use egui_pop::api::strips::{Indicator, IndicatorState};
use egui_pop::model::context::FrameSpec;
use helios_core::Message;
use serde_json::{Value as Json, json};

use crate::contexts::{CULLING, DARKROOM, LIBRARY};

/// The source the items' events are published from.
const SOURCE: &str = "helios.status";

/// The library's images shown (the filter's matches).
pub const IMAGES: &str = "helios.status.images";
/// The darkroom's zoom.
pub const ZOOM: &str = "helios.status.zoom";
/// The darkroom's history: the steps there are to undo.
pub const HISTORY: &str = "helios.status.history";
/// Culling: the images picked and rejected.
pub const CULLED: &str = "helios.status.culled";

/// What the items say, measured by the shell each frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measure {
    /// Images shown in the library.
    pub images: usize,
    /// Of them, picked and rejected.
    pub picked: usize,
    pub rejected: usize,
    /// The zoom on screen (1 is 100 %), once an image is shown.
    pub zoom: Option<f64>,
    /// Steps to undo.
    pub history: usize,
}

/// The items, declared with the frames they show in.
pub fn indicators(frames: &[FrameSpec]) -> Vec<Indicator> {
    let frame = |id| frames.iter().find(|f| f.id == id);
    let item = |id: &str, label: &str, weight: i32| {
        Indicator::new(id, label, id).weight(weight).reduce(reduce)
    };
    let mut items = Vec::new();
    if let Some(library) = frame(LIBRARY) {
        items.push(item(IMAGES, "", 30).during(library));
    }
    if let Some(darkroom) = frame(DARKROOM) {
        items.push(item(ZOOM, "", 20).during(darkroom));
        items.push(item(HISTORY, "", 25).during(darkroom));
    }
    if let Some(culling) = frame(CULLING) {
        items.push(item(CULLED, "", 30).during(culling));
    }
    items
}

/// An item's event: its text, its tooltip and its count, if any.
fn reduce(state: &mut IndicatorState, event: &egui_pop::api::host::Event) {
    if let Some(text) = event.data["text"].as_str() {
        state.text = text.to_string();
    }
    if let Some(detail) = event.data["detail"].as_str() {
        state.detail = detail.to_string();
    }
    state.count = event.data["count"].as_u64();
}

/// What each item said last, to publish only what changed.
#[derive(Debug, Default)]
pub struct Items {
    said: BTreeMap<&'static str, Json>,
}

impl Items {
    /// Publishes what changed since the last frame, in the interface's
    /// language (a change of language says it again).
    pub fn frame(&mut self, app: &AppHandle, ctx: &egui::Context, m: &Measure) {
        let t = |msg: Message| crate::i18n::text(ctx, msg);
        let images = json!({
            "text": t(Message::new("gui-status-images", "{ $count } images")
                .with("count", m.images)),
        });
        let zoom = json!({
            "text": m.zoom.map(|z| format!(
                "{} %",
                egui_pop::viewport::navigator::percent_text(ctx, z * 100.0)
            )).unwrap_or_default(),
            "detail": t(Message::new("gui-status-zoom", "Zoom")),
        });
        let history = json!({
            "text": t(Message::new("gui-status-history", "History")),
            "detail": t(Message::new(
                "gui-status-history-detail",
                "{ $count } steps to undo"
            ).with("count", m.history)),
            "count": m.history,
        });
        let culled = json!({
            "text": t(Message::new("gui-status-culled", "{ $picked } picked, { $rejected } rejected")
                .with("picked", m.picked)
                .with("rejected", m.rejected)),
        });
        for (id, data) in [
            (IMAGES, images),
            (ZOOM, zoom),
            (HISTORY, history),
            (CULLED, culled),
        ] {
            if self.said.get(id) != Some(&data) {
                app.publish(SOURCE, None, id, data.clone());
                self.said.insert(id, data);
            }
        }
    }
}
