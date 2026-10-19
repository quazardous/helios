//! Helios's themes: thin overlays of egui-pop's themes (`egui_pop::theme`),
//! in the Design Tokens format (DTCG). egui-pop resolves them, maps them
//! onto egui's visuals and style, and keeps the typed tokens in the
//! context ([`Tokens::of`]). Widgets read the egui style or the tokens,
//! never literal colors.
//!
//! Each Helios theme `$extends` an egui-pop base (`pop-dark`, `pop-light`,
//! `pop-high-contrast`) and keeps only what is Helios's own: its component
//! tokens ([`HELIOS_TOKENS`]), which egui-pop's contract does not have,
//! among them a neutral surround behind photos. A theme without them (a user's
//! theme extending an egui-pop base directly) still works: [`Extra`] falls
//! back to the nearest egui-pop token.

use std::collections::BTreeMap;

use egui::Color32;
pub use egui_pop::theme::{ThemeSpec, Themes, Tokens};
use serde_json::Value as Json;

/// The theme Helios starts with, and falls back to.
pub const DEFAULT: &str = "helios-dark";

/// Helios's built-in themes, in the order they are offered.
const BUILT_IN: &[(&str, &str)] = &[
    (
        "helios-dark",
        include_str!("../themes/helios-dark.tokens.json"),
    ),
    (
        "helios-light",
        include_str!("../themes/helios-light.tokens.json"),
    ),
    (
        "helios-high-contrast",
        include_str!("../themes/helios-high-contrast.tokens.json"),
    ),
];

/// The tokens Helios reads beyond egui-pop's contract: color tokens, with
/// their meaning.
pub const HELIOS_TOKENS: &[(&str, &str)] = &[
    (
        "viewport.surround",
        "behind the photo being edited, neutral",
    ),
    ("state.done", "a node computed and current"),
    ("state.running", "a node being computed"),
    ("state.stale", "a node whose content is out of date"),
    ("state.failed", "a node whose computation failed"),
    ("state.pending", "a decision waiting for an answer"),
    (
        "button.danger.background",
        "the primary button of a destructive dialog",
    ),
    ("button.danger.text", "the label on that button"),
];

/// egui-pop's built-in themes and Helios's.
pub fn themes() -> Themes {
    let mut themes = Themes::default();
    for (name, text) in BUILT_IN {
        themes.add_text(name, text).expect("built-in themes parse");
    }
    themes
}

/// The themes a user picks from: every theme known but egui-pop's bases,
/// which Helios's own extend.
pub fn choosable(themes: &Themes) -> Vec<String> {
    themes
        .names()
        .filter(|n| !egui_pop::theme::BUILT_IN_THEMES.contains(n))
        .map(String::from)
        .collect()
}

/// A theme resolved with the user's overrides (raw values by token path),
/// ready to install. An unknown theme name is reported by egui-pop in the
/// problems; the caller decides what to fall back to.
pub fn resolve(themes: &Themes, name: &str, overrides: &BTreeMap<String, Json>) -> Tokens {
    let mut spec = ThemeSpec::new(name);
    spec.overrides.clone_from(overrides);
    Tokens::new(themes.resolve(&spec))
}

/// Helios's own tokens, typed, each with an egui-pop fallback.
#[derive(Clone, Debug, PartialEq)]
pub struct Extra {
    pub viewport_surround: Color32,
    pub state_done: Color32,
    pub state_running: Color32,
    pub state_stale: Color32,
    pub state_failed: Color32,
    pub state_pending: Color32,
    pub danger_background: Color32,
    pub danger_text: Color32,
}

impl Extra {
    pub fn of(t: &Tokens) -> Extra {
        let c = |path: &str, or: Color32| t.color(path).unwrap_or(or);
        Extra {
            viewport_surround: c("viewport.surround", t.surface.canvas),
            state_done: c("state.done", t.status.success),
            state_running: c("state.running", t.status.info),
            state_stale: c("state.stale", t.text.muted),
            state_failed: c("state.failed", t.status.danger),
            state_pending: c("state.pending", t.accent.default),
            danger_background: c("button.danger.background", t.status.danger),
            danger_text: c("button.danger.text", t.surface.panel),
        }
    }
}
