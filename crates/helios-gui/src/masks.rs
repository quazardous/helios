//! Drawn masks on the viewport: the selected mask's shapes outlined over
//! the image, the selected shape's handles dragged, shapes created.
//!
//! - **What is drawn** comes from `helios_module_masks::outline` for each
//!   shape member of the mask edited (a `mask_combine` node): its edge and
//!   the end of its feather, in image pixels, mapped to the screen by the
//!   viewport. Every one is egui-pop's overlay: the selected shape's
//!   edge with its handles (the handle kit, 24-point targets; a path's
//!   Bézier handles as controls), the other shapes faint, each feather
//!   dashed. A path point's control lines are painted under the
//!   overlays (the viewport's `under_overlays` hook), the path being
//!   drawn above them (`after_paint`).
//! - **A drag** is one gesture of the value store: its Begin opens it,
//!   each Update writes the parameters the handle moves (from the values
//!   at the gesture's start), End closes it (one undo step), Cancel
//!   (Escape) puts the values back with no step ([`Store::cancel`]).
//! - **Snapping** is the viewport's `before_handle` hook: Shift
//!   constrains the angle (a rotation to 15° steps, a move to the axes and
//!   diagonals), Ctrl (Cmd) moves the handle a tenth of the pointer.
//! - **Editing state** lives in the store as transient values (never an
//!   undo step): the mask and the shape selected, the selected path
//!   point, the tool and the path being drawn.
//!
//! Everything is a command too (`mask.*`, see [`Command`]), so keys, the
//! palette and agents reach the same actions.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use egui::{Painter, Pos2, Stroke};
use egui_pop::viewport::hooks::HandleMove;
use egui_pop::viewport::overlay::{self, HIT_RADIUS, Handle, HandleKind, Overlay, Shape};
use egui_pop::viewport::tokens::ViewportTokens;
use egui_pop::viewport::{Cause, Mapping, Phase, Signal, Verdict, ViewChange};
use helios_core::{Graph, Message, ModuleRef, NodeId, Params, Value};
use helios_module_masks::shape::{Outline, PathPoint, path_points, path_values};
use helios_module_masks::{MaskCircle, MaskEllipse, MaskGradient, MaskPath};
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, GestureId, Set, Store, node_path};

/// The mask edited: a `mask_combine` node's name (empty: the graph's
/// first).
pub const MASK: &str = "view/masks/mask";
/// The shape selected: a member's node name (empty: none).
pub const SHAPE: &str = "view/masks/shape";
/// The selected point of a path (-1: none).
pub const POINT: &str = "view/masks/point";
/// The tool: `none`, or `path` while a path is being drawn.
pub const TOOL: &str = "view/masks/tool";
/// The points of the path being drawn, `x, y` after one another.
pub const DRAFT: &str = "view/masks/draft";

/// The tools.
pub const TOOLS: [&str; 2] = ["none", "path"];

/// The shape kinds, by the name their commands use, and their modules.
pub const KINDS: [(&str, &str); 4] = [
    ("circle", "mask_circle"),
    ("ellipse", "mask_ellipse"),
    ("gradient", "mask_gradient"),
    ("path", "mask_path"),
];

/// A rotation snaps to steps of this many degrees with Shift held.
pub const ANGLE_STEP: f64 = 15.0;
/// With Ctrl held a handle moves this share of the pointer.
pub const FINE: f64 = 0.1;
/// A gradient's rotation and feather handles sit at least this far
/// along its line from the anchor, in points.
const LINE_HANDLE: f64 = 80.0;

/// Declares the editing state's values: all transient.
pub fn declare(store: &mut Store) {
    store.declare(MASK, Descriptor::text("mask edited", "").transient());
    store.declare(SHAPE, Descriptor::text("shape selected", "").transient());
    store.declare(
        POINT,
        Descriptor::int("path point selected", -1, 63, -1).transient(),
    );
    store.declare(
        TOOL,
        Descriptor::choice("mask tool", &TOOLS, "none").transient(),
    );
    let mut draft = Descriptor::from_spec(
        &helios_core::ParamSpec {
            name: "path being drawn".into(),
            description: "x, y of each point clicked so far".into(),
            kind: helios_core::ParamKind::Floats {
                count: 2 * helios_module_masks::shape::MAX_PATH_POINTS as u32,
                min: -1e9,
                max: 1e9,
                default: Vec::new(),
            },
        },
        None,
    );
    draft.transient = true;
    store.declare(DRAFT, draft);
}

fn text(store: &Store, path: &str) -> String {
    match store.get(path).map(|e| &e.value) {
        Some(Value::Text(t)) => t.clone(),
        _ => String::new(),
    }
}

/// The selected path point, if any.
pub fn point(store: &Store) -> Option<usize> {
    store
        .get(POINT)
        .and_then(|e| e.value.as_i64())
        .and_then(|p| usize::try_from(p).ok())
}

/// The tool in use.
pub fn tool(store: &Store) -> String {
    store
        .get(TOOL)
        .and_then(|e| e.value.as_choice().map(String::from))
        .unwrap_or_else(|| "none".into())
}

/// The path being drawn.
pub fn draft(store: &Store) -> Vec<[f64; 2]> {
    store
        .get(DRAFT)
        .and_then(|e| e.value.as_list())
        .map(|v| v.chunks_exact(2).map(|p| [p[0], p[1]]).collect())
        .unwrap_or_default()
}

/// Writes a transient editing value (refused only while another
/// origin's gesture holds it).
fn put(store: &mut Store, path: &str, value: Value) {
    let _ = store.set(Set::new(path, Change::To(value), Origin::User));
}

fn set_draft(store: &mut Store, points: &[[f64; 2]]) {
    put(
        store,
        DRAFT,
        Value::List(points.iter().flat_map(|p| [p[0], p[1]]).collect()),
    );
}

/// Selects a shape (and no point); the Parameters panel follows it.
pub fn select(store: &mut Store, shape: &str) {
    put(store, SHAPE, Value::Text(shape.into()));
    put(store, POINT, Value::Int(-1));
    if !shape.is_empty() {
        put(store, crate::params::SELECTION, Value::Text(shape.into()));
    }
}

