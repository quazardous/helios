//! Helios's places and modes on egui-pop's context stack (its ADR 0012):
//! where the user is (the library, the darkroom on an image, the
//! workspace's network) and how the window behaves (culling, mask
//! editing, compare), each with its arrangement or its panels, its keys,
//! egui-pop's breadcrumb and mode chips at the start of the status bar
//! (Helios's own items following the place, `crate::status_items`), and
//! the session that comes back when the workspace opens again. The frames themselves are
//! `helios_template::contexts`'s, shared with the MCP server.
//!
//! egui-pop keeps the stack (`LocalContexts`) beside the workspace's
//! layout (`workspaces/<key>/context`), written inside each transition,
//! so a kill keeps the last one. At start the saved stack is checked
//! against today's frames and what still exists ([`stack`]): a darkroom
//! whose image is gone, or another workspace's network, is cut, and a
//! toast says so.
//!
//! The image's own states follow the stack ([`Follow`]): the value
//! store's before-and-after flag and mask tool push and pop their modes,
//! and a mode left from the stack (Escape, its chip, an agent) turns them
//! off.

use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::contexts::{self, ContextsPlugin, Entry, Envelope, Request, State};
use egui_pop::api::{AppHandle, Ctx, Plugin, PluginError};
use egui_pop::model::commands::Origin;
pub use egui_pop::model::ids::FrameId;
use egui_pop::model::ids::PluginId;
use helios_values::{Change, Set, Store};
use serde_json::Value as Json;

use crate::host::Workspace;
pub use helios_template::contexts::{
    COMPARE, COMPARING, CULLING, DARKROOM, LIBRARY, MASK_EDITING, ORIENTATION, Opened, TOOL,
    WORKSPACE,
};

pub const PLUGIN: PluginId = PluginId::from_static("helios.contexts");

/// The root crumb of the breadcrumb: the workspace the window shows,
/// named after the application (a name, the same in every language).
pub const ROOT: &str = "Helios";

/// Whether an image of the vault is still there, by id (a copy present).
pub type ImageCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// The check of the vault the desktop opens, given once it has one
/// (`ShellBuilder::images`).
pub type Images = Arc<Mutex<Option<ImageCheck>>>;

/// egui-pop's context stack, checking at start what the saved one names:
/// the workspace's network must be this window's, the darkroom's file or
/// image must still be there (`images`, when the desktop has a vault).
pub fn stack(workspace: Workspace, images: Images) -> ContextsPlugin {
    ContextsPlugin::default()
        .root(ROOT)
        .valid(move |frame, payload| {
            if *frame == WORKSPACE {
                return match (workspace.get(), payload.as_str()) {
                    (Some(k), Some(p)) if k == p => None,
                    _ => Some("it is another workspace's network".into()),
                };
            }
            if *frame == DARKROOM {
                return match Opened::of(payload) {
                    Some(Opened::File(p)) if p.exists() => None,
                    Some(Opened::File(p)) => Some(format!("{} is gone", p.display())),
                    Some(Opened::Image(id)) => {
                        let check = images
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .clone();
                        match check {
                            Some(there) if !there(&id) => Some(format!("image {id} is gone")),
                            _ => None,
                        }
                    }
                    None => Some("it names no image".into()),
                };
            }
            None
        })
}

/// Helios's frames, declared to the stack (the network guarded: only
/// the shown workspace's), and Helios's status bar items, each shown
/// where it means something.
pub struct Frames(pub Workspace);

impl Plugin for Frames {
    fn id(&self) -> PluginId {
        PLUGIN
    }

    fn title(&self) -> String {
        "Helios places and modes".into()
    }

    fn requires(&self) -> Vec<PluginId> {
        vec![contexts::PLUGIN]
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        let frames = helios_template::contexts::frames();
        for item in crate::status_items::indicators(&frames) {
            ctx.indicator(item);
        }
        for f in frames {
            let shown = self.0.clone();
            ctx.frame(helios_template::contexts::guard_network(f, move || {
                shown.get()
            }))?;
        }
        Ok(())
    }
}

/// The stack shown.
pub fn state(app: &AppHandle) -> State {
    contexts::state(app).unwrap_or_default()
}

/// What the darkroom shows, if the user is in it.
pub fn darkroom(app: &AppHandle) -> Option<Opened> {
    state(app)
        .places
        .iter()
        .rev()
        .find(|e| e.frame == DARKROOM)
        .and_then(|e| Opened::of(&e.payload))
}

