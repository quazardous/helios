//! A parameter's words in the interface's language: its label, its
//! description, its choices, its value as shown, a slider's gestures.

use egui_pop::egui;
use helios_core::{Message, Value};
use helios_values::Descriptor;

/// The translations of `ctx`, when its plugin is installed.
fn i18n(ctx: &egui::Context) -> Option<egui_pop::i18n::I18n> {
    egui_pop::i18n::plugin::of(ctx)
}

/// A message in the language of `ctx`, else its English.
pub fn text(ctx: &egui::Context, m: impl Into<Message>) -> String {
    let m = m.into();
    match i18n(ctx) {
        Some(i) => helios_i18n::message(&i, &m),
        None => m.english,
    }
}

/// The language `ctx` shows (`en` without the translation plugin).
pub fn locale(ctx: &egui::Context) -> String {
    i18n(ctx).map_or_else(|| "en".into(), |i| i.locale())
}

/// A number with `fraction` digits after the language's decimal
/// separator, trailing zeros kept.
pub fn fixed(locale: &str, value: f64, fraction: usize) -> String {
    let s = format!("{value:.fraction$}");
    let sep = egui_pop::i18n::format::separators(locale);
    if sep.decimal == "." {
        s
    } else {
        s.replace('.', sep.decimal)
    }
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
            let s = fixed(locale, x, d.precision as usize);
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
    key.and_then(|k| i18n(ctx)?.get(&k.key(), &[]))
        .unwrap_or_else(|| english.to_string())
}

/// The parameter's name: the last part of its path.
pub fn param_name(path: &str) -> &str {
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

/// A slider's gestures, as the pointer profile gives them: the finer and
/// coarser modifiers, and when the wheel steps it.
pub fn gesture_hint(ctx: &egui::Context) -> String {
    let g = egui_pop::core::slider::gestures(ctx);
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let keys = |m: egui::Modifiers| egui::ModifierNames::NAMES.format(&m, mac);
    let wheel = if g.wheel_steps {
        Message::new("gui-param-hint-wheel-over", "the wheel over it")
    } else if g.wheel.any() {
        Message::new(
            "gui-param-hint-wheel-focused-or",
            "the wheel once focused or with { $keys }",
        )
        .with("keys", keys(g.wheel))
    } else {
        Message::new("gui-param-hint-wheel-focused", "the wheel once focused")
    };
    text(
        ctx,
        Message::new(
            "gui-param-hint",
            "Drag to change ({ $fine } finer, { $coarse } coarser), arrows to step, { $wheel }; double click resets; Enter types a value or an expression (x is the value)",
        )
        .with("fine", keys(g.fine))
        .with("coarse", keys(g.coarse))
        .with("wheel", wheel),
    )
}