/// The module of a shape kind (`circle`...).
pub fn module_of(kind: &str) -> Option<&'static str> {
    KINDS.iter().find(|(k, _)| *k == kind).map(|(_, m)| *m)
}

fn module_ref(module: &str) -> Option<ModuleRef> {
    Some(match module {
        "mask_parametric" => ModuleRef::new(helios_module_masks::MaskParametric),
        "mask_circle" => ModuleRef::new(MaskCircle),
        "mask_ellipse" => ModuleRef::new(MaskEllipse),
        "mask_gradient" => ModuleRef::new(MaskGradient),
        "mask_path" => ModuleRef::new(MaskPath),
        _ => return None,
    })
}

fn module_name(graph: &Graph, id: NodeId) -> Option<String> {
    graph.manifest(id).ok().flatten().map(|m| m.name)
}

/// The mask edited: the store's when the graph has it, else the graph's
/// first `mask_combine`.
pub fn current_mask(graph: &Graph, store: &Store) -> Option<String> {
    let chosen = text(store, MASK);
    if !chosen.is_empty() && graph.find(&chosen).is_some() {
        return Some(chosen);
    }
    graph
        .ids()
        .find(|id| module_name(graph, *id).as_deref() == Some("mask_combine"))
        .and_then(|id| graph.name(id).ok().map(String::from))
}

/// A shape member of a mask, as the store holds it now.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub node: String,
    pub module: String,
    pub params: Params,
    pub outline: Outline,
}

/// A node's parameters as the store holds them (the graph's for those it
/// does not).
pub(crate) fn params_of(graph: &Graph, store: &Store, id: NodeId) -> Params {
    let name = graph.name(id).unwrap_or_default().to_string();
    let mut p = graph.params(id).ok().flatten().unwrap_or_default();
    if let Ok(Some(m)) = graph.manifest(id) {
        for spec in &m.params {
            if let Some(e) = store.get(&node_path(&name, &spec.name)) {
                p.set(&spec.name, e.value.clone());
            }
        }
    }
    p
}

/// The shape members of a mask, in port order.
pub fn members(graph: &Graph, store: &Store, mask: &str) -> Vec<Member> {
    let Some(id) = graph.find(mask) else {
        return Vec::new();
    };
    let Ok(inputs) = graph.input_nodes(id) else {
        return Vec::new();
    };
    inputs
        .into_iter()
        .flatten()
        .filter_map(|src| {
            let module = module_name(graph, src)?;
            if !KINDS.iter().any(|(_, m)| *m == module) {
                return None;
            }
            let params = params_of(graph, store, src);
            let outline = helios_module_masks::outline(&module, &params).ok()?;
            Some(Member {
                node: graph.name(src).ok()?.to_string(),
                module,
                params,
                outline,
            })
        })
        .collect()
}

/// The handle kit's kind for a handle of `outline()`.
pub fn kind_of(name: &str) -> HandleKind {
    match name.split('.').next().unwrap_or(name) {
        "centre" => HandleKind::Move,
        "radius" | "radius_x" | "radius_y" => HandleKind::Size,
        "rotation" => HandleKind::Rotate,
        "feather" => HandleKind::Feather,
        "before" | "after" => HandleKind::Control,
        // A path's corners.
        _ => HandleKind::Point,
    }
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: [f64; 2], k: f64) -> [f64; 2] {
    [a[0] * k, a[1] * k]
}

fn len(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}

fn lerp(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    add(a, scale(sub(b, a), t))
}

/// An angle in degrees, in (-180, 180].
fn degrees(d: [f64; 2]) -> f64 {
    d[1].atan2(d[0]).to_degrees()
}

