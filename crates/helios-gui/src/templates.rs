//! Workspace templates on the desktop (see `docs/templates.md`): the
//! built-in ones, what they may name here (the modules the desktop links,
//! the composites Helios ships, its panels and commands), their text in
//! the interface's language, and a workspace created from one.
//!
//! A workspace made from a template keeps a copy of it, its composites
//! bundled, among the workspace's own files
//! (`<config>/workspaces/<key>/template.helios-template`), named in its
//! `workspace.json`; its layout is written as the workspace's layout
//! document, which egui-pop loads when the workspace opens.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui_pop::i18n::I18n;
use egui_pop::model::panels::PanelSpec;
use helios_core::ModuleRef;
use helios_shell::commands::CommandSpec;
use helios_template::{Built, Bundle, Catalogue};
use serde_json::{Value as Json, json};

/// Where a workspace keeps the template it was made from.
pub const TEMPLATE_DIR: &str = helios_template::workspace::ORIGIN_DIR;

/// The built-in templates.
pub fn builtin() -> Vec<Bundle> {
    helios_template::shipped::templates()
}

/// The templates to create a workspace from: the built-in ones, then the
/// user's (`<config>/templates`) that read.
pub fn all(config: &Path) -> Vec<Bundle> {
    let mut all = builtin();
    all.extend(
        helios_template::workspace::user_templates(config)
            .into_iter()
            .filter_map(Result::ok),
    );
    all
}

/// A built-in template by id.
pub fn find(id: &str) -> Option<Bundle> {
    builtin().into_iter().find(|b| b.template.id == id)
}

/// The modules the desktop links and the composites Helios ships.
pub fn modules() -> Catalogue {
    use helios_module_masks as m;
    let mut c = Catalogue::new()
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .module(ModuleRef::new(m::MaskCombine))
        .module(ModuleRef::new(m::MaskFeather))
        .module(ModuleRef::new(m::MaskBlur))
        .module(ModuleRef::new(m::MaskLevels))
        .module(ModuleRef::new(m::MaskBlend))
        .module(ModuleRef::new(m::MaskFromLuma))
        .module(ModuleRef::new(m::MaskParametric))
        .module(ModuleRef::new(m::MaskRaster))
        .module(ModuleRef::new(m::MaskCircle))
        .module(ModuleRef::new(m::MaskEllipse))
        .module(ModuleRef::new(m::MaskGradient))
        .module(ModuleRef::new(m::MaskPath))
        .module(ModuleRef::new(m::MaskBrush));
    for file in helios_template::shipped::composites() {
        c.add_composite(file);
    }
    c
}

/// What a template may name on this desktop: [`modules`], and the panels
/// and commands given (the shell's).
pub fn catalogue(panels: &[PanelSpec], commands: Vec<CommandSpec>) -> Catalogue {
    modules()
        .panels(panels.to_vec())
        .commands(commands.into_iter().map(|c| c.id))
}

/// Adds the templates' text to the interface's catalogues, each under
/// its own keys (a plugin's rule: `<id>-…`).
pub fn add_texts(i18n: &I18n, bundles: &[Bundle]) -> Result<(), String> {
    for b in bundles {
        let catalogues: Vec<helios_i18n::plugins::PluginCatalogue> = b
            .texts
            .iter()
            .map(|t| helios_i18n::plugins::PluginCatalogue {
                locale: t.locale.clone(),
                name: t.name.clone(),
                source: t.source.clone(),
            })
            .collect();
        helios_i18n::plugins::add(i18n, &b.template.id, &[], &catalogues)
            .map_err(|e| format!("template {}: {e}", b.template.id))?;
    }
    Ok(())
}

/// A workspace's own files: `<config>/workspaces/<key>`.
pub fn workspace_dir(config: &Path, workspace: &Path) -> PathBuf {
    helios_template::workspace::Files::new(config, workspace).own
}

/// The layout document egui-pop keeps for a workspace, under the
/// interface's settings (`<config>/ui`).
pub fn layout_file(config: &Path, workspace: &Path) -> PathBuf {
    helios_template::workspace::Files::new(config, workspace).layout
}

/// A workspace created from a template.
pub struct Created {
    /// The workspace's directory, as opened.
    pub workspace: PathBuf,
    /// The template, as built.
    pub built: Built,
}

/// Creates a workspace from a template: the directory (made when
/// missing), a copy of the template among the workspace's own files, its
/// layout as the workspace's layout document. Refused, before anything
/// is written, when the template does not check against `catalogue`.
pub fn create_workspace(
    template: &Bundle,
    catalogue: &Catalogue,
    workspace: &Path,
    config: &Path,
) -> Result<Created, String> {
    create_answered(template, &BTreeMap::new(), catalogue, workspace, config)
}

/// As [`create_workspace`], the template's parameters answered (by id).
pub fn create_answered(
    template: &Bundle,
    answers: &BTreeMap<String, Json>,
    catalogue: &Catalogue,
    workspace: &Path,
    config: &Path,
) -> Result<Created, String> {
    helios_template::workspace::create(template, answers, catalogue, workspace, config)
        .map(|(workspace, built)| Created { workspace, built })
}

/// The template a workspace was made from, if it was.
pub fn workspace_template(config: &Path, workspace: &Path) -> Option<Result<Bundle, String>> {
    let dir = workspace_dir(config, workspace).join(TEMPLATE_DIR);
    dir.is_dir().then(|| Bundle::load(&dir))
}

/// A JSON object read from a file; an empty one when it is missing or
/// unreadable.
pub fn read_json(path: &Path) -> Json {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Json>(&t).ok())
        .filter(Json::is_object)
        .unwrap_or_else(|| json!({}))
}
