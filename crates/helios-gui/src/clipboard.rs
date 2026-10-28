//! Copy and paste through egui-pop's clipboard (`egui_pop::api::clipboard`,
//! ADR 0015 there; Helios's side is `docs/design/clipboard.md`).
//!
//! egui-pop keeps the items (its default owner, in memory, shared by the
//! windows), runs the `clipboard.copy` and `clipboard.paste` steps, the
//! Edit commands (`edit.cut`, `edit.copy`, `edit.paste`,
//! `edit.pasteSpecial`, greyed when nothing focused answers) and the
//! agents' `clipboard.*`. Helios brings:
//!
//! - **its types**, each `application/x-helios.<type>+json` with a
//!   readable text fallback: a parameter's value ([`param`]), nodes of
//!   the workspace's network ([`nodes`]), drawn mask shapes ([`mask`]),
//!   scaled to the image they are pasted onto, images of the library
//!   ([`images`]), imported when pasted into another library or from
//!   another application's files;
//! - **a transform**, `helios.mask-as-nodes`: shapes pasted into the
//!   graph view become nodes;
//! - **a sanitiser** for what leaves for other applications: the
//!   Save-as-template rules (`helios_template::workspace::strip_for_outside`):
//!   a local path becomes its file name, secrets and embedded data go,
//!   and the copy's toast says so; a text of several lines is stripped
//!   line by line ([`strip_lines`]);
//! - **endpoints**, the focused component's source and target: a
//!   parameter's widget ([`PARAM_ENDPOINT`]), the graph view
//!   ([`GRAPH_ENDPOINT`]), the Masks panel and the image
//!   ([`MASKS_ENDPOINT`], [`MASK_IMAGE_ENDPOINT`]), the grid and the
//!   filmstrip ([`LIBRARY_ENDPOINT`]). A paste whose edit is
//!   the shell's (the workspace's network, the image's masks) runs as a command at the next frame
//!   ([`defer`]);
//! - **one undo step per paste**: egui-pop's history group is a group of
//!   Helios's value store (`host::HeliosHost::begin_group`);
//! - **files and images with other applications** ([`system`]): "Copy
//!   files" ([`copy_files`]), the rendered image copied ([`rendered`]),
//!   and files or an image another application copied pasted into the
//!   library ([`paste_from_system`]), while egui-pop's system clipboard
//!   carries text only.

pub mod images;
pub mod mask;
pub mod nodes;
pub mod param;
pub mod system;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use egui_pop::api::clipboard::{
    self, Answer, Boundary, ClipType, CopyRequest, FormatId, PasteRequest, Repr, Sanitiser, Slot,
    TEXT, Transform, TransformSpec, Transformed,
};
use egui_pop::api::steps::Outcome;
use egui_pop::api::{AppHandle, Ctx, Plugin, PluginError};
use egui_pop::model::commands::Origin as PopOrigin;
use egui_pop::model::ids::PluginId;
use helios_values::Store;
use serde_json::Value as Json;

/// One parameter's value.
pub const PARAM: FormatId = FormatId::from_static("application/x-helios.param+json");

/// The endpoint of the parameter widget with the focus (or the one a
/// command names).
pub const PARAM_ENDPOINT: &str = "helios.param";

/// Nodes of the workspace's network, with the wires between them.
pub const NODES: FormatId = FormatId::from_static("application/x-helios.nodes+json");

/// The graph view's endpoint: its selection, the workspace's network.
pub const GRAPH_ENDPOINT: &str = "helios.graph";

/// Drawn mask shapes, with the size of the image they were drawn on.
pub const MASK: FormatId = FormatId::from_static("application/x-helios.mask+json");

/// The Masks panel's endpoint: the mask edited.
pub const MASKS_ENDPOINT: &str = "helios.masks";

/// The image's endpoint, for its masks: the same as the Masks panel's.
pub const MASK_IMAGE_ENDPOINT: &str = "helios.masks.image";