fn wrap(deg: f64) -> f64 {
    let d = deg.rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// Where a handle is drawn: where `outline()` puts it, except a
/// gradient's rotation and feather handles, slid along the line to at
/// least [`LINE_HANDLE`] points from the anchor so they never cover it.
pub fn placed(member: &Member, index: usize, m: Option<&Mapping>) -> [f64; 2] {
    let h = &member.outline.handles[index];
    if member.module != "mask_gradient" || !(h.name == "rotation" || h.name == "feather") {
        return h.at;
    }
    let Some(m) = m else { return h.at };
    let r = member.params.float("rotation").to_radians();
    let t = [r.cos(), -r.sin()];
    let reach = (LINE_HANDLE * f64::from(m.ppp) / m.zoom).max(50.0);
    let sign = if h.name == "rotation" { 1.0 } else { -1.0 };
    // `outline()` puts both 50 pixels along the line.
    let p = add(h.at, scale(t, sign * (reach - 50.0)));
    if p[0].is_finite() && p[1].is_finite() {
        p
    } else {
        h.at
    }
}

/// The handles shown for a member: all but a path's Bézier handles,
/// shown for the selected point only and when they are off their
/// corner (on it, the corner takes the drag).
pub fn shown(member: &Member, point: Option<usize>, m: Option<&Mapping>) -> Vec<Handle> {
    member
        .outline
        .handles
        .iter()
        .enumerate()
        .filter(|(i, h)| {
            let Some((side, n)) = h.name.split_once('.') else {
                return true;
            };
            if side == "corner" {
                return true;
            }
            let n: usize = n.parse().unwrap_or(usize::MAX);
            let corner = member.outline.handles[i - if side == "before" { 1 } else { 2 }].at;
            point == Some(n) && len(sub(h.at, corner)) > 1e-9
        })
        .map(|(i, h)| Handle {
            index: i as u32,
            kind: kind_of(&h.name),
            at: placed(member, i, m),
        })
        .collect()
}

/// The overlay of the selected shape: its edge and its handles.
pub fn overlay_of(member: &Member, point: Option<usize>, m: Option<&Mapping>) -> Overlay {
    Overlay::new(&member.node, edge_of(&member.outline))
        .handles(shown(member, point, m))
        .selected()
}

fn edge_of(o: &Outline) -> Shape {
    Shape::Path {
        points: o.edge.clone(),
        closed: o.closed,
    }
}

/// What is drawn of a member besides the selected shape's overlay: its
/// edge faint when it is not the one selected, its feather dashed (faint
/// too then), none with handles.
pub fn guides_of(member: &Member, selected: bool) -> Vec<Overlay> {
    let o = &member.outline;
    let style = |g: Overlay| if selected { g } else { g.faint() };
    let mut out = Vec::new();
    if !selected {
        out.push(style(Overlay::guide(&member.node, edge_of(o))));
    }
    let feather = format!("{}/feather", member.node);
    if o.closed {
        if !o.feather.is_empty() {
            let shape = Shape::Path {
                points: o.feather.clone(),
                closed: true,
            };
            out.push(style(Overlay::guide(&feather, shape).dashed()));
        }
    } else {
        // A gradient's band: one side's two points, then the other's.
        for side in o.feather.chunks(2) {
            let shape = Shape::Path {
                points: side.to_vec(),
                closed: false,
            };
            out.push(style(Overlay::guide(&feather, shape).dashed()));
        }
    }
    out
}

/// The parameters a handle at `to` writes, from the shape's parameters
/// at the gesture's start.
pub fn drag_writes(
    module: &str,
    handle: &str,
    start: &Params,
    to: [f64; 2],
) -> Result<Vec<(String, Value)>, String> {
    let f = |v: f64| Value::Float(v);
    let c = centre_of(start);
    let d = sub(to, c);
    let w = |pairs: &[(&str, f64)]| {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), f(*v)))
            .collect::<Vec<_>>()
    };
    Ok(match (module, handle) {
        ("mask_path", h) => {
            let values = start.get("points").and_then(Value::as_list).unwrap_or(&[]);
            let mut points = path_points(values)?;
            let (side, n) = h
                .split_once('.')
                .ok_or_else(|| format!("no handle `{h}` on a path"))?;
            let n: usize = n.parse().map_err(|_| format!("no handle `{h}`"))?;
            let p = points
                .get_mut(n)
                .ok_or_else(|| format!("no point {n} on the path"))?;
            match side {
                "corner" => {
                    let delta = sub(to, p.corner);
                    p.corner = to;
                    p.before = add(p.before, delta);
                    p.after = add(p.after, delta);
                }
                // The other handle mirrors it: a smooth point.
                "before" => {
                    p.before = to;
                    p.after = sub(scale(p.corner, 2.0), to);
                }
                "after" => {
                    p.after = to;
                    p.before = sub(scale(p.corner, 2.0), to);
                }
                _ => return Err(format!("no handle `{h}` on a path")),
            }
            vec![("points".into(), path_values(&points))]
        }
        (_, "centre") => w(&[("x", to[0]), ("y", to[1])]),
        ("mask_circle", "radius") => w(&[("radius", len(d))]),
        ("mask_circle", "feather") => w(&[("feather", (len(d) - start.float("radius")).max(0.0))]),
        ("mask_ellipse", "radius_x") => w(&[("radius_x", len(d)), ("rotation", degrees(d))]),
        ("mask_ellipse", "radius_y") => {
            w(&[("radius_y", len(d)), ("rotation", wrap(degrees(d) - 90.0))])
        }
        ("mask_ellipse", "feather") => {
            let (a, b) = (start.float("radius_x"), start.float("radius_y"));
            let proportional = start.get("proportional").and_then(Value::as_bool) == Some(true);
            let feather = if proportional && a > 0.0 {
                (len(d) / a - 1.0) * a.max(b)
            } else {
                len(d) - a
            };
            w(&[("feather", feather.max(0.0))])
        }
        ("mask_gradient", "rotation") => w(&[("rotation", degrees([d[0], -d[1]]))]),
        ("mask_gradient", "feather") => {
            let r = start.float("rotation").to_radians();
            let n = [-r.sin(), -r.cos()];
            w(&[("feather", 2.0 * (d[0] * n[0] + d[1] * n[1]).abs())])
        }
        (m, h) => return Err(format!("no handle `{h}` on `{m}`")),
    })
}

/// A shape's centre or anchor (`x`, `y`; a path has none: the origin).
fn centre_of(params: &Params) -> [f64; 2] {
    let n = |k: &str| params.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    [n("x"), n("y")]
}

/// A point on a cubic Bézier segment.
fn bezier(p: [[f64; 2]; 4], t: f64) -> [f64; 2] {
    let s = 1.0 - t;
    let k = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
    (0..4).fold([0.0, 0.0], |a, i| add(a, scale(p[i], k[i])))
}

fn segment(points: &[PathPoint], i: usize) -> [[f64; 2]; 4] {
    let (a, b) = (points[i], points[(i + 1) % points.len()]);
    [a.corner, a.after, b.before, b.corner]
}

/// The segment of a closed path nearest `at` and where on it (`t`), with
/// the distance, in image pixels.
pub fn nearest(points: &[PathPoint], at: [f64; 2]) -> Option<(usize, f64, f64)> {
    let mut best: Option<(usize, f64, f64)> = None;
    for i in 0..points.len() {
        let s = segment(points, i);
        for k in 0..=64 {
            let t = f64::from(k) / 64.0;
            let dist = len(sub(bezier(s, t), at));
            if best.is_none_or(|b| dist < b.2) {
                best = Some((i, t, dist));
            }
        }
    }
    best
}

/// The path with a point added on its segment `i` at `t`, the curve kept
/// exactly (de Casteljau's split); the new point's index.
pub fn split(points: &[PathPoint], i: usize, t: f64) -> (Vec<PathPoint>, usize) {
    let [p0, p1, p2, p3] = segment(points, i);
    let (a, b, c) = (lerp(p0, p1, t), lerp(p1, p2, t), lerp(p2, p3, t));
    let (ab, bc) = (lerp(a, b, t), lerp(b, c, t));
    let at = lerp(ab, bc, t);
    let mut out = points.to_vec();
    let j = (i + 1) % points.len();
    out[i].after = a;
    out[j].before = c;
    let new = PathPoint {
        corner: at,
        before: ab,
        after: bc,
    };
    out.insert(i + 1, new);
    (out, i + 1)
}

