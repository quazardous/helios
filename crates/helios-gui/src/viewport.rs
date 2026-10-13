//! The image viewport: egui-pop's generic viewport (`egui_pop::viewport`)
//! fed by Helios's engine.
//!
//! egui-pop draws: pan and zoom in physical pixels (100 % is one image
//! pixel on one screen pixel), the zoom ladder, a proxy of the whole image
//! then the exact tiles of what is visible, the GPU path, before and
//! after, the pixel readout. Helios supplies the pixels and the colour:
//!
//! - **The pixels** come from [`EngineSource`]s: the edit ("after") and
//!   the edit as it was opened ("before"), sharing one engine and its
//!   cache. An edit (a value of the store moved) shows the new graph and
//!   the viewport renders again.
//! - **The display transform** is a matrix from the working space (linear
//!   Rec.2020) to the display's primaries (sRGB for now), clipped and
//!   encoded by egui-pop: in its shader on the GPU path.
//! - **The view** (zoom, centre, fit) and the comparison live in the value
//!   store at `view/viewport/*`, so keys, the palette and agents move them
//!   like any value. The viewport's View signals drive them: a drag is one
//!   gesture holding them against other origins, and none of it is an
//!   undoable edit (the values are transient).
//! - **Drawn masks**: the selected mask's shapes are outlined over the
//!   image and the selected shape's handles edit it (see
//!   [`crate::masks`]).
//! - **Pictures**: `viewport.display` is what is on screen;
//!   `viewport.scene` is the float data of the view before the display
//!   transform.

use std::sync::{Arc, Mutex, PoisonError};

use egui::Ui;
use egui_pop::viewport::display::{DisplayTarget, DisplayTransform, Matrix};
use egui_pop::viewport::source::{Readout, TileSource};
use egui_pop::viewport::tiles::{exact_scale, scaled_size};
use egui_pop::viewport::view::{MAX_ZOOM, MIN_ZOOM};
use egui_pop::viewport::{Compare, Phase, Signal, View, ViewportState, Zoom};
use helios_core::{Bus, Engine, Graph, Image, NodeId, Region, Value};
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, GestureId, Set, Store};

use crate::engine_source::{EngineSource, Shown, Stats};

/// The value paths of the view.
pub const ZOOM: &str = "view/viewport/zoom";
pub const CENTER_X: &str = "view/viewport/center_x";
pub const CENTER_Y: &str = "view/viewport/center_y";
pub const FIT: &str = "view/viewport/fit";
/// Before and after side by side.
pub const COMPARE: &str = "view/viewport/compare";
/// Where they meet, as a fraction of the view's width ("before" left).
pub const SPLIT: &str = "view/viewport/split";

/// Declares the view's values in a store.
pub fn declare(store: &mut Store) {
    // Physical screen pixels per image pixel: 100 % is one image pixel on
    // one pixel of the screen, whatever the window's scale.
    let mut zoom = Descriptor::float("zoom", MIN_ZOOM, MAX_ZOOM, 1.0);
    zoom.unit = Some("%".into());
    zoom.display_scale = 100.0;
    zoom.precision = 1;
    // Where one looks is not an edit: never undone, never saved.
    store.declare(ZOOM, zoom.transient());
    store.declare(
        CENTER_X,
        Descriptor::float("centre x", -1e6, 1e6, 0.0).transient(),
    );
    store.declare(
        CENTER_Y,
        Descriptor::float("centre y", -1e6, 1e6, 0.0).transient(),
    );
    store.declare(FIT, Descriptor::bool("fit to the view", true).transient());
    store.declare(
        COMPARE,
        Descriptor::bool("before and after", false).transient(),
    );
    store.declare(
        SPLIT,
        Descriptor::float("before and after split", 0.0, 1.0, 0.5).transient(),
    );
}

