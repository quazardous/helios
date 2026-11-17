//! The panels that follow the photo shown ([`crate::FOLLOWS_PHOTO`]: the
//! scopes, the navigator) say when what they show is not current, on the
//! bus (egui-pop's `panel.state`); egui-pop's chrome shows it, the content
//! dimmed with a busy mark, and the panels draw none of it.
//!
//! Their state is one, read from what they wait on:
//!
//! - **updating** while the preview they measure renders (an edit,
//!   another photo's network);
//! - **stale** from the moment another photo is selected until its
//!   network is shown;
//! - **failed** when the preview's last render failed, with its reason;
//! - **ready** otherwise.
//!
//! It is asked on the bus only when it changes.

use egui_pop::api::AppHandle;
use egui_pop::api::bus::{PanelState, PanelStateAsk};

use crate::preview::Status;

/// Who asks the panels' states.
pub const OWNER: &str = "helios.photo_panels";

/// The state of the panels following the photo: what the preview does,
/// and whether a photo selected is still being opened.
pub fn state(preview: &Status, opening: bool) -> PanelState {
    match preview {
        Status::Busy => PanelState::Updating,
        _ if opening => PanelState::Stale,
        Status::Failed(why) => PanelState::Failed {
            reason: why.clone(),
        },
        Status::Done => PanelState::Ready,
    }
}

/// The state last asked of the panels following the photo.
#[derive(Debug, Default)]
pub struct PhotoPanels {
    asked: Option<PanelState>,
}

impl PhotoPanels {
    /// Asks `panel.state` of every panel following the photo when their
    /// state changed.
    pub fn follow(&mut self, app: &AppHandle, preview: &Status, opening: bool) {
        let now = state(preview, opening);
        if self.asked.as_ref() == Some(&now) {
            return;
        }
        for panel in crate::FOLLOWS_PHOTO {
            app.request_panel_state(PanelStateAsk::new(panel, now.clone(), OWNER));
        }
        self.asked = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_render_under_way_wins_then_the_opening_then_a_failure() {
        assert_eq!(state(&Status::Busy, true), PanelState::Updating);
        assert_eq!(state(&Status::Done, true), PanelState::Stale);
        assert_eq!(
            state(&Status::Failed("no input".into()), true),
            PanelState::Stale
        );
        assert_eq!(
            state(&Status::Failed("no input".into()), false),
            PanelState::Failed {
                reason: "no input".into()
            }
        );
        assert_eq!(state(&Status::Done, false), PanelState::Ready);
    }
}
