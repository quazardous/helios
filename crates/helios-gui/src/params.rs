//! Parameter widgets, generated from value descriptors and bound to the
//! value store (see `docs/widgets.md`). A widget never holds its value: it
//! reads the store every frame and writes through it, as the user.
//!
//! - A float or an int is a slider over the soft range: a drag is one
//!   gesture (one undo step, agents kept off meanwhile), the wheel and the
//!   arrows step (Shift coarse, Ctrl finer), a double click resets, Enter
//!   types a value (units allowed, `,` or `.`); a typed value past the soft
//!   range is forced up to the hard one, and says so.
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

use egui::{Key, Sense, Ui, WidgetInfo, WidgetType};
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

/// What the widgets remember between frames: never values.
#[derive(Default)]
pub struct Params {
    /// The sequence number of each path when last seen, and when another
    /// origin moved it.
    seen: BTreeMap<String, (u64, Option<f64>)>,
    gesture: Option<(String, GestureId)>,
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
            edit.request_focus();
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
                            request(ui.ctx(), Request::Notify(Level::Info, n));
                        }
                    }
                } else {
                    request(
                        ui.ctx(),
                        Request::Notify(
                            Level::Warning,
                            format!("{}: `{typed}` is not a number", d.label),
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
        if response.has_focus() {
            if let Some(change) = ui.input(steps) {
                write(&mut s, path, change, None, false);
            }
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                self.typing = Some((
                    path.into(),
                    format!("{:.*}", d.precision as usize, v * d.display_scale),
                    true,
                ));
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.5 {
                let n = scroll.signum() as i64;
                let coarse = ui.input(|i| i.modifiers.shift);
                write(
                    &mut s,
                    path,
                    if coarse {
                        Change::Coarse(n)
                    } else {
                        Change::Step(n)
                    },
                    None,
                    false,
                );
            }
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
            painter.rect_stroke(
                rect,
                visuals.widgets.inactive.corner_radius,
                visuals.selection.stroke,
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
            "Drag, wheel or arrows to change (Shift: finer drag, coarser steps); double click resets; Enter types a value"
        ))
    }

    /// The Parameters panel: the selected node's values, or every node's,
    /// grouped by node in the manifest's order.
    pub fn panel_ui(&mut self, ui: &mut Ui, store: &Mutex<Store>, selection: Option<&str>) {
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