/// Images of the library, by id, with their names and files.
pub const IMAGES: FormatId = FormatId::from_static("application/x-helios.images+json");

/// The grid's and the filmstrip's endpoint: the library's images.
pub const LIBRARY_ENDPOINT: &str = "helios.library";

/// Shapes pasted into the graph view become nodes.
pub const MASK_AS_NODES: &str = "helios.mask-as-nodes";

/// The plugin declaring Helios's types and sanitiser.
pub const PLUGIN: PluginId = PluginId::from_static("helios.clipboard");

/// What leaves for other applications loses local paths, secrets and
/// embedded data.
pub const SANITISER: &str = "helios.outside";

/// Helios's clipboard types and sanitiser, declared to egui-pop (gone
/// with the plugin). egui-pop's `ClipboardPlugin` connects before it.
pub struct Types;

impl Plugin for Types {
    fn id(&self) -> PluginId {
        PLUGIN
    }

    fn title(&self) -> String {
        "Helios's clipboard types".into()
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        let failed = |e: egui_pop::api::CtxError| PluginError::Failed(e.to_string());
        ctx.clip_type(
            ClipType::new(PARAM, param::VERSION, "a parameter")
                .fallbacks(|p| vec![Repr::new(TEXT, Json::String(param::text_of(p)))]),
        )
        .map_err(failed)?
        .clip_type(
            ClipType::new(NODES, nodes::VERSION, "nodes")
                .fallbacks(|p| {
                    let text = nodes::Clip::from_json(p)
                        .map(|c| c.text())
                        .unwrap_or_default();
                    vec![Repr::new(TEXT, Json::String(text))]
                })
                .parts(nodes::parts),
        )
        .map_err(failed)?
        .clip_type(
            ClipType::new(MASK, mask::VERSION, "shapes")
                .fallbacks(|p| {
                    let text = mask::Clip::from_json(p)
                        .map(|c| c.text())
                        .unwrap_or_default();
                    vec![Repr::new(TEXT, Json::String(text))]
                })
                .parts(mask::parts),
        )
        .map_err(failed)?
        .clip_type(
            ClipType::new(IMAGES, images::VERSION, "images")
                .fallbacks(|p| {
                    let text = images::Clip::from_json(p)
                        .map(|c| c.text())
                        .unwrap_or_default();
                    vec![Repr::new(TEXT, Json::String(text))]
                })
                .parts(images::parts),
        )
        .map_err(failed)?
        .clip_transform(Transform::new(
            TransformSpec::new(MASK_AS_NODES, MASK, NODES),
            |r| {
                let clip = mask::Clip::from_json(&r.data)?;
                Ok(Transformed::exact(mask::as_nodes(&clip).to_json()))
            },
        ))
        .map_err(failed)?
        .clip_sanitiser(Sanitiser::new(SANITISER, Boundary::System, strip_lines))
        .map_err(failed)?;
        Ok(())
    }
}

/// Strips what leaves for other applications
/// (`helios_template::workspace::strip_for_outside`), a text of several
/// lines (a list of names) line by line: each line is a value of its
/// own, so a long list is not taken for embedded data.
pub fn strip_lines(data: &mut Json) -> Vec<String> {
    match data {
        Json::String(text) if text.contains('\n') => {
            let mut lines: Vec<Json> = text.lines().map(|l| Json::String(l.into())).collect();
            let mut out = Vec::new();
            for line in &mut lines {
                out.extend(helios_template::workspace::strip_for_outside(line));
            }
            *text = lines
                .iter()
                .filter_map(Json::as_str)
                .collect::<Vec<_>>()
                .join("\n");
            out
        }
        _ => helios_template::workspace::strip_for_outside(data),
    }
}

