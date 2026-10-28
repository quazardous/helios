//! Helios's tips, over egui-pop's tips (`egui_pop::api::tips`): one idea
//! per file, `crates/helios-gui/tips/<id>.md`, built in. egui-pop shows
//! them (the card beside what it is about, Help > Tips…, the rules that
//! choose one) and remembers what was dismissed; their French is in
//! `i18n/fr/tips.ftl`, their English generated in `i18n/en/tips.ftl`.

use egui_pop::api::tips::{TIPS, TIPS_PLUGIN};
use egui_pop::api::{Ctx, Plugin, PluginError};
use egui_pop::model::ids::PluginId;

/// The plugin declaring Helios's tips; its id is their origin, which
/// prefixes their ids and groups them in Help > Tips….
pub const PLUGIN: PluginId = PluginId::from_static("helios");

/// A tip's file, by its id.
macro_rules! tip {
    ($id:literal) => {
        (
            concat!($id, ".md"),
            include_str!(concat!("../tips/", $id, ".md")),
        )
    };
}

/// Helios's tips, `(file name, text)`.
pub const FILES: &[(&str, &str)] = &[
    tip!("helios.palette"),
    tip!("helios.system-menu"),
    tip!("helios.fullscreen"),
    tip!("helios.keyboard-shortcuts"),
    tip!("helios.move-panels"),
    tip!("helios.profiles"),
    tip!("helios.zoom"),
    tip!("helios.mask-display"),
    tip!("helios.typed-values"),
    tip!("helios.reset-value"),
    tip!("helios.rate"),
    tip!("helios.scopes-linear"),
    tip!("helios.decisions"),
];

/// Registers Helios's tips with egui-pop's tips service. They come by
/// themselves only once the desktop asks
/// (`Shell::tips_by_themselves`).
#[derive(Debug, Default)]
pub struct Tips;

impl Plugin for Tips {
    fn id(&self) -> PluginId {
        PLUGIN
    }

    fn title(&self) -> String {
        "Helios".into()
    }

    fn requires(&self) -> Vec<PluginId> {
        vec![TIPS_PLUGIN]
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        let tips = ctx
            .app()
            .service(&TIPS)
            .ok_or_else(|| PluginError::Failed("the tips plugin is not connected".into()))?;
        let errors = tips.add_files(PLUGIN.as_str(), FILES);
        if !errors.is_empty() {
            return Err(PluginError::Failed(format!("Helios's tips: {errors:?}")));
        }
        Ok(())
    }
}
