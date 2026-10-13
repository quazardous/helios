//! Parameter widgets, generated from value descriptors and bound to the
//! value store (see `docs/widgets.md`). A widget never holds its value: it
//! reads the store every frame and writes through it, as the user.
//!
//! - A float or an int is a slider over the soft range: a drag is one
//!   gesture (one undo step, agents kept off meanwhile), the arrows step
//!   (Shift coarse), a double click resets, Enter types a value (units
//!   allowed, `,` or `.`); a typed value past the soft range is forced up
//!   to the hard one, and says so. The wheel scrolls the panel: a slider
//!   takes it only with the focus or while Alt is held. Steps in a row (a
//!   held arrow, a turned wheel) are one undo step.
//! - A bool is a toggle, a choice a list (the arrows step it), a list of
//!   numbers a row each, a text a field.
//! - Marks: changed from its default (a dot that resets), held by someone
//!   else's gesture, just moved by another origin (who, in the tooltip),
//!   outside the soft range.
//! - The widget's id comes from the path, and so does its picture's name
//!   (`param:<path>`); the context menu resets, copies the path or the
//!   value: commands, like everything else.

use std::collections::BTreeMap;
use std::sync::Mutex;

use egui::{Key, Modifiers, Sense, Ui, WidgetInfo, WidgetType};
use helios_core::Value;
use helios_shell::commands::Origin;
use helios_values::{Change, Descriptor, Entry, GestureId, Set, Store, Type};
use serde_json::json;

use crate::dialogs::{Level, Request, request};

/// The node whose parameters the panel shows (a node name; empty or absent:
/// every node). The graph editor sets it.
pub const SELECTION: &str = "view/graph/selection";

/// How long a change by someone else stays marked, in seconds.
const MOVED_SECONDS: f64 = 2.0;

/// Steps further apart than this, in seconds, are undo steps of their own.
const STEPS_GAP: f64 = 0.5;

/// What the widgets remember between frames: never values.
#[derive(Default)]
pub struct Params {
    /// The sequence number of each path when last seen, and when another
    /// origin moved it.
    seen: BTreeMap<String, (u64, Option<f64>)>,
    gesture: Option<(String, GestureId)>,
    /// Steps in a row on one path (keys, the wheel): one group, and when
    /// its last step was.
    steps: Option<(String, GestureId, f64)>,
    /// A value being typed: its path, the text, and whether it was just
    /// opened (its text then starts selected).
    typing: Option<(String, String, bool)>,
    /// The application, for the parameters' context menu.
    app: Option<egui_pop::api::AppHandle>,
}

/// Parses a typed number: units after it are ignored, a comma is a point.
pub fn parse_typed(text: &str) -> Option<f64> {
    let t = text.trim().replace(',', ".");
    let end = t
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+'))))
        .map_or(t.len(), |(i, _)| i);
    t[..end].parse().ok()
}

/// A value as shown: display scale, precision, unit.
pub fn shown(d: &Descriptor, v: &Value) -> String {
    match v {
        Value::Float(x) => {
            let s = format!("{:.*}", d.precision as usize, x * d.display_scale);
            match &d.unit {
                Some(u) => format!("{s} {u}"),
                None => s,
            }
        }
        Value::Int(x) => match &d.unit {
            Some(u) => format!("{x} {u}"),
            None => x.to_string(),
        },
        Value::Bool(b) => if *b { "on" } else { "off" }.into(),
        Value::Choice(c) | Value::Text(c) => c.clone(),
        Value::List(l) => format!("{l:?}"),
        Value::Ints(l) => format!("{l:?}"),
    }
}

fn write(
    store: &mut Store,
    path: &str,
    change: Change,
    gesture: Option<GestureId>,
    force: bool,
) -> Option<helios_values::Applied> {
    let mut set = Set::new(path, change, Origin::User);
    set.gesture = gesture;
    set.force = force;
    store.set(set).ok()
}

impl Params {
    /// A step of a slider (an arrow, a wheel notch), in the group of the
    /// steps just before it on the same path.
    fn step(&mut self, s: &mut Store, path: &str, change: Change, label: &str, now: f64) {
        let group = match &mut self.steps {
            Some((p, g, last)) if p == path && now - *last <= STEPS_GAP => {
                *last = now;
                *g
            }
            _ => {
                if let Some((_, g, _)) = self.steps.take() {
                    s.end(g);
                }
                let g = s.begin_group(Origin::User, &format!("step {label}"));
                self.steps = Some((path.into(), g, now));
                g
            }
        };
        write(s, path, change, Some(group), false);
    }

    /// Ends a row of steps once it has paused.
    fn end_steps(&mut self, store: &Mutex<Store>, now: f64) {
        if self
            .steps
            .as_ref()
            .is_some_and(|(_, _, last)| now - last > STEPS_GAP)
            && let Some((_, g, _)) = self.steps.take()
        {
            store.lock().expect("value store lock").end(g);
        }
    }

