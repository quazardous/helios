//! The graph editor: the network the viewport shows, drawn with egui
//! shapes. The graph stays the one source of truth: this widget reads it,
//! and every edit is a command (`graph.connect`, `graph.node.add`, …) run
//! by the shell, so keys, the palette and agents do the same.
//!
//! - **Semantic zoom.** Zoomed out, the overview shows the principal nodes
//!   the core proposes (`Graph::layout`), each with a badge counting the
//!   nodes stacked under it (a click unfolds them); in between, every node
//!   with its ports and wires; zoomed in, also the parameters that differ
//!   from their defaults.
//! - **Headers** are the normalized node status (`NodeStatus`): family
//!   colour, state, where it ran.
//! - **The view** (zoom, pan) and the nodes the user moved are values
//!   (`view/graph/*`); the selection is `view/graph/selection`, which the
//!   Parameters panel follows.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use helios_core::{Family, Graph, NodeId, NodeState, NodeStatus, Value};
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, GestureId, Set, Store};
use serde_json::json;

use crate::dialogs::{MenuItem, Request, context_menu, request};

pub const ZOOM: &str = "view/graph/zoom";
pub const PAN_X: &str = "view/graph/pan_x";
pub const PAN_Y: &str = "view/graph/pan_y";

/// Below this zoom, the overview; above `DETAIL_PARAMS`, parameters too.
pub const OVERVIEW: f64 = 0.6;
pub const DETAIL_PARAMS: f64 = 1.6;

const NODE_W: f32 = 170.0;
const HEADER_H: f32 = 26.0;
const PORT_H: f32 = 18.0;
const COLUMN: f32 = 230.0;
const ROW: f32 = 130.0;

fn position_path(node: &str, axis: &str) -> String {
    format!("view/graph/node/{node}/{axis}")
}

/// Declares the editor's view values.
pub fn declare(store: &mut Store) {
    store.declare(ZOOM, Descriptor::float("graph zoom", 0.2, 4.0, 1.0));
    store.declare(PAN_X, Descriptor::float("graph pan x", -1e6, 1e6, 0.0));
    store.declare(PAN_Y, Descriptor::float("graph pan y", -1e6, 1e6, 0.0));
}

#[derive(Default)]
pub struct GraphView {
    /// A wire being drawn from an output: node, port.
    wiring: Option<(String, String)>,
    /// A node being moved, with its gesture.
    moving: Option<(String, GestureId)>,
    panning: Option<GestureId>,
    /// Stacks the user unfolded in the overview.
    unfolded: BTreeSet<String>,
}

/// One node as drawn this frame.
struct Drawn {
    name: String,
    rect: Rect,
    inputs: Vec<(String, Pos2)>,
    outputs: Vec<(String, Pos2)>,
}

fn float(store: &Store, path: &str) -> Option<f64> {
    store.get(path).and_then(|e| e.value.as_f64())
}

fn set(store: &mut Store, path: &str, v: f64, gesture: Option<GestureId>) {
    if store.get(path).is_none() {
        store.declare(path, Descriptor::float(path, -1e6, 1e6, v));
    }
    let mut s = Set::new(path, Change::To(Value::Float(v)), Origin::User);
    s.gesture = gesture;
    let _ = store.set(s);
}

/// A family's colour, from the theme's tokens.
fn family_color(ui: &Ui, family: Family) -> Color32 {
    let v = ui.visuals();
    match family {
        Family::Tone => v.selection.stroke.color,
        Family::Color => Color32::from_rgb(110, 170, 110).lerp_to_gamma(v.text_color(), 0.15),
        Family::Detail => v.warn_fg_color,
        Family::Geometry => v.hyperlink_color,
        Family::Merge => v.weak_text_color(),
        Family::Mask => v.selection.bg_fill,
        Family::Ai => v.error_fg_color.lerp_to_gamma(v.selection.stroke.color, 0.5),
        Family::Io => v.widgets.inactive.fg_stroke.color,
        Family::Other => v.widgets.noninteractive.bg_stroke.color,
    }
}

fn state_color(ui: &Ui, state: NodeState) -> Color32 {
    let v = ui.visuals();
    match state {
        NodeState::Done => v.selection.stroke.color,
        NodeState::Running => v.hyperlink_color,
        NodeState::Stale => v.warn_fg_color,
        NodeState::Failed => v.error_fg_color,
        NodeState::Pending => v.warn_fg_color.lerp_to_gamma(v.error_fg_color, 0.5),
        NodeState::Idle => v.widgets.noninteractive.bg_stroke.color,
    }
}

