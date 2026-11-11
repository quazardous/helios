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
//! - A host dresses what the framework draws through [`Drawings`]: the
//!   label shown (a translation), a choice's words, its own touches on a
//!   slider (a picker button), a verdict on each slider change, the marks
//!   beside a part ([`Marks`]: changed, held, moved) and, once a part is
//!   drawn, its response ([`Dressing`]: names, menus, focus, hover).
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
use crate::model::{Control, HIT_AREA, Mode, Representation, Response, ValueSpec, ValueType};
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
    scrub_named(ui, control, label, &control.label, value, width)
}

/// [`scrub`], named `name` to assistive technology and in its tooltip.
fn scrub_named(
    ui: &mut Ui,
    control: &Control,
    label: Option<&str>,
    name: &str,
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
        // The travel before the drag threshold counts too (this frame's
        // delta is added below), or a scrub lands short.
        let before = ui.input(|i| {
            i.pointer
                .press_origin()
                .zip(i.pointer.interact_pos())
                .map_or(0.0, |(o, p)| p.x - o.x)
        }) - response.drag_delta().x;
        let scale = if fine { control.feel.fine } else { 1.0 };
        ui.data_mut(|d| d.insert_temp(start_id, (*value, f64::from(before) * scale)));
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
    response.widget_info(|| egui::WidgetInfo::slider(true, v, name));
    let response = if label.is_none() {
        response.on_hover_text(name)
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

/// The marks beside a part, as the host knows them: drawn in a gutter on
/// each side of the part's slot (the changed dot before it, the held and
/// moved marks after it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    /// Changed from its default: a dot that resets on a click.
    pub changed: bool,
    /// Held by someone else's gesture: who.
    pub held: Option<String>,
    /// Just moved by someone else: who.
    pub moved: Option<String>,
}

/// The gutter before a part that has marks, in points (the changed dot).
pub const MARK_BEFORE: f32 = 10.0;
/// The gutter after a part that has marks, in points (held, moved).
pub const MARK_AFTER: f32 = 12.0;

/// A part just drawn, handed to [`Drawings::dress`].
#[derive(Debug)]
pub struct Dressing<'a> {
    pub key: &'a str,
    pub control: &'a Control,
    /// The part's own rect (its slot less the marks' gutters).
    pub rect: Rect,
    /// The widget that answers for it, when one does (not for a host's
    /// own drawing).
    pub response: Option<&'a egui::Response>,
    /// Its label's own widget, when drawn apart from it (a drop-down's).
    pub label: Option<&'a egui::Response>,
    /// Where its changed dot is, when shown.
    pub changed: Option<Rect>,
    /// Where its moved mark is, when shown.
    pub moved: Option<Rect>,
}

/// The host's own drawings, for its integrated controls (a curve
/// editor, colour wheels): the framework knows nothing of them. The
/// other methods dress what the framework draws; each does nothing by
/// default.
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

    /// The label shown for part `key` (a translation); the control's
    /// when `None`.
    fn label(&mut self, _ctx: &egui::Context, _key: &str, _control: &Control) -> Option<String> {
        None
    }

    /// The words shown for one choice of part `key`; the choice itself
    /// when `None`.
    fn choice(
        &mut self,
        _ctx: &egui::Context,
        _key: &str,
        _control: &Control,
        _choice: &str,
    ) -> Option<String> {
        None
    }

    /// The host's touches on part `key`'s slider before it shows (a
    /// picker button).
    fn slider<'a>(
        &mut self,
        _ctx: &egui::Context,
        _key: &str,
        _control: &Control,
        slider: Slider<'a>,
    ) -> Slider<'a> {
        slider
    }

    /// The host's verdict on a slider's or a knob's change of part `key`,
    /// once the control's detents have held it: a veto or a redirect wins.
    fn verdict(&mut self, _key: &str, _control: &Control, _change: &slider::Change) -> Verdict {
        Verdict::Go
    }

    /// The marks beside part `key`; `None` keeps the part's whole slot.
    fn marks(&mut self, _key: &str) -> Option<Marks> {
        None
    }

    /// Called once a part is drawn: the host's own touches on its
    /// response (names, menus, focus, hover text).
    fn dress(&mut self, _ui: &mut Ui, _dressing: Dressing<'_>) {}
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
    /// The widget that answers for it, when one does.
    pub response: Option<egui::Response>,
    /// Its label's own widget, when drawn apart from it.
    pub label: Option<egui::Response>,
    /// Its quad button (a picker) was clicked.
    pub quad: bool,
}