    /// The application whose declared context menu the widgets open.
    pub fn set_app(&mut self, app: egui_pop::api::AppHandle) {
        self.app = Some(app);
    }

    /// The marks of a path this frame: (moved by, at what time).
    fn moved(&mut self, path: &str, e: &Entry, now: f64) -> Option<Origin> {
        let seen = self.seen.entry(path.into()).or_insert((e.seq, None));
        if seen.0 != e.seq {
            let by_other = e.origin.as_ref().is_some_and(|o| *o != Origin::User);
            seen.0 = e.seq;
            seen.1 = by_other.then_some(now);
        }
        match seen.1 {
            Some(at) if now - at < MOVED_SECONDS => e.origin.clone(),
            _ => None,
        }
    }

    /// One parameter's row: its widget and marks.
    pub fn param_ui(&mut self, ui: &mut Ui, store: &Mutex<Store>, path: &str) {
        let now = ui.input(|i| i.time);
        let s = store.lock().expect("value store lock");
        let Some(entry) = s.get(path).cloned() else {
            return;
        };
        let held = s.held_by(path).filter(|o| **o != Origin::User).cloned();
        drop(s);
        let moved = self.moved(path, &entry, now);
        if moved.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(MOVED_SECONDS));
        }
        let d = &entry.descriptor;
        ui.push_id(path, |ui| {
            ui.horizontal(|ui| {
                // Changed from its default: a dot that resets.
                let changed = entry.value != d.default;
                let (dot, dot_response) =
                    ui.allocate_exact_size(egui::vec2(10.0, 18.0), Sense::click());
                if changed {
                    ui.painter().circle_filled(
                        dot.center(),
                        3.0,
                        ui.visuals().selection.stroke.color,
                    );
                    let r = dot_response.on_hover_text("Changed from its default: click to reset");
                    if r.clicked() {
                        let mut s = store.lock().expect("value store lock");
                        write(&mut s, path, Change::Reset, None, false);
                    }
                    crate::name_rect(ui.ctx(), &format!("param:{path}:changed"), dot);
                }
                let response = match (d.ty, &entry.value) {
                    (Type::Float | Type::Int, _) => self.slider(ui, store, path, &entry),
                    (Type::Bool, Value::Bool(b)) => {
                        let mut on = *b;
                        let r = ui.checkbox(&mut on, &d.label);
                        if r.changed() {
                            let mut s = store.lock().expect("value store lock");
                            write(&mut s, path, Change::To(Value::Bool(on)), None, false);
                        }
                        r
                    }
                    (Type::Choice, Value::Choice(c)) => {
                        let mut picked = c.clone();
                        let r = egui::ComboBox::from_id_salt(path)
                            .selected_text(&picked)
                            .show_ui(ui, |ui| {
                                for o in &d.choices {
                                    ui.selectable_value(&mut picked, o.clone(), o);
                                }
                            })
                            .response;
                        ui.label(&d.label);
                        if picked != *c {
                            let mut s = store.lock().expect("value store lock");
                            write(&mut s, path, Change::To(Value::Choice(picked)), None, false);
                        }
                        r
                    }
                    (Type::Text, Value::Text(t)) => {
                        let mut text = t.clone();
                        ui.label(&d.label);
                        let r = ui.text_edit_singleline(&mut text);
                        if r.lost_focus() && text != *t {
                            let mut s = store.lock().expect("value store lock");
                            write(&mut s, path, Change::To(Value::Text(text)), None, false);
                        }
                        r
                    }
                    (_, v) => {
                        ui.label(&d.label);
                        ui.weak(shown(d, v))
                    }
                };
                crate::name_rect(ui.ctx(), &format!("param:{path}"), response.rect);
                if let Some(app) = &self.app {
                    egui_pop::api::menus::context_menu_with(
                        &response,
                        app,
                        &crate::plugins::PARAM,
                        json!({ "path": path }),
                    );
                }
                if let Some(by) = &held {
                    let r = mark(ui, ui.visuals().warn_fg_color, false);
                    r.on_hover_text(format!("Held by {}", origin_name(by)));
                }
                if let Some(by) = &moved {
                    let r = mark(ui, ui.visuals().selection.stroke.color, true);
                    crate::name_rect(ui.ctx(), &format!("param:{path}:moved"), r.rect);
                    r.on_hover_text(format!("Moved by {}", origin_name(by)));
                }
            });
        });
    }

    /// A slider for a number, over the soft range.
    fn slider(
        &mut self,
        ui: &mut Ui,
        store: &Mutex<Store>,
        path: &str,
        entry: &Entry,
    ) -> egui::Response {
        let d = &entry.descriptor;
        let v = entry.value.as_f64().unwrap_or(0.0);
        let (lo, hi) = d.soft.or(d.hard).unwrap_or((0.0, 1.0));
        // Room is kept on the right for the marks.
        let width = (ui.available_width() - 28.0).clamp(80.0, 320.0);
        let height = ui.spacing().interact_size.y + 4.0;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), Sense::click_and_drag());
        let id = response.id;
        let visuals = ui.visuals().clone();
        let outside = v < lo || v > hi;

        // Typing a value.
        if self.typing.as_ref().is_some_and(|(p, _, _)| p == path) {
            let (_, text, fresh) = self.typing.as_mut().expect("typing");
            let edit_id = id.with("typed");
            let edit = ui.put(rect, egui::TextEdit::singleline(text).id(edit_id));
            // The focus is taken once, when typing opens: a click elsewhere
            // takes it away and ends the typing, the value untouched.
            let opened = *fresh;
            if opened {
                edit.request_focus();
            } else if !edit.has_focus() && !edit.lost_focus() {
                self.typing = None;
                return edit;
            }
            if std::mem::take(fresh) {
                // Typing replaces the value shown.
                let mut state =
                    egui::text_edit::TextEditState::load(ui.ctx(), edit_id).unwrap_or_default();
                let all = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(text.chars().count()),
                );
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), edit_id);
            }
            let (enter, escape) =
                ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if enter {
                let typed = self.typing.take().map(|(_, t, _)| t).unwrap_or_default();
                if let Some(x) = parse_typed(&typed) {
                    let value = x / d.display_scale;
                    let force = value < lo || value > hi;
                    let mut s = store.lock().expect("value store lock");
                    let change = Change::To(match d.ty {
                        Type::Int => Value::Int(value.round() as i64),
                        _ => Value::Float(value),
                    });
                    if let Some(a) = write(&mut s, path, change, None, force) {
                        let note = match (force, a.clamped) {
                            (_, helios_values::Clamped::Hard) => {
                                Some(format!("{}: held to {}", d.label, shown(d, &a.value)))
                            }
                            (true, _) => Some(format!("{}: past its usual range", d.label)),
                            _ => None,
                        };
                        if let Some(n) = note {
                            request(ui.ctx(), Request::Notify(Level::Info, n.into()));
                        }
                    }
                } else {
                    request(
                        ui.ctx(),
                        Request::Notify(
                            Level::Warning,
                            format!("{}: `{typed}` is not a number", d.label).into(),
                        ),
                    );
                }
            } else if escape {
                self.typing = None;
            }
            return edit;
        }

        // Dragging: one gesture.
        let mut s = store.lock().expect("value store lock");
        if response.drag_started() {
            let g = s.begin_gesture(Origin::User, &format!("drag {}", d.label));
            self.gesture = Some((path.into(), g));
        }
        let gesture = self
            .gesture
            .as_ref()
            .filter(|(p, _)| p == path)
            .map(|(_, g)| *g);
        if response.dragged() && gesture.is_some() {
            let fine = ui.input(|i| if i.modifiers.shift { 0.1 } else { 1.0 });
            let delta = f64::from(response.drag_delta().x / rect.width()) * (hi - lo) * fine;
            if delta != 0.0 {
                let to = v + delta;
                let change = Change::To(match d.ty {
                    Type::Int => Value::Int(to.round() as i64),
                    _ => Value::Float(to),
                });
                write(&mut s, path, change, gesture, false);
            }
        }
        if response.drag_stopped()
            && let Some((_, g)) = self.gesture.take()
        {
            s.end(g);
        }
        if response.double_clicked() {
            write(&mut s, path, Change::Reset, None, false);
        } else if response.clicked() {
            response.request_focus();
        }
        // Wheel and arrows: steps.
        let steps = |i: &egui::InputState| -> Option<Change> {
            let n = if i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowUp) {
                1
            } else if i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowDown) {
                -1
            } else {
                0
            };
            (n != 0).then_some({
                if i.modifiers.shift {
                    Change::Coarse(n)
                } else {
                    Change::Step(n)
                }
            })
        };
        let now = ui.input(|i| i.time);
        if response.has_focus() {
            if let Some(change) = ui.input(steps) {
                self.step(&mut s, path, change, &d.label, now);
            }
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                self.typing = Some((
                    path.into(),
                    format!("{:.*}", d.precision as usize, v * d.display_scale),
                    true,
                ));
            }
        }
        // The wheel scrolls the panel, unless the slider has the focus or
        // Alt is held over it: then each notch (a touchpad's stream as one)
        // steps it and the panel stays.
        let wheel_held = ui.input(|i| i.modifiers.matches_logically(Modifiers::ALT));
        if response.hovered() && (response.has_focus() || wheel_held) {
            let (wheel, coarse) = ui.input(|i| {
                let sum: f32 = i
                    .events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::MouseWheel { delta, .. } => Some(delta.y + delta.x),
                        _ => None,
                    })
                    .sum();
                (sum, i.modifiers.shift)
            });
            if wheel != 0.0 {
                let n = if wheel > 0.0 { 1 } else { -1 };
                let change = if coarse {
                    Change::Coarse(n)
                } else {
                    Change::Step(n)
                };
                self.step(&mut s, path, change, &d.label, now);
            }
            // egui spreads a notch's scroll over a few frames: what is left
            // of it is eaten too, so the panel stays.
            ui.input_mut(|i| {
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                i.events
                    .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
            });
        }
        drop(s);

        // Paint: the track, the filled part, the label and the value.
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            visuals.widgets.inactive.corner_radius,
            visuals.widgets.inactive.bg_fill,
        );
        let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
        let fill = if outside {
            visuals.warn_fg_color.gamma_multiply(0.5)
        } else {
            visuals.selection.bg_fill
        };
        let filled = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.left() + rect.width() * t, rect.bottom()),
        );
        painter.rect_filled(filled, visuals.widgets.inactive.corner_radius, fill);
        if response.has_focus() {
            // Snapped to whole pixels at this window's scale.
            let mut ring = visuals.selection.stroke;
            ring.width = egui_pop::core::scale::stroke(ui.ctx(), ring.width);
            painter.rect_stroke(
                rect,
                visuals.widgets.inactive.corner_radius,
                ring,
                egui::StrokeKind::Inside,
            );
        }
        let text = visuals.text_color();
        let font = egui::TextStyle::Body.resolve(ui.style());
        painter.text(
            rect.left_center() + egui::vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            &d.label,
            font.clone(),
            text,
        );
        painter.text(
            rect.right_center() - egui::vec2(6.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            shown(d, &entry.value),
            font,
            if outside { visuals.warn_fg_color } else { text },
        );
        let label = d.label.clone();
        let enabled = ui.is_enabled();
        response.widget_info(|| {
            let mut info = WidgetInfo::slider(enabled, v * d.display_scale, &label);
            info.typ = WidgetType::Slider;
            info
        });
        response.on_hover_text(format!(
            "{}\n{} · default {}\n{}",
            if d.description.is_empty() { &d.label } else { &d.description },
            path,
            shown(d, &d.default),
            "Drag or arrows to change, the wheel once focused or with Alt (Shift: finer drag, coarser steps); double click resets; Enter types a value"
        ))
    }

    /// The Parameters panel: the selected node's values, or every node's,
    /// grouped by node in the manifest's order.
    pub fn panel_ui(&mut self, ui: &mut Ui, store: &Mutex<Store>, selection: Option<&str>) {
        self.end_steps(store, ui.input(|i| i.time));
        let paths: Vec<(String, String)> = {
            let s = store.lock().expect("value store lock");
            let mut entries = s.list("node/");
            // In the order the manifests declare them.
            entries.sort_by_key(|(_, e)| e.order);
            entries
                .into_iter()
                .filter_map(|(p, _)| {
                    let rest = p.strip_prefix("node/")?;
                    let (node, _) = rest.rsplit_once('/')?;
                    Some((node.to_string(), p.to_string()))
                })
                .filter(|(node, _)| selection.is_none_or(|s| s == node))
                .collect()
        };
        if paths.is_empty() {
            ui.weak("No parameters: nothing is shown yet");
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("helios.params")
            .show(ui, |ui| {
                let mut current = None;
                for (node, path) in &paths {
                    if current != Some(node) {
                        ui.add_space(4.0);
                        ui.strong(node);
                        current = Some(node);
                    }
                    self.param_ui(ui, store, path);
                }
            });
    }
}

/// A mark after a widget, painted (not every font has the glyphs): a
/// diamond, or a dot.
fn mark(ui: &mut Ui, color: egui::Color32, diamond: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(12.0, 18.0), Sense::hover());
    let c = rect.center();
    if diamond {
        let s = 4.5;
        let points = vec![
            c + egui::vec2(0.0, -s),
            c + egui::vec2(s, 0.0),
            c + egui::vec2(0.0, s),
            c + egui::vec2(-s, 0.0),
        ];
        ui.painter().add(egui::Shape::convex_polygon(
            points,
            color,
            egui::Stroke::NONE,
        ));
    } else {
        ui.painter().circle_filled(c, 4.0, color);
    }
    response
}

fn origin_name(o: &Origin) -> String {
    match o {
        Origin::User => "you".into(),
        Origin::Agent(id) => format!("agent {id}"),
        Origin::Script(id) => format!("script {id}"),
        Origin::Midi => "a MIDI controller".into(),
        Origin::Remote(id) => format!("remote {id}"),
    }
}
