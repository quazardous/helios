//! The window's own frame, drawn by Helios instead of the system's: a title
//! bar to drag (double click maximizes), the minimize, maximize and close
//! buttons, and resize handles along the edges. Window managers draw
//! frames differently, and some (GNOME on Wayland) leave it to the
//! application; drawing it ourselves makes it look the same everywhere,
//! as VS Code does.

use egui::{
    Color32, CursorIcon, FontId, Rect, ResizeDirection, Sense, Stroke, Ui, Vec2, ViewportCommand,
    pos2, vec2,
};

pub const TITLE_HEIGHT: f32 = 32.0;
const BUTTON_WIDTH: f32 = 46.0;
/// How far from an edge the pointer resizes the window.
const GRIP: f32 = 5.0;

#[derive(Clone, Copy, PartialEq)]
enum Button {
    Minimize,
    Maximize,
    Close,
}

/// Draws a window button and says whether it was clicked.
fn button(ui: &mut Ui, kind: Button, maximized: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(BUTTON_WIDTH, TITLE_HEIGHT), Sense::click());
    let visuals = ui.visuals();
    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();
    let (fill, ink) = match (kind, hovered || pressed) {
        (Button::Close, true) => (Color32::from_rgb(196, 43, 28), Color32::WHITE),
        (_, true) => (
            visuals.widgets.hovered.weak_bg_fill,
            visuals.widgets.hovered.fg_stroke.color,
        ),
        (_, false) => (
            Color32::TRANSPARENT,
            visuals.widgets.inactive.fg_stroke.color,
        ),
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, fill);
    let c = rect.center();
    let s = 5.0;
    let stroke = Stroke::new(1.0, ink);
    match kind {
        Button::Minimize => {
            painter.line_segment([pos2(c.x - s, c.y), pos2(c.x + s, c.y)], stroke);
        }
        Button::Maximize if maximized => {
            // Two overlapping squares: restore.
            let back =
                Rect::from_min_size(pos2(c.x - s + 2.0, c.y - s), Vec2::splat(2.0 * s - 2.0));
            let front =
                Rect::from_min_size(pos2(c.x - s, c.y - s + 2.0), Vec2::splat(2.0 * s - 2.0));
            painter.rect_stroke(back, 1.0, stroke, egui::StrokeKind::Inside);
            painter.rect_filled(front, 1.0, fill);
            painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Inside);
        }
        Button::Maximize => {
            let r = Rect::from_center_size(c, Vec2::splat(2.0 * s));
            painter.rect_stroke(r, 1.0, stroke, egui::StrokeKind::Inside);
        }
        Button::Close => {
            painter.line_segment([pos2(c.x - s, c.y - s), pos2(c.x + s, c.y + s)], stroke);
            painter.line_segment([pos2(c.x + s, c.y - s), pos2(c.x - s, c.y + s)], stroke);
        }
    }
    response
        .on_hover_text(match kind {
            Button::Minimize => "Minimize",
            Button::Maximize if maximized => "Restore",
            Button::Maximize => "Maximize",
            Button::Close => "Close",
        })
        .clicked()
}

/// What the user asked of the window through its frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Minimize,
    ToggleMaximize,
    Close,
}

/// Draws the title bar in `ui` (the top panel's); returns what was asked.
pub fn title_bar(ui: &mut Ui, title: &str) -> Option<Request> {
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let bar = ui.max_rect();
    // The bar itself drags the window; widgets on it take their clicks first.
    let drag = ui.interact(bar, ui.id().with("drag"), Sense::click_and_drag());
    let mut request = None;
    if drag.double_clicked() {
        request = Some(Request::ToggleMaximize);
    } else if drag.drag_started() {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    // The buttons first: a label put on the bar moves the layout's cursor.
    let mut buttons = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bar)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    {
        let ui = &mut buttons;
        ui.spacing_mut().item_spacing.x = 0.0;
        if button(ui, Button::Close, maximized) {
            request = Some(Request::Close);
        }
        if button(ui, Button::Maximize, maximized) {
            request = Some(Request::ToggleMaximize);
        }
        if button(ui, Button::Minimize, maximized) {
            request = Some(Request::Minimize);
        }
    }
    // Labels, not painted text, so assistive technology and tests read
    // them; they take no clicks, so the bar still drags under them.
    let label = |text: &str, weak: bool| {
        let rich = egui::RichText::new(text).font(FontId::proportional(13.0));
        egui::Label::new(if weak { rich.weak() } else { rich })
            .selectable(false)
            .sense(Sense::hover())
    };
    ui.put(
        Rect::from_center_size(bar.center(), vec2(bar.width() / 2.0, TITLE_HEIGHT)),
        label(title, false),
    );
    ui.put(
        Rect::from_min_size(pos2(bar.left() + 12.0, bar.top()), vec2(80.0, TITLE_HEIGHT)),
        label("Helios", true),
    );
    request
}

/// Lets the window be resized from its edges and corners, as a system
/// frame would. Call it before drawing anything else.
pub fn resize_handles(ctx: &egui::Context) {
    let (maximized, fullscreen) = ctx.input(|i| {
        (
            i.viewport().maximized.unwrap_or(false),
            i.viewport().fullscreen.unwrap_or(false),
        )
    });
    if maximized || fullscreen {
        return;
    }
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let r = ctx.content_rect();
    let (left, right) = (pos.x - r.left() < GRIP, r.right() - pos.x < GRIP);
    let (top, bottom) = (pos.y - r.top() < GRIP, r.bottom() - pos.y < GRIP);
    let direction = match (left, right, top, bottom) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (_, true, true, _) => ResizeDirection::NorthEast,
        (true, _, _, true) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, ..) => ResizeDirection::West,
        (_, true, ..) => ResizeDirection::East,
        (_, _, true, _) => ResizeDirection::North,
        (_, _, _, true) => ResizeDirection::South,
        _ => return,
    };
    ctx.set_cursor_icon(match direction {
        ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
        ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
    });
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}