/// A card's request to its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardEvent {
    /// Show a folded part: `controls.reveal {node, key}`.
    Reveal(String),
    /// Show every part: `controls.reveal_all {node}`.
    RevealAll,
    /// Put a part back to its default (its changed dot clicked).
    Reset(String),
    /// A part's quad button (a picker) was clicked.
    Quad(String),
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
    let mut events = Vec::new();
    for p in placement.items.iter().filter(|p| p.visibility.is_shown()) {
        let Some(r) = p.rect else { continue };
        let slot = Rect::from_min_size(origin + vec2(r.x, r.y), vec2(r.w, r.h)).shrink(GAP / 2.0);
        if p.kind == ItemKind::Chrome {
            let mut child = part_ui(ui, &p.key, slot);
            chrome(&mut child, p, slot);
            continue;
        }
        let Some(control) = p.control.as_deref().and_then(|id| registry.get(id)) else {
            continue;
        };
        let marks = drawings.marks(&p.key);
        let rect = if marks.is_some() {
            Rect::from_min_max(
                slot.min + vec2(MARK_BEFORE, 0.0),
                slot.max - vec2(MARK_AFTER, 0.0),
            )
        } else {
            slot
        };
        let (changed, moved) = match &marks {
            Some(m) => draw_marks(ui, &p.key, m, slot, rect, &mut events),
            None => (None, None),
        };
        let mut child = part_ui(ui, &p.key, rect);
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
        if out.quad {
            events.push(CardEvent::Quad(p.key.clone()));
        }
        drawings.dress(
            &mut child,
            Dressing {
                key: &p.key,
                control,
                rect,
                response: out.response.as_ref(),
                label: out.label.as_ref(),
                changed,
                moved,
            },
        );
        edits.extend(out.edits);
        drawn.insert(p.key.clone(), out.drawn);
    }
    if let Some(m) = &placement.more
        && m.count + m.inapplicable.len() > 0
    {
        let toolbar = placement
            .items
            .first()
            .is_some_and(|p| p.mode == Mode::Toolbar);
        events.extend(more(ui, placement, toolbar));
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

/// A child ui for one part in `rect`: left to right, clipped to it.
fn part_ui(ui: &mut Ui, key: &str, rect: Rect) -> Ui {
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .id_salt(("hud-part", key))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    // A part never draws past its slot.
    child.set_clip_rect(rect.expand(1.0).intersect(ui.clip_rect()));
    child
}

/// A part's marks in its gutters: the changed dot before it (a click
/// asks for a reset), then the held and moved marks after it. Returns
/// where the changed dot and the moved mark went.
fn draw_marks(
    ui: &mut Ui,
    key: &str,
    marks: &Marks,
    slot: Rect,
    part: Rect,
    events: &mut Vec<CardEvent>,
) -> (Option<Rect>, Option<Rect>) {
    let y = part.center().y;
    let row = 18.0_f32.min(slot.height());
    // The dot's place answers clicks whether shown or not, so a part's
    // focus order and its first stop do not change with its value.
    let dot = Rect::from_center_size(
        pos2(slot.left() + MARK_BEFORE / 2.0, y),
        vec2(MARK_BEFORE, row),
    );
    let r = ui.interact(
        dot,
        ui.make_persistent_id(("hud-changed", key)),
        Sense::click(),
    );
    let mut changed = None;
    if marks.changed {
        ui.painter()
            .circle_filled(dot.center(), 3.0, ui.visuals().selection.stroke.color);
        let r = r.on_hover_text("Changed from its default: click to reset");
        if r.clicked() {
            events.push(CardEvent::Reset(key.to_owned()));
        }
        changed = Some(dot);
    }
    let after = |dy: f32| {
        Rect::from_center_size(
            pos2(slot.right() - MARK_AFTER / 2.0, y + dy),
            vec2(MARK_AFTER, row / 2.0),
        )
    };
    // Both: the held mark above, the moved one below.
    let both = marks.held.is_some() && marks.moved.is_some();
    let shift = if both { row / 4.0 } else { 0.0 };
    if let Some(by) = &marks.held {
        let at = after(-shift);
        let r = ui.interact(at, ui.make_persistent_id(("hud-held", key)), Sense::hover());
        ui.painter().circle_filled(
            at.center(),
            4.0_f32.min(at.height() / 2.0),
            ui.visuals().warn_fg_color,
        );
        r.on_hover_text(format!("Held by {by}"));
    }
    let mut moved = None;
    if let Some(by) = &marks.moved {
        let at = after(shift);
        let r = ui.interact(
            at,
            ui.make_persistent_id(("hud-moved", key)),
            Sense::hover(),
        );
        let c = at.center();
        let d = 4.5_f32.min(at.height() / 2.0);
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                c + vec2(0.0, -d),
                c + vec2(d, 0.0),
                c + vec2(0.0, d),
                c + vec2(-d, 0.0),
            ],
            ui.visuals().selection.stroke.color,
            egui::Stroke::NONE,
        ));
        r.on_hover_text(format!("Moved by {by}"));
        moved = Some(at);
    }
    (changed, moved)
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
    let only = |edits: Vec<Edit>, drawn: Drawn| PartOutput {
        edits,
        drawn,
        response: None,
        label: None,
        quad: false,
    };
    let with = |m: Managed, drawn: Drawn| PartOutput {
        edits: m.edits,
        drawn,
        response: m.response,
        label: m.label,
        quad: m.quad,
    };
    if let Integration::Integrated { fallback } = &look.integration {
        // Its own drawing goes only where its full form was given.
        if look.representation == Representation::Full
            && let Some(edits) = drawings.draw(ui, key, control, rect, value)
        {
            return only(edits, Drawn::Integrated);
        }
        let Some(fallback) = fallback else {
            missing(ui, control, rect);
            return only(Vec::new(), Drawn::Missing);
        };
        let drawn = Drawn::Fallback(fallback.clone());
        return match fallback {
            Fallback::Variant(id) => {
                let look = Look {
                    variant: Some(id.clone()),
                    integration: Integration::Managed,
                    ..look.clone()
                };
                with(
                    managed(ui, key, control, &look, rect, value, drawings),
                    drawn,
                )
            }
            Fallback::ValueType => {
                let look = Look {
                    integration: Integration::Managed,
                    ..look.clone()
                };
                with(
                    managed(ui, key, control, &look, rect, value, drawings),
                    drawn,
                )
            }
            Fallback::Parts => only(
                parts(ui, registry, key, control, rect, value, drawings),
                drawn,
            ),
        };
    }
    with(
        managed(ui, key, control, look, rect, value, drawings),
        Drawn::Managed,
    )
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