impl GraphView {
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        graph: Option<&Graph>,
        statuses: &[NodeStatus],
        store: &Mutex<Store>,
        selection: Option<&str>,
    ) {
        let Some(graph) = graph else {
            ui.weak("No graph: open an image");
            return;
        };
        let (rect, background) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        crate::name_rect(ui.ctx(), "graph.view", rect);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);

        let mut s = store.lock().expect("value store lock");
        let zoom = float(&s, ZOOM).unwrap_or(1.0);
        let pan = vec2(
            float(&s, PAN_X).unwrap_or(0.0) as f32,
            float(&s, PAN_Y).unwrap_or(0.0) as f32,
        );
        // Background: pan by drag (one gesture), zoom by wheel about the
        // pointer, a click clears the selection.
        if background.drag_started() {
            self.panning = Some(s.begin_gesture(Origin::User, "pan the graph"));
        }
        if background.dragged()
            && let Some(g) = self.panning
        {
            let d = background.drag_delta();
            set(&mut s, PAN_X, f64::from(pan.x + d.x), Some(g));
            set(&mut s, PAN_Y, f64::from(pan.y + d.y), Some(g));
        }
        if background.drag_stopped()
            && let Some(g) = self.panning.take()
        {
            s.end(g);
        }
        if background.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let to = (zoom * f64::from((scroll * 0.0015).exp())).clamp(0.2, 4.0);
                let at = background.hover_pos().unwrap_or(rect.center()) - rect.min;
                // The point under the pointer stays put.
                let k = (to / zoom) as f32;
                let new_pan = at - (at - pan) * k;
                let g = s.begin_group(Origin::User, "zoom the graph");
                set(&mut s, ZOOM, to, Some(g));
                set(&mut s, PAN_X, f64::from(new_pan.x), Some(g));
                set(&mut s, PAN_Y, f64::from(new_pan.y), Some(g));
                s.end(g);
            }
        }
        if background.clicked() {
            let _ = s.set(Set::new(
                crate::params::SELECTION,
                Change::To(Value::Text(String::new())),
                Origin::User,
            ));
        }
        if background.double_clicked() {
            request(
                ui.ctx(),
                Request::Command {
                    id: "graph.node.add".into(),
                    args: json!(null),
                },
            );
        }
        let to_screen = |p: Pos2| rect.min + (p.to_vec2() * zoom as f32) + pan;

        // Layout: the core's ranks and groups, moved nodes where the user put them.
        let hints = graph.layout();
        let overview = zoom < OVERVIEW;
        let name = |id: NodeId| graph.name(id).unwrap_or("?").to_string();
        let mut per_rank: BTreeMap<usize, usize> = BTreeMap::new();
        let mut placed: BTreeMap<usize, Pos2> = BTreeMap::new();
        for h in &hints {
            let shown = !overview
                || h.principal
                || self.unfolded.contains(&name(h.group));
            if !shown {
                continue;
            }
            let slot = per_rank.entry(h.rank).or_insert(0);
            let auto = pos2(40.0 + h.rank as f32 * COLUMN, 40.0 + *slot as f32 * ROW);
            *slot += 1;
            let n = name(h.node);
            let at = match (
                float(&s, &position_path(&n, "x")),
                float(&s, &position_path(&n, "y")),
            ) {
                (Some(x), Some(y)) => pos2(x as f32, y as f32),
                _ => auto,
            };
            placed.insert(h.node.0, at);
        }
        let stacked = |head: NodeId| hints.iter().filter(|h| h.group == head && h.node != head).count();
        // Where a hidden node's wires go in the overview: its stack's head.
        let shown_as = |id: NodeId| -> Option<NodeId> {
            if placed.contains_key(&id.0) {
                Some(id)
            } else {
                hints.get(id.0).map(|h| h.group).filter(|g| placed.contains_key(&g.0))
            }
        };

        let status = |id: NodeId| statuses.iter().find(|st| st.node == id);
        let mut drawn: Vec<Drawn> = Vec::new();
        let font = egui::FontId::proportional((13.0 * zoom as f32).clamp(8.0, 22.0));
        let small = egui::FontId::proportional((11.0 * zoom as f32).clamp(7.0, 18.0));
        for (&i, &at) in &placed {
            let id = NodeId(i);
            let n = name(id);
            let manifest = graph.manifest(id).ok().flatten();
            let inputs: Vec<String> = manifest
                .as_ref()
                .map(|m| m.inputs.iter().map(|p| p.name.clone()).collect())
                .unwrap_or_default();
            let outputs = graph.output_names(id).unwrap_or_default();
            let params: Vec<String> = if zoom >= DETAIL_PARAMS {
                s.list(&format!("node/{n}/"))
                    .into_iter()
                    .filter(|(_, e)| e.value != e.descriptor.default)
                    .map(|(p, e)| {
                        let param = p.rsplit('/').next().unwrap_or(p);
                        format!("{param}: {}", crate::params::shown(&e.descriptor, &e.value))
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let ports = if overview { 0 } else { inputs.len().max(outputs.len()) };
            let h = HEADER_H + ports as f32 * PORT_H + params.len() as f32 * PORT_H + 6.0;
            let r = Rect::from_min_size(to_screen(at), vec2(NODE_W, h) * zoom as f32);
            let st = status(id);
            let family = manifest.as_ref().map_or(Family::Io, |m| m.family);
            let visuals = ui.visuals().clone();
            let selected = selection == Some(n.as_str());
            painter.rect_filled(r, 6.0, visuals.window_fill);
            let ring = st.map_or(visuals.widgets.noninteractive.bg_stroke.color, |st| state_color(ui, st.state));
            painter.rect_stroke(
                r,
                6.0,
                Stroke::new(if selected { 2.5 } else { 1.5 }, if selected { visuals.selection.stroke.color } else { ring }),
                egui::StrokeKind::Inside,
            );
            // The family's colour, a bar on the left of the header.
            let header = Rect::from_min_size(r.min, vec2(r.width(), HEADER_H * zoom as f32));
            painter.rect_filled(
                Rect::from_min_size(header.min, vec2(4.0, header.height())),
                2.0,
                family_color(ui, family),
            );
            painter.text(
                header.left_center() + vec2(10.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &n,
                font.clone(),
                visuals.strong_text_color(),
            );
            if !overview && let Some(st) = st {
                painter.text(
                    header.right_center() - vec2(8.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    st.runs_on,
                    small.clone(),
                    visuals.weak_text_color(),
                );
            }
            // A node: click selects, drag moves (one gesture).
            let id_ui = egui::Id::new(("graph.node", &n));
            let resp = ui.interact(header, id_ui, Sense::click_and_drag());
            crate::name_rect(ui.ctx(), &format!("graph.node:{n}"), r);
            if resp.clicked() || resp.drag_started() {
                let _ = s.set(Set::new(
                    crate::params::SELECTION,
                    Change::To(Value::Text(n.clone())),
                    Origin::User,
                ));
            }
            if resp.drag_started() {
                self.moving = Some((n.clone(), s.begin_gesture(Origin::User, &format!("move {n}"))));
            }
            if resp.dragged()
                && let Some((m, g)) = &self.moving
                && *m == n
            {
                let d = resp.drag_delta() / zoom as f32;
                set(&mut s, &position_path(&n, "x"), f64::from(at.x + d.x), Some(*g));
                set(&mut s, &position_path(&n, "y"), f64::from(at.y + d.y), Some(*g));
            }
            if resp.drag_stopped()
                && let Some((_, g)) = self.moving.take()
            {
                s.end(g);
            }
            context_menu(
                &resp,
                vec![
                    MenuItem::Command {
                        id: "graph.node.add".into(),
                        args: json!({ "after": n }),
                    },
                    MenuItem::Command {
                        id: "graph.node.remove".into(),
                        args: json!({ "name": n }),
                    },
                ],
            );
            // The overview: a badge counts the stacked nodes; a click unfolds.
            if overview {
                let count = stacked(id);
                if count > 0 {
                    let c = r.right_top() + vec2(-4.0, 4.0);
                    let badge = Rect::from_center_size(c, vec2(22.0, 16.0));
                    painter.rect_filled(badge, 8.0, visuals.selection.bg_fill);
                    painter.text(
                        badge.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("+{count}"),
                        small.clone(),
                        visuals.strong_text_color(),
                    );
                    crate::name_rect(ui.ctx(), &format!("graph.badge:{n}"), badge);
                    let b = ui.interact(badge, id_ui.with("badge"), Sense::click());
                    if b.on_hover_text(format!("{count} nodes stacked under {n}: click to unfold")).clicked() {
                        if !self.unfolded.insert(n.clone()) {
                            self.unfolded.remove(&n);
                        }
                    }
                }
            }
            // Ports.
            let mut ins = Vec::new();
            let mut outs = Vec::new();
            if !overview {
                for (k, p) in inputs.iter().enumerate() {
                    let y = r.top() + (HEADER_H + (k as f32 + 0.5) * PORT_H) * zoom as f32;
                    let at = pos2(r.left(), y);
                    painter.circle_filled(at, 4.5, visuals.text_color());
                    painter.text(at + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, p, small.clone(), visuals.weak_text_color());
                    crate::name_rect(ui.ctx(), &format!("graph.port:{n}:in:{p}"), Rect::from_center_size(at, Vec2::splat(12.0)));
                    ins.push((p.clone(), at));
                }
                for (k, p) in outputs.iter().enumerate() {
                    let y = r.top() + (HEADER_H + (k as f32 + 0.5) * PORT_H) * zoom as f32;
                    let at = pos2(r.right(), y);
                    painter.circle_filled(at, 4.5, visuals.text_color());
                    painter.text(at - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, p, small.clone(), visuals.weak_text_color());
                    let port_rect = Rect::from_center_size(at, Vec2::splat(14.0));
                    crate::name_rect(ui.ctx(), &format!("graph.port:{n}:out:{p}"), port_rect);
                    let pr = ui.interact(port_rect, id_ui.with(("out", p)), Sense::drag());
                    if pr.drag_started() {
                        self.wiring = Some((n.clone(), p.clone()));
                    }
                    outs.push((p.clone(), at));
                }
                for (k, line) in params.iter().enumerate() {
                    let y = r.top() + (HEADER_H + (ports + k) as f32 * PORT_H + PORT_H * 0.5) * zoom as f32;
                    painter.text(pos2(r.left() + 10.0, y), egui::Align2::LEFT_CENTER, line, small.clone(), visuals.text_color());
                }
            }
            drawn.push(Drawn {
                name: n,
                rect: r,
                inputs: ins,
                outputs: outs,
            });
        }

        // Wires: between ports in detail, between stacks in the overview.
        let wire = |a: Pos2, b: Pos2, color: Color32| {
            let dx = ((b.x - a.x).abs() * 0.5).max(30.0);
            let curve = egui::epaint::CubicBezierShape::from_points_stroke(
                [a, a + vec2(dx, 0.0), b - vec2(dx, 0.0), b],
                false,
                Color32::TRANSPARENT,
                Stroke::new(1.5, color),
            );
            painter.add(curve);
        };
        let ink = ui.visuals().weak_text_color();
        for to in graph.ids() {
            let Ok(sources) = graph.input_ports(to) else { continue };
            for (k, src) in sources.iter().enumerate() {
                let Some((from, port)) = src else { continue };
                if overview {
                    let (Some(a), Some(b)) = (shown_as(*from), shown_as(to)) else { continue };
                    if a == b {
                        continue;
                    }
                    let (Some(ra), Some(rb)) = (
                        drawn.iter().find(|d| d.name == name(a)).map(|d| d.rect),
                        drawn.iter().find(|d| d.name == name(b)).map(|d| d.rect),
                    ) else {
                        continue;
                    };
                    wire(ra.right_center(), rb.left_center(), ink);
                } else {
                    let a = drawn
                        .iter()
                        .find(|d| d.name == name(*from))
                        .and_then(|d| d.outputs.iter().find(|(p, _)| p == port).map(|(_, at)| *at));
                    let b = drawn.iter().find(|d| d.name == name(to)).and_then(|d| d.inputs.get(k).map(|(_, at)| *at));
                    if let (Some(a), Some(b)) = (a, b) {
                        wire(a, b, ink);
                    }
                }
            }
        }
        // A wire being drawn; dropped on an input, it connects.
        if let Some((from, port)) = self.wiring.clone() {
            let start = drawn
                .iter()
                .find(|d| d.name == from)
                .and_then(|d| d.outputs.iter().find(|(p, _)| *p == port).map(|(_, at)| *at));
            let pointer = ui.input(|i| i.pointer.latest_pos());
            if let (Some(a), Some(b)) = (start, pointer) {
                wire(a, b, ui.visuals().selection.stroke.color);
            }
            if ui.input(|i| i.pointer.any_released()) {
                self.wiring = None;
                let target = pointer.and_then(|p| {
                    drawn.iter().find_map(|d| {
                        d.inputs
                            .iter()
                            .find(|(_, at)| at.distance(p) < 12.0)
                            .map(|(inp, _)| (d.name.clone(), inp.clone()))
                    })
                });
                if let Some((to, to_port)) = target {
                    request(
                        ui.ctx(),
                        Request::Command {
                            id: "graph.connect".into(),
                            args: json!({ "from": from, "from_port": port, "to": to, "to_port": to_port }),
                        },
                    );
                }
            }
        }
        drop(s);
    }
}