/// An image opened: the darkroom on it, under the place the user was in
/// (the library, the root), replacing the darkroom of another image.
pub fn open(app: &AppHandle, opened: &Opened, origin: &Origin) {
    let state = state(app);
    let darkroom = Entry {
        frame: DARKROOM,
        payload: opened.payload(),
    };
    if state.places.last() == Some(&darkroom) {
        return;
    }
    let mut places: Vec<Entry> = state
        .places
        .iter()
        .take_while(|e| e.frame != DARKROOM)
        .cloned()
        .collect();
    places.push(darkroom);
    contexts::request(app, Envelope::new(Request::Go { places }), origin);
}

/// Leaves the darkroom (its image could not be opened): back where the
/// user came from.
pub fn leave_darkroom(app: &AppHandle, origin: &Origin) {
    if let Some(depth) = state(app).places.iter().position(|e| e.frame == DARKROOM) {
        contexts::request(app, Envelope::new(Request::Jump { depth }), origin);
    }
}

/// Goes to a place at the top of the workspace (the library, its
/// network).
pub fn go(app: &AppHandle, frame: FrameId, payload: Json, origin: &Origin) -> Result<Json, String> {
    let places = vec![Entry { frame, payload }];
    match contexts::request(app, Envelope::new(Request::Go { places }), origin) {
        egui_pop::api::steps::Outcome::Done(r) => r,
        egui_pop::api::steps::Outcome::Vetoed { by, reason } => {
            Err(format!("vetoed by {by}: {reason}"))
        }
    }
}

/// An agent's `context.*` tool (the node's MCP tools, the same `cmd` and
/// arguments as on the workspace's files), answered by this desktop: the
/// window moves at once, through the same `context.request` step as the
/// user's moves, and the answer has the same fields as on the files.
pub fn agent_tool(app: &AppHandle, request: &Json, agent: &str) -> Result<Json, String> {
    use helios_template::contexts::tools;
    let cmd = request["cmd"].as_str().unwrap_or_default();
    if cmd == "context.read" {
        let state = state(app);
        return Ok(serde_json::json!({
            "rev": state.rev,
            "place": state.place(),
            "state": state,
            "frames": helios_template::contexts::frames(),
        }));
    }
    let env = tools::envelope_of(cmd, request)?;
    let frames = app
        .service(&contexts::CONTEXTS)
        .ok_or("this desktop keeps no context stack")?
        .frames();
    // What the machine says it leads to, or why it is refused, before
    // the owner is asked (the owner's holds may refuse it still).
    let t = egui_pop::model::context::apply(&state(app), &frames, &[], &env)
        .map_err(|r| format!("refused ({:?}, by {}): {}", r.kind, r.by, r.reason))?;
    if env.dry_run {
        return Ok(tools::answer(&t, true));
    }
    match contexts::request(app, env, &Origin::Agent(agent.into())) {
        egui_pop::api::steps::Outcome::Done(Ok(_)) => {}
        egui_pop::api::steps::Outcome::Done(Err(e)) => return Err(format!("refused: {e}")),
        egui_pop::api::steps::Outcome::Vetoed { by, reason } => {
            return Err(format!("vetoed by {by}: {reason}"));
        }
    }
    let mut answer = tools::answer(&t, false);
    let now = state(app);
    answer["rev"] = now.rev.into();
    answer["state"] = serde_json::to_value(&now).map_err(|e| e.to_string())?;
    Ok(answer)
}

/// Culling on (`Some(true)`), off, or toggled.
pub fn culling(app: &AppHandle, on: Option<bool>, origin: &Origin) -> Result<Json, String> {
    let now = state(app).mode_of(ORIENTATION.as_str()) == Some(&CULLING);
    let request = match on.unwrap_or(!now) {
        true if now => return Ok(Json::Null),
        true => Request::set(CULLING),
        false if !now => return Ok(Json::Null),
        false => Request::ClearMode { group: ORIENTATION },
    };
    match contexts::request(app, Envelope::new(request), origin) {
        egui_pop::api::steps::Outcome::Done(r) => r,
        egui_pop::api::steps::Outcome::Vetoed { by, reason } => {
            Err(format!("vetoed by {by}: {reason}"))
        }
    }
}