/// Default parameters for a shape placed in `visible` (image pixels:
/// `[x0, y0, x1, y1]`), sized to it.
pub fn placed_shape(module: &str, visible: [f64; 4]) -> Params {
    let c = [
        f64::midpoint(visible[0], visible[2]),
        f64::midpoint(visible[1], visible[3]),
    ];
    let s = (visible[2] - visible[0])
        .min(visible[3] - visible[1])
        .max(1.0);
    let f = Value::Float;
    let p = Params::new().with("x", f(c[0])).with("y", f(c[1]));
    match module {
        "mask_circle" => p.with("radius", f(s / 6.0)).with("feather", f(s / 24.0)),
        "mask_ellipse" => p
            .with("radius_x", f(s / 4.0))
            .with("radius_y", f(s / 7.0))
            .with("feather", f(s / 24.0)),
        "mask_gradient" => p.with("feather", f(s / 4.0)),
        _ => p,
    }
}

/// A path's parameters from the corners clicked.
pub fn path_params(corners: &[[f64; 2]]) -> Params {
    let points: Vec<PathPoint> = corners
        .iter()
        .map(|c| PathPoint::corner(c[0], c[1]))
        .collect();
    Params::new()
        .with("points", path_values(&points))
        .with("feather", Value::Float(0.0))
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let l = ab.length_sq();
    let t = if l > 0.0 {
        ((p - a).dot(ab) / l).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a + ab * t).distance(p)
}

/// The distance from a screen point to a polyline in image pixels, in
/// points.
pub fn distance_to(points: &[[f64; 2]], closed: bool, m: &Mapping, p: Pos2) -> f32 {
    let s: Vec<Pos2> = points.iter().map(|q| m.image_to_screen(*q)).collect();
    let n = s.len();
    let pairs = if closed { n } else { n.saturating_sub(1) };
    (0..pairs)
        .map(|i| distance_to_segment(p, s[i], s[(i + 1) % n]))
        .fold(f32::INFINITY, f32::min)
}

/// How a handle is held to its line by Shift.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Constraint {
    /// Its angle around this point, to [`ANGLE_STEP`].
    Angle([f64; 2]),
    /// Its move from where it started, to the axes and diagonals.
    Line,
    None,
}

/// What the `before_handle` and `before_view` hooks know, set before
/// each frame.
#[derive(Default)]
struct Snap {
    constraints: BTreeMap<(String, u32), Constraint>,
    /// A primary click this frame is the editor's (a path point, a
    /// shape's edge or a handle): the pointer profile's click on the
    /// image (Lightroom Classic's fit or 100 %) leaves the view alone.
    claims_click: bool,
}

/// What the paint hooks draw, set before each frame.
#[derive(Default)]
struct Faint {
    /// A selected path point's Bézier handles: corner, handle.
    controls: Vec<([f64; 2], [f64; 2])>,
    draft: Vec<[f64; 2]>,
    pointer: Option<Pos2>,
}

/// A double click on the canvas the editor took (adding a path point),
/// or passed on to a pointer profile that leaves the view.
#[derive(Default)]
struct Double {
    /// The selected path's edge, to test the double click against.
    edge: Option<Vec<[f64; 2]>>,
    taken: Option<[f64; 2]>,
    /// The pointer profile's double click leaves the view (darktable's),
    /// as the viewport said before it was drawn.
    leaves: bool,
    /// A double click the editor did not take, under such a profile.
    left: bool,
}

/// What Shift does to a handle's drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShiftSnap {
    /// Turns it around a point to [`ANGLE_STEP`] steps.
    Angle,
    /// Holds its move to the axes and diagonals.
    Axes,
}

/// The gestures a handle takes, said in the status line while it is
/// hovered: Ctrl moves any handle finely; the rest depend on the handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gestures {
    pub shift: Option<ShiftSnap>,
    /// A double click resets it (a rotation, a feather, a path corner).
    pub reset: bool,
    /// The wheel adjusts it (a size, a feather, a rotation).
    pub wheel: bool,
}

impl Gestures {
    /// The gestures of the handle `name` (`radius`, `corner.2`...).
    pub fn of(name: &str, shift: Option<ShiftSnap>) -> Gestures {
        Gestures {
            shift,
            reset: name.starts_with("corner.") || matches!(name, "rotation" | "feather"),
            wheel: matches!(
                name,
                "radius" | "radius_x" | "radius_y" | "feather" | "rotation"
            ),
        }
    }

