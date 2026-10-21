//! egui-pop's theme editor, as a page of the settings window: Settings ›
//! Appearance › Theme editor.
//!
//! `ThemeEditorPlugin` holds the editor (its state, the `theme.token` and
//! `theme.load` steps, the `theme.*` agent commands); [`Page`] mounts its
//! view in the settings window. The editor works on the theme installed
//! (Helios's theme in use, with its overrides): every edit is installed
//! before the next frame, so the whole window follows. Helios's themes
//! come as a light and dark pair (`helios-light` is the variant of
//! `helios-dark`), which the editor edits as a pair, each variant with its
//! own changes. Its files are opened and saved through the files plugin's
//! chooser (egui-pop's default plugins), which the editor takes when the
//! host gives none. The shell takes the edits as the theme's overrides
//! (`Shell::follow_theme`), so a profile saves them.

use egui_pop::api::settings_pages::SettingsPage;
use egui_pop::api::theme_editor;
use egui_pop::api::{Ctx, Plugin, PluginError};
use egui_pop::model::ids::PluginId;

pub const PLUGIN: PluginId = PluginId::from_static("helios.theme_editor");

/// The page's id, as `app.settings.page` takes it.
pub const PAGE: &str = "helios.theme-editor";

/// The settings window's section the page goes under.
pub const SECTION: &str = "Appearance";

/// The theme editor's page in the settings window.
pub struct Page;

impl Plugin for Page {
    fn id(&self) -> PluginId {
        PLUGIN
    }

    fn title(&self) -> String {
        "Theme editor".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        ctx.settings_page(
            SettingsPage::new(PAGE, "Theme editor")
                .under(SECTION)
                .keywords(&["theme", "colour", "color", "tokens", "contrast", "palette"])
                .description(
                    "The theme in use, token by token: each change applies at once, with its contrast; a light and dark pair is edited as a pair.",
                ),
            theme_editor::show,
        );
        Ok(())
    }
}