/// Keeps the value store's place on the stack's place shown: the steps
/// made from now on are made there (`helios_values::Step::place`), which
/// egui-pop's undo toast reads to say "Undone in <place>" with Go there.
/// Called each frame, once the interface's logic ran.
pub fn follow_place(app: &AppHandle, values: &mut Store) {
    let Some(now) = contexts::state(app) else {
        return;
    };
    let place: Vec<helios_values::Place> = now
        .places
        .into_iter()
        .map(|e| helios_values::Place {
            frame: e.frame.as_str().to_string(),
            payload: e.payload,
        })
        .collect();
    if values.place() != place.as_slice() {
        values.set_place(place);
    }
}

/// A state of the image kept in the value store and on the stack as a
/// mode: what each was at the last frame.
#[derive(Clone, Copy, Debug, Default)]
struct Pair {
    last: Option<(bool, bool)>,
}

/// Which side changed since the last frame, when they differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lead {
    /// The value changed (the viewport, a command): the mode follows.
    Value,
    /// The stack changed (Escape, a chip, an agent, a restore): the
    /// value follows.
    Stack,
}

impl Pair {
    fn lead(self, value: bool, mode: bool) -> Option<Lead> {
        if value == mode {
            return None;
        }
        Some(match self.last {
            Some((v, _)) if v != value => Lead::Value,
            _ => Lead::Stack,
        })
    }
}

/// Keeps the image's own states and their modes together, each frame:
/// the before-and-after flag ([`COMPARE`], pushed) and the mask tool
/// drawing a path ([`MASK_EDITING`], pushed, never restored).
#[derive(Debug, Default)]
pub struct Follow {
    compare: Pair,
    mask: Pair,
}

impl Follow {
    pub fn frame(&mut self, app: &AppHandle, values: &mut Store) {
        if app.service(&contexts::CONTEXTS).is_none() {
            return;
        }
        let user = Origin::User;
        // Compare.
        let value = values
            .get(crate::viewport::COMPARE)
            .and_then(|e| e.value.as_bool())
            .unwrap_or(false);
        let mode = state(app).mode_of(COMPARING.as_str()) == Some(&COMPARE);
        match self.compare.lead(value, mode) {
            Some(Lead::Value) => toggle_mode(app, COMPARE, COMPARING, value, &user),
            Some(Lead::Stack) => write(
                values,
                crate::viewport::COMPARE,
                helios_core::Value::Bool(mode),
            ),
            None => {}
        }
        self.compare.last = Some((value_of_compare(values), compare_on(app)));
        // Drawing a mask's path.
        let value = crate::masks::tool(values) != "none";
        let mode = state(app).mode_of(TOOL.as_str()) == Some(&MASK_EDITING);
        match self.mask.lead(value, mode) {
            Some(Lead::Value) => toggle_mode(app, MASK_EDITING, TOOL, value, &user),
            Some(Lead::Stack) => {
                let tool = if mode { "path" } else { "none" };
                write(
                    values,
                    crate::masks::TOOL,
                    helios_core::Value::Choice(tool.into()),
                );
                write(
                    values,
                    crate::masks::DRAFT,
                    helios_core::Value::List(Vec::new()),
                );
            }
            None => {}
        }
        self.mask.last = Some((
            crate::masks::tool(values) != "none",
            state(app).mode_of(TOOL.as_str()) == Some(&MASK_EDITING),
        ));
    }
}

fn value_of_compare(values: &Store) -> bool {
    values
        .get(crate::viewport::COMPARE)
        .and_then(|e| e.value.as_bool())
        .unwrap_or(false)
}

fn compare_on(app: &AppHandle) -> bool {
    state(app).mode_of(COMPARING.as_str()) == Some(&COMPARE)
}

/// Pushes a mode, or turns its group off.
fn toggle_mode(
    app: &AppHandle,
    mode: FrameId,
    group: egui_pop::model::ids::GroupId,
    on: bool,
    origin: &Origin,
) {
    let request = if on {
        Request::push(mode)
    } else {
        Request::ClearMode { group }
    };
    contexts::request(app, Envelope::new(request), origin);
}

fn write(values: &mut Store, path: &str, value: helios_core::Value) {
    let _ = values.set(Set::new(
        path,
        Change::To(value),
        helios_shell::commands::Origin::User,
    ));
}