/// What a managed part drew.
struct Managed {
    edits: Vec<Edit>,
    response: Option<egui::Response>,
    label: Option<egui::Response>,
    quad: bool,
}

impl Managed {
    fn edits(edits: Vec<Edit>) -> Self {
        Self {
            edits,
            response: None,
            label: None,
            quad: false,
        }
    }

    fn of(edits: Vec<Edit>, response: egui::Response) -> Self {
        Self {
            edits,
            response: Some(response),
            label: None,
            quad: false,
        }
    }
}

/// A managed part by its value type, dressed by the host.
fn managed(
    ui: &mut Ui,
    key: &str,
    control: &Control,
    look: &Look,
    rect: Rect,
    value: &mut Value,
    drawings: &mut dyn Drawings,
) -> Managed {
    let ctx = ui.ctx().clone();
    let label = drawings
        .label(&ctx, key, control)
        .unwrap_or_else(|| control.label.clone());
    let label = label.as_str();
    let set = |value: &Value, done: bool| {
        vec![Edit::Set {
            key: key.to_owned(),
            to: value.clone(),
            done,
        }]
    };
    match &control.value {
        ValueSpec::Number(n) => {
            let mut v = value.as_f64().unwrap_or(n.default);
            let (out, quad) = number_with(ui, key, control, label, look, rect, &mut v, drawings);
            *value = if n.integer {
                Value::Int(v.round() as i64)
            } else {
                Value::Float(v)
            };
            let edits = out
                .changes
                .into_iter()
                .map(|change| Edit::Number {
                    key: key.to_owned(),
                    change,
                })
                .collect();
            Managed {
                quad,
                ..Managed::of(edits, out.response)
            }
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
            let face = control
                .face_or_default(look.variant.as_deref())
                .map_or("checkbox", |v| v.id.as_str());
            let r = match face {
                "push-button" => {
                    ui.toggle_value(&mut on, if shown.is_empty() { "●" } else { shown })
                }
                "lamp" => lamp(ui, on, shown),
                // The switch's stand-in: the checkbox, until egui-pop has one.
                _ => egui_pop::core::widgets::checkbox(ui, &mut on, shown),
            };
            let r = if shown.is_empty() {
                r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, label));
                r.on_hover_text(label)
            } else {
                r
            };
            let mut edits = Vec::new();
            if r.changed() {
                *value = Value::Bool(on);
                edits = set(value, true);
            }
            Managed::of(edits, r)
        }
        ValueSpec::OneOf { choices, default } => {
            let mut chosen = match value {
                Value::Choice(c) => c.clone(),
                _ => default.clone(),
            };
            let before = chosen.clone();
            let full = look.representation == Representation::Full;
            let label_r = full.then(|| ui.label(label));
            let width = ui.available_width();
            let words: Vec<String> = choices
                .iter()
                .map(|c| {
                    drawings
                        .choice(&ctx, key, control, c)
                        .unwrap_or_else(|| c.clone())
                })
                .collect();
            let shown = choices
                .iter()
                .position(|c| *c == chosen)
                .map_or_else(|| chosen.clone(), |i| words[i].clone());
            let face = control
                .face_or_default(look.variant.as_deref())
                .map_or("menu", |v| v.id.as_str());
            let r = match face {
                "segmented" => {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        for (c, w) in choices.iter().zip(&words) {
                            ui.selectable_value(&mut chosen, c.clone(), w);
                        }
                    })
                    .response
                }
                "radio" => {
                    ui.vertical(|ui| {
                        for (c, w) in choices.iter().zip(&words) {
                            ui.radio_value(&mut chosen, c.clone(), w);
                        }
                    })
                    .response
                }
                _ => {
                    egui::ComboBox::from_id_salt(("hud-choice", key))
                        .selected_text(shown)
                        .width(width)
                        .show_ui(ui, |ui| {
                            for (c, w) in choices.iter().zip(&words) {
                                ui.selectable_value(&mut chosen, c.clone(), w);
                            }
                        })
                        .response
                }
            };
            let r = match &label_r {
                Some(l) => r.labelled_by(l.id),
                None => r.on_hover_text(label),
            };
            let mut edits = Vec::new();
            if chosen != before {
                *value = Value::Choice(chosen);
                edits = set(value, true);
            }
            Managed {
                label: label_r,
                ..Managed::of(edits, r)
            }
        }
        ValueSpec::Other { stored } => match stored {
            ValueType::Points => Managed::edits(points(ui, key, label, rect, value)),
            ValueType::Colour => Managed::edits(colour(ui, key, label, rect, value)),
            ValueType::Text => {
                let mut text = match value {
                    Value::Text(t) => t.clone(),
                    _ => String::new(),
                };
                let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(rect.width()));
                let r = r.on_hover_text(label);
                let mut edits = Vec::new();
                if r.changed() {
                    *value = Value::Text(text);
                }
                // Typing is reported as it goes; leaving the field ends it.
                if r.changed() || r.lost_focus() {
                    edits = set(value, r.lost_focus());
                }
                Managed::of(edits, r)
            }
            _ => {
                let r = ui.label(format!("{label}: {}", value.to_json()));
                Managed::of(Vec::new(), r)
            }
        },
    }
}

