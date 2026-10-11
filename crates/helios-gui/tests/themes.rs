//! Helios's themes over egui-pop's: they resolve cleanly, carry Helios's
//! own tokens, keep egui-pop's contrast, and overrides behave.

use std::collections::BTreeMap;

use egui_pop::tokens::contrast::{self, Level};
use helios_gui::theme::{self, Extra, HELIOS_TOKENS};
use serde_json::json;

const HELIOS: [&str; 3] = ["helios-dark", "helios-light", "helios-high-contrast"];

#[test]
fn built_in_themes_resolve_cleanly_with_helios_tokens() {
    let themes = theme::themes();
    for name in HELIOS {
        let t = theme::resolve(&themes, name, &BTreeMap::new());
        let r = t.resolved();
        assert!(r.problems().is_empty(), "{name}: {:?}", r.problems());
        for (path, _) in HELIOS_TOKENS {
            assert!(r.color(path).is_some(), "{name}: `{path}` is not a color");
        }
    }
}

#[test]
fn built_in_themes_keep_egui_pops_contrast() {
    let themes = theme::themes();
    for name in HELIOS {
        let level = if name.contains("high-contrast") {
            Level::High
        } else {
            Level::Normal
        };
        let t = theme::resolve(&themes, name, &BTreeMap::new());
        let failures = contrast::check(t.resolved(), level);
        assert!(failures.is_empty(), "{name}: {failures:?}");
        // The destructive button's label reads on its fill.
        let extra = Extra::of(&t);
        let ratio = contrast::contrast_ratio(
            t.resolved().color("button.danger.text").unwrap(),
            t.resolved().color("button.danger.background").unwrap(),
        );
        assert!(ratio >= 4.5, "{name}: {ratio}");
        assert_ne!(extra.danger_background, extra.danger_text);
    }
}

#[test]
fn only_helios_themes_are_offered() {
    let names = theme::choosable(&theme::themes());
    assert_eq!(names.len(), HELIOS.len(), "{names:?}");
    for name in HELIOS {
        assert!(names.iter().any(|n| n == name), "{names:?}");
    }
}

#[test]
fn overrides_win_and_unknown_ones_are_reported() {
    let overrides = BTreeMap::from([
        ("accent.default".to_string(), json!("#3080ff")),
        ("state.done".to_string(), json!("{accent.default}")),
        ("acent".to_string(), json!("#ff0000")),
    ]);
    let t = theme::resolve(&theme::themes(), "helios-dark", &overrides);
    assert_eq!(t.accent.default, egui::Color32::from_rgb(0x30, 0x80, 0xff));
    assert_eq!(Extra::of(&t).state_done, t.accent.default);
    let problems: Vec<String> = t
        .resolved()
        .problems()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("acent"), "{problems:?}");
}

#[test]
fn a_theme_without_helios_tokens_falls_back_to_egui_pops() {
    let t = theme::resolve(&theme::themes(), "pop-light", &BTreeMap::new());
    let extra = Extra::of(&t);
    assert_eq!(extra.state_failed, t.status.danger);
    assert_eq!(extra.viewport_surround, t.surface.canvas);
}
