//! Helios's places and modes on egui-pop's context stack (its ADR 0012):
//! where the user is (the library, the darkroom on an image, the
//! workspace's network) and how the window behaves (culling, mask
//! editing, compare), each with its arrangement or its panels, its keys,
//! a breadcrumb and mode chips in the status bar. The frames themselves
//! are `helios_template::contexts`'s, shared with the MCP server.
//!
//! The image's own states follow the stack ([`Follow`]): the value
//! store's before-and-after flag and mask tool push and pop their modes,
//! and a mode left from the stack (Escape, its chip, an agent) turns them
//! off.

use egui_pop::api::contexts::{self, Entry, Envelope, Request, State};
use egui_pop::api::zones::{self, Block};
use egui_pop::api::{AppHandle, Ctx, Plugin, PluginError};
use egui_pop::model::commands::Origin;
pub use egui_pop::model::ids::FrameId;
use egui_pop::model::ids::{BlockId, PluginId};
use helios_values::{Change, Set, Store};
use serde_json::Value as Json;

pub use helios_template::contexts::{
    COMPARE, COMPARING, CULLING, DARKROOM, LIBRARY, MASK_EDITING, ORIENTATION, Opened, TOOL,
    WORKSPACE,
};

pub const PLUGIN: PluginId = PluginId::from_static("helios.contexts");

/// The breadcrumb and the mode chips, in the status bar.
pub const BLOCK: BlockId = BlockId::from_static("helios.contexts.where");

/// Helios's frames, declared to the stack (the network guarded: only
/// the shown workspace's), and where the status bar says where the user
/// is.
pub struct Frames(pub crate::host::Workspace);

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
        for f in helios_template::contexts::frames() {
            let shown = self.0.clone();
            ctx.frame(helios_template::contexts::guard_network(f, move || {
                shown.get()
            }))?;
        }
        ctx.block(zones::STATUS_BAR, Block::add(BLOCK, 10, where_now));
        Ok(())
    }
}

/// The breadcrumb and the chips, once the user is somewhere else than
/// the workspace's root or a mode is on.
fn where_now(ui: &mut egui::Ui, app: &AppHandle) {
    let state = contexts::state(app).unwrap_or_default();
    if state.places.is_empty() && state.modes.is_empty() {
        return;
    }
    contexts::breadcrumb(ui, app);
    contexts::mode_chips(ui, app);
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
