//! Parameter widgets, generated from value descriptors and bound to the
//! value store (see `docs/widgets.md`). A widget never holds its value: it
//! reads the store every frame and writes through it, as the user.
//!
//! - A float or an int is egui-pop's slider (`egui_pop::core::slider`),
//!   built from the descriptor: a drag is one gesture (one undo step,
//!   agents kept off meanwhile; Ctrl finer, Shift coarser, Escape gives it
//!   up), the arrows step, a double click resets, Enter types a value or
//!   an expression (units allowed, `,` or `.`); a typed value past the
//!   soft range is forced up to the hard one, and says so. The wheel
//!   scrolls the panel: a slider takes it only with the focus or while Alt
//!   is held. Steps in a row (a held arrow, a turned wheel) are one undo
//!   step. Each finished change runs egui-pop's `slider.change` step
//!   first, so a plugin can veto, snap or follow it.
//! - A bool is a toggle, a choice a list (the arrows step it), a text a
//!   field. A list of numbers folds into a summary (`12 points` for a
//!   path's points, else `32 values`) that opens below its row, a point
//!   a line, so a long list never pushes the next rows out of view.
//! - Marks: changed from its default (a dot that resets), held by someone
//!   else's gesture, just moved by another origin (who, in the tooltip),
//!   outside the soft range.
//! - The widget's id comes from the path, and so does its picture's name
//!   (`param:<path>`); the context menu resets, copies the path or the
//!   value: commands, like everything else.

use std::collections::BTreeMap;
use std::sync::Mutex;

use egui::{Sense, Ui};
use egui_pop::api::sliders;
use egui_pop::core::icon::{Icon, Tone};
use egui_pop::core::slider::{Cause, Slider, SliderOutput};
use helios_core::{Message, Value};
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
    /// Each slider's id and bar as last drawn, to close its typed entry
    /// on a press elsewhere (see [`Params::close_typing`]).
    bars: BTreeMap<String, (egui::Id, egui::Rect)>,
    /// The application, for the parameters' context menu.
    app: Option<egui_pop::api::AppHandle>,
}

/// The journal event a parameter's picker button publishes (`path`): a
/// picker tool answers it by sampling the image.
pub const PICKER_EVENT: &str = "param.picker";

/// Parses a typed number or expression as the sliders do: units after it
/// are ignored, a comma is a decimal separator.
pub fn parse_typed(text: &str) -> Option<f64> {
    egui_pop::core::slider::parse_typed(text)
}

/// A value as shown in English: display scale, precision, unit.
pub fn shown(d: &Descriptor, v: &Value) -> String {
    shown_in("en", d, v)
}

/// A value as shown in a language: display scale, precision with the
/// language's decimal separator, unit. Exposures are signed (`+0.70 EV`),
/// as darktable shows them.
pub fn shown_in(locale: &str, d: &Descriptor, v: &Value) -> String {
    let with_unit = |s: String| match d.unit.as_deref() {
        Some("%") => {
            let gap = egui_pop::i18n::format::separators(locale).percent;
            format!("{s}{}%", if gap.is_empty() { " " } else { gap })
        }
        Some(u) => format!("{s} {u}"),
        None => s,
    };
    match v {
        Value::Float(x) => {
            let x = x * d.display_scale;
            let s = crate::i18n::fixed(locale, x, d.precision as usize);
            let signed = d.unit.as_deref() == Some("EV") && !s.starts_with('-') && x > 0.0;
            with_unit(if signed { format!("+{s}") } else { s })
        }
        Value::Int(x) => with_unit(x.to_string()),
        Value::Bool(b) => if *b { "on" } else { "off" }.into(),
        Value::Choice(c) | Value::Text(c) => c.clone(),
        Value::List(l) => format!("{l:?}"),
        Value::Ints(l) => format!("{l:?}"),
    }
}

/// A node parameter's text in the language of `ctx`: the message `key`
/// (derived from its module and name), else its English.
fn translated(ctx: &egui::Context, key: Option<helios_i18n::MessageId>, english: &str) -> String {
    key.and_then(|k| crate::i18n::of(ctx)?.get(&k.key(), &[]))
        .unwrap_or_else(|| english.to_string())
}

/// The parameter's name: the last part of its path.
fn param_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// A parameter's label in the language of `ctx` (`param-<module>-<name>`'s
/// `.label`), else the descriptor's English.
pub fn label(ctx: &egui::Context, path: &str, d: &Descriptor) -> String {
    let key = d
        .module
        .as_deref()
        .map(|m| helios_i18n::ids::param_label(m, param_name(path)));
    translated(ctx, key, &d.label)
}