    /// The status line's hint: each gesture, in the language of `ctx`.
    pub fn hint(&self, ctx: &egui::Context) -> String {
        let mut parts = Vec::new();
        match self.shift {
            Some(ShiftSnap::Angle) => parts.push(Message::new(
                "gui-masks-gesture-shift-angle",
                "Shift: snap the angle to 15° steps",
            )),
            Some(ShiftSnap::Axes) => parts.push(Message::new(
                "gui-masks-gesture-shift-axes",
                "Shift: along the axes and diagonals",
            )),
            None => {}
        }
        parts.push(Message::new("gui-masks-gesture-ctrl", "Ctrl: fine moves"));
        if self.reset {
            parts.push(Message::new(
                "gui-masks-gesture-reset",
                "double-click: reset",
            ));
        }
        if self.wheel {
            parts.push(Message::new("gui-masks-gesture-wheel", "wheel: adjust"));
        }
        parts
            .into_iter()
            .map(|m| crate::i18n::text(ctx, m))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// A drag under way.
struct Drag {
    gesture: GestureId,
    node: String,
    module: String,
    handle: String,
    start: Params,
}

/// The wheel over a handle: one group while it turns.
struct Wheel {
    gesture: GestureId,
    last: f64,
}

/// What the viewport's mask editing keeps between frames: never values.
#[derive(Default)]
pub struct Masks {
    drag: Option<Drag>,
    wheel: Option<Wheel>,
    snap: Rc<RefCell<Snap>>,
    faint: Rc<RefCell<Faint>>,
    double: Rc<RefCell<Double>>,
    hooked: bool,
    /// The members drawn this frame.
    members: Vec<Member>,
    /// The selected shape's overlay this frame.
    overlays: Vec<Overlay>,
    /// The other shapes and the feathers this frame, no handles.
    guides: Vec<Overlay>,
    /// The gestures of the handle under the pointer this frame.
    hovered: Option<Gestures>,
    /// The primary button goes up this frame (a click may end).
    primary_up: bool,
}

/// A request to the mask editor, from a command.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Select a mask, a shape and a path point (each optional).
    Select {
        mask: Option<String>,
        shape: Option<String>,
        point: Option<i64>,
    },
    /// Add a shape of a kind (`circle`...): placed at the view's centre
    /// and sized to it, its parameters overridden by `params`; a path
    /// with `corners` is made at once, without them the path tool starts.
    Add {
        kind: String,
        params: Params,
        corners: Vec<[f64; 2]>,
    },
    /// Close the path being drawn into a shape.
    Close,
    /// Stop the tool, dropping the path being drawn.
    Cancel,
    /// Add a point to the selected path, on its edge nearest `at`.
    PointAdd { at: [f64; 2] },
    /// Remove a point of the selected path (the selected point by
    /// default).
    PointRemove { point: Option<usize> },
    /// Make a path point smooth (handles along its neighbours) or sharp
    /// (handles on its corner).
    PointSmooth { point: Option<usize>, smooth: bool },
    /// Move a handle of a shape (the selected one by default) to `to`,
    /// one undo step.
    Move {
        shape: Option<String>,
        handle: String,
        to: [f64; 2],
    },
    /// Move a member of the mask (the selected one by default) to
    /// `index` of `group` (its own group, last, by default): rewiring,
    /// not an undo step.
    MemberMove {
        member: Option<String>,
        group: Option<String>,
        index: Option<usize>,
    },
    /// Take a member (the selected one by default) out of the mask:
    /// rewiring, not an undo step. A person taking out a group that
    /// holds members is asked first, unless `confirmed` (see
    /// [`crate::mask_tree::confirm_take_out`]).
    MemberDelete {
        member: Option<String>,
        confirmed: bool,
    },
    /// Invert a member (the selected one by default), or back; one undo
    /// step.
    MemberInvert { member: Option<String> },
    /// Set a member's opacity (the selected one by default), or toggle
    /// it between none and full; one undo step.
    MemberOpacity {
        member: Option<String>,
        opacity: Option<f64>,
    },
    /// Name a member (the selected one by default; empty: its node's
    /// name); one undo step.
    MemberRename {
        member: Option<String>,
        label: String,
    },
    /// Lock a member (the selected one by default), or unlock it
    /// (`locked`; without it, the other way round); one undo step.
    MemberLock {
        member: Option<String>,
        locked: Option<bool>,
    },
    /// Add copied shapes (a `helios.mask` payload) to the mask edited,
    /// scaled to the image open (see `crate::clipboard::mask`).
    Paste { clip: serde_json::Value },
}

impl Masks {
    /// The overlays drawn last frame (the selected shape's).
    pub fn overlays(&self) -> &[Overlay] {
        &self.overlays
    }

    /// Everything the viewport draws: the guides (the other shapes, the
    /// feathers), then the selected shape's overlay above them.
    pub fn drawn(&self) -> Vec<Overlay> {
        self.guides.iter().chain(&self.overlays).cloned().collect()
    }

    /// The guides drawn last frame: the other shapes faint, the
    /// feathers dashed.
    pub fn guides(&self) -> &[Overlay] {
        &self.guides
    }

    /// The members drawn last frame.
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// The gestures of the handle under the pointer last frame, if one
    /// is.
    pub fn hovered(&self) -> Option<Gestures> {
        self.hovered
    }

    /// What the status line says while a handle is hovered: its
    /// gestures, in the language of `ctx`.
    pub fn hint(&self, ctx: &egui::Context) -> Option<String> {
        self.hovered.map(|g| g.hint(ctx))
    }

    /// Installs the hooks on the viewport's state, once.
    pub fn hook(&mut self, hooks: &mut egui_pop::viewport::Hooks) {
        if self.hooked {
            return;
        }
        self.hooked = true;
        let snap = self.snap.clone();
        hooks.before_handle = Some(Box::new(move |mv: &HandleMove, _m: &Mapping| {
            let s = snap.borrow();
            let key = (mv.overlay.clone(), mv.handle);
            let start = mv.start;
            let mut to = mv.to;
            if mv.modifiers.command {
                to = add(start, scale(sub(to, start), FINE));
            }
            if mv.modifiers.shift {
                to = match s.constraints.get(&key).copied().unwrap_or(Constraint::None) {
                    Constraint::Angle(pivot) => {
                        let d = sub(to, pivot);
                        let a = (degrees(d) / ANGLE_STEP).round() * ANGLE_STEP;
                        let r = len(d);
                        add(pivot, [r * a.to_radians().cos(), r * a.to_radians().sin()])
                    }
                    Constraint::Line => {
                        let d = sub(to, start);
                        let a = (degrees(d) / 45.0).round() * 45.0;
                        let u = [a.to_radians().cos(), a.to_radians().sin()];
                        add(start, scale(u, d[0] * u[0] + d[1] * u[1]))
                    }
                    Constraint::None => to,
                };
            }
            if to == mv.to {
                Verdict::Allow
            } else {
                Verdict::Redirect(to)
            }
        }));
        let faint = self.faint.clone();
        hooks.under_overlays = Some(Box::new(move |painter: &Painter, m: &Mapping| {
            paint_controls(painter, m, &faint.borrow());
        }));
        let faint = self.faint.clone();
        hooks.after_paint = Some(Box::new(move |painter: &Painter, m: &Mapping| {
            paint_draft(painter, m, &faint.borrow());
        }));
        // A click the editor takes is not also the profile's view
        // action (Lightroom Classic's click toggles fit and 100 %): drawing a path
        // or selecting a shape keeps the view where it is.
        let snap = self.snap.clone();
        hooks.before_view = Some(Box::new(move |change: &ViewChange, _m: &Mapping| {
            if change.cause == Cause::Click && snap.borrow().claims_click {
                Verdict::Veto
            } else {
                Verdict::Allow
            }
        }));
        let double = self.double.clone();
        hooks.on_double_click = Some(Box::new(
            move |pos: Pos2,
                  m: &Mapping,
                  default: &mut dyn FnMut() -> Option<egui_pop::viewport::View>| {
                let mut d = double.borrow_mut();
                let near = d
                    .edge
                    .as_ref()
                    .is_some_and(|e| distance_to(e, true, m, pos) <= HIT_RADIUS);
                if near {
                    d.taken = Some(m.screen_to_image(pos));
                    None
                } else {
                    // egui-pop signals the profile's "leave the view"
                    // only without a hook: say it from here.
                    let to = default();
                    if to.is_none() && d.leaves {
                        d.left = true;
                    }
                    to
                }
            },
        ));
    }