/// Working space (linear Rec.2020) to the display's primaries (sRGB for
/// now; a monitor profile later), by rows.
pub fn display_matrix() -> [[f32; 3]; 3] {
    use helios_photo::color;
    let m = color::mul(
        &color::inverse(&color::SRGB_TO_XYZ).expect("invertible primaries"),
        &color::REC2020_TO_XYZ,
    );
    m.map(|row| [row[0] as f32, row[1] as f32, row[2] as f32])
}

/// Helios's display transform: the working space to the display's
/// primaries, then egui-pop's clamp and encoding for the window's target.
pub fn display_transform() -> Matrix {
    Matrix(display_matrix())
}

/// The display transform on the CPU, for an sRGB display: for tests and
/// for pictures of the scene.
pub fn display(rgb: [f32; 3]) -> [u8; 3] {
    display_transform()
        .stage(&DisplayTarget::srgb())
        .encode_8bit(rgb)
}

fn float(store: &Store, path: &str) -> f64 {
    store
        .get(path)
        .and_then(|e| e.value.as_f64())
        .unwrap_or(0.0)
}

fn flag(store: &Store, path: &str, default: bool) -> bool {
    store
        .get(path)
        .and_then(|e| e.value.as_bool())
        .unwrap_or(default)
}

/// The view the store holds.
pub fn view_of(store: &Store) -> View {
    if flag(store, FIT, true) {
        View::fit()
    } else {
        View::at(
            float(store, ZOOM),
            [float(store, CENTER_X), float(store, CENTER_Y)],
        )
    }
}

/// The comparison the store holds.
pub fn compare_of(store: &Store) -> Compare {
    if flag(store, COMPARE, false) {
        Compare::Split(float(store, SPLIT) as f32)
    } else {
        Compare::Off
    }
}

fn write(store: &mut Store, path: &str, value: Value, gesture: Option<GestureId>) {
    let mut set = Set::new(path, Change::To(value), Origin::User);
    set.gesture = gesture;
    // Refused only while another origin's gesture holds it: that one wins.
    let _ = store.set(set);
}

/// Writes a view into the store: fit, or a zoom and a centre.
pub fn write_view(store: &mut Store, view: &View, gesture: Option<GestureId>) {
    match view.zoom {
        Zoom::Fit => write(store, FIT, Value::Bool(true), gesture),
        Zoom::Px(z) => {
            write(store, ZOOM, Value::Float(z), gesture);
            write(store, CENTER_X, Value::Float(view.center[0]), gesture);
            write(store, CENTER_Y, Value::Float(view.center[1]), gesture);
            write(store, FIT, Value::Bool(false), gesture);
        }
    }
}

/// What the zoom commands ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoomTo {
    Fit,
    /// 100 %: one image pixel per physical screen pixel.
    Actual,
    /// One step up the zoom ladder.
    In,
    /// One step down.
    Out,
}

/// The graph shown, kept in step with the value store.
struct Edited {
    graph: Graph,
    node: NodeId,
    port: String,
}

/// The viewport panel: egui-pop's widget and its state, the sources of
/// "after" (the edit) and "before" (the edit as it was opened).
pub struct Viewport {
    engine: Arc<Engine>,
    state: ViewportState,
    after: EngineSource,
    before: EngineSource,
    edited: Option<Edited>,
    /// The drag under way, a gesture of the store.
    gesture: Option<GestureId>,
    /// The values under the pointer, once they arrived.
    readout: Option<Readout>,
    /// Editing the drawn masks: drags, hooks, what was drawn.
    masks: crate::masks::Masks,
    pub stats: Arc<Stats>,
}

