//! A plugin's tools on the image (`[ui] rack`, its `tools`, the `ui`
//! interface): handles over the photo on the plugin's own values.
//!
//! - **Declared**, not drawn by the plugin: each tool is a
//!   `helios_rack::plugin::PluginTool`, its handles `egui_hud` handle
//!   kinds read from the plugin's declaration (`egui_hud-handles/1`).
//!   [`plugin_controls::install`](crate::plugin_controls::install) hands
//!   the enabled plugins' tools here ([`set`]).
//! - **Shown** over the open photo by the viewport, as egui-pop's
//!   overlays: each handle in its look, a radius as a circle, a feather
//!   dashed past it. A tool with a `when` switch shows while that value
//!   is on.
//! - **Edited** as the masks' tools are (`helios_rack::tools`): a drag is
//!   one store gesture, one undo step in the plugins' history scope when
//!   it ends, nothing when Escape gives it up; a double click resets, the
//!   wheel turns one step; Shift snaps as the handle declares
//!   ([`crate::handle_snap`], the masks' rules too). The plugin is told
//!   each committed change by `ui/changed` (the plugins' listener of
//!   `plugin_controls`), never a drag's frames.
//! - **On the host's parameters**: a tool that names a `module`
//!   (`mask_circle`) is drawn on each node of that module in the photo's
//!   graph and moves those nodes' parameters (`node/<node>/<param>`),
//!   each gesture one undo step on the photo's own line. It shows only
//!   while the plugin holds the `edit.controls` capability, declared and
//!   granted (checked each frame, so a grant or a revoke shows at once);
//!   until then it is refused, its reason on the plugin's Settings
//!   section and in the log ([`refusal`]).

use std::cell::RefCell;

use egui_pop::viewport::overlay::{Handle, Overlay, Shape};
use egui_pop::viewport::{Phase, Signal};
use helios_controls::render::overlay_kind;
use std::sync::Arc;

use helios_core::{Graph, Params, Value};
use helios_plugin::Broker;
use helios_plugin::manifest::EDIT_CONTROLS;
use helios_rack::handle::{HandleKind, Params as _};
use helios_rack::plugin::{self, PluginTool};
use helios_rack::tools::{self, ParamsOf};
use helios_shell::commands::Origin;
use helios_values::{Store, node_path};

use crate::handle_snap::Snaps;

/// The prefix of a plugin tool's overlay id: `plugin-tool:<id>/<tool>`.
pub const OVERLAY: &str = "plugin-tool:";

/// A plugin's tool, with the keys of the plugin's values it reads.
#[derive(Clone, Debug)]
pub struct Declared {
    pub plugin: String,
    pub keys: Vec<String>,
    pub tool: PluginTool,
}

impl Declared {
    /// The plugin's values, as the handles read them.
    fn own(&self, store: &Store) -> Params {
        self.keys.iter().fold(Params::new(), |p, k| {
            match store.get(&plugin::path(&self.plugin, k)) {
                Some(e) => p.with(k, e.value.clone()),
                None => p,
            }
        })
    }

    /// Whether it shows: its switch on, or it has none.
    fn on(&self, store: &Store) -> bool {
        self.tool.when.as_ref().is_none_or(|w| {
            store
                .get(&plugin::path(&self.plugin, w))
                .is_some_and(|e| e.value == Value::Bool(true))
        })
    }
}

/// Why a plugin's tool is not shown on the host's parameters, if it
/// edits them and the plugin does not hold `edit.controls`: it does
/// not declare it, or the user has not granted it.
pub fn refusal(broker: &Broker, plugin: &str, tool: &PluginTool) -> Option<String> {
    refusal_reason(broker, plugin, tool).map(|why| format!("tool `{}` {why}", tool.tool.control))
}

/// [`refusal`]'s reason alone, without the tool's name.
pub fn refusal_reason(broker: &Broker, plugin: &str, tool: &PluginTool) -> Option<String> {
    let module = tool.host()?;
    broker
        .allowed(plugin, EDIT_CONTROLS)
        .err()
        .map(|e| format!("on `{module}`: {e}, not shown"))
}