/// The `lamp` face: a light, lit when the value is on, and its label;
/// read only.
fn lamp(ui: &mut Ui, on: bool, label: &str) -> egui::Response {
    let side = ui.spacing().interact_size.y;
    let r = ui
        .horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
            let v = ui.visuals();
            let fill = if on {
                v.selection.bg_fill
            } else {
                v.extreme_bg_color
            };
            ui.painter().circle(
                rect.center(),
                side * 0.3,
                fill,
                egui::Stroke::new(1.0, v.widgets.inactive.fg_stroke.color),
            );
            if !label.is_empty() {
                ui.label(label);
            }
        })
        .response;
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, false, on, label));
    r
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
            .id_salt(("hud-points", key))
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
    let label = control.label.clone();
    number_with(
        ui,
        &control.id,
        control,
        &label,
        look,
        rect,
        value,
        &mut NoDrawings,
    )
    .0
}

/// A slider or knob hook: the control's detents (unless `free`), then
/// the host's verdict.
fn hooked<'a>(
    key: &'a str,
    control: &'a Control,
    free: bool,
    drawings: &'a mut dyn Drawings,
) -> impl FnMut(&slider::Change) -> Verdict + 'a {
    move |c| {
        let mut change = *c;
        if c.cause == slider::Cause::Drag {
            change.to = control.detented(c.to, free);
        }
        match drawings.verdict(key, control, &change) {
            Verdict::Veto => return Verdict::Veto,
            Verdict::Redirect(v) => change.to = v,
            Verdict::Go => {}
        }
        if change.to == c.to {
            Verdict::Go
        } else {
            Verdict::Redirect(change.to)
        }
    }
}