/// The endpoints, registered once the shell is assembled.
pub struct Endpoints {
    pub param: Rc<param::Endpoint>,
    pub graph: Rc<nodes::Endpoint>,
    pub masks: Rc<mask::Endpoint>,
    pub images: Rc<images::Endpoint>,
}

impl Endpoints {
    /// Registers the endpoints on the application.
    pub fn register(
        app: &AppHandle,
        values: Arc<Mutex<Store>>,
        viewport: Rc<RefCell<crate::viewport::Viewport>>,
        library: Rc<RefCell<crate::library::Library>>,
        system: system::Shared,
    ) -> Endpoints {
        let param = Rc::new(param::Endpoint::new(values.clone()));
        clipboard::register(
            app,
            PARAM_ENDPOINT,
            Some(param.clone()),
            Some(param.clone()),
        );
        let graph = Rc::new(nodes::Endpoint::new(values.clone()));
        clipboard::register(
            app,
            GRAPH_ENDPOINT,
            Some(graph.clone()),
            Some(graph.clone()),
        );
        let masks = Rc::new(mask::Endpoint::new(values, viewport));
        for name in [MASKS_ENDPOINT, MASK_IMAGE_ENDPOINT] {
            clipboard::register(app, name, Some(masks.clone()), Some(masks.clone()));
        }
        let images = Rc::new(images::Endpoint::new(library, system));
        clipboard::register(
            app,
            LIBRARY_ENDPOINT,
            Some(images.clone()),
            Some(images.clone()),
        );
        Endpoints {
            param,
            graph,
            masks,
            images,
        }
    }
}

/// The widget with the focus, when it lies inside `area` (a panel's
/// canvas whose parts take the focus themselves, a node of the graph, a
/// row of a tree): the endpoint is bound to it this pass, so the Edit
/// commands reach the endpoint whichever part has the focus.
///
/// The widget's rectangle is read from the passes' widget lists, never
/// through `Context::read_response`: building a response surrenders the
/// focus of a widget a press did not hover (a tree whose row was
/// clicked), which took the keys away from the Masks panel's tree.
pub fn focused_within(ctx: &egui::Context, area: egui::Rect) -> Option<egui::Id> {
    let id = ctx.memory(egui::Memory::focused)?;
    let rect = ctx.viewport(|v| {
        v.this_pass
            .widgets
            .get(id)
            .or_else(|| v.prev_pass.widgets.get(id))
            .map(|w| w.rect)
    })?;
    area.contains(rect.center()).then_some(id)
}

/// Runs a command at the next frame, as whom: a paste whose edit the
/// shell makes (the workspace's network, the image's masks). The paste
/// answered its plan already; a failure then shows as a warning.
pub fn defer(app: &AppHandle, command: &str, args: Json, origin: &PopOrigin) {
    let ctx = app.egui();
    crate::dialogs::request(
        &ctx,
        crate::dialogs::Request::Run(command.into(), args, crate::host::to_helios(origin)),
    );
    ctx.request_repaint();
}

/// An outcome as a result: a veto is a refusal with its reason.
pub fn result(out: Outcome) -> Result<Json, String> {
    match out {
        Outcome::Done(r) => r,
        Outcome::Vetoed { reason, .. } => Err(reason),
    }
}

/// Copies a parameter as `param.copyValue` and `param.copyPath` do: the
/// item is the parameter (pasted back as its value); its text is the
/// value as shown (`+0.30 EV`) or the path.
pub fn copy_param(
    app: &AppHandle,
    e: &param::Endpoint,
    path: &str,
    text: param::Text,
    origin: &PopOrigin,
) -> Result<Json, String> {
    e.pin(path, text);
    let out = clipboard::copy(app, CopyRequest::new(PARAM_ENDPOINT), origin);
    e.unpin();
    result(out)
}

