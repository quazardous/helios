//! The renderers (feature `render`): a control drawn with egui-pop's
//! visual parts, under the ergonomic contract. A renderer never holds the
//! value: it is handed the value of the frame and reports the changes,
//! which the caller writes through the value flow ([`crate::flow`]).
//!
//! - [`card`]: a whole card from a [`resolve`](crate::resolve) result:
//!   every shown part at its rect, its chrome, and the folded parts under
//!   the "More (n)" row ([`more`]), whose hover gives the reasons and
//!   whose rows ask for `controls.reveal` ([`CardEvent`]).
//! - [`part`]: one part at its rect, by its placement's [`Look`]
//!   (representation, orientation, variant, integration). A managed part
//!   is drawn by its value type's renderer; an integrated one by the
//!   host's [`Drawings`] where its own drawing fits (its full
//!   representation), else by its declared fallback ([`Fallback`]):
//!   another variant, the value type's standard renderer, or a case's
//!   parts.
//! - Numbers: the `slider` variant as egui-pop's slider (full: label and
//!   value inside the bar; compact: the value alone, the label in the
//!   tooltip; vertical: a fader, the value at the top and the label at the
//!   bottom), the `knob` variant as egui-pop's knob, the `scrub` variant
//!   and every simplified form as a scrub field ([`scrub`]).
//! - Other value types: a toggle, a drop-down, `points` as numbered rows
//!   of x and y, a `colour` as a swatch and its channels, text as a
//!   field.
//! - [`configure`] and [`detent_hook`] give egui-pop's slider and knob a
//!   control's steps, sign and detents.
//! - [`chip`]: the HUD read-out (`Exposure +0.67 EV`).
//! - [`show`]: a control in a representation and a variant, outside a
//!   card.

use std::collections::BTreeMap;

use egui_pop::core::kit::{Glyph, Kit};
use egui_pop::core::knob::Knob;
use egui_pop::core::sections;
use egui_pop::core::slider::{self, Slider, Verdict};
use egui_pop::egui::{self, Key, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};

use crate::card::ItemKind;
use crate::flow::Cause;
use crate::model::{Control, HIT_AREA, Mode, Representation, ValueSpec, ValueType};
use crate::registry::Registry;
use crate::resolve::{CardPlacement, Placement};
use crate::slot::{Fallback, Integration, Orientation};
use crate::value::Value;

/// One change a renderer reports.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Change {
    pub to: f64,
    pub cause: Cause,
    /// The gesture is over (every change but a drag's frames).
    pub done: bool,
    /// The drag was given up (Escape): back to `to`, no trace.
    pub cancelled: bool,
}

/// What a renderer did in a frame.
#[derive(Debug)]
pub struct Output {
    pub response: egui::Response,
    pub changes: Vec<Change>,
}

/// A slider cause as a control's.
pub fn cause_of(c: slider::Cause) -> Cause {
    match c {
        slider::Cause::Drag => Cause::Drag,
        slider::Cause::Key => Cause::Key,
        slider::Cause::Wheel => Cause::Wheel,
        slider::Cause::Typed => Cause::Typed,
        slider::Cause::Reset => Cause::Reset,
        slider::Cause::Access => Cause::Access,
    }
}

/// Gives egui-pop's slider a control's steps, sign, unit and default.
pub fn configure<'a>(slider: Slider<'a>, control: &Control) -> Slider<'a> {
    let Some(n) = control.value.number() else {
        return slider;
    };
    let mut s = slider
        .hard(n.hard.0..=n.hard.1)
        .step(n.step)
        .coarse(n.coarse)
        .precision(n.precision as usize)
        .display_scale(n.display_scale)
        .default(n.default);
    if let Some(u) = &n.unit {
        s = s.unit(u);
    }
    if n.signed {
        s = s.signed();
    }
    if n.integer {
        s = s.integer();
    }
    s
}

/// Whether the modifier that turns detents off (Alt) is held.
pub fn detents_off(ui: &Ui) -> bool {
    ui.input(|i| i.modifiers.alt)
}

/// A slider hook holding a drag on the control's detents (unless `free`):
/// the drag stays relative, so moving on crosses the detent.
pub fn detent_hook(control: &Control, free: bool) -> impl FnMut(&slider::Change) -> Verdict + '_ {
    move |c| {
        if c.cause != slider::Cause::Drag {
            return Verdict::Go;
        }
        let held = control.detented(c.to, free);
        if held == c.to {
            Verdict::Go
        } else {
            Verdict::Redirect(held)
        }
    }
}

/// The `slider` variant: egui-pop's slider with the control's steps and
/// detents.
pub fn slider_row(ui: &mut Ui, control: &Control, label: &str, value: &mut f64) -> Output {
    let soft = control.value.number().map_or((0.0, 1.0), |n| n.soft);
    let free = detents_off(ui);
    let out = configure(Slider::new(value, label, soft.0..=soft.1), control)
        .on_change(detent_hook(control, free))
        .show(ui);
    let mut changes: Vec<Change> = out
        .changes
        .iter()
        .map(|c| Change {
            to: c.to,
            cause: cause_of(c.cause),
            done: c.done,
            cancelled: false,
        })
        .collect();
    if out.response.drag_stopped() && !changes.iter().any(|c| c.done) {
        changes.push(Change {
            to: *value,
            cause: Cause::Drag,
            done: true,
            cancelled: true,
        });
    }
    Output {
        response: out.response,
        changes,
    }
}

