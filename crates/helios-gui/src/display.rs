//! What the image is drawn in: the Wide-gamut preview setting, off by
//! default, and what the viewport's window wants from it.
//!
//! Off, the window wants sRGB, as every surface shows it. On, it wants
//! Rec.2020 primaries with a 2.2 power (the encoding a Wayland compositor
//! with the colour-management protocol maps to the screen itself); the
//! viewport draws it when the window's surface carries it, and sRGB
//! otherwise, with the reason (egui-pop's `display::resolve`). Only the
//! image asks for it: egui's interface stays sRGB.
//!
//! The debug bridge's `helios.display` collector says what the display
//! offers (`helios_display::probe`: the session, the compositor's colour
//! management) and what the window resolved (wanted, surface, drawn and
//! why); egui-pop's `viewport` collector, registered here too, gives it
//! per viewport.

use std::sync::OnceLock;

use egui_pop::api::{AppHandle, Ctx, Plugin, PluginError};
use egui_pop::core::debug::FnCollector;
use egui_pop::model::ids::{PluginId, SettingId};
use egui_pop::model::values::Descriptor;
use egui_pop::viewport::display::{self as pop_display, DisplayTarget};
use serde_json::{Value as Json, json};

use crate::viewport::rec2020_gamma22;

pub const PLUGIN: PluginId = PluginId::from_static("helios.display");

/// The Wide-gamut preview setting, at `settings/display.wide-gamut-preview`.
pub const SETTING_WIDE_GAMUT: SettingId = SettingId::from_static("display.wide-gamut-preview");

/// Whether the Wide-gamut preview setting is on.
pub fn wide_gamut(app: &AppHandle) -> bool {
    app.store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&format!("settings/{SETTING_WIDE_GAMUT}"))
        .and_then(|e| e.value.as_bool())
        .unwrap_or(false)
}

/// What the viewport's window wants: Rec.2020 with gamma 2.2 for a
/// wide-gamut preview, sRGB otherwise.
pub fn wanted(wide_gamut: bool) -> DisplayTarget {
    if wide_gamut {
        rec2020_gamma22()
    } else {
        DisplayTarget::srgb()
    }
}

/// What the display offers, asked once per process (one connection to
/// the compositor).
pub fn offer() -> &'static helios_display::Offer {
    static OFFER: OnceLock<helios_display::Offer> = OnceLock::new();
    OFFER.get_or_init(helios_display::probe)
}

/// The `helios.display` snapshot: the setting, what the display offers,
/// and what the main window resolved.
pub fn snapshot(ctx: &egui::Context, wide_gamut: bool) -> Json {
    let offer = offer();
    let r = pop_display::resolution(ctx);
    json!({
        "wide_gamut_preview": wide_gamut,
        "session": offer.session,
        "color_management": offer.color_management,
        "error": offer.error,
        "resolution": {
            "wanted": r.wanted.describe(),
            "surface": r.surface.describe(),
            "drawn": r.drawn.describe(),
            "faithful": r.faithful(),
            "reason": r.reason(),
        },
        "expected": helios_display::drawn(offer),
    })
}

fn collector(app: AppHandle) -> FnCollector {
    FnCollector::new(
        "helios.display",
        "What the display offers (session, the compositor's colour management) and what the image is drawn in.",
    )
    .snapshot(move |ctx| snapshot(ctx, wide_gamut(&app)))
}

/// Declares the display's settings and its debug collectors.
pub struct Display;

impl Plugin for Display {
    fn id(&self) -> PluginId {
        PLUGIN
    }

    fn title(&self) -> String {
        "Display".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        ctx.setting(
            SETTING_WIDE_GAMUT,
            Descriptor::bool("Wide-gamut preview", false).description(
                "Shows the image in Rec.2020 where the window can carry it (Wayland with colour management), so colours outside sRGB are not clipped. Elsewhere the image stays sRGB.",
            ),
        )?;
        ctx.collector(collector(ctx.app()))?;
        ctx.collector(egui_pop::viewport::debug::collector())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_says_what_is_drawn_and_why() {
        let ctx = egui::Context::default();
        pop_display::want(&ctx, egui::ViewportId::ROOT, wanted(true));
        let s = snapshot(&ctx, true);
        assert_eq!(s["wide_gamut_preview"], true);
        // No surface configured for it: sRGB, with the reason.
        assert_eq!(s["resolution"]["faithful"], false);
        assert!(s["resolution"]["drawn"].as_str().unwrap().contains("sRGB"));
        assert!(s["resolution"]["reason"].is_string());
        assert_eq!(s["expected"]["encoding"], "sRGB");
    }

    #[test]
    fn off_wants_srgb_and_on_wants_rec2020() {
        assert_eq!(wanted(false), DisplayTarget::srgb());
        assert_eq!(wanted(true), rec2020_gamma22());
    }
}