impl Default for Viewport {
    fn default() -> Self {
        let budget = helios_core::engine::memory_share(
            helios_core::engine::DEFAULT_CACHE_SHARE,
            helios_core::engine::physical_memory(),
        );
        let engine = Arc::new(Engine::with_budget(Arc::new(Bus::new()), budget));
        let mut state = ViewportState::new();
        state.set_display_transform(Arc::new(display_transform()));
        let stats = Arc::<Stats>::default();
        let after = EngineSource::new(
            "helios-viewport",
            engine.clone(),
            state.sink(),
            stats.clone(),
        );
        let before = EngineSource::new(
            "helios-viewport-before",
            engine.clone(),
            state.before_sink(),
            stats.clone(),
        );
        Viewport {
            engine,
            state,
            after,
            before,
            edited: None,
            gesture: None,
            readout: None,
            masks: crate::masks::Masks::default(),
            stats,
        }
    }
}

impl Viewport {
    /// Shows a node's output; the store mirrors its parameters
    /// (`helios_values::bind_node`) and [`Viewport::sync`] follows them.
    /// The output as it is now is the "before" of a comparison.
    pub fn set_source(&mut self, graph: Graph, node: NodeId, port: &str) -> Result<(), String> {
        let shown = Shown::new(Arc::new(graph.clone()), node, port)?
            .ok_or("the node has no source feeding it")?;
        self.after.show(Some(shown.clone()));
        self.before.show(Some(shown));
        self.edited = Some(Edited {
            graph,
            node,
            port: port.into(),
        });
        Ok(())
    }

    /// Shows the graph as it is now (after a node was added).
    fn reshow(&mut self) {
        let Some(e) = &self.edited else { return };
        match Shown::new(Arc::new(e.graph.clone()), e.node, &e.port) {
            Ok(shown) => self.after.show(shown),
            Err(e) => eprintln!("helios-gui: viewport: {e}"),
        }
    }

    /// The mask editing: what was drawn last frame.
    pub fn masks(&self) -> &crate::masks::Masks {
        &self.masks
    }

    /// The part of the image in view, in image pixels (the whole image
    /// before the first frame).
    fn visible(&self) -> Option<[f64; 4]> {
        let [w, h] = self.after.size()?;
        let (w, h) = (f64::from(w), f64::from(h));
        let v = self
            .state
            .mapping()
            .and_then(|m| m.visible())
            .unwrap_or([0.0, 0.0, w, h]);
        let v = [v[0].max(0.0), v[1].max(0.0), v[2].min(w), v[3].min(h)];
        Some(if v[2] > v[0] && v[3] > v[1] {
            v
        } else {
            [0.0, 0.0, w, h]
        })
    }

    /// Adds a member to the mask edited (a shape, or a parametric member
    /// reading the masked module's input), wired into a free port of its
    /// `mask_combine`, its parameters bound to the store; selects it.
    fn add_shape(
        &mut self,
        store: &mut Store,
        module: &str,
        params: &helios_core::Params,
    ) -> Result<String, String> {
        let e = self.edited.as_mut().ok_or("no image is open")?;
        let mask = crate::masks::current_mask(&e.graph, store)
            .ok_or("the image's network has no mask to draw in (a mask_combine node)")?;
        let net = crate::mask_tree::network(&e.graph, store, Some(&mask));
        let input = crate::mask_tree::blend_input(&e.graph, &net);
        let (id, name) = crate::masks::add_member(&mut e.graph, &mask, module, params, input)?;
        helios_values::bind_node(store, &e.graph, id)?;
        self.reshow();
        if crate::masks::KINDS.iter().any(|(_, m)| *m == module) {
            crate::masks::select(store, &name);
        } else {
            write(store, crate::masks::SHAPE, Value::Text(String::new()), None);
            write(
                store,
                crate::params::SELECTION,
                Value::Text(name.clone()),
                None,
            );
        }
        Ok(name)
    }

    /// The graph brought up to the store, and the mask network a member
    /// belongs to (the selected one's without one).
    fn mask_network(
        &mut self,
        store: &mut Store,
        member: Option<&str>,
    ) -> Result<crate::mask_tree::Network, String> {
        let e = self.edited.as_mut().ok_or("no image is open")?;
        helios_values::sync(store, &mut e.graph)?;
        Ok(crate::mask_tree::network(&e.graph, store, member))
    }