/// The `scrub` variant: a field whose value drags at the control's
/// sensitivity. Ctrl is the fine mode, Alt turns detents off, Escape
/// gives the drag up, a double click resets, the arrows step (Page Up and
/// Page Down the coarse step), the wheel steps it under the pointer.
pub fn scrub(
    ui: &mut Ui,
    control: &Control,
    label: Option<&str>,
    value: &mut f64,
    width: f32,
) -> Output {
    let n = control.value.number().cloned();
    let height = ui.spacing().interact_size.y.max(HIT_AREA);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let id = response.id;
    let mut changes = Vec::new();
    let mut push = |to: f64, cause: Cause, done: bool, cancelled: bool| {
        changes.push(Change {
            to,
            cause,
            done,
            cancelled,
        });
    };
    let (fine, free) = ui.input(|i| (i.modifiers.command, i.modifiers.alt));
    let start_id = id.with("scrub-start");
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(start_id, (*value, 0.0f64)));
    }
    if response.dragged()
        && let Some((start, acc)) = ui.data(|d| d.get_temp::<(f64, f64)>(start_id))
    {
        let dx = f64::from(response.drag_delta().x);
        if dx != 0.0 {
            // Fine mode slows what is added from now on, never jumps.
            let scale = if fine { control.feel.fine } else { 1.0 };
            let acc = acc + dx * scale;
            ui.data_mut(|d| d.insert_temp(start_id, (start, acc)));
            let to = control.scrubbed(start, acc, false, free);
            if to != *value {
                *value = to;
                push(to, Cause::Scrub, false, false);
            }
        }
    }
    if response.drag_stopped()
        && let Some((start, _)) = ui.data_mut(|d| d.remove_temp::<(f64, f64)>(start_id))
    {
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            *value = start;
            push(start, Cause::Scrub, true, true);
        } else {
            push(*value, Cause::Scrub, true, false);
        }
    }
    if response.double_clicked()
        && let Some(n) = &n
        && *value != n.default
    {
        *value = n.default;
        push(n.default, Cause::Reset, true, false);
    }
    if response.clicked() {
        response.request_focus();
    }
    if response.has_focus() {
        let (steps, pages) = ui.input(|i| {
            let c = |k: Key| i64::from(i.num_presses(k) as u8);
            (
                c(Key::ArrowRight) + c(Key::ArrowUp) - c(Key::ArrowLeft) - c(Key::ArrowDown),
                c(Key::PageUp) - c(Key::PageDown),
            )
        });
        if steps != 0 || pages != 0 {
            let mut to = control.nudged(*value, steps, fine, false);
            if pages != 0 {
                to = control.nudged(to, pages, false, true);
            }
            if to != *value {
                *value = to;
                push(to, Cause::Key, true, false);
            }
        }
    }
    if response.hovered() {
        let notches: f32 = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                    _ => None,
                })
                .sum()
        });
        if notches != 0.0 {
            let to = control.nudged(*value, notches.signum() as i64, fine, false);
            if to != *value {
                *value = to;
                push(to, Cause::Wheel, true, false);
            }
        }
    }
    let visuals = ui.style().interact(&response);
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        visuals.corner_radius,
        ui.visuals().extreme_bg_color,
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
    let shown = n.as_ref().map_or_else(String::new, |n| n.format(*value));
    let font = egui::TextStyle::Body.resolve(ui.style());
    let pad = 6.0;
    // The label only where it fits beside the value: else in the
    // tooltip, as a narrow slider's.
    let fits = |l: &str| {
        let w = |t: &str| {
            ui.painter()
                .layout_no_wrap(t.to_owned(), font.clone(), egui::Color32::PLACEHOLDER)
                .size()
                .x
        };
        w(l) + w(&shown) + 3.0 * pad <= rect.width()
    };
    let label = label.filter(|l| fits(l));
    if let Some(l) = label {
        painter.text(
            rect.left_center() + vec2(pad, 0.0),
            egui::Align2::LEFT_CENTER,
            l,
            font.clone(),
            ui.visuals().weak_text_color(),
        );
    }
    painter.text(
        rect.right_center() - vec2(pad, 0.0),
        egui::Align2::RIGHT_CENTER,
        &shown,
        font,
        visuals.text_color(),
    );
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let v = *value;
    let name = control.label.clone();
    response.widget_info(|| egui::WidgetInfo::slider(true, v, &name));
    let response = if label.is_none() {
        response.on_hover_text(&control.label)
    } else {
        response
    };
    Output { response, changes }
}

/// The HUD read-out: one line, the control's label and value.
pub fn chip(ui: &mut Ui, control: &Control, value: f64) -> egui::Response {
    let shown = control
        .value
        .number()
        .map_or_else(String::new, |n| n.format(value));
    egui::Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(ui.visuals().window_stroke)
        .corner_radius(HIT_AREA / 2.0)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.weak(&control.label);
                ui.strong(shown);
            });
        })
        .response
}