    /// Before the viewport is drawn: the selected mask's members, the
    /// overlay of the selected shape, what the hooks need, and the
    /// gestures of the handle under `pointer`.
    pub fn prepare(
        &mut self,
        graph: Option<&Graph>,
        store: &Store,
        mapping: Option<&Mapping>,
        pointer: Option<Pos2>,
    ) {
        let mask = graph.and_then(|g| current_mask(g, store));
        self.members = match (graph, &mask) {
            (Some(g), Some(mask)) => members(g, store, mask),
            _ => Vec::new(),
        };
        let selected = text(store, SHAPE);
        let point = point(store);
        let mut faint = Faint {
            draft: if tool(store) == "path" {
                draft(store)
            } else {
                Vec::new()
            },
            pointer,
            ..Faint::default()
        };
        let mut snap = self.snap.borrow_mut();
        snap.constraints.clear();
        self.overlays.clear();
        self.guides.clear();
        let mut edge = None;
        for member in &self.members {
            let on = member.node == selected;
            self.guides.extend(guides_of(member, on));
            if !on {
                continue;
            }
            let mut o = overlay_of(member, point, mapping);
            // A locked shape is shown, not edited: no handles, and no
            // point added on its edge.
            let locked = crate::mask_tree::is_locked(store, &member.node);
            if locked {
                o.handles.clear();
            }
            let centre = centre_of(&member.params);
            for h in &o.handles {
                let name = &member.outline.handles[h.index as usize].name;
                let c = match (member.module.as_str(), name.as_str()) {
                    (_, "centre") => Constraint::Line,
                    (_, n) if n.starts_with("corner.") => Constraint::Line,
                    ("mask_ellipse", "radius_x" | "radius_y") | ("mask_gradient", "rotation") => {
                        Constraint::Angle(centre)
                    }
                    ("mask_path", n) => {
                        // A Bézier handle turns around its corner.
                        let i: usize = n
                            .split_once('.')
                            .and_then(|(_, i)| i.parse().ok())
                            .unwrap_or(0);
                        member
                            .outline
                            .handles
                            .get(3 * i)
                            .map_or(Constraint::None, |c| Constraint::Angle(c.at))
                    }
                    _ => Constraint::None,
                };
                snap.constraints.insert((o.id.clone(), h.index), c);
                if member.module == "mask_path" && name.starts_with(['b', 'a']) {
                    let i = h.index as usize / 3 * 3;
                    faint.controls.push((member.outline.handles[i].at, h.at));
                }
            }
            if member.module == "mask_path" && !locked {
                edge = Some(member.outline.edge.clone());
            }
            self.overlays.push(o);
        }
        self.hovered = match (pointer, mapping) {
            (Some(pos), Some(m)) => overlay::hit(&self.overlays, m, pos).and_then(|(i, h)| {
                let o = &self.overlays[i];
                let member = self.members.iter().find(|mb| mb.node == o.id)?;
                let name = &member.outline.handles.get(h.index as usize)?.name;
                let shift = match snap.constraints.get(&(o.id.clone(), h.index)) {
                    Some(Constraint::Angle(_)) => Some(ShiftSnap::Angle),
                    Some(Constraint::Line) => Some(ShiftSnap::Axes),
                    _ => None,
                };
                Some(Gestures::of(name, shift))
            }),
            _ => None,
        };
        snap.claims_click = self.primary_up
            && (tool(store) == "path"
                || match (pointer, mapping) {
                    (Some(pos), Some(m)) => {
                        overlay::hit(&self.overlays, m, pos).is_some()
                            || self.members.iter().any(|mb| {
                                distance_to(&mb.outline.edge, mb.outline.closed, m, pos)
                                    <= HIT_RADIUS
                            })
                    }
                    _ => false,
                });
        *self.faint.borrow_mut() = faint;
        let mut d = self.double.borrow_mut();
        d.edge = if tool(store) == "path" { None } else { edge };
    }

    /// Before the viewport is drawn: whether the primary button goes up
    /// this frame, so a click the editor takes is known before the
    /// viewport acts on it.
    pub fn set_primary_up(&mut self, up: bool) {
        self.primary_up = up;
    }

    /// The member named `node`, as drawn this frame.
    fn member(&self, node: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.node == node)
    }

    /// After the viewport is drawn: applies its handle signals to the
    /// store. `now` is the frame's time in seconds.
    pub fn signals(&mut self, signals: &[Signal], store: &mut Store, now: f64) {
        for signal in signals {
            match signal {
                Signal::Handle {
                    overlay,
                    handle,
                    phase,
                    at,
                    ..
                } => self.on_handle(overlay, *handle, *phase, *at, store),
                Signal::HandleReset {
                    overlay, handle, ..
                } => self.on_reset(overlay, *handle, store),
                Signal::HandleWheel {
                    overlay,
                    handle,
                    delta,
                    ..
                } => self.on_wheel(overlay, *handle, *delta, store, now),
                _ => {}
            }
        }
        if let Some(w) = &self.wheel
            && now - w.last > 0.5
        {
            store.end(w.gesture);
            self.wheel = None;
        }
    }

    fn handle_name(&self, node: &str, index: u32) -> Option<(String, String, Params)> {
        let m = self.member(node)?;
        let h = m.outline.handles.get(index as usize)?;
        Some((m.module.clone(), h.name.clone(), m.params.clone()))
    }