    /// After the network was rewired: the groups' values declared again
    /// as the graph holds them (no undo step), the image rendered again.
    fn rewired(&mut self, store: &mut Store, groups: &[NodeId]) -> Result<(), String> {
        let e = self.edited.as_ref().ok_or("no image is open")?;
        for g in groups {
            helios_values::bind_node(store, &e.graph, *g)?;
        }
        self.reshow();
        Ok(())
    }

    /// Runs a command on a member of the mask (the `Member*` commands of
    /// [`crate::masks::Command`]).
    fn member(
        &mut self,
        command: crate::masks::Command,
        origin: &Origin,
        store: &mut Store,
    ) -> Result<serde_json::Value, String> {
        use crate::mask_tree as tree;
        use crate::masks::Command;
        use serde_json::json;
        let named = match &command {
            Command::MemberMove { member, .. }
            | Command::MemberDelete { member }
            | Command::MemberInvert { member }
            | Command::MemberOpacity { member, .. }
            | Command::MemberRename { member, .. } => member.clone(),
            _ => return Err("not a member command".into()),
        };
        let net = self.mask_network(store, named.as_deref())?;
        let member = tree::member_or_selected(&net, store, named.as_deref())?;
        let shape = match store.get(crate::masks::SHAPE).map(|e| &e.value) {
            Some(Value::Text(t)) => t.clone(),
            _ => String::new(),
        };
        match command {
            Command::MemberMove { group, index, .. } => {
                let item = net.item(&member)?;
                let group = group
                    .or_else(|| item.group.clone())
                    .ok_or("the module's mask stays the root")?;
                let e = self.edited.as_mut().ok_or("no image is open")?;
                let groups = tree::move_member(
                    &mut e.graph,
                    &net,
                    &member,
                    &group,
                    index.unwrap_or(usize::MAX),
                )?;
                self.rewired(store, &groups)?;
                if shape == member {
                    write(store, crate::masks::MASK, Value::Text(group.clone()), None);
                }
                Ok(json!({ "member": member, "group": group }))
            }
            Command::MemberDelete { .. } => {
                let parent = net
                    .item(&member)?
                    .group
                    .clone()
                    .ok_or("the module's mask cannot be deleted from its tree")?;
                let e = self.edited.as_mut().ok_or("no image is open")?;
                let group = tree::remove_member(&mut e.graph, &net, &member)?;
                self.rewired(store, &[group])?;
                let selection = tree::selected(&net, store);
                if selection.is_some_and(|s| net.is_within(&s, &member)) {
                    tree::select(store, &net, &parent);
                }
                Ok(json!({ "member": member }))
            }
            Command::MemberInvert { .. } => {
                let on = tree::toggle_invert(store, &net, &member, origin)?;
                Ok(json!({ "member": member, "inverted": on }))
            }
            Command::MemberOpacity { opacity, .. } => {
                let to = tree::set_opacity(store, &net, &member, opacity, origin)?;
                Ok(json!({ "member": member, "opacity": to }))
            }
            Command::MemberRename { label, .. } => {
                tree::rename(store, &net, &member, &label, origin)?;
                Ok(json!({ "member": member, "label": label.trim() }))
            }
            _ => unreachable!("matched above"),
        }
    }

    /// Writes some of a node's parameters as one undo step.
    fn write_step(
        store: &mut Store,
        label: &str,
        origin: &Origin,
        node: &str,
        writes: Vec<(String, Value)>,
    ) -> Result<(), String> {
        let g = store.begin_group(origin.clone(), label);
        let mut out = Ok(());
        for (param, value) in writes {
            let path = helios_values::node_path(node, &param);
            if let Err(e) = store.set(Set::new(&path, Change::To(value), origin.clone()).gesture(g))
            {
                out = Err(e.to_string());
                break;
            }
        }
        store.end(g);
        out
    }