/// A control in a representation, its variant `variant` (the default when
/// `None` or unknown), outside a card: in the width available, one row
/// high, horizontal, through the same renderers as a card's parts.
pub fn show(
    ui: &mut Ui,
    control: &Control,
    representation: Representation,
    variant: Option<&str>,
    value: &mut f64,
) -> Output {
    if representation == Representation::Simplified {
        return Output {
            response: chip(ui, control, *value),
            changes: Vec::new(),
        };
    }
    let row = ui.spacing().interact_size.y.max(HIT_AREA);
    let width = match representation {
        Representation::Full => ui.available_width().clamp(120.0, 320.0),
        _ => 110.0,
    };
    let (rect, _) = ui.allocate_exact_size(vec2(width, row), Sense::hover());
    let look = Look {
        representation,
        orientation: Orientation::Horizontal,
        variant: variant.map(str::to_owned),
        integration: Integration::Managed,
    };
    number_at(ui, control, &look, rect, value)
}

/// How a part is drawn: what its placement says.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub representation: Representation,
    pub orientation: Orientation,
    /// The variant (the control's default when `None`).
    pub variant: Option<String>,
    /// Who draws it on this surface.
    pub integration: Integration,
}

impl Look {
    /// A placement's look: its representation (full when it has none),
    /// orientation, variant and integration.
    pub fn of(p: &Placement) -> Self {
        Self {
            representation: p.representation.unwrap_or(Representation::Full),
            orientation: p.orientation.unwrap_or(Orientation::Horizontal),
            variant: p.variant.clone(),
            integration: p.integration.clone().unwrap_or_default(),
        }
    }

    /// Managed, in its default variant.
    pub fn managed(representation: Representation, orientation: Orientation) -> Self {
        Self {
            representation,
            orientation,
            variant: None,
            integration: Integration::Managed,
        }
    }
}

/// One edit a card's renderer reports, by the part's key.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// A number's change, inside a gesture or at its end.
    Number { key: String, change: Change },
    /// Another value set (a toggle, a choice, a curve's point, a
    /// colour's channel); `done` at the end of its gesture.
    Set { key: String, to: Value, done: bool },
}

impl Edit {
    pub fn key(&self) -> &str {
        match self {
            Edit::Number { key, .. } | Edit::Set { key, .. } => key,
        }
    }
}

/// The host's own drawings, for its integrated controls (a curve
/// editor, colour wheels): the framework knows nothing of them.
pub trait Drawings {
    /// Draws part `key` (control `control`) in `rect` with the host's own
    /// drawing; `None` when the host has none for it, or it does not fit:
    /// the part's fallback is drawn instead.
    fn draw(
        &mut self,
        ui: &mut Ui,
        key: &str,
        control: &Control,
        rect: Rect,
        value: &mut Value,
    ) -> Option<Vec<Edit>>;
}

/// No drawings of the host's own: every integrated part falls back.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoDrawings;

impl Drawings for NoDrawings {
    fn draw(
        &mut self,
        _: &mut Ui,
        _: &str,
        _: &Control,
        _: Rect,
        _: &mut Value,
    ) -> Option<Vec<Edit>> {
        None
    }
}

/// What drew a part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drawn {
    /// The standard renderer for its value type.
    Managed,
    /// The host's own drawing.
    Integrated,
    /// Its fallback, as declared.
    Fallback(Fallback),
    /// An integrated part with no fallback and no drawing here: a
    /// placeholder (the exception the inspector counts).
    Missing,
}

/// What a part did in a frame.
#[derive(Debug)]
pub struct PartOutput {
    pub edits: Vec<Edit>,
    pub drawn: Drawn,
}

/// A card's request to its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardEvent {
    /// Show a folded part: `controls.reveal {node, key}`.
    Reveal(String),
    /// Show every part: `controls.reveal_all {node}`.
    RevealAll,
}

/// What a card did in a frame.
#[derive(Debug)]
pub struct CardOutput {
    pub response: egui::Response,
    pub edits: Vec<Edit>,
    pub events: Vec<CardEvent>,
    /// How each shown part was drawn, by key.
    pub drawn: BTreeMap<String, Drawn>,
}

/// The gap between two parts, in points.
const GAP: f32 = 2.0;