thread_local! {
    /// The enabled plugins' tools: the interface thread's, where the
    /// viewport is drawn.
    static TOOLS: RefCell<Vec<Declared>> = const { RefCell::new(Vec::new()) };
    /// The broker their capabilities are asked of.
    static BROKER: RefCell<Option<Arc<Broker>>> = const { RefCell::new(None) };
}

/// The enabled plugins' tools, from now on, their capabilities asked of
/// `broker`.
pub fn set(tools: Vec<Declared>, broker: Option<Arc<Broker>>) {
    TOOLS.with(|t| *t.borrow_mut() = tools);
    BROKER.with(|b| *b.borrow_mut() = broker);
}

/// Whether a tool may show: one on the plugin's own values always, one
/// on the host's parameters while the plugin holds `edit.controls`.
fn allowed(d: &Declared) -> bool {
    d.tool.host().is_none()
        || BROKER.with_borrow(|b| {
            b.as_ref()
                .is_some_and(|b| refusal(b, &d.plugin, &d.tool).is_none())
        })
}

/// A tool shown this frame: on the plugin's values, or on one node of
/// the photo's graph.
#[derive(Clone, Debug)]
struct Shown {
    decl: Declared,
    /// What its paths are under: the plugin's id, or the node's name.
    node: String,
}

impl Shown {
    fn id(&self) -> String {
        let d = &self.decl;
        match d.tool.host() {
            None => format!("{OVERLAY}{}/{}", d.plugin, d.tool.tool.control),
            Some(_) => format!(
                "{OVERLAY}{}/{}/{}",
                d.plugin, d.tool.tool.control, self.node
            ),
        }
    }

    /// The values its handles read: the plugin's, or the node's
    /// parameters as the store holds them.
    fn params(&self, store: &Store, graph: Option<&Graph>) -> Params {
        if self.decl.tool.host().is_none() {
            return self.decl.own(store);
        }
        match graph.and_then(|g| Some((g, g.find(&self.node)?))) {
            Some((g, id)) => crate::masks::params_of(g, store, id),
            None => Params::new(),
        }
    }
}

/// The tools to show over `graph`: each of the plugins' own once, each
/// on the host's parameters on every node of its module whose
/// parameters its handles move are in the store.
fn shown(tools: Vec<Declared>, store: &Store, graph: Option<&Graph>) -> Vec<Shown> {
    let mut out = Vec::new();
    for d in tools {
        match d.tool.host() {
            None => out.push(Shown {
                node: d.plugin.clone(),
                decl: d,
            }),
            Some(module) => {
                let Some(g) = graph else { continue };
                let writes: Vec<String> = d
                    .tool
                    .tool
                    .handles
                    .iter()
                    .flat_map(tools::ToolHandle::writes)
                    .collect();
                for id in g.ids() {
                    let of = g
                        .manifest(id)
                        .ok()
                        .flatten()
                        .is_some_and(|m| m.name == module);
                    let Ok(node) = g.name(id) else { continue };
                    let has = writes.iter().all(|p| {
                        store
                            .get(&node_path(node, p))
                            .is_some_and(|e| e.value.as_f64().is_some())
                    });
                    if of && has {
                        out.push(Shown {
                            node: node.to_owned(),
                            decl: d.clone(),
                        });
                    }
                }
            }
        }
    }
    out
}

/// The enabled plugins' tools.
pub fn declared() -> Vec<Declared> {
    TOOLS.with(|t| t.borrow().clone())
}

/// The plugins' tools on the viewport: what was drawn last frame, and the
/// gesture under way.
#[derive(Debug, Default)]
pub struct PluginTools {
    shown: Vec<Shown>,
    /// The values each tool shown read last frame, by overlay id.
    params: Vec<Params>,
    overlays: Vec<Overlay>,
    guides: Vec<Overlay>,
    drag: Option<tools::Drag>,
    wheel: Option<(tools::Turn, String, String)>,
}

