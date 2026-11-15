//! The presets headless: each draws its egui-pop widget, named for
//! assistive technologies and agents, in each of its states.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_pop::core::motion;
use egui_pop::core::ui::Tone;
use egui_pop::egui::accesskit::Role;
use egui_pop::egui::{self, Ui};
use egui_pop::theme::{self, ThemeSpec};
use helios_ui::{Action, Empty, Loading, Notice, Search, Section, Value};

/// A harness drawing `draw` in Helios's base theme, its motion clock
/// frozen at 0 s (a busy value started at -10 s shows its spinner).
fn harness<S: 'static>(state: S, draw: fn(&mut Ui, &mut S)) -> Harness<'static, S> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(480.0, 360.0))
        .build_ui_state(
            move |ui, s: &mut S| {
                motion::freeze(ui.ctx(), 0.0);
                draw(ui, s);
            },
            state,
        );
    theme::install(&h.ctx, &ThemeSpec::new("pop-dark"));
    h.run();
    h
}

#[test]
fn every_search_is_a_text_input_named_by_its_scope() {
    for search in Search::ALL {
        let h = harness((String::new(), search), |ui, (q, s)| {
            let _ = helios_ui::search(ui, q, *s);
        });
        let node = h.get_by_label(search.label());
        assert_eq!(node.accesskit_node().role(), Role::TextInput, "{search:?}");
    }
    let labels: std::collections::BTreeSet<_> = Search::ALL.iter().map(|s| s.label()).collect();
    assert_eq!(labels.len(), Search::ALL.len(), "one name per scope");
}

#[test]
fn a_search_keeps_what_is_typed() {
    let mut h = harness(String::new(), |ui, q| {
        let _ = helios_ui::search(ui, q, Search::Library);
    });
    h.get_by_label(Search::Library.label()).focus();
    h.run();
    h.get_by_label(Search::Library.label()).type_text("beach");
    h.run();
    assert_eq!(h.state(), "beach");
}

#[test]
fn actions_are_buttons_in_their_tone() {
    assert_eq!(Action::Primary.tone(), Tone::Accent);
    assert_eq!(Action::Secondary.tone(), Tone::Neutral);
    assert_eq!(Action::Danger.tone(), Tone::Danger);
    let mut h = harness(None::<Action>, |ui, clicked| {
        ui.horizontal(|ui| {
            for kind in Action::ALL {
                if helios_ui::action(ui, kind, kind.name()).clicked() {
                    *clicked = Some(kind);
                }
            }
        });
    });
    for kind in Action::ALL {
        let node = h.get_by_label(kind.name());
        assert_eq!(node.accesskit_node().role(), Role::Button, "{kind:?}");
    }
    h.get_by_label("primary").click();
    h.run();
    assert_eq!(*h.state(), Some(Action::Primary));
}

#[test]
fn a_section_shows_its_title_and_description() {
    let h = harness((), |ui, ()| {
        let _ = helios_ui::section(
            ui,
            &Section::new("Copy").description("Where the photos go."),
        );
    });
    assert_eq!(h.get_by_label("Copy").accesskit_node().role(), Role::Label);
    h.get_by_label("Where the photos go.");
}

#[test]
fn every_empty_case_says_why_and_offers_its_action() {
    for case in Empty::ALL {
        let mut h = harness((case, false), |ui, (case, clicked)| {
            if helios_ui::empty(ui, *case) {
                *clicked = true;
            }
        });
        h.get_by_label(case.title());
        h.get_by_label(case.text());
        match case.action() {
            Some(action) => {
                let button = h.get_by_label(action);
                assert_eq!(button.accesskit_node().role(), Role::Button, "{case:?}");
                button.click();
                h.run();
                assert!(h.state().1, "{case:?}: the action is reported");
            }
            None => assert!(h.query_by_role(Role::Button).is_none(), "{case:?}"),
        }
    }
}

#[test]
fn a_failure_says_its_reason_and_offers_retry() {
    let mut h = harness(false, |ui, retried| {
        if helios_ui::failed(ui, "Could not read the library", "The disk did not answer.") {
            *retried = true;
        }
    });
    h.get_by_label("Could not read the library");
    h.get_by_label("The disk did not answer.");
    h.get_by_label("Retry").click();
    h.run();
    assert!(*h.state());
}

#[test]
fn loading_is_a_busy_region() {
    for shape in [Loading::Text(2), Loading::Rows(3)] {
        let h = harness(shape, |ui, shape| {
            let _ = helios_ui::loading(ui, *shape);
        });
        assert!(
            h.get_by_label("Loading").accesskit_node().is_busy(),
            "{shape:?}"
        );
    }
}

fn spinners(h: &Harness<'_, Value<String>>) -> usize {
    h.query_all_by_role(Role::ProgressIndicator).count()
}

fn value_harness(v: Value<String>) -> Harness<'static, Value<String>> {
    harness(v, |ui, v| {
        let _ = helios_ui::value(ui, "Size", v);
    })
}

#[test]
fn a_value_is_drawn_in_its_state() {
    let fresh = value_harness(Value::Fresh("12 GB".into()));
    fresh.get_by_label("12 GB");
    assert_eq!(spinners(&fresh), 0);

    let pending = value_harness(Value::Pending { since: -10.0 });
    assert_eq!(spinners(&pending), 1, "a long fetch shows its spinner");

    let quick = value_harness(Value::Pending { since: 0.0 });
    assert_eq!(
        spinners(&quick),
        0,
        "a fetch just started shows nothing yet"
    );

    let refreshing = value_harness(Value::Refreshing {
        value: "11 GB".into(),
        since: -10.0,
    });
    refreshing.get_by_label("11 GB");
    assert_eq!(spinners(&refreshing), 1);

    let failed = value_harness(Value::Failed {
        reason: "The disk did not answer".into(),
    });
    let mark = failed.get_by_role(Role::Image);
    let said = format!(
        "{} {}",
        mark.accesskit_node().label().unwrap_or_default(),
        mark.accesskit_node().description().unwrap_or_default()
    );
    assert!(said.contains("Size: failed"), "{said}");
    assert_eq!(spinners(&failed), 0);

    assert_eq!(
        [
            Value::Fresh(1).state(),
            Value::<i32>::Pending { since: 0.0 }.state(),
            Value::Refreshing {
                value: 1,
                since: 0.0
            }
            .state(),
            Value::<i32>::Failed {
                reason: String::new()
            }
            .state(),
        ],
        ["fresh", "pending", "refreshing", "failed"]
    );
}

#[test]
fn notices_map_to_toast_levels() {
    use egui_pop::api::toasts::Level;
    let levels: Vec<Level> = Notice::ALL.iter().map(|n| n.toast("x").level).collect();
    assert_eq!(
        levels,
        [Level::Info, Level::Success, Level::Warning, Level::Error]
    );
}
