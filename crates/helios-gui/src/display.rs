//! What the image is drawn in: the Wide-gamut preview setting, off by
//! default, and what the viewport's window wants from it.
//!
//! Off, the window wants sRGB, as every surface shows it. On, it wants
//! Rec.2020 primaries with a 2.2 power (the encoding a Wayland compositor
//! with the colour-management protocol maps to the screen itself); the
//! viewport draws it when the window's surface carries it, and sRGB
//! otherwise, with the reason (egui-pop's `display::resolve`). Only the
//! image asks for it: egui's interface stays sRGB.

use egui_pop::api::{AppHandle, Ctx, Plugin, PluginError};
use egui_pop::model::ids::{PluginId, SettingId};
use egui_pop::model::values::Descriptor;
use egui_pop::viewport::display::DisplayTarget;

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

/// Declares the display's settings.
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_wants_srgb_and_on_wants_rec2020() {
        assert_eq!(wanted(false), DisplayTarget::srgb());
        assert_eq!(wanted(true), rec2020_gamma22());
    }
}