/// Pastes onto a parameter as `param.paste` does: a copied parameter of
/// the same kind, or text read with the parameter's type.
pub fn paste_param(
    app: &AppHandle,
    e: &param::Endpoint,
    path: &str,
    dry_run: bool,
    origin: &PopOrigin,
) -> Result<Json, String> {
    e.pin(path, param::Text::Value);
    let mut req = PasteRequest::new(PARAM_ENDPOINT);
    if dry_run {
        req = req.dry_run();
    }
    let out = clipboard::paste(app, req, origin);
    e.unpin();
    result(out)
}

/// "Copy files": the files of the images the library acts on, put on
/// the system clipboard as a file manager copies them. Their paths go
/// out whole: the user asked for the files (a plain copy gives their
/// names only). Images without a file at hand are left out and said.
pub fn copy_files(e: &images::Endpoint, system: &system::Shared) -> Result<Json, String> {
    let clip = e.clip()?;
    let mut files = Vec::new();
    let mut missing = Vec::new();
    for image in clip.images {
        match image.path {
            Some(p) if p.exists() => files.push(p),
            _ => missing.push(image.name),
        }
    }
    if files.is_empty() {
        return Err("no file to copy: the images have none at hand".into());
    }
    system.borrow_mut().set_files(&files)?;
    Ok(serde_json::json!({ "files": files, "missing": missing }))
}

/// Pastes into the library what another application copied: files, or
/// an image (written as a PNG file first). It is read from the system
/// clipboard now, kept in the owner's [`system::SLOT`] (what the user
/// copied in Helios stays) and pasted from there through egui-pop's
/// step, as external content. The user's only: an agent never reads the
/// system clipboard.
pub fn paste_from_system(
    app: &AppHandle,
    system: &system::Shared,
    dry_run: bool,
    origin: &PopOrigin,
) -> Result<Json, String> {
    if *origin != PopOrigin::User {
        return Err("only the user pastes what other applications copied".into());
    }
    let item = system::external_item(&mut **system.borrow_mut(), &system::paste_dir())?;
    let slot = Slot::parse(system::SLOT);
    let answer = clipboard::with_owner(app, |c| c.put(&slot, item, origin))
        .ok_or("no clipboard: the ClipboardPlugin is not connected")?;
    if let Answer::Done(Err(e)) = answer {
        return Err(e);
    }
    let mut req = PasteRequest::new(LIBRARY_ENDPOINT).slot(slot);
    if dry_run {
        req = req.dry_run();
    }
    result(clipboard::paste(app, req, origin))
}

/// The image the viewport shows, rendered whole at full resolution and
/// made 8-bit sRGB as an 8-bit PNG export is: what "Copy the image" puts
/// on the system clipboard. Without the mask display laid over it.
pub fn rendered(
    engine: &helios_core::Engine,
    shown: &crate::engine_source::Shown,
) -> Result<image::RgbaImage, String> {
    let image = engine
        .render(&shown.graph, shown.node, &shown.port, shown.extent)
        .map_err(|e| e.to_string())?;
    let rgba = helios_export::srgb8_rgba(&image).map_err(|m| m.english)?;
    let r = image.region();
    image::RgbaImage::from_raw(r.width, r.height, rgba)
        .ok_or_else(|| "the rendered image has a wrong size".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_is_stripped_line_by_line() {
        // Forty names make a long text, but no line is embedded data.
        let names: Vec<String> = (0..40).map(|i| format!("IMG_{i:04}.CR3")).collect();
        let mut list = Json::String(names.join("\n"));
        assert!(strip_lines(&mut list).is_empty());
        assert_eq!(list, Json::String(names.join("\n")));
        // A local path on a line still becomes its file name.
        let mut list = Json::String("a.jpg\n/home/someone/b.jpg".into());
        assert_eq!(strip_lines(&mut list), ["a local path"]);
        assert_eq!(list, Json::String("a.jpg\nb.jpg".into()));
        // One line: the template's rules as they are.
        let mut one = Json::String("x".repeat(300));
        assert_eq!(strip_lines(&mut one), ["embedded data"]);
    }
}