/// A whole card from its resolution: every shown part at its rect (from
/// the cursor, in the rack's width), its chrome, then the "More (n)" row
/// when anything is folded. `values` holds each part's value by key; the
/// renderers write into it as they go and report each change.
pub fn card(
    ui: &mut Ui,
    registry: &Registry,
    placement: &CardPlacement,
    values: &mut BTreeMap<String, Value>,
    drawings: &mut dyn Drawings,
) -> CardOutput {
    let (width, height) = placement.layout.as_ref().map_or_else(
        || (ui.available_width(), 0.0),
        |l| (l.rack.width, l.height()),
    );
    let origin = ui.cursor().min;
    let (_, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let mut edits = Vec::new();
    let mut drawn = BTreeMap::new();
    for p in placement.items.iter().filter(|p| p.visibility.is_shown()) {
        let Some(r) = p.rect else { continue };
        let rect = Rect::from_min_size(origin + vec2(r.x, r.y), vec2(r.w, r.h)).shrink(GAP / 2.0);
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .id_salt(("hudget-part", &p.key))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        // A part never draws past its slot.
        child.set_clip_rect(rect.expand(1.0).intersect(ui.clip_rect()));
        if p.kind == ItemKind::Chrome {
            chrome(&mut child, p, rect);
            continue;
        }
        let Some(control) = p.control.as_deref().and_then(|id| registry.get(id)) else {
            continue;
        };
        let value = values
            .entry(p.key.clone())
            .or_insert_with(|| default_of(control));
        let out = part(
            &mut child,
            registry,
            &p.key,
            control,
            &Look::of(p),
            rect,
            value,
            drawings,
        );
        edits.extend(out.edits);
        drawn.insert(p.key.clone(), out.drawn);
    }
    let mut events = Vec::new();
    if let Some(m) = &placement.more
        && m.count + m.inapplicable.len() > 0
    {
        let toolbar = placement
            .items
            .first()
            .is_some_and(|p| p.mode == Mode::Toolbar);
        events = more(ui, placement, toolbar);
    }
    CardOutput {
        response,
        edits,
        events,
        drawn,
    }
}

/// A part's value at its control's default.
pub fn default_of(control: &Control) -> Value {
    match &control.value {
        ValueSpec::Number(n) => Value::Float(n.default),
        ValueSpec::Bool { default } => Value::Bool(*default),
        ValueSpec::OneOf { default, .. } => Value::Choice(default.clone()),
        ValueSpec::Other { stored } => match stored {
            ValueType::Text => Value::Text(String::new()),
            _ => Value::Floats(Vec::new()),
        },
    }
}

/// A piece of chrome in its rect: a legend, a separator, a help line, an
/// icon's room.
fn chrome(ui: &mut Ui, p: &Placement, rect: Rect) {
    let kind = p
        .key
        .strip_prefix("chrome:")
        .and_then(|k| k.split('#').next())
        .unwrap_or_default();
    let text = p.label.clone().unwrap_or_default();
    match kind {
        "legend" => {
            ui.strong(text);
        }
        "help" => {
            ui.add(egui::Label::new(egui::RichText::new(text).weak().small()).wrap());
        }
        "separator" => {
            let y = rect.center().y;
            ui.painter().hline(
                rect.x_range(),
                y,
                ui.visuals().widgets.noninteractive.bg_stroke,
            );
        }
        _ => {}
    }
}

/// One part in `rect`, by its look: the host's drawing where its own
/// drawing fits, else its fallback; a managed part by its value type.
#[allow(
    clippy::too_many_arguments,
    reason = "a part's key, control, look, rect and value, and where to draw"
)]
pub fn part(
    ui: &mut Ui,
    registry: &Registry,
    key: &str,
    control: &Control,
    look: &Look,
    rect: Rect,
    value: &mut Value,
    drawings: &mut dyn Drawings,
) -> PartOutput {
    if let Integration::Integrated { fallback } = &look.integration {
        // Its own drawing goes only where its full form was given.
        if look.representation == Representation::Full
            && let Some(edits) = drawings.draw(ui, key, control, rect, value)
        {
            return PartOutput {
                edits,
                drawn: Drawn::Integrated,
            };
        }
        let Some(fallback) = fallback else {
            missing(ui, control, rect);
            return PartOutput {
                edits: Vec::new(),
                drawn: Drawn::Missing,
            };
        };
        let edits = match fallback {
            Fallback::Variant(id) => {
                let look = Look {
                    variant: Some(id.clone()),
                    integration: Integration::Managed,
                    ..look.clone()
                };
                managed(ui, key, control, &look, rect, value)
            }
            Fallback::ValueType => {
                let look = Look {
                    integration: Integration::Managed,
                    ..look.clone()
                };
                managed(ui, key, control, &look, rect, value)
            }
            Fallback::Parts => parts(ui, registry, key, control, rect, value, drawings),
        };
        return PartOutput {
            edits,
            drawn: Drawn::Fallback(fallback.clone()),
        };
    }
    PartOutput {
        edits: managed(ui, key, control, look, rect, value),
        drawn: Drawn::Managed,
    }
}