    /// A shape of the mask edited, as the store holds it now: `name`, or
    /// the selected one.
    fn shape(&self, store: &Store, name: Option<&str>) -> Result<crate::masks::Member, String> {
        let selected = match name {
            Some(n) => n.to_string(),
            None => match store.get(crate::masks::SHAPE).map(|e| &e.value) {
                Some(Value::Text(t)) if !t.is_empty() => t.clone(),
                _ => return Err("no shape is selected".into()),
            },
        };
        let e = self.edited.as_ref().ok_or("no image is open")?;
        let mask = crate::masks::current_mask(&e.graph, store).ok_or("the image has no mask")?;
        crate::masks::members(&e.graph, store, &mask)
            .into_iter()
            .find(|m| m.node == selected)
            .ok_or_else(|| format!("`{selected}` is not a shape of the mask `{mask}`"))
    }

    /// The selected shape, a path.
    fn path(&self, store: &Store) -> Result<crate::masks::Member, String> {
        let m = self.shape(store, None)?;
        if m.module == "mask_path" {
            Ok(m)
        } else {
            Err(format!("`{}` is not a path", m.node))
        }
    }

    /// Runs a mask command (see [`crate::masks::Command`]).
    pub fn mask(
        &mut self,
        command: crate::masks::Command,
        origin: Origin,
        store: &mut Store,
    ) -> Result<serde_json::Value, String> {
        use crate::masks::{self as masks, Command};
        use serde_json::json;
        let stop = |store: &mut Store| {
            write(store, masks::TOOL, Value::Choice("none".into()), None);
            write(store, masks::DRAFT, Value::List(Vec::new()), None);
        };
        match command {
            Command::Select { mask, shape, point } => {
                if let Some(mask) = mask {
                    let e = self.edited.as_ref().ok_or("no image is open")?;
                    if e.graph.find(&mask).is_none() {
                        return Err(format!("no mask `{mask}`"));
                    }
                    write(store, masks::MASK, Value::Text(mask), None);
                }
                if let Some(shape) = shape {
                    if !shape.is_empty() {
                        self.shape(store, Some(&shape))?;
                    }
                    masks::select(store, &shape);
                }
                if let Some(p) = point {
                    write(store, masks::POINT, Value::Int(p), None);
                }
                Ok(json!({}))
            }
            Command::Add { kind, params, .. } if kind == "parametric" => {
                let name = self.add_shape(store, "mask_parametric", &params)?;
                Ok(json!({ "member": name }))
            }
            Command::Add {
                kind,
                params,
                corners,
            } => {
                let module =
                    masks::module_of(&kind).ok_or_else(|| format!("no shape kind `{kind}`"))?;
                let mut p = if module == "mask_path" {
                    if corners.is_empty() {
                        write(store, masks::DRAFT, Value::List(Vec::new()), None);
                        write(store, masks::TOOL, Value::Choice("path".into()), None);
                        return Ok(json!({ "tool": "path" }));
                    }
                    if corners.len() < 3 {
                        return Err("a path needs three points at least".into());
                    }
                    masks::path_params(&corners)
                } else {
                    masks::placed_shape(module, self.visible().ok_or("no image is open")?)
                };
                for (k, v) in params.iter() {
                    p.set(k, v.clone());
                }
                let name = self.add_shape(store, module, &p)?;
                Ok(json!({ "shape": name }))
            }
            Command::Close => {
                if masks::tool(store) != "path" {
                    return Err("no path is being drawn".into());
                }
                let corners = masks::draft(store);
                if corners.len() < 3 {
                    return Err("a path needs three points at least".into());
                }
                let name = self.add_shape(store, "mask_path", &masks::path_params(&corners))?;
                stop(store);
                Ok(json!({ "shape": name }))
            }
            Command::Cancel => {
                stop(store);
                Ok(json!({}))
            }
            Command::PointAdd { at } => {
                let m = self.path(store)?;
                let (points, n) = masks::inserted(&m.params, at)?;
                let label = format!("add a point to {}", m.node);
                Self::write_step(
                    store,
                    &label,
                    &origin,
                    &m.node,
                    vec![("points".into(), points)],
                )?;
                write(store, masks::POINT, Value::Int(n as i64), None);
                Ok(json!({ "point": n }))
            }
            Command::PointRemove { point } => {
                let m = self.path(store)?;
                let n = point
                    .or_else(|| masks::point(store))
                    .ok_or("no path point is selected")?;
                let points = masks::removed(&m.params, n)?;
                let label = format!("remove a point of {}", m.node);
                Self::write_step(
                    store,
                    &label,
                    &origin,
                    &m.node,
                    vec![("points".into(), points)],
                )?;
                write(store, masks::POINT, Value::Int(-1), None);
                Ok(json!({}))
            }
            Command::PointSmooth { point, smooth } => {
                let m = self.path(store)?;
                let n = point
                    .or_else(|| masks::point(store))
                    .ok_or("no path point is selected")?;
                let points = masks::smoothed(&m.params, n, smooth)?;
                let what = if smooth { "smooth" } else { "sharpen" };
                let label = format!("{what} a point of {}", m.node);
                Self::write_step(
                    store,
                    &label,
                    &origin,
                    &m.node,
                    vec![("points".into(), points)],
                )?;
                Ok(json!({}))
            }
            Command::Move { shape, handle, to } => {
                let m = self.shape(store, shape.as_deref())?;
                let writes = masks::drag_writes(&m.module, &handle, &m.params, to)?;
                let label = format!("drag {} {}", m.node, handle.split('.').next().unwrap_or(""));
                Self::write_step(store, &label, &origin, &m.node, writes)?;
                Ok(json!({}))
            }
            member => self.member(member, &origin, store),
        }
    }

