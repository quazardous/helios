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

use std::path::{Path, PathBuf};

use egui_pop::i18n::I18n;
use egui_pop::model::panels::PanelSpec;
use helios_core::ModuleRef;
use helios_shell::commands::CommandSpec;
use helios_template::{Built, Bundle, Catalogue, parse_composite};
use serde_json::{Value as Json, json};

use crate::host::workspace_key;

/// The built-in templates, each as its bundle's files.
const BUILTIN: &[&[(&str, &str)]] = &[&[
    (
        "template.json",
        include_str!("../../../templates/portrait-retouch.helios-template/template.json"),
    ),
    (
        "layout.json",
        include_str!("../../../templates/portrait-retouch.helios-template/layout.json"),
    ),
    (
        "i18n/en/template.ftl",
        include_str!("../../../templates/portrait-retouch.helios-template/i18n/en/template.ftl"),
    ),
    (
        "i18n/fr/template.ftl",
        include_str!("../../../templates/portrait-retouch.helios-template/i18n/fr/template.ftl"),
    ),
]];

/// The composite files Helios ships.
const COMPOSITES: &[&str] = &[include_str!(
    "../../../templates/composites/helios.masked-adjustment@1.helios-composite.json"
)];

/// Where a workspace keeps the template it was made from.
pub const TEMPLATE_DIR: &str = "template.helios-template";

/// The built-in templates.
pub fn builtin() -> Vec<Bundle> {
    BUILTIN
        .iter()
        .map(|files| Bundle::from_files(files).expect("the built-in templates read"))
        .collect()
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
    for text in COMPOSITES {
        c.add_composite(parse_composite(text).expect("the shipped composites read"));
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
    config.join("workspaces").join(workspace_key(workspace))
}

/// The layout document egui-pop keeps for a workspace, under the
/// interface's settings (`<config>/ui`).
pub fn layout_file(config: &Path, workspace: &Path) -> PathBuf {
    config
        .join("ui")
        .join("workspaces")
        .join(workspace_key(workspace))
        .join("layout.json")
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
    let report = template.validate(catalogue);
    if !report.is_ok() {
        return Err(format!(
            "template {}: {}",
            template.template.id,
            report.problems.join("; ")
        ));
    }
    let mut copy = template.clone();
    copy.bundle_composites(catalogue)?;
    let built = copy.build(catalogue)?;
    std::fs::create_dir_all(workspace).map_err(|e| format!("{}: {e}", workspace.display()))?;
    let workspace = workspace
        .canonicalize()
        .map_err(|e| format!("{}: {e}", workspace.display()))?;
    let own = workspace_dir(config, &workspace);
    copy.save(&own.join(TEMPLATE_DIR))?;
    let pointer = own.join("workspace.json");
    let mut json = read_json(&pointer);
    if json.get("profile").is_none() {
        json["profile"] = json!("default");
    }
    json["template"] = json!({ "id": copy.template.id, "version": copy.template.version });
    write(&pointer, &json.to_string())?;
    write(&layout_file(config, &workspace), &built.layout.to_json())?;
    Ok(Created { workspace, built })
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

fn write(path: &Path, text: &str) -> Result<(), String> {
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, text))
        .map_err(|e| format!("{}: {e}", path.display()))
}
