//! The focus the Settings window edits (docs/design/windows.md, P9): the
//! main window's (its library, its workspace) or the import window's (the
//! library it imports into, no workspace). Opened from a window, the
//! Settings edit its focus; their header names it and a switch picks the
//! other (egui-pop's `admin::focus`). Editing a focus attaches the store
//! holding its values and offers its value scopes.

use std::path::Path;

use egui_pop::api::AppHandle;
use egui_pop::api::admin::focus::{SettingsFoci, SettingsFocus};
use egui_pop::api::windows::WindowId;
use helios_core::Message;

use super::{BRIDGE, lock};

/// The main window's focus.
pub const MAIN: &str = "main";
/// The import window's focus, while it is open.
pub const IMPORT: &str = "import";

/// The import window's id (`shell::import_window::WINDOW`).
const IMPORT_WINDOW: &str = "import";

/// Helios's foci, as the Settings app asks for them.
pub(super) struct Foci;

impl SettingsFoci for Foci {
    fn foci(&self, app: &AppHandle) -> Vec<SettingsFocus> {
        let (library, workspace) = match app.service(&BRIDGE) {
            Some(b) => {
                let st = lock(&b.state);
                (st.library.clone(), st.workspace_name.clone())
            }
            None => (None, None),
        };
        let ctx = app.egui();
        let main_window = app.window_of_group(egui::ViewportId::ROOT);
        let mut out = vec![SettingsFocus::new(
            MAIN,
            main_label(&ctx, library.as_deref(), workspace.as_deref()),
            &[main_window],
        )];
        let import = WindowId::new(IMPORT_WINDOW);
        if app.window_viewport(&import).is_some() {
            out.push(SettingsFocus::new(
                IMPORT,
                import_label(&ctx, library.as_deref()),
                &[import],
            ));
        }
        out
    }

    fn edit(&self, app: &AppHandle, focus: &str) -> Result<(), String> {
        let bridge = app.service(&BRIDGE).ok_or("no settings bridge")?;
        let import = match focus {
            MAIN => false,
            IMPORT => true,
            other => return Err(format!("no focus `{other}`")),
        };
        lock(&bridge.state).import_edited = import;
        super::reattach(app);
        Ok(())
    }
}

/// The library's name, from its project layer's (`project/<folder>`).
pub(super) fn library_name(project: &str) -> String {
    let folder = project.strip_prefix("project/").unwrap_or(project);
    Path::new(folder)
        .file_name()
        .map_or_else(|| folder.to_string(), |n| n.to_string_lossy().into_owned())
}

fn main_label(ctx: &egui::Context, library: Option<&str>, workspace: Option<&str>) -> String {
    let window = crate::i18n::text(ctx, Message::new("gui-settings-focus-main", "Main window"));
    let m = match (library, workspace) {
        (Some(l), Some(w)) => Message::new(
            "gui-settings-focus-workspace",
            "{ $window }: workspace { $workspace }, library { $library }",
        )
        .with("window", window)
        .with("workspace", w)
        .with("library", l),
        (None, Some(w)) => Message::new(
            "gui-settings-focus-workspace-alone",
            "{ $window }: workspace { $workspace }",
        )
        .with("window", window)
        .with("workspace", w),
        (Some(l), None) => Message::new(
            "gui-settings-focus-library",
            "{ $window }: library { $library }",
        )
        .with("window", window)
        .with("library", l),
        (None, None) => return window,
    };
    crate::i18n::text(ctx, m)
}

fn import_label(ctx: &egui::Context, library: Option<&str>) -> String {
    let window = crate::i18n::text(
        ctx,
        Message::new("gui-settings-focus-import", "Import window"),
    );
    match library {
        Some(l) => crate::i18n::text(
            ctx,
            Message::new(
                "gui-settings-focus-library",
                "{ $window }: library { $library }",
            )
            .with("window", window)
            .with("library", l),
        ),
        None => window,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_library_is_named_by_its_folder() {
        assert_eq!(super::library_name("project//photos"), "photos");
        assert_eq!(super::library_name("project//home/me/Pictures"), "Pictures");
    }
}
