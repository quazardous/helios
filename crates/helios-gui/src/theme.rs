//! A resolved theme (`helios-theme`) as egui's visuals and style: the one
//! place tokens become egui values. Widgets read the egui style or the
//! theme's tokens, never literal colors.
//!
//! Font families are not applied yet: egui's bundled fonts stay until the
//! font files a theme names can be loaded.

use egui::{Color32, CornerRadius, Stroke, TextStyle};
use helios_theme::Resolved;

pub fn color(r: &Resolved, token: &str) -> Color32 {
    let [cr, cg, cb, ca] = r.color(token);
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(b(cr), b(cg), b(cb), b(ca))
}

fn radius(r: &Resolved, token: &str) -> CornerRadius {
    CornerRadius::same(r.size(token).round().clamp(0.0, 255.0) as u8)
}

/// Whether a color is a dark one (for egui's dark mode switches).
fn dark(c: Color32) -> bool {
    let l = 0.2126 * f32::from(c.r()) + 0.7152 * f32::from(c.g()) + 0.0722 * f32::from(c.b());
    l < 128.0
}

/// Applies a theme to egui.
pub fn apply(ctx: &egui::Context, r: &Resolved) {
    let panel = color(r, "bg.panel");
    let text = color(r, "fg.text");
    let border = color(r, "border");
    let accent = color(r, "accent.base");
    let widget_radius = radius(r, "radius.widget");
    let mut visuals = if dark(panel) {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = panel;
    visuals.window_fill = color(r, "bg.window");
    visuals.extreme_bg_color = color(r, "bg.extreme");
    visuals.text_edit_bg_color = Some(color(r, "bg.extreme"));
    visuals.faint_bg_color = color(r, "bg.widget");
    visuals.code_bg_color = color(r, "bg.widget");
    visuals.window_stroke = Stroke::new(1.0, border);
    visuals.window_corner_radius = radius(r, "radius.window");
    visuals.menu_corner_radius = radius(r, "radius.window");
    visuals.hyperlink_color = accent;
    visuals.warn_fg_color = color(r, "warning");
    visuals.error_fg_color = color(r, "danger");
    visuals.weak_text_color = Some(color(r, "fg.weak"));
    visuals.selection.bg_fill = color(r, "selection");
    visuals.selection.stroke = Stroke::new(1.0, accent);
    let w = &mut visuals.widgets;
    for (state, fill) in [
        (&mut w.noninteractive, panel),
        (&mut w.inactive, color(r, "bg.widget")),
        (&mut w.hovered, color(r, "bg.widget_hover")),
        (&mut w.active, color(r, "bg.widget_active")),
        (&mut w.open, color(r, "bg.widget_hover")),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.fg_stroke = Stroke::new(1.0, text);
        state.corner_radius = widget_radius;
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, border);
    w.hovered.bg_stroke = Stroke::new(1.0, border);
    w.active.bg_stroke = Stroke::new(1.0, accent);
    w.hovered.fg_stroke = Stroke::new(1.0, color(r, "fg.strong"));
    w.active.fg_stroke = Stroke::new(1.0, color(r, "fg.strong"));

    let size = r.size("font.size").max(6.0);
    let space = r.size("space.item");
    ctx.all_styles_mut(|style| {
        style.visuals = visuals.clone();
        let fonts = &mut style.text_styles;
        for (text_style, scale) in [
            (TextStyle::Body, 1.0),
            (TextStyle::Button, 1.0),
            (TextStyle::Monospace, 1.0),
            (TextStyle::Small, 0.85),
            (TextStyle::Heading, 1.4),
        ] {
            if let Some(font) = fonts.get_mut(&text_style) {
                font.size = size * scale;
            }
        }
        style.spacing.item_spacing = egui::vec2(space, space * 0.66);
    });
}
