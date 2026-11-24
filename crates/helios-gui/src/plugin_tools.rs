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
//!   wheel turns one step. The plugin is told each committed change by
//!   `ui/changed` (the plugins' listener of `plugin_controls`), never a
//!   drag's frames.

use std::cell::RefCell;

use egui_pop::viewport::overlay::{Handle, Overlay, Shape};
use egui_pop::viewport::{Phase, Signal};
use helios_controls::render::overlay_kind;
use helios_core::{Params, Value};
use helios_rack::handle::{HandleKind, Params as _};
use helios_rack::plugin::{self, PluginTool};
use helios_rack::tools::{self, ParamsOf};
use helios_shell::commands::Origin;
use helios_values::Store;

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
    fn id(&self) -> String {
        format!("{OVERLAY}{}/{}", self.plugin, self.tool.tool.control)
    }

    /// The plugin's values, as the handles read them.
    fn params(&self, store: &Store) -> Params {
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

thread_local! {
    /// The enabled plugins' tools: the interface thread's, where the
    /// viewport is drawn.
    static TOOLS: RefCell<Vec<Declared>> = const { RefCell::new(Vec::new()) };
}

/// The enabled plugins' tools, from now on.
pub fn set(tools: Vec<Declared>) {
    TOOLS.with(|t| *t.borrow_mut() = tools);
}

/// The enabled plugins' tools.
pub fn declared() -> Vec<Declared> {
    TOOLS.with(|t| t.borrow().clone())
}

/// The plugins' tools on the viewport: what was drawn last frame, and the
/// gesture under way.
#[derive(Debug, Default)]
pub struct PluginTools {
    shown: Vec<Declared>,
    overlays: Vec<Overlay>,
    guides: Vec<Overlay>,
    drag: Option<tools::Drag>,
    wheel: Option<(tools::Turn, String, String)>,
}

impl PluginTools {
    /// Before the viewport is drawn: the tools shown over the open photo
    /// (none without one) and their handles where the values put them.
    pub fn prepare(&mut self, store: &Store, open: bool) {
        self.shown = if open {
            declared().into_iter().filter(|d| d.on(store)).collect()
        } else {
            Vec::new()
        };
        self.overlays.clear();
        self.guides.clear();
        for d in &self.shown {
            let params = d.params(store);
            let p = ParamsOf(&params);
            let handles = d
                .tool
                .tool
                .handles
                .iter()
                .enumerate()
                .map(|(i, h)| Handle {
                    index: i as u32,
                    kind: overlay_kind(h.handle.look),
                    at: h.position(&params),
                })
                .collect();
            let id = d.id();
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
        }
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

    fn find(&self, overlay: &str, index: u32) -> Option<(&Declared, String)> {
        let d = self.shown.iter().find(|d| d.id() == overlay)?;
        let h = d.tool.tool.handles.get(index as usize)?;
        Some((d, h.name().to_owned()))
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
                    let Some((d, name)) = self.find(overlay, *handle) else {
                        continue;
                    };
                    let steps = crate::controls::steps()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    tools::reset(
                        store,
                        crate::controls::registry(),
                        &steps,
                        &d.tool.tool,
                        &d.plugin,
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
                let Some((d, name)) = self.find(overlay, index) else {
                    return;
                };
                let start = d.params(store);
                self.drag =
                    tools::Drag::begin(store, &d.tool.tool, &d.plugin, &name, start, Origin::User);
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
        let Some((d, name)) = self.find(overlay, index) else {
            return;
        };
        let (d, params) = (d.clone(), d.params(store));
        let Some(spec) = d.tool.tool.handle(&name).cloned() else {
            return;
        };
        let same = matches!(&self.wheel, Some((_, o, h)) if o == overlay && *h == name);
        if !same && let Some((t, ..)) = self.wheel.take() {
            end_turn(t, store);
        }
        if self.wheel.is_none() {
            let Some(t) =
                tools::Turn::begin(store, &d.tool.tool, &d.plugin, &name, Origin::User, now)
            else {
                return;
            };
            self.wheel = Some((t, overlay.to_owned(), name));
        }
        if let Some((t, ..)) = &mut self.wheel {
            t.add(store, &spec, &d.plugin, &params, delta, now);
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
