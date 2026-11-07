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
use helios_core::{Graph, ModuleRef, NodeId};
use helios_load::Kind;
use helios_shell::commands::CommandSpec;
use helios_template::{Built, Bundle, Catalogue, Source};
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

/// The modules the desktop links and the composites Helios ships: the
/// raw chain the node runs on a raw (levels, white balance, highlights,
/// demosaicing, camera colours), the tools on a picture, the masks.
pub fn modules() -> Catalogue {
    use helios_module_masks as m;
    let mut c = Catalogue::new()
        .module(ModuleRef::new(helios_module_rawprepare::RawPrepare))
        .module(ModuleRef::new(helios_module_whitebalance::WhiteBalance))
        .module(ModuleRef::new(helios_module_highlights::Highlights))
        .module(ModuleRef::new(helios_module_demosaic::Demosaic))
        .module(ModuleRef::new(helios_module_colorin::ColorIn))
        .module(ModuleRef::new(helios_module_exposure::Exposure))
        .module(ModuleRef::new(helios_module_sigmoid::Sigmoid))
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

/// A file through a workspace's network (`bundle`'s), to show: the graph
/// and the node the viewport shows. A network taking a raw (a raw or an
/// image source) is given a raw file's sensor mosaic with its camera; a
/// network reading only a raw refuses a picture. A raw the network cannot
/// develop (a picture network, a raw that is not a mosaic) shows its
/// embedded preview: exports go through [`develop_file`], which never
/// takes it.
pub fn open_file(path: &Path, bundle: &Bundle) -> Result<(Graph, NodeId), String> {
    through(path, bundle, true)
}

/// As [`open_file`], for an export: a raw goes through the raw chain or
/// is refused with the reason, never through its embedded preview.
pub fn develop_file(path: &Path, bundle: &Bundle) -> Result<(Graph, NodeId), String> {
    through(path, bundle, false)
}

fn through(path: &Path, bundle: &Bundle, preview: bool) -> Result<(Graph, NodeId), String> {
    let catalogue = modules();
    let network = &bundle.template.network;
    let name = &bundle.template.meta.name;
    let raw = Kind::of(path) == Kind::Raw;
    if network.reads_raw() && !raw {
        return Err(format!(
            "{}: “{name}” develops raw files; open this picture in a workspace for pictures",
            path.display(),
        ));
    }
    let built = if raw && network.takes_raw() {
        let developed = helios_load::load(path).and_then(|loaded| {
            let camera = loaded
                .camera
                .ok_or_else(|| format!("{}: no camera to develop it with", path.display()))?
                .to_context();
            bundle.build_with(
                &catalogue,
                Source::Raw {
                    mosaic: &loaded.image,
                    camera: &camera,
                },
                None,
            )
        });
        match developed {
            Ok(built) => built,
            // A raw that is not a mosaic (sRAW, a linear DNG) is shown from
            // its preview where the network takes a picture too.
            Err(_) if preview && !network.reads_raw() => embedded(path, bundle)?,
            Err(e) => return Err(e),
        }
    } else if raw {
        if !preview {
            return Err(format!(
                "{}: “{name}” takes pictures only, its network cannot develop a raw; \
                 export it from a workspace whose network takes raws",
                path.display(),
            ));
        }
        embedded(path, bundle)?
    } else {
        let image = helios_load::load(path)?.image;
        bundle.build_with(&catalogue, Source::Picture(&image), None)?
    };
    Ok((built.graph, built.view.0))
}

/// A raw's embedded preview through `bundle`'s network, as a picture.
fn embedded(path: &Path, bundle: &Bundle) -> Result<Built, String> {
    let image = helios_load::preview(path)?
        .ok_or_else(|| format!("{}: no preview to show", path.display()))?;
    bundle.build_with(&modules(), Source::Picture(&image), None)
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