/// [`number_at`] for part `key`, labelled `label`, dressed by the host;
/// also says whether its quad button was clicked.
#[allow(
    clippy::too_many_arguments,
    reason = "a part's key, control, label, look, rect and value, and its host"
)]
fn number_with(
    ui: &mut Ui,
    key: &str,
    control: &Control,
    label: &str,
    look: &Look,
    rect: Rect,
    value: &mut f64,
    drawings: &mut dyn Drawings,
) -> (Output, bool) {
    let face = control
        .face_or_default(look.variant.as_deref())
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
    let ctx = ui.ctx().clone();
    match (face, look.representation) {
        ("meter", Representation::Full) => (
            meter(ui, control, label, *value, rect.size(), vertical),
            false,
        ),
        // A meter in less room reads its value.
        ("readout" | "meter", _) => (readout(ui, control, label, *value, rect.width()), false),
        ("scrub", Representation::Full) => (
            scrub_named(ui, control, Some(label), label, value, rect.width()),
            false,
        ),
        (_, Representation::Simplified) | ("scrub", _) => (
            scrub_named(ui, control, None, label, value, rect.width()),
            false,
        ),
        // The endless knob's stand-in: the knob, until egui-pop has one.
        ("knob" | "endless-knob", _) => (
            knob_in(ui, key, control, label, rect, value, drawings),
            false,
        ),
        (_, representation) => {
            let soft = control.value.number().map_or((0.0, 1.0), |n| n.soft);
            let free = detents_off(ui);
            let size = if vertical {
                rect.size()
            } else {
                vec2(rect.width(), row.min(rect.height()))
            };
            let mut s = configure(Slider::new(value, label, soft.0..=soft.1), control).size(size);
            if control.feel.response == Response::Logarithmic {
                s = s.logarithmic();
            }
            if vertical {
                s = s.vertical();
            }
            if representation == Representation::Compact {
                s = s.compact();
            }
            let s = drawings.slider(&ctx, key, control, s);
            let out = s.on_change(hooked(key, control, free, drawings)).show(ui);
            let quad = out.quad_clicked();
            (slider_output(out.response, &out.changes, *value), quad)
        }
    }
}