/// An integrated part with nothing to draw here.
fn missing(ui: &mut Ui, control: &Control, rect: Rect) {
    ui.painter().rect_stroke(
        rect,
        2.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    ui.weak(format!("{}: no managed form", control.label));
}

/// A case's parts, each in its managed slot, laid out in `rect` as a
/// card of their own (the case's fallback). Their values are the case
/// value's members, by part name, when it is an object of them.
fn parts(
    ui: &mut Ui,
    registry: &Registry,
    key: &str,
    control: &Control,
    rect: Rect,
    value: &mut Value,
    drawings: &mut dyn Drawings,
) -> Vec<Edit> {
    let Some(case) = &control.case else {
        missing(ui, control, rect);
        return Vec::new();
    };
    let card = crate::card::Card::from_case(case);
    let keys = crate::context::ContextKeys::default();
    let columns = (rect.width() / keys.density.column()).max(0.25);
    let mut keys = keys;
    keys.set_box(crate::context::CardBox::new(
        crate::model::Surface::Panel,
        (columns * 12.0).floor() as u32,
        (rect.height() / keys.density.row() * 12.0).floor() as u32,
    ));
    let Ok(placement) = crate::resolve::resolve(
        registry,
        &card,
        &keys,
        &crate::resolve::Layouts::default(),
        &crate::resolve::Values::new(),
        &crate::resolve::Moment::default(),
        None,
    ) else {
        missing(ui, control, rect);
        return Vec::new();
    };
    // The case's value holds no members: each part starts at its default.
    let mut inner = BTreeMap::new();
    let out = card_at(ui, registry, &placement, &mut inner, drawings);
    let _ = value;
    out.into_iter()
        .map(|e| match e {
            Edit::Number { key: k, change } => Edit::Number {
                key: format!("{key}.{k}"),
                change,
            },
            Edit::Set { key: k, to, done } => Edit::Set {
                key: format!("{key}.{k}"),
                to,
                done,
            },
        })
        .collect()
}

/// A card drawn from the cursor (a case's parts), its edits only.
fn card_at(
    ui: &mut Ui,
    registry: &Registry,
    placement: &CardPlacement,
    values: &mut BTreeMap<String, Value>,
    drawings: &mut dyn Drawings,
) -> Vec<Edit> {
    card(ui, registry, placement, values, drawings).edits
}

/// A managed part by its value type.
fn managed(
    ui: &mut Ui,
    key: &str,
    control: &Control,
    look: &Look,
    rect: Rect,
    value: &mut Value,
) -> Vec<Edit> {
    let label = control.label.as_str();
    match &control.value {
        ValueSpec::Number(n) => {
            let mut v = value.as_f64().unwrap_or(n.default);
            let out = number_at(ui, control, look, rect, &mut v);
            *value = if n.integer {
                Value::Int(v.round() as i64)
            } else {
                Value::Float(v)
            };
            out.changes
                .into_iter()
                .map(|change| Edit::Number {
                    key: key.to_owned(),
                    change,
                })
                .collect()
        }
        ValueSpec::Bool { default } => {
            let mut on = match value {
                Value::Bool(b) => *b,
                _ => *default,
            };
            let shown = if look.representation == Representation::Full {
                label
            } else {
                ""
            };
            let r = egui_pop::core::widgets::checkbox(ui, &mut on, shown);
            let r = if shown.is_empty() {
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, label));
                r.on_hover_text(label)
            } else {
                r
            };
            if r.changed() {
                *value = Value::Bool(on);
                return vec![Edit::Set {
                    key: key.to_owned(),
                    to: value.clone(),
                    done: true,
                }];
            }
            Vec::new()
        }
        ValueSpec::OneOf { choices, default } => {
            let mut chosen = match value {
                Value::Choice(c) => c.clone(),
                _ => default.clone(),
            };
            let before = chosen.clone();
            let full = look.representation == Representation::Full;
            let label_id = full.then(|| ui.label(label).id);
            let width = ui.available_width();
            let r = egui::ComboBox::from_id_salt(("hudget-choice", key))
                .selected_text(chosen.clone())
                .width(width)
                .show_ui(ui, |ui| {
                    for c in choices {
                        ui.selectable_value(&mut chosen, c.clone(), c);
                    }
                })
                .response;
            match label_id {
                Some(id) => {
                    let _ = r.labelled_by(id);
                }
                None => {
                    let _ = r.on_hover_text(label);
                }
            }
            if chosen != before {
                *value = Value::Choice(chosen);
                return vec![Edit::Set {
                    key: key.to_owned(),
                    to: value.clone(),
                    done: true,
                }];
            }
            Vec::new()
        }
        ValueSpec::Other { stored } => match stored {
            ValueType::Points => points(ui, key, label, rect, value),
            ValueType::Colour => colour(ui, key, label, rect, value),
            ValueType::Text => {
                let mut text = match value {
                    Value::Text(t) => t.clone(),
                    _ => String::new(),
                };
                let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(rect.width()));
                let r = r.on_hover_text(label);
                if r.changed() {
                    *value = Value::Text(text);
                    return vec![Edit::Set {
                        key: key.to_owned(),
                        to: value.clone(),
                        done: r.lost_focus(),
                    }];
                }
                Vec::new()
            }
            _ => {
                ui.label(format!("{label}: {}", value.to_json()));
                Vec::new()
            }
        },
    }
}

/// The `points` value type's standard renderer: one numbered row per
/// point, its x and y as compact sliders over 0 to 1, in as many rows as
/// the rect holds (scrolled beyond).
fn points(ui: &mut Ui, key: &str, label: &str, rect: Rect, value: &mut Value) -> Vec<Edit> {
    let mut list = match value {
        Value::Floats(l) => l.clone(),
        _ => Vec::new(),
    };
    let mut edits = Vec::new();
    let row = ui.spacing().interact_size.y;
    let index_w = 18.0;
    let gap = ui.spacing().item_spacing.x;
    let half = ((rect.width() - index_w - 2.0 * gap) / 2.0).max(slider::MIN_WIDTH);
    let mut changed: Option<bool> = None;
    let rows = |ui: &mut Ui, list: &mut Vec<f64>, changed: &mut Option<bool>| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for i in 0..list.len() / 2 {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        vec2(index_w, row),
                        egui::Label::new(egui::RichText::new(format!("{}", i + 1)).weak()),
                    );
                    for (j, axis) in ["x", "y"].into_iter().enumerate() {
                        let at = 2 * i + j;
                        let out = Slider::new(
                            &mut list[at],
                            &format!("{label} {} {axis}", i + 1),
                            0.0..=1.0,
                        )
                        .step(0.001)
                        .precision(3)
                        .compact()
                        .size(vec2(half, row))
                        .show(ui);
                        for c in &out.changes {
                            *changed = Some(changed.unwrap_or(false) || c.done);
                        }
                    }
                });
            }
        });
    };
    if (list.len() / 2) as f32 * (row + 1.0) > rect.height() + 0.5 {
        egui::ScrollArea::vertical()
            .id_salt(("hudget-points", key))
            .max_height(rect.height())
            .auto_shrink([false, true])
            .show(ui, |ui| rows(ui, &mut list, &mut changed));
    } else {
        rows(ui, &mut list, &mut changed);
    }
    if let Some(done) = changed {
        *value = Value::Floats(list);
        edits.push(Edit::Set {
            key: key.to_owned(),
            to: value.clone(),
            done,
        });
    }
    edits
}