    fn on_handle(&mut self, node: &str, index: u32, phase: Phase, at: [f64; 2], store: &mut Store) {
        match phase {
            Phase::Begin => {
                let Some((module, handle, start)) = self.handle_name(node, index) else {
                    return;
                };
                if let Some(d) = self.drag.take() {
                    store.end(d.gesture);
                }
                if let Some((_, n)) = handle.split_once('.') {
                    put(store, POINT, Value::Int(n.parse().unwrap_or(-1)));
                }
                let label = format!("drag {node} {}", handle.split('.').next().unwrap_or(""));
                self.drag = Some(Drag {
                    gesture: store.begin_gesture(Origin::User, &label),
                    node: node.into(),
                    module,
                    handle,
                    start,
                });
            }
            Phase::Update | Phase::End => {
                if let Some(d) = &self.drag
                    && d.node == node
                {
                    write_drag(d, at, store);
                }
                if phase == Phase::End
                    && let Some(d) = self.drag.take()
                {
                    store.end(d.gesture);
                }
            }
            Phase::Cancel => {
                if let Some(d) = self.drag.take() {
                    store.cancel(d.gesture);
                }
            }
            Phase::Once => {}
        }
    }

    /// A double click on a handle resets what it adjusts: a rotation to
    /// 0, a feather to none, a path point to a sharp corner.
    fn on_reset(&mut self, node: &str, index: u32, store: &mut Store) {
        let Some((module, handle, params)) = self.handle_name(node, index) else {
            return;
        };
        let writes = match handle.split_once('.') {
            Some(("corner", n)) => {
                let Ok(n) = n.parse() else { return };
                match smoothed(&params, n, false) {
                    Ok(v) => vec![("points".to_string(), v)],
                    Err(_) => return,
                }
            }
            _ => match handle.as_str() {
                "rotation" => vec![("rotation".into(), Value::Float(0.0))],
                "feather" => vec![("feather".into(), Value::Float(0.0))],
                _ => return,
            },
        };
        let _ = module;
        let g = store.begin_group(Origin::User, &format!("reset {node} {handle}"));
        for (param, value) in writes {
            let _ = store.set(
                Set::new(&node_path(node, &param), Change::To(value), Origin::User).gesture(g),
            );
        }
        store.end(g);
    }

    /// The wheel over a handle adjusts its quantity: a size or a feather
    /// by a ratio, a rotation by degrees; one undo step while it turns.
    fn on_wheel(&mut self, node: &str, index: u32, delta: f32, store: &mut Store, now: f64) {
        let Some((_, handle, params)) = self.handle_name(node, index) else {
            return;
        };
        let k = f64::from(delta);
        let (param, value) = match handle.as_str() {
            "radius" | "radius_x" | "radius_y" | "feather" => {
                let p = handle.as_str();
                (p, (params.float(p) * (k * 0.002).exp()).max(0.0))
            }
            "rotation" => ("rotation", wrap(params.float("rotation") + k * 0.1)),
            _ => return,
        };
        let gesture = if let Some(w) = &mut self.wheel {
            w.last = now;
            w.gesture
        } else {
            let g = store.begin_group(Origin::User, &format!("adjust {node} {handle}"));
            self.wheel = Some(Wheel {
                gesture: g,
                last: now,
            });
            g
        };
        let _ = store.set(
            Set::new(
                &node_path(node, param),
                Change::To(Value::Float(value)),
                Origin::User,
            )
            .gesture(gesture),
        );
    }

    /// A click on the canvas (not a drag): adds a point to the path being
    /// drawn (on its first point: closes it), else selects the path point
    /// or the shape under it. Returns the corners of a path to create.
    pub fn click(&mut self, pos: Pos2, m: &Mapping, store: &mut Store) -> Option<Vec<[f64; 2]>> {
        if tool(store) == "path" {
            let mut points = draft(store);
            let at = m.screen_to_image(pos);
            if points.len() >= 3 && m.image_to_screen(points[0]).distance(pos) <= HIT_RADIUS {
                return Some(points);
            }
            // The second click of a double click lands on the first.
            if points
                .last()
                .is_some_and(|p| m.image_to_screen(*p).distance(pos) < 4.0)
            {
                return None;
            }
            if points.len() < helios_module_masks::shape::MAX_PATH_POINTS {
                points.push(at);
                set_draft(store, &points);
            }
            return None;
        }
        if let Some((i, h)) = overlay::hit(&self.overlays, m, pos) {
            let member = self.member(&self.overlays[i].id)?;
            let name = &member.outline.handles[h.index as usize].name;
            if let Some(("corner", n)) = name.split_once('.') {
                put(store, POINT, Value::Int(n.parse().unwrap_or(-1)));
            }
            return None;
        }
        let near = self
            .members
            .iter()
            .map(|mb| (mb, distance_to(&mb.outline.edge, mb.outline.closed, m, pos)))
            .filter(|(_, d)| *d <= HIT_RADIUS)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((mb, _)) = near
            && text(store, SHAPE) != mb.node
        {
            let node = mb.node.clone();
            select(store, &node);
        }
        None
    }

    /// A double click the editor took this frame (on the selected path's
    /// edge), in image pixels.
    pub fn take_double(&mut self) -> Option<[f64; 2]> {
        self.double.borrow_mut().taken.take()
    }

    /// Before the viewport is drawn: whether the pointer profile's double
    /// click on the image leaves the view.
    pub fn set_double_click_leaves(&mut self, leaves: bool) {
        self.double.borrow_mut().leaves = leaves;
    }

    /// Whether a double click the editor did not take asked to leave the
    /// view since the last call.
    pub fn take_left(&mut self) -> bool {
        std::mem::take(&mut self.double.borrow_mut().left)
    }