    /// Brings the shown graph up to the store's node values; a change
    /// renders again.
    pub fn sync(&mut self, store: &mut Store) {
        let Some(e) = &mut self.edited else { return };
        match helios_values::sync(store, &mut e.graph) {
            Ok(0) => {}
            Ok(_) => match Shown::new(Arc::new(e.graph.clone()), e.node, &e.port) {
                Ok(shown) => self.after.show(shown),
                Err(e) => eprintln!("helios-gui: viewport: {e}"),
            },
            Err(e) => eprintln!("helios-gui: viewport: {e}"),
        }
    }

    /// The size of the image shown, in pixels.
    pub fn image_size(&self) -> Option<[u32; 2]> {
        self.after.size()
    }

    /// The graph shown, to bind its nodes to a store.
    pub fn graph(&self) -> Option<&Graph> {
        self.edited.as_ref().map(|e| &e.graph)
    }

    /// egui-pop's viewport state: its tiles, its last mapping, its hooks.
    pub fn state(&self) -> &ViewportState {
        &self.state
    }

    /// egui-pop's viewport state, to set hooks.
    pub fn state_mut(&mut self) -> &mut ViewportState {
        &mut self.state
    }

    /// The values under the pointer, once they arrived.
    pub fn readout(&self) -> Option<&Readout> {
        self.readout.as_ref()
    }

