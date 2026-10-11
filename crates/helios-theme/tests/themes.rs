//! Themes: the built-in ones are complete, inheritance and aliases
//! resolve, problems fall back instead of breaking.

use std::collections::BTreeMap;

use helios_theme::{CONTRACT, Theme, Themes, Value};
use serde_json::json;

#[test]
fn built_in_themes_answer_the_whole_contract_cleanly() {
    let themes = Themes::default();
    for name in ["helios-dark", "helios-light", "helios-high-contrast"] {
        let r = themes.resolve(name, &BTreeMap::new());
        assert!(r.problems.is_empty(), "{name}: {:?}", r.problems);
        assert_eq!(r.values.len(), CONTRACT.len(), "{name}");
    }
}

#[test]
fn a_theme_extends_another_and_aliases_follow() {
    let themes = Themes::default();
    let dark = themes.resolve("helios-dark", &BTreeMap::new());
    let contrast = themes.resolve("helios-high-contrast", &BTreeMap::new());
    // Given by the high contrast theme.
    assert_eq!(contrast.color("bg.panel"), [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(contrast.size("font.size"), 15.0);
    // Inherited from the dark one, through an alias.
    assert_eq!(contrast.color("state.failed"), dark.color("state.failed"));
    assert_eq!(dark.color("state.failed"), dark.color("danger"));
}

#[test]
fn user_overrides_win_and_unknown_ones_are_reported() {
    let themes = Themes::default();
    let overrides = BTreeMap::from([
        ("accent.base".to_string(), json!("#3080ff")),
        ("acent".to_string(), json!("#ff0000")),
    ]);
    let r = themes.resolve("helios-dark", &overrides);
    assert_eq!(
        r.color("accent.base"),
        [48.0 / 255.0, 128.0 / 255.0, 1.0, 1.0]
    );
    assert_eq!(r.problems, ["override `acent`: no such token"]);
}

#[test]
fn a_broken_theme_falls_back_and_says_why() {
    let mut themes = Themes::default();
    let broken = r##"{
      "$extends": "nowhere",
      "fg": { "$type": "color", "text": { "$value": "{fg.weak}" }, "weak": { "$value": "{fg.text}" } },
      "radius": { "widget": { "$type": "color", "$value": "#ffffff" } }
    }"##;
    themes.add(Theme::parse("broken", broken).unwrap());
    let r = themes.resolve("broken", &BTreeMap::new());
    let dark = themes.resolve("helios-dark", &BTreeMap::new());
    assert!(
        r.problems.iter().any(|p| p.contains("no theme `nowhere`")),
        "{:?}",
        r.problems
    );
    assert!(
        r.problems.iter().any(|p| p.contains("alias cycle")),
        "{:?}",
        r.problems
    );
    assert!(
        r.problems
            .iter()
            .any(|p| p.contains("`radius.widget` is a color")),
        "{:?}",
        r.problems
    );
    // Every token still has a value.
    assert_eq!(r.values.len(), CONTRACT.len());
    assert_eq!(r.color("fg.text"), dark.color("fg.text"));
}

#[test]
fn an_exported_theme_reads_back_the_same() {
    let themes = Themes::default();
    let overrides = BTreeMap::from([("danger".to_string(), json!("#ff0000cc"))]);
    let r = themes.resolve("helios-light", &overrides);
    let file = r.to_tokens().to_string();
    let mut again = Themes::default();
    let mut theme = Theme::parse("exported", &file).unwrap();
    // Exported under its own name; added back under another, beside it.
    assert_eq!(theme.name, "helios-light");
    theme.name = "mine".into();
    again.add(theme);
    let back = again.resolve("helios-light", &BTreeMap::new());
    let exported = again.resolve("mine", &BTreeMap::new());
    assert!(exported.problems.is_empty(), "{:?}", exported.problems);
    assert_eq!(exported.values.get("danger"), r.values.get("danger"));
    assert_ne!(back.values.get("danger"), r.values.get("danger"));
    assert!(matches!(exported.values.get("font.ui"), Some(Value::Font(f)) if f[0] == "Ubuntu"));
}