    /// Whether a drag is under way.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

fn write_drag(d: &Drag, at: [f64; 2], store: &mut Store) {
    match drag_writes(&d.module, &d.handle, &d.start, at) {
        Ok(writes) => {
            for (param, value) in writes {
                let _ = store.set(
                    Set::new(&node_path(&d.node, &param), Change::To(value), Origin::User)
                        .gesture(d.gesture),
                );
            }
        }
        Err(e) => eprintln!("helios-gui: masks: {e}"),
    }
}

/// A path's points with point `n` made smooth (its handles along the
/// line between its neighbours, a third of the way) or sharp (on its
/// corner).
pub fn smoothed(params: &Params, n: usize, smooth: bool) -> Result<Value, String> {
    let values = params.get("points").and_then(Value::as_list).unwrap_or(&[]);
    let mut points = path_points(values)?;
    let k = points.len();
    if n >= k {
        return Err(format!("no point {n} on the path"));
    }
    let mut p = points[n];
    if smooth {
        let prev = points[(n + k - 1) % k].corner;
        let next = points[(n + 1) % k].corner;
        let t = scale(sub(next, prev), 1.0 / 6.0);
        p.before = sub(p.corner, t);
        p.after = add(p.corner, t);
    } else {
        p.before = p.corner;
        p.after = p.corner;
    }
    points[n] = p;
    Ok(path_values(&points))
}

/// The path's points with point `n` removed (a path keeps 3 at least).
pub fn removed(params: &Params, n: usize) -> Result<Value, String> {
    let values = params.get("points").and_then(Value::as_list).unwrap_or(&[]);
    let mut points = path_points(values)?;
    if n >= points.len() {
        return Err(format!("no point {n} on the path"));
    }
    if points.len() <= 3 {
        return Err("a path keeps three points at least".into());
    }
    points.remove(n);
    Ok(path_values(&points))
}

/// The path's points with one added on its edge nearest `at`; its index.
pub fn inserted(params: &Params, at: [f64; 2]) -> Result<(Value, usize), String> {
    let values = params.get("points").and_then(Value::as_list).unwrap_or(&[]);
    let points = path_points(values)?;
    if points.len() >= helios_module_masks::shape::MAX_PATH_POINTS {
        return Err(format!(
            "a path holds {} points at most",
            helios_module_masks::shape::MAX_PATH_POINTS
        ));
    }
    let (i, t, _) = nearest(&points, at).ok_or("the path has no points")?;
    let (points, n) = split(&points, i, t);
    Ok((path_values(&points), n))
}

fn dashed(painter: &Painter, pts: &[Pos2], closed: bool, stroke: Stroke) {
    let mut pts = pts.to_vec();
    if closed && let Some(first) = pts.first().copied() {
        pts.push(first);
    }
    painter.extend(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0));
}

/// A path point's control lines, from its corner to each Bézier handle:
/// under the overlays, so the handles are drawn over them.
fn paint_controls(painter: &Painter, m: &Mapping, f: &Faint) {
    let t = ViewportTokens::of(painter.ctx());
    let s = |p: &[f64; 2]| m.image_to_screen(*p);
    // Widths in whole pixels of the window drawn in, so lines stay crisp
    // at every scale (egui-pop's `scale::snap`).
    let w = |points: f32| egui_pop::core::scale::snap(points, m.ppp);
    for (corner, handle) in &f.controls {
        painter.line_segment([s(corner), s(handle)], Stroke::new(w(1.0), t.guide));
    }
}

/// The path being drawn: its corners so far, and a dashed line on to the
/// pointer.
fn paint_draft(painter: &Painter, m: &Mapping, f: &Faint) {
    let t = ViewportTokens::of(painter.ctx());
    let s = |p: &[f64; 2]| m.image_to_screen(*p);
    let w = |points: f32| egui_pop::core::scale::snap(points, m.ppp);
    if !f.draft.is_empty() {
        let mut pts: Vec<Pos2> = f.draft.iter().map(s).collect();
        let stroke = Stroke::new(w(1.5), t.guide);
        painter.add(egui::Shape::line(pts.clone(), stroke));
        if let Some(p) = f.pointer {
            pts.push(p);
            let n = pts.len();
            dashed(painter, &pts[n - 2..], false, Stroke::new(w(1.0), t.guide));
        }
        for (i, p) in f.draft.iter().enumerate() {
            let kind = if i == 0 {
                HandleKind::Move
            } else {
                HandleKind::Point
            };
            overlay::paint_handle(painter, s(p), kind, overlay::HandleState::Rest, &t);
        }
    }
}

/// Adds a shape node to a mask: a free port of its `mask_combine` takes
/// it. Returns the new node's id and name.
pub fn add_node(
    graph: &mut Graph,
    mask: &str,
    module: &str,
    params: &Params,
) -> Result<(NodeId, String), String> {
    add_member(graph, mask, module, params, None)
}

/// Adds a member node to a mask, a shape or a parametric member reading
/// `input` (the image the masked module reads): a free port of its
/// `mask_combine` takes it. Returns the new node's id and name.
pub fn add_member(
    graph: &mut Graph,
    mask: &str,
    module: &str,
    params: &Params,
    input: Option<(NodeId, String)>,
) -> Result<(NodeId, String), String> {
    let e = |e: helios_core::Error| e.to_string();
    let module_ref =
        module_ref(module).ok_or_else(|| format!("`{module}` is not a member a mask adds"))?;
    let combine = graph
        .find(mask)
        .ok_or_else(|| format!("no mask `{mask}` in the image's network"))?;
    let manifest = graph
        .manifest(combine)
        .map_err(e)?
        .ok_or_else(|| format!("`{mask}` is not a mask"))?;
    let free = graph
        .input_nodes(combine)
        .map_err(e)?
        .iter()
        .position(Option::is_none)
        .ok_or_else(|| {
            format!(
                "the mask `{mask}` holds {} members already",
                manifest.inputs.len()
            )
        })?;
    let port = manifest.inputs[free].name.clone();
    let kind = KINDS.iter().find(|(_, m)| *m == module).map_or(
        module.strip_prefix("mask_").unwrap_or("member"),
        |(k, _)| *k,
    );
    // One more name than there are nodes: one of them is free.
    let name = (1..=graph.ids().count() + 1)
        .map(|n| {
            if n == 1 {
                kind.to_string()
            } else {
                format!("{kind}{n}")
            }
        })
        .find(|n| graph.find(n).is_none())
        .expect("a free name");
    let reads = match module_ref.manifest().inputs.first() {
        Some(p) => Some((
            p.name.clone(),
            input.ok_or_else(|| format!("`{module}` needs an image to read"))?,
        )),
        None => None,
    };
    let id = graph.add_module(&name, module_ref, params).map_err(e)?;
    if let Some((reads, (src, out))) = reads {
        graph.connect(src, &out, id, &reads).map_err(e)?;
    }
    graph.connect(id, "mask", combine, &port).map_err(e)?;
    Ok((id, name))
}