    /// The float data of the view, before the display transform: the
    /// visible part of the image at the exact tiles' scale, rendered now
    /// (from the engine's cache once those tiles are on screen).
    pub fn scene(&self) -> Option<Image> {
        let m = self.state.mapping()?;
        let s = self.after.shown()?;
        let scale = exact_scale(&m);
        let v = m.visible()?;
        let size = scaled_size(m.image, scale);
        let x0 = (v[0] * scale).floor() as i64;
        let y0 = (v[1] * scale).floor() as i64;
        let x1 = ((v[2] * scale).ceil() as i64).min(size[0]);
        let y1 = ((v[3] * scale).ceil() as i64).min(size[1]);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let region = Region::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32);
        self.engine
            .render_at(&s.graph, s.node, &s.port, region, scale)
            .ok()
    }

    /// Moves the view as a command does (through the viewport's
    /// `before_view` hook) and writes it into the store.
    pub fn zoom(&mut self, to: ZoomTo, store: &Mutex<Store>) {
        let mut store = store.lock().unwrap_or_else(PoisonError::into_inner);
        let mut view = view_of(&store);
        match to {
            ZoomTo::Fit => {
                self.state.propose(&mut view, View::fit());
            }
            ZoomTo::Actual => {
                let to = match (self.state.mapping(), self.after.size()) {
                    (Some(m), _) => m.zoom_around(m.rect.center(), 1.0),
                    (None, Some([w, h])) => View::actual([f64::from(w) / 2.0, f64::from(h) / 2.0]),
                    (None, None) => return,
                };
                self.state.propose(&mut view, to);
            }
            ZoomTo::In => {
                self.state.zoom_in(&mut view);
            }
            ZoomTo::Out => {
                self.state.zoom_out(&mut view);
            }
        }
        write_view(&mut store, &view, None);
    }

    /// Draws the viewport; reads and writes the view in `store`.
    pub fn ui(&mut self, ui: &mut Ui, store: &Mutex<Store>) {
        let (mut view, compare) = {
            let store = store.lock().unwrap_or_else(PoisonError::into_inner);
            let pointer = ui.input(|i| i.pointer.hover_pos());
            self.masks.hook(self.state.hooks_mut());
            self.masks.prepare(
                self.edited.as_ref().map(|e| &e.graph),
                &store,
                self.state.mapping().as_ref(),
                pointer,
            );
            (view_of(&store), compare_of(&store))
        };
        let out = egui_pop::viewport::Viewport::new("helios.viewport")
            .source(&self.after)
            .before(&self.before)
            .overlays(self.masks.overlays())
            .compare(compare)
            .readout(true)
            .label("Image")
            .show(ui, &mut self.state, &mut view);
        let rect = out.response.rect;
        crate::name_rect(ui.ctx(), "viewport.display", rect);
        if self.edited.is_none() {
            ui.put(
                rect,
                egui::Label::new(egui::RichText::new("No image").weak()),
            );
        }
        self.readout = out.readout;
        let mut store = store.lock().unwrap_or_else(PoisonError::into_inner);
        let now = ui.input(|i| i.time);
        self.masks.signals(&out.signals, &mut store, now);
        if let Some(m) = out.mapping {
            if out.response.clicked()
                && !self.masks.dragging()
                && let Some(pos) = out.response.interact_pointer_pos()
                && self.masks.click(pos, &m, &mut store).is_some()
                && let Err(e) = self.mask(crate::masks::Command::Close, Origin::User, &mut store)
            {
                eprintln!("helios-gui: masks: {e}");
            }
            if let Some(at) = self.masks.take_double()
                && let Err(e) = self.mask(
                    crate::masks::Command::PointAdd { at },
                    Origin::User,
                    &mut store,
                )
            {
                eprintln!("helios-gui: masks: {e}");
            }
        }
        for signal in out.signals {
            match signal {
                Signal::View { view, phase, .. } => match phase {
                    Phase::Begin => {
                        if self.gesture.is_none() {
                            self.gesture = Some(store.begin_gesture(Origin::User, "pan the image"));
                        }
                    }
                    Phase::Update | Phase::Once => write_view(&mut store, &view, self.gesture),
                    Phase::End | Phase::Cancel => {
                        write_view(&mut store, &view, self.gesture);
                        if let Some(g) = self.gesture.take() {
                            store.end(g);
                        }
                    }
                },
                Signal::Compare {
                    compare: Compare::Split(t),
                    ..
                } => write(&mut store, SPLIT, Value::Float(f64::from(t)), None),
                _ => {}
            }
        }
    }
}
