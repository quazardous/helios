//! Helios's text and numbers in the interface's language.
//!
//! What Helios builds for people (a toast, a dialog's title, a refusal) is
//! a [`Message`]: a key with its English (`helios_core::Message`, found by
//! `cargo xtask i18n extract`). The shell and the panels show it through
//! the catalogues of their egui context (each window's, each test's own),
//! never through a process-wide one. Numbers are written with the
//! language's separators (`0,50` in French).

use egui_pop::i18n::I18n;
use helios_core::Message;

/// The catalogues egui-pop's translation plugin installed in `ctx`.
pub fn of(ctx: &egui::Context) -> Option<I18n> {
    egui_pop::i18n::plugin::of(ctx)
}

/// A message in the language of `ctx`, else its English.
pub fn text(ctx: &egui::Context, m: impl Into<Message>) -> String {
    let m = m.into();
    match of(ctx) {
        Some(i) => helios_i18n::message(&i, &m),
        None => m.english,
    }
}

/// The language `ctx` shows (`en` without the translation plugin).
pub fn locale(ctx: &egui::Context) -> String {
    of(ctx).map_or_else(|| "en".into(), |i| i.locale())
}

/// A number with `fraction` digits after the language's decimal
/// separator, trailing zeros kept (a slider's value keeps its width).
pub fn fixed(locale: &str, value: f64, fraction: usize) -> String {
    let s = format!("{value:.fraction$}");
    let sep = egui_pop::i18n::format::separators(locale);
    if sep.decimal == "." {
        s
    } else {
        s.replace('.', sep.decimal)
    }
}

/// A number with at most `fraction` digits, trailing zeros dropped and
/// thousands grouped, as the language writes it.
pub fn number(locale: &str, value: f64, fraction: usize) -> String {
    egui_pop::i18n::format::number(locale, value, Some(fraction))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_follow_the_language() {
        assert_eq!(fixed("fr", 0.5, 2), "0,50");
        assert_eq!(fixed("en", -1.26, 1), "-1.3");
        assert_eq!(fixed("de", 3.0, 0), "3");
        assert_eq!(number("fr", 12345.678, 1), "12\u{202f}345,7");
    }
}
