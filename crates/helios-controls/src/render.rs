//! The renderers (feature `render`): a control drawn with egui-pop's
//! visual parts, under the ergonomic contract. A renderer never holds the
//! value: it is handed the value of the frame and reports the changes,
//! which the caller writes through the value flow ([`crate::flow`]).
//!
//! - [`configure`] and [`detent_hook`] give egui-pop's slider a control's
//!   steps, sign and detents: the `slider` variant in a panel row.
//! - [`scrub`]: the `scrub` variant, a field whose value drags at the
//!   control's sensitivity (1 EV per 200 points for exposure), with fine
//!   mode, detents, double click to reset, arrows and the wheel.
//! - [`chip`]: the HUD read-out (`Exposure +0.67 EV`).
//! - [`show`]: a control in a representation and a variant, what the
//!   controls gallery lays out side by side.

use egui_pop::core::slider::{self, Slider, Verdict};
use egui_pop::egui::{self, Key, Sense, Ui, vec2};

use crate::flow::Cause;
use crate::model::{Control, HIT_AREA, Representation};

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
    let name = label.unwrap_or(&control.label).to_owned();
    response.widget_info(|| egui::WidgetInfo::slider(true, v, &name));
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
/// `None` or unknown).
pub fn show(
    ui: &mut Ui,
    control: &Control,
    representation: Representation,
    variant: Option<&str>,
    value: &mut f64,
) -> Output {
    let variant = control
        .variant_or_default(variant)
        .map_or("slider", |v| v.id.as_str());
    match representation {
        Representation::Full if variant == "slider" => {
            slider_row(ui, control, &control.label, value)
        }
        Representation::Full => {
            let w = ui.available_width().clamp(120.0, 320.0);
            scrub(ui, control, Some(&control.label), value, w)
        }
        Representation::Compact => scrub(ui, control, None, value, 110.0),
        Representation::Simplified => Output {
            response: chip(ui, control, *value),
            changes: Vec::new(),
        },
    }
}