/// A parameter's description in the language of `ctx`, else its English.
pub fn description(ctx: &egui::Context, path: &str, d: &Descriptor) -> String {
    let key = d
        .module
        .as_deref()
        .map(|m| helios_i18n::ids::param_description(m, param_name(path)));
    translated(ctx, key, &d.description)
}

/// One value of a choice parameter in the language of `ctx`, else as it is.
pub fn choice(ctx: &egui::Context, path: &str, d: &Descriptor, value: &str) -> String {
    let key = d
        .module
        .as_deref()
        .map(|m| helios_i18n::ids::param_choice(m, param_name(path), value));
    translated(ctx, key, value)
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
        let label = label(ui.ctx(), path, d);
        // A list shown open: its values under its row, so many per line.
        let mut open_list = None;
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
                        let r = ui.checkbox(&mut on, &label);
                        if r.changed() {
                            let mut s = store.lock().expect("value store lock");
                            write(&mut s, path, Change::To(Value::Bool(on)), None, false);
                        }
                        r
                    }
                    (Type::Choice, Value::Choice(c)) => {
                        let mut picked = c.clone();
                        let ctx = ui.ctx().clone();
                        let r = egui::ComboBox::from_id_salt(path)
                            .selected_text(choice(&ctx, path, d, &picked))
                            .show_ui(ui, |ui| {
                                for o in &d.choices {
                                    ui.selectable_value(
                                        &mut picked,
                                        o.clone(),
                                        choice(&ctx, path, d, o),
                                    );
                                }
                            })
                            .response;
                        ui.label(&label);
                        if picked != *c {
                            let mut s = store.lock().expect("value store lock");
                            write(&mut s, path, Change::To(Value::Choice(picked)), None, false);
                        }
                        r
                    }
                    (Type::Text, Value::Text(t)) => {
                        let mut text = t.clone();
                        ui.label(&label);
                        let r = ui.text_edit_singleline(&mut text);
                        if r.lost_focus() && text != *t {
                            let mut s = store.lock().expect("value store lock");
                            write(&mut s, path, Change::To(Value::Text(text)), None, false);
                        }
                        r
                    }
                    (_, v @ (Value::List(_) | Value::Ints(_))) => {
                        let (r, open) = list_row(ui, path, d, &label, v);
                        open_list = open;
                        r
                    }
                    (_, v) => {
                        ui.label(&label);
                        ui.weak(shown_in(&crate::i18n::locale(ui.ctx()), d, v))
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
            if let Some(per_line) = open_list {
                list_values(ui, d, &entry.value, per_line);
            }
        });
    }

    /// A slider for a number: egui-pop's slider built from the
    /// descriptor (soft and hard ranges, steps, unit, precision, default,
    /// a logarithmic bar and a picker when the module asks). Its gestures
    /// (fine and coarse drags, keys, the wheel only once focused, typed
    /// expressions, a double click's reset) are egui-pop's; each finished
    /// change runs the `slider.change` step, then is written to the store
    /// as the user: a drag as one gesture, steps in a row as one group.
    fn slider(
        &mut self,
        ui: &mut Ui,
        store: &Mutex<Store>,
        path: &str,
        entry: &Entry,
    ) -> egui::Response {
        let d = &entry.descriptor;
        let shown_label = label(ui.ctx(), path, d);
        let ctx = ui.ctx().clone();
        let mut v = entry.value.as_f64().unwrap_or(0.0);
        let mut spec = crate::dialogs::descriptor(d);
        spec.label.clone_from(&shown_label);
        let picker = crate::i18n::text(
            &ctx,
            Message::new("gui-param-picker", "Pick { $label } from the image")
                .with("label", &shown_label),
        );
        // Room is kept on the right for the marks.
        let width = (ui.available_width() - 28.0).clamp(120.0, 320.0);
        let height = ui.spacing().interact_size.y;
        self.close_typing(ui, path);
        let app = self.app.clone();
        let out = ui
            .allocate_ui(egui::vec2(width, height), |ui| {
                ui.set_width(width);
                let mut slider = Slider::from_descriptor(&mut v, &spec);
                if d.unit.as_deref() == Some("EV") {
                    // As darktable shows exposures: `+0.70 EV`.
                    slider = slider.signed();
                }
                if d.logarithmic {
                    slider = slider.logarithmic();
                }
                if d.picker {
                    slider = slider.quad(Icon::Picker, &picker);
                }
                if let Some(app) = &app {
                    slider = slider.on_change(sliders::hook(app, path));
                }
                slider.show(ui)
            })
            .inner;
        self.bars
            .insert(path.into(), (out.response.id, out.response.rect));
        if out.quad_clicked()
            && let Some(app) = &self.app
        {
            app.publish("helios.params", None, PICKER_EVENT, json!({ "path": path }));
        }
        self.apply(&ctx, store, path, d, &shown_label, &out);

        let locale = crate::i18n::locale(&ctx);
        let about = description(&ctx, path, d);
        out.response.on_hover_text(format!(
            "{}\n{}\n{}",
            if about.is_empty() { &shown_label } else { &about },
            crate::i18n::text(
                &ctx,
                Message::new("gui-param-default", "{ $path } · default { $value }")
                    .with("path", path)
                    .with("value", shown_in(&locale, d, &d.default)),
            ),
            crate::i18n::text(
                &ctx,
                Message::new(
                    "gui-param-hint",
                    "Drag to change (Ctrl finer, Shift coarser), arrows to step, the wheel once focused or with Alt; double click resets; Enter types a value or an expression (x is the value)",
                ),
            ),
        ))
    }

    /// Closes a slider's typed entry, its text dropped, on a press outside
    /// its bar or on Escape, and lets go of the keyboard: the entry would
    /// otherwise take the focus back on every frame it is drawn
    /// (egui-pop's typed entry asks for it each frame), and a click on the
    /// image would leave the keys in the field.
    fn close_typing(&self, ui: &Ui, path: &str) {
        let Some((id, bar)) = self.bars.get(path).copied() else {
            return;
        };
        let typing = egui_pop::core::slider::typing_id(id);
        if ui.data(|d| d.get_temp::<String>(typing)).is_none() {
            return;
        }
        let away = ui.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !bar.contains(p))
        });
        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if away || escape {
            ui.data_mut(|d| d.remove::<String>(typing));
            ui.memory_mut(|m| m.surrender_focus(typing.with("edit")));
        }
    }

    /// Writes what a slider changed this frame to the store, as the user.
    fn apply(
        &mut self,
        ctx: &egui::Context,
        store: &Mutex<Store>,
        path: &str,
        d: &Descriptor,
        shown_label: &str,
        out: &SliderOutput,
    ) {
        let value = |x: f64| match d.ty {
            Type::Int => Value::Int(x.round() as i64),
            _ => Value::Float(x),
        };
        let (lo, hi) = d.soft.or(d.hard).unwrap_or((f64::MIN, f64::MAX));
        let now = ctx.input(|i| i.time);
        let mut s = store.lock().expect("value store lock");
        let mut drag_ended = false;
        for c in &out.changes {
            match c.cause {
                // A drag's frames: one gesture, other origins kept off.
                Cause::Drag if !c.done => {
                    let held = self
                        .gesture
                        .as_ref()
                        .filter(|(p, _)| p == path)
                        .map(|(_, g)| *g);
                    let g = held.unwrap_or_else(|| {
                        let g = s.begin_gesture(Origin::User, &format!("drag {}", d.label));
                        self.gesture = Some((path.into(), g));
                        g
                    });
                    write(&mut s, path, Change::To(value(c.to)), Some(g), false);
                }
                // Its release, as the step left it: one undo step.
                Cause::Drag => {
                    drag_ended = true;
                    match self.gesture.take() {
                        Some((p, g)) if p == path => {
                            write(&mut s, path, Change::To(value(c.to)), Some(g), false);
                            s.end(g);
                        }
                        other => {
                            self.gesture = other;
                            write(&mut s, path, Change::To(value(c.to)), None, false);
                        }
                    }
                }
                Cause::Key | Cause::Wheel | Cause::Access => {
                    self.step(&mut s, path, Change::To(value(c.to)), &d.label, now);
                }
                Cause::Reset => {
                    write(&mut s, path, Change::Reset, None, false);
                }
                // Typed past the soft range: forced, up to the hard one.
                Cause::Typed => {
                    let force = c.to < lo || c.to > hi;
                    let note =
                        write(&mut s, path, Change::To(value(c.to)), None, force).and_then(|a| {
                            match (force, a.clamped) {
                                (_, helios_values::Clamped::Hard) => Some(
                                    Message::new(
                                        "gui-param-held",
                                        "{ $label }: held to { $value }",
                                    )
                                    .with("label", shown_label)
                                    .with(
                                        "value",
                                        shown_in(&crate::i18n::locale(ctx), d, &a.value),
                                    ),
                                ),
                                (true, _) => Some(
                                    Message::new(
                                        "gui-param-past-range",
                                        "{ $label }: past its usual range",
                                    )
                                    .with("label", shown_label),
                                ),
                                _ => None,
                            }
                        });
                    if let Some(n) = note {
                        request(ctx, Request::Notify(Level::Info, n));
                    }
                }
            }
        }
        // A drag given up (Escape, or vetoed at its release): what it
        // wrote is put back and leaves no undo step.
        if out.response.drag_stopped()
            && !drag_ended
            && self.gesture.as_ref().is_some_and(|(p, _)| p == path)
            && let Some((_, g)) = self.gesture.take()
        {
            s.cancel(g);
        }
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

/// How many values a line of an open list holds, when they are not
/// points.
const VALUES_PER_LINE: usize = 8;

/// What a list's summary counts, and how many values a line of it holds
/// open: a path's or a brush's points (their values each), else values.
fn list_items(d: &Descriptor, path: &str, len: usize) -> (Message, usize) {
    use helios_module_masks::shape::{BRUSH_POINT_VALUES, PATH_POINT_VALUES};
    let per_point = match (d.module.as_deref(), param_name(path)) {
        (Some("mask_path"), "points") => Some(PATH_POINT_VALUES),
        (Some("mask_brush"), "points") => Some(BRUSH_POINT_VALUES),
        _ => None,
    };
    match per_point {
        Some(n) => (
            Message::new("gui-params-points", "{ $count } points").with("count", len / n),
            n,
        ),
        None => (
            Message::new("gui-params-values", "{ $count } values").with("count", len),
            VALUES_PER_LINE,
        ),
    }
}

/// A list's row: its label, then a summary (`12 points`) that a click on
/// it or on the chevron before it opens or closes. A list folded keeps
/// its row as narrow as the others. Returns the summary's response and,
/// open, how many values a line holds.
fn list_row(
    ui: &mut Ui,
    path: &str,
    d: &Descriptor,
    label: &str,
    v: &Value,
) -> (egui::Response, Option<usize>) {
    let len = match v {
        Value::List(l) => l.len(),
        Value::Ints(l) => l.len(),
        _ => 0,
    };
    let (summary, per_line) = list_items(d, path, len);
    let ctx = ui.ctx().clone();
    let summary = crate::i18n::text(&ctx, summary);
    let id = egui::Id::new(("helios.params.open", path));
    let open = ctx.data(|m| m.get_temp::<bool>(id)).unwrap_or(false);
    let (icon, hint) = if open {
        (
            Icon::ChevronDown,
            Message::new("gui-params-hide-values", "Hide the values"),
        )
    } else {
        (
            Icon::ChevronRight,
            Message::new("gui-params-show-values", "Show the values"),
        )
    };
    let hint = crate::i18n::text(&ctx, hint);
    let chevron = ui
        .add(icon.tone(Tone::Muted).label(&hint).sense_click())
        .on_hover_text(&hint);
    ui.label(label);
    let r = ui
        .add(egui::Label::new(egui::RichText::new(&summary).weak()).sense(Sense::click()))
        .on_hover_text(&hint);
    if chevron.clicked() || r.clicked() {
        ctx.data_mut(|m| m.insert_temp(id, !open));
    }
    (r, open.then_some(per_line))
}

/// An open list's values under its row: `per_line` values a line (a
/// point), numbered from 1, wrapped to the panel's width.
fn list_values(ui: &mut Ui, d: &Descriptor, v: &Value, per_line: usize) {
    let locale = crate::i18n::locale(ui.ctx());
    let (values, fraction): (Vec<f64>, usize) = match v {
        Value::List(l) => (l.clone(), d.precision as usize),
        Value::Ints(l) => (l.iter().map(|&x| x as f64).collect(), 0),
        _ => return,
    };
    ui.indent("values", |ui| {
        for (i, line) in values.chunks(per_line.max(1)).enumerate() {
            let line: Vec<String> = line
                .iter()
                .map(|x| crate::i18n::fixed(&locale, *x, fraction))
                .collect();
            ui.add(egui::Label::new(format!("{}: {}", i + 1, line.join("  "))).wrap());
        }
    });
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
