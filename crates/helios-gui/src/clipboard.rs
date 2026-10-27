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
//!   the workspace's network ([`nodes`]);
//! - **a sanitiser** for what leaves for other applications: the
//!   Save-as-template rules (`helios_template::workspace::strip_for_outside`):
//!   a local path becomes its file name, secrets and embedded data go,
//!   and the copy's toast says so;
//! - **endpoints**, the focused component's source and target: a
//!   parameter's widget ([`PARAM_ENDPOINT`]), the graph view
//!   ([`GRAPH_ENDPOINT`]). A paste whose edit is the shell's (the
//!   workspace's network) runs as a command at the next frame
//!   ([`defer`]);
//! - **one undo step per paste**: egui-pop's history group is a group of
//!   Helios's value store (`host::HeliosHost::begin_group`).

pub mod nodes;
pub mod param;

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use egui_pop::api::clipboard::{
    self, Boundary, ClipType, CopyRequest, FormatId, PasteRequest, Repr, Sanitiser, TEXT,
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
        .clip_sanitiser(Sanitiser::new(SANITISER, Boundary::System, |data| {
            helios_template::workspace::strip_for_outside(data)
        }))
        .map_err(failed)?;
        Ok(())
    }
}

/// The endpoints, registered once the shell is assembled.
pub struct Endpoints {
    pub param: Rc<param::Endpoint>,
    pub graph: Rc<nodes::Endpoint>,
}

impl Endpoints {
    /// Registers the endpoints on the application.
    pub fn register(app: &AppHandle, values: Arc<Mutex<Store>>) -> Endpoints {
        let param = Rc::new(param::Endpoint::new(values.clone()));
        clipboard::register(
            app,
            PARAM_ENDPOINT,
            Some(param.clone()),
            Some(param.clone()),
        );
        let graph = Rc::new(nodes::Endpoint::new(values));
        clipboard::register(
            app,
            GRAPH_ENDPOINT,
            Some(graph.clone()),
            Some(graph.clone()),
        );
        Endpoints { param, graph }
    }
}

/// The widget with the focus, when it lies inside `area` (a panel's
/// canvas whose parts take the focus themselves, a node of the graph, a
/// row of a tree): the endpoint is bound to it this pass, so the Edit
/// commands reach the endpoint whichever part has the focus.
pub fn focused_within(ctx: &egui::Context, area: egui::Rect) -> Option<egui::Id> {
    let id = ctx.memory(egui::Memory::focused)?;
    let r = ctx.read_response(id)?;
    area.contains(r.rect.center()).then_some(id)
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