/// The `readout` face: the value as the control formats it, read only.
fn readout(ui: &mut Ui, control: &Control, label: &str, value: f64, width: f32) -> Output {
    let text = control
        .value
        .number()
        .map_or_else(|| value.to_string(), |n| n.format(value));
    let height = ui.spacing().interact_size.y.max(HIT_AREA);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let v = ui.visuals();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        &text,
        egui::TextStyle::Button.resolve(ui.style()),
        v.text_color(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Label, true, format!("{label} {text}"))
    });
    Output {
        response: response.on_hover_text(label),
        changes: Vec::new(),
    }
}

/// The `meter` face: a bar filled to the value over the soft range, read
/// only, along its orientation.
fn meter(
    ui: &mut Ui,
    control: &Control,
    label: &str,
    value: f64,
    size: egui::Vec2,
    vertical: bool,
) -> Output {
    let (lo, hi) = control.value.number().map_or((0.0, 1.0), |n| n.soft);
    let share = if hi > lo {
        ((value - lo) / (hi - lo)).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let v = ui.visuals();
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, v.extreme_bg_color);
    let fill = if vertical {
        Rect::from_min_max(
            pos2(rect.left(), rect.bottom() - rect.height() * share),
            rect.max,
        )
    } else {
        Rect::from_min_max(
            rect.min,
            pos2(rect.left() + rect.width() * share, rect.bottom()),
        )
    };
    painter.rect_filled(fill, 2.0, v.selection.bg_fill);
    let text = control
        .value
        .number()
        .map_or_else(|| value.to_string(), |n| n.format(value));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::ProgressIndicator,
            true,
            format!("{label} {text}"),
        )
    });
    Output {
        response: response.on_hover_text(format!("{label}: {text}")),
        changes: Vec::new(),
    }
}

/// A knob in `rect`, with the control's steps and detents.
fn knob_in(
    ui: &mut Ui,
    key: &str,
    control: &Control,
    label: &str,
    rect: Rect,
    value: &mut f64,
    drawings: &mut dyn Drawings,
) -> Output {
    let soft = control.value.number().map_or((0.0, 1.0), |n| n.soft);
    let free = detents_off(ui);
    let mut k = Knob::new(value, label, soft.0..=soft.1).size(rect.size());
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
    let out = k.on_change(hooked(key, control, free, drawings)).show(ui);
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
    let id = ui.make_persistent_id("hud-more");
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
        sections::band(ui, "hud-more", "More", Some(count), open)
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

    /// A host that renames every part, marks it changed and moved, and
    /// counts what it dressed.
    #[derive(Default)]
    struct Dressed {
        dressed: Vec<(String, bool, bool, bool)>,
    }

    impl Drawings for Dressed {
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

        fn label(&mut self, _: &egui::Context, key: &str, _: &Control) -> Option<String> {
            Some(format!("{key} renamed"))
        }

        fn marks(&mut self, _: &str) -> Option<Marks> {
            Some(Marks {
                changed: true,
                held: None,
                moved: Some("an agent".into()),
            })
        }

        fn dress(&mut self, _: &mut Ui, d: Dressing<'_>) {
            self.dressed.push((
                d.key.to_owned(),
                d.response.is_some(),
                d.changed.is_some(),
                d.moved.is_some(),
            ));
        }
    }

    #[test]
    fn a_host_dresses_each_part_its_label_and_its_marks_in_gutters() {
        let (r, c) = card_of_three();
        let p = resolved(&r, &c, Layout::default());
        let mut host = Dressed::default();
        let (out, names) = draw(&r, &p, &mut host);
        assert!(out.events.is_empty());
        // The curve and the wheel fall back to their rows, renamed.
        assert!(names.iter().any(|n| n == "curve renamed 2 y"), "{names:?}");
        for (key, _, changed, moved) in &host.dressed {
            assert!(*changed && *moved, "{key}: marks in the gutters");
        }
        assert_eq!(host.dressed.len(), 2, "the folded part is not drawn");
    }
}