impl PluginTools {
    /// Before the viewport is drawn: the tools shown over the open photo
    /// (`graph`, none without one) and their handles where the values put
    /// them, each handle's Shift rule said to `snaps`.
    pub fn prepare(&mut self, store: &Store, graph: Option<&Graph>, snaps: &Snaps) {
        self.shown = if graph.is_some() {
            let on = declared()
                .into_iter()
                .filter(|d| d.on(store) && allowed(d))
                .collect();
            shown(on, store, graph)
        } else {
            Vec::new()
        };
        self.overlays.clear();
        self.guides.clear();
        self.params.clear();
        for s in &self.shown {
            let d = &s.decl;
            let params = s.params(store, graph);
            let p = ParamsOf(&params);
            let id = s.id();
            let handles = d
                .tool
                .tool
                .handles
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    snaps.insert(&id, i as u32, h.handle.snap, h.handle.kind.pivot(&p));
                    Handle {
                        index: i as u32,
                        kind: overlay_kind(h.handle.look),
                        at: h.position(&params),
                    }
                })
                .collect();
            for h in &d.tool.tool.handles {
                let circle = |c: [f64; 2], r: f64| Shape::Circle {
                    center: c,
                    radius: r.max(0.0),
                };
                match &h.handle.kind {
                    HandleKind::Radius {
                        centre,
                        radius,
                        angle: None,
                        ..
                    } => self.guides.push(Overlay::guide(
                        &id,
                        circle(centre.at(&p), p.float(radius).unwrap_or(0.0)),
                    )),
                    HandleKind::Feather {
                        centre,
                        radius,
                        feather,
                        angle: None,
                        proportional: None,
                    } => self.guides.push(
                        Overlay::guide(
                            format!("{id}/feather"),
                            circle(
                                centre.at(&p),
                                p.float(radius).unwrap_or(0.0) + p.float(feather).unwrap_or(0.0),
                            ),
                        )
                        .dashed(),
                    ),
                    _ => {}
                }
            }
            self.overlays.push(
                Overlay::new(
                    id,
                    Shape::Path {
                        points: Vec::new(),
                        closed: false,
                    },
                )
                .handles(handles)
                .selected(),
            );
            self.params.push(params);
        }
    }

    /// The handles drawn last frame, each an edit area of the image: its
    /// id (`plugin.handle.<plugin>.<tool>[.<node>].<handle>`), where it
    /// is, and the path of the value it adjusts.
    pub fn handle_areas(&self) -> Vec<crate::masks::HandleArea> {
        let mut out = Vec::new();
        for (s, o) in self.shown.iter().zip(&self.overlays) {
            let d = &s.decl;
            for (h, k) in d.tool.tool.handles.iter().zip(&o.handles) {
                let Some(p) = h.writes().into_iter().next() else {
                    continue;
                };
                let node = match d.tool.host() {
                    None => String::new(),
                    Some(_) => format!(".{}", s.node),
                };
                out.push(crate::masks::HandleArea {
                    at: k.at,
                    id: format!(
                        "plugin.handle.{}.{}{node}.{}",
                        d.plugin,
                        d.tool.tool.control,
                        h.name()
                    ),
                    path: d.tool.tool.target.path(&s.node, &p),
                });
            }
        }
        out
    }

    /// What the viewport draws: the guides, then the handles above.
    pub fn drawn(&self) -> Vec<Overlay> {
        self.guides.iter().chain(&self.overlays).cloned().collect()
    }

    /// The tools' overlays drawn last frame, one per tool shown.
    pub fn overlays(&self) -> &[Overlay] {
        &self.overlays
    }

    /// Whether a drag is under way.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn find(&self, overlay: &str, index: u32) -> Option<(usize, String)> {
        let i = self.shown.iter().position(|s| s.id() == overlay)?;
        let h = self.shown[i].decl.tool.tool.handles.get(index as usize)?;
        Some((i, h.name().to_owned()))
    }

    /// After the viewport is drawn: applies its signals on the tools'
    /// handles to the store. `now` is the frame's time in seconds.
    pub fn signals(&mut self, signals: &[Signal], store: &mut Store, now: f64) {
        for signal in signals {
            match signal {
                Signal::Handle {
                    overlay,
                    handle,
                    phase,
                    at,
                    ..
                } if overlay.starts_with(OVERLAY) => {
                    self.on_handle(overlay, *handle, *phase, *at, store);
                }
                Signal::HandleReset {
                    overlay, handle, ..
                } if overlay.starts_with(OVERLAY) => {
                    let Some((i, name)) = self.find(overlay, *handle) else {
                        continue;
                    };
                    let s = &self.shown[i];
                    let steps = crate::controls::steps()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    tools::reset(
                        store,
                        crate::controls::registry(),
                        &steps,
                        &s.decl.tool.tool,
                        &s.node,
                        &name,
                        Origin::User,
                    );
                }
                Signal::HandleWheel {
                    overlay,
                    handle,
                    delta,
                    ..
                } if overlay.starts_with(OVERLAY) => {
                    self.on_wheel(overlay, *handle, f64::from(*delta), store, now);
                }
                _ => {}
            }
        }
        if self
            .wheel
            .as_ref()
            .is_some_and(|(t, ..)| now - t.last > 0.5)
            && let Some((t, ..)) = self.wheel.take()
        {
            end_turn(t, store);
        }
    }

    fn on_handle(
        &mut self,
        overlay: &str,
        index: u32,
        phase: Phase,
        at: [f64; 2],
        store: &mut Store,
    ) {
        match phase {
            Phase::Begin => {
                if let Some(d) = self.drag.take() {
                    commit(d, store);
                }
                let Some((i, name)) = self.find(overlay, index) else {
                    return;
                };
                let s = &self.shown[i];
                let start = self.params[i].clone();
                self.drag = tools::Drag::begin(
                    store,
                    &s.decl.tool.tool,
                    &s.node,
                    &name,
                    start,
                    Origin::User,
                );
            }
            Phase::Update | Phase::End => {
                if let Some(d) = &self.drag
                    && let Err(e) = d.update(store, at)
                {
                    log::debug!("plugin tool: {e}");
                }
                if phase == Phase::End
                    && let Some(d) = self.drag.take()
                {
                    commit(d, store);
                }
            }
            Phase::Cancel => {
                if let Some(d) = self.drag.take() {
                    d.cancel(store);
                }
            }
            Phase::Once => {}
        }
    }

    fn on_wheel(&mut self, overlay: &str, index: u32, delta: f64, store: &mut Store, now: f64) {
        let Some((i, name)) = self.find(overlay, index) else {
            return;
        };
        let (s, params) = (self.shown[i].clone(), self.params[i].clone());
        let Some(spec) = s.decl.tool.tool.handle(&name).cloned() else {
            return;
        };
        let same = matches!(&self.wheel, Some((_, o, h)) if o == overlay && *h == name);
        if !same && let Some((t, ..)) = self.wheel.take() {
            end_turn(t, store);
        }
        if self.wheel.is_none() {
            let Some(t) =
                tools::Turn::begin(store, &s.decl.tool.tool, &s.node, &name, Origin::User, now)
            else {
                return;
            };
            self.wheel = Some((t, overlay.to_owned(), name));
        }
        if let Some((t, ..)) = &mut self.wheel {
            t.add(store, &spec, &s.node, &params, delta, now);
        }
    }
}

fn commit(d: tools::Drag, store: &mut Store) {
    let steps = crate::controls::steps()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    d.commit(store, crate::controls::registry(), &steps);
}

fn end_turn(t: tools::Turn, store: &mut Store) {
    let steps = crate::controls::steps()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    t.end(store, crate::controls::registry(), &steps);
}