/// The `colour` value type's standard renderer: a swatch, then its
/// channels (hue in turns, chroma, luminance) as compact sliders.
fn colour(ui: &mut Ui, key: &str, label: &str, rect: Rect, value: &mut Value) -> Vec<Edit> {
    let mut list = match value {
        Value::Floats(l) => l.clone(),
        _ => Vec::new(),
    };
    list.resize(list.len().max(3), 0.0);
    let row = ui.spacing().interact_size.y;
    let gap = ui.spacing().item_spacing.x;
    let side = row.min(rect.height());
    let (swatch, sr) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    let hsva = egui::ecolor::Hsva::new(
        list[0].rem_euclid(1.0) as f32,
        list[1].clamp(0.0, 1.0) as f32,
        (0.5 + list[2] * 0.5).clamp(0.0, 1.0) as f32,
        1.0,
    );
    ui.painter().rect(
        swatch,
        3.0,
        egui::Color32::from(hsva),
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    sr.on_hover_text(label);
    let names = ["Hue", "Chroma", "Luminance"];
    let ranges = [(0.0, 1.0), (0.0, 1.0), (-1.0, 1.0)];
    let w = ((rect.width() - side - 3.0 * gap) / 3.0).max(slider::MIN_WIDTH);
    let mut changed: Option<bool> = None;
    for (i, name) in names.into_iter().enumerate() {
        let (lo, hi) = ranges[i];
        let out = Slider::new(&mut list[i], &format!("{label} {name}"), lo..=hi)
            .step(0.001)
            .precision(3)
            .compact()
            .size(vec2(w, row.min(rect.height())))
            .show(ui);
        for c in &out.changes {
            changed = Some(changed.unwrap_or(false) || c.done);
        }
    }
    match changed {
        Some(done) => {
            *value = Value::Floats(list);
            vec![Edit::Set {
                key: key.to_owned(),
                to: value.clone(),
                done,
            }]
        }
        None => Vec::new(),
    }
}

/// A number in `rect` by its look: the `knob` variant as a knob, the
/// `scrub` variant and every simplified form as a scrub field, the
/// slider (horizontal or vertical, full or compact) otherwise.
pub fn number_at(
    ui: &mut Ui,
    control: &Control,
    look: &Look,
    rect: Rect,
    value: &mut f64,
) -> Output {
    let variant = control
        .variant_or_default(look.variant.as_deref())
        .map_or("slider", |v| v.id.as_str());
    let row = ui.spacing().interact_size.y;
    let vertical = look.orientation == Orientation::Vertical;
    // A one-row form keeps the row's height, centred in its slot.
    // A fader keeps a fader's width, centred in a wide slot.
    let rect = if vertical {
        Rect::from_center_size(
            rect.center(),
            vec2(rect.width().min(row * 3.0), rect.height()),
        )
    } else {
        rect
    };
    let bar = |h: f32| {
        if vertical {
            rect
        } else {
            Rect::from_center_size(rect.center(), vec2(rect.width(), h.min(rect.height())))
        }
    };
    let mut child = ui.new_child(UiBuilder::new().max_rect(bar(row.max(HIT_AREA))));
    let ui = &mut child;
    match (variant, look.representation) {
        ("scrub", Representation::Full) => {
            scrub(ui, control, Some(&control.label), value, rect.width())
        }
        (_, Representation::Simplified) | ("scrub", _) => {
            let out = scrub(ui, control, None, value, rect.width());
            let label = control.label.clone();
            Output {
                response: out.response.on_hover_text(label),
                changes: out.changes,
            }
        }
        ("knob", _) => knob_in(ui, control, rect, value),
        (_, representation) => {
            let soft = control.value.number().map_or((0.0, 1.0), |n| n.soft);
            let free = detents_off(ui);
            let size = if vertical {
                rect.size()
            } else {
                vec2(rect.width(), row.min(rect.height()))
            };
            let mut s = configure(Slider::new(value, &control.label, soft.0..=soft.1), control)
                .on_change(detent_hook(control, free))
                .size(size);
            if vertical {
                s = s.vertical();
            }
            if representation == Representation::Compact {
                s = s.compact();
            }
            let out = s.show(ui);
            slider_output(out.response, &out.changes, *value)
        }
    }
}

/// A knob in `rect`, with the control's steps and detents.
fn knob_in(ui: &mut Ui, control: &Control, rect: Rect, value: &mut f64) -> Output {
    let soft = control.value.number().map_or((0.0, 1.0), |n| n.soft);
    let free = detents_off(ui);
    let mut k = Knob::new(value, &control.label, soft.0..=soft.1).size(rect.size());
    if let Some(n) = control.value.number() {
        k = k
            .hard(n.hard.0..=n.hard.1)
            .step(n.step)
            .coarse(n.coarse)
            .precision(n.precision as usize)
            .display_scale(n.display_scale)
            .default(n.default);
        if let Some(u) = &n.unit {
            k = k.unit(u);
        }
        if n.signed {
            k = k.signed();
        }
        if n.integer {
            k = k.integer();
        }
        // The control's sensitivity: its soft range over that many points.
        let span = n.soft.1 - n.soft.0;
        if control.feel.sensitivity > 0.0 && span > 0.0 {
            k = k.travel((span / control.feel.sensitivity) as f32);
        }
    }
    let out = k.on_change(detent_hook(control, free)).show(ui);
    slider_output(out.response, &out.changes, *value)
}

/// A slider's or a knob's changes as a renderer's: a drag given up
/// (Escape) comes back as a cancelled change.
fn slider_output(response: egui::Response, changes: &[slider::Change], value: f64) -> Output {
    let mut changes: Vec<Change> = changes
        .iter()
        .map(|c| Change {
            to: c.to,
            cause: cause_of(c.cause),
            done: c.done,
            cancelled: false,
        })
        .collect();
    if response.drag_stopped() && !changes.iter().any(|c| c.done) {
        changes.push(Change {
            to: value,
            cause: Cause::Drag,
            done: true,
            cancelled: true,
        });
    }
    Output { response, changes }
}

/// The "More (n)" row under a card: egui-pop's section band (the
/// disclosure mark, "More", the count), or in a toolbar its "More tools"
/// button with the count. Its hover lists the folded parts and why; open,
/// it lists them, each to reveal, and "Show all".
pub fn more(ui: &mut Ui, placement: &CardPlacement, toolbar: bool) -> Vec<CardEvent> {
    let Some(m) = &placement.more else {
        return Vec::new();
    };
    let id = ui.make_persistent_id("hudget-more");
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let count = m.count as u64;
    let response = if toolbar {
        let side = ui.spacing().interact_size.y.max(HIT_AREA);
        let (rect, r) = ui.allocate_exact_size(vec2(side * 1.8, side), Sense::click());
        let kit = Kit::of(ui.ctx());
        let color = ui.style().interact(&r).text_color();
        kit.glyph(Glyph::More).paint(
            ui.painter(),
            Rect::from_center_size(
                pos2(rect.left() + side / 2.0, rect.center().y),
                Vec2::splat(side * 0.6),
            ),
            color,
        );
        ui.painter().text(
            pos2(rect.left() + side, rect.center().y),
            egui::Align2::LEFT_CENTER,
            count.to_string(),
            egui::TextStyle::Small.resolve(ui.style()),
            color,
        );
        let title = m.title();
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &title));
        r
    } else {
        sections::band(ui, "hudget-more", "More", Some(count), open)
    };
    let response = response.on_hover_ui(|ui| {
        for f in &m.folded {
            ui.label(format!(
                "{}: {}",
                f.label.as_deref().unwrap_or(&f.key),
                f.reason.text
            ));
        }
        for f in &m.inapplicable {
            ui.weak(format!(
                "{} (not here): {}",
                f.label.as_deref().unwrap_or(&f.key),
                f.reason.text
            ));
        }
    });
    if response.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(id, open));
    }
    let mut events = Vec::new();
    if open {
        for f in &m.folded {
            let name = f.label.as_deref().unwrap_or(&f.key);
            if ui
                .button(format!("Show {name}"))
                .on_hover_text(&f.reason.text)
                .clicked()
            {
                events.push(CardEvent::Reveal(f.key.clone()));
            }
        }
        if m.folded.len() > 1 && ui.button("Show all").clicked() {
            events.push(CardEvent::RevealAll);
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{Card, CardItem};
    use crate::context::{CardBox, ContextKeys, Tier};
    use crate::model::Surface;
    use crate::registry::implicit;
    use crate::resolve::{Layout, Layouts, Moment, Values, resolve};
    use crate::value::{ParamDescriptor, ParamType, Widget};

    /// A curve, a colour wheel and an expert number, as implicit controls.
    fn card_of_three() -> (Registry, Card) {
        let mut curve = ParamDescriptor::new(
            ParamType::Floats,
            "Curve",
            Value::Floats(vec![0.0, 0.0, 1.0, 1.0]),
        );
        curve.widget = Some(Widget::Curve);
        let mut lift = ParamDescriptor::new(
            ParamType::Floats,
            "Lift",
            Value::Floats(vec![0.0, 0.0, 0.0]),
        );
        lift.widget = Some(Widget::Wheel);
        let mut amount = ParamDescriptor::new(ParamType::Float, "Amount", Value::Float(0.5));
        amount.hard = Some((0.0, 1.0));
        let c = implicit("node/n/curve", &curve);
        let w = implicit("node/n/lift", &lift);
        let a = implicit("node/n/amount", &amount);
        let card = Card::new(vec![
            CardItem::control("curve", &c.id),
            CardItem::control("lift", &w.id),
            CardItem::control("amount", &a.id).tier(Tier::Expert),
        ]);
        (Registry::with([c, w, a]).unwrap(), card)
    }

    fn resolved(registry: &Registry, card: &Card, layout: Layout) -> CardPlacement {
        let keys = ContextKeys::new(CardBox::new(Surface::Panel, 12, 240));
        resolve(
            registry,
            card,
            &keys,
            &Layouts::person(layout),
            &Values::new(),
            &Moment::default(),
            None,
        )
        .unwrap()
    }

    /// The host's own drawing for every integrated part.
    struct Own;

    impl Drawings for Own {
        fn draw(
            &mut self,
            ui: &mut Ui,
            _: &str,
            c: &Control,
            _: Rect,
            _: &mut Value,
        ) -> Option<Vec<Edit>> {
            ui.label(format!("{} drawn", c.label));
            Some(Vec::new())
        }
    }

    /// The card drawn once, and the names of what it put in the
    /// accessibility tree.
    fn draw(
        registry: &Registry,
        placement: &CardPlacement,
        drawings: &mut dyn Drawings,
    ) -> (CardOutput, Vec<String>) {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut out = None;
        let full = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut values = BTreeMap::from([
                ("curve".to_owned(), Value::Floats(vec![0.0, 0.0, 1.0, 1.0])),
                ("lift".to_owned(), Value::Floats(vec![0.1, 0.2, 0.3])),
            ]);
            out = Some(card(ui, registry, placement, &mut values, drawings));
        });
        // epaint panics when a frame's texture changes are dropped unread.
        let mut full = full;
        full.textures_delta.clear();
        let names = full
            .platform_output
            .accesskit_update
            .map(|u| {
                u.nodes
                    .iter()
                    .filter_map(|(_, n)| n.label().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        (out.unwrap(), names)
    }

    #[test]
    fn a_curve_and_a_wheel_draw_their_own_in_full_and_fall_back_where_it_cannot_go() {
        let (r, c) = card_of_three();
        // Full, with the host's drawings: its own.
        let p = resolved(&r, &c, Layout::default());
        let (out, _) = draw(&r, &p, &mut Own);
        assert_eq!(out.drawn["curve"], Drawn::Integrated);
        assert_eq!(out.drawn["lift"], Drawn::Integrated);
        // Full, without drawings: the value type's renderer.
        let (out, names) = draw(&r, &p, &mut NoDrawings);
        assert_eq!(out.drawn["curve"], Drawn::Fallback(Fallback::ValueType));
        assert!(names.iter().any(|n| n == "Curve 2 y"), "{names:?}");
        assert!(names.iter().any(|n| n == "Lift Chroma"), "{names:?}");
        // Compact (a row): the fallback, even with drawings.
        let compact = Layout::default()
            .represent("curve", Representation::Compact)
            .represent("lift", Representation::Compact);
        let p = resolved(&r, &c, compact);
        assert_eq!(
            p.get("curve").unwrap().representation,
            Some(Representation::Compact)
        );
        let (out, names) = draw(&r, &p, &mut Own);
        assert_eq!(out.drawn["curve"], Drawn::Fallback(Fallback::ValueType));
        assert_eq!(out.drawn["lift"], Drawn::Fallback(Fallback::ValueType));
        assert!(names.iter().any(|n| n == "Curve 1 x"), "{names:?}");
    }

    #[test]
    fn folded_parts_go_under_the_more_row_named_with_its_count() {
        let (r, c) = card_of_three();
        let p = resolved(&r, &c, Layout::default());
        assert_eq!(p.more.as_ref().map(|m| m.count), Some(1));
        let (out, names) = draw(&r, &p, &mut Own);
        assert!(!out.drawn.contains_key("amount"), "folded, not drawn");
        assert!(names.iter().any(|n| n == "More, 1"), "{names:?}");
        assert!(out.events.is_empty());
    }

    #[test]
    fn a_number_draws_as_its_look_says() {
        let (r, _) = card_of_three();
        let amount = r.get("implicit:node/n/amount").unwrap();
        for (representation, orientation) in [
            (Representation::Full, Orientation::Horizontal),
            (Representation::Compact, Orientation::Horizontal),
            (Representation::Simplified, Orientation::Horizontal),
            (Representation::Full, Orientation::Vertical),
        ] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let full = ctx.run_ui(egui::RawInput::default(), |ui| {
                let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(120.0, 96.0));
                let mut v = 0.5;
                number_at(
                    ui,
                    amount,
                    &Look::managed(representation, orientation),
                    rect,
                    &mut v,
                );
            });
            let mut full = full;
            full.textures_delta.clear();
            let update = full.platform_output.accesskit_update.unwrap();
            let node = update
                .nodes
                .iter()
                .map(|(_, n)| n)
                .find(|n| n.label() == Some("Amount"))
                .unwrap_or_else(|| panic!("{representation:?} {orientation:?}: no Amount"));
            assert_eq!(node.numeric_value(), Some(0.5));
            let vertical = node.orientation() == Some(egui::accesskit::Orientation::Vertical);
            assert_eq!(
                vertical,
                orientation == Orientation::Vertical && representation == Representation::Full
            );
        }
    }
}
