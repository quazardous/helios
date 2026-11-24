//! The presets headless: each draws its egui-pop widget, named for
//! assistive technologies and agents, in each of its states.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_pop::core::motion;
use egui_pop::core::ui::Tone;
use egui_pop::egui::accesskit::Role;
use egui_pop::egui::{self, Ui};
use egui_pop::theme::{self, ThemeSpec};
use helios_ui::{
    Action, Empty, Kind, Late, Loading, Notice, Progress, Scroll, Search, Section, Value,
};

/// A harness drawing `draw` in egui-pop's dark theme, its motion clock
/// frozen at 0 s (no animation asks for frames).
fn harness<S: 'static>(state: S, draw: fn(&mut Ui, &mut S)) -> Harness<'static, S> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(480.0, 360.0))
        .build_ui_state(
            move |ui, s: &mut S| {
                draw(ui, s);
            },
            state,
        );
    // A given clock: no animation asks for frames (set once: setting it
    // asks for a frame).
    let _clock = motion::freeze(&h.ctx, 0.0);
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
fn an_empty_case_shows_in_the_language_installed() {
    let mut h = harness((), |ui, ()| {
        let _ = helios_ui::empty(ui, Empty::NoPhotos);
    });
    let keys = |case: Empty| {
        let action = case.action_key().expect("NoPhotos has an action");
        [case.title_key().key, case.text_key().key, action.key]
    };
    let [title, text, action] = keys(Empty::NoPhotos);
    let french = egui_pop::core::strings::MapStrings::new("fr")
        .with(title, "Aucune photo pour l’instant")
        .with(text, "Importez des photos pour les voir ici.")
        .with(action, "Importer des photos…");
    egui_pop::core::strings::install(&h.ctx, std::sync::Arc::new(french));
    h.run();
    h.get_by_label("Aucune photo pour l’instant");
    h.get_by_label("Importez des photos pour les voir ici.");
    h.get_by_label("Importer des photos…");
    // Every case has keys of its own.
    let mut all = std::collections::BTreeSet::new();
    for case in Empty::ALL {
        assert!(all.insert(case.title_key().key), "{case:?}");
        assert!(all.insert(case.text_key().key), "{case:?}");
        if let Some(a) = case.action_key() {
            assert!(all.insert(a.key), "{case:?}");
        }
    }
}

#[test]
fn a_row_action_is_a_button_lower_than_an_action() {
    let h = harness(Vec::<egui::Rect>::new(), |ui, rects| {
        rects.clear();
        ui.horizontal(|ui| {
            rects.push(helios_ui::action(ui, Action::Secondary, "Cancel").rect);
            rects.push(helios_ui::row_action(ui, Action::Secondary, "Cancel row").rect);
            rects.push(helios_ui::row_action(ui, Action::Danger, "Delete row").rect);
        });
    });
    for name in ["Cancel row", "Delete row"] {
        assert_eq!(h.get_by_label(name).accesskit_node().role(), Role::Button);
    }
    let rects = h.state().clone();
    assert!(
        rects[1].height() < rects[0].height(),
        "lower than the action: {rects:?}"
    );
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

#[test]
fn notices_map_to_toast_levels() {
    use egui_pop::api::toasts::Level;
    let levels: Vec<Level> = Notice::ALL.iter().map(|n| n.toast("x").level).collect();
    assert_eq!(
        levels,
        [Level::Info, Level::Success, Level::Warning, Level::Error]
    );
}

#[test]
fn a_restriction_shows_its_text_and_lifts_by_its_remove_button() {
    let mut h = harness(false, |ui, removed| {
        if helios_ui::restriction(ui, "Album: Trip") {
            *removed = true;
        }
    });
    h.get_by_label("Album: Trip");
    h.get_by_label("Remove Album: Trip").click();
    h.run();
    assert!(*h.state(), "the removal is reported");
}

/// A folder's path far wider than its side bar.
const LONG: &str = "Folder: C:\\Users\\someone\\AppData\\Local\\Temp\\a-rather-long-folder-name-for-a-side-bar\\raw";

#[test]
fn a_restriction_wider_than_its_line_is_cut_in_its_middle_and_stays_removable() {
    let mut h = harness(false, |ui, removed| {
        ui.set_max_width(200.0);
        ui.horizontal_wrapped(|ui| {
            if helios_ui::restriction(ui, LONG) {
                *removed = true;
            }
        });
    });
    let right = h.ctx.content_rect().min.x + 200.0 + 1.0;
    let chip = h
        .query_all_by_label_contains("…")
        .next()
        .expect("the text cut");
    let label = chip.accesskit_node().label().unwrap_or_default();
    assert!(
        label.starts_with("Folder: ") && label.ends_with("raw"),
        "its start and end kept: {label}"
    );
    assert!(chip.rect().max.x <= right, "{:?} within 200", chip.rect());
    // The remove button is named by the whole text, and at hand.
    let name = format!("Remove {LONG}");
    let remove = h.get_by_label(&name);
    assert!(remove.rect().max.x <= right, "{:?}", remove.rect());
    remove.click();
    h.run();
    assert!(*h.state(), "the removal is reported");
}

#[test]
fn a_late_value_keeps_its_room_and_says_its_state() {
    let mut h = harness(
        (Late::<String>::Pending, None, false),
        |ui, (late, rect, retry)| {
            let r = ui.horizontal(|ui| {
                *retry |= helios_ui::value(ui, Value::new("Free space", Kind::Size, late.clone()));
                ui.label("after").rect
            });
            *rect = Some(r.inner);
        },
    );
    let cell = |h: &Harness<'static, (Late<String>, Option<egui::Rect>, bool)>| {
        let node = h.get_by_role_and_label(Role::Status, "Free space");
        let a = node.accesskit_node();
        (a.value(), a.is_busy())
    };
    let pending_after = h.state().1;
    assert_eq!(cell(&h), (Some("Loading".to_string()), true));

    h.state_mut().0 = Late::Fresh("412.7 GB".into());
    h.run();
    assert_eq!(cell(&h), (Some("412.7 GB".to_string()), false));
    assert_eq!(
        h.state().1,
        pending_after,
        "nothing moved when the value landed"
    );

    h.state_mut().0 = Late::Refreshing("412.7 GB".into());
    h.run();
    assert_eq!(cell(&h), (Some("412.7 GB".to_string()), true));
    assert_eq!(h.state().1, pending_after);

    h.state_mut().0 = Late::Failed("The disk did not answer.".into());
    h.run();
    assert_eq!(cell(&h).0.as_deref(), Some("The disk did not answer."));
    h.get_by_label("Retry Free space").click();
    h.run();
    assert!(h.state().2, "Retry is reported");
}

#[test]
fn every_kind_has_its_own_room() {
    let samples: std::collections::BTreeSet<_> = Kind::ALL.iter().map(|k| k.sample()).collect();
    assert_eq!(samples.len(), Kind::ALL.len());
}

#[test]
fn a_choice_is_a_combo_box_named_by_what_it_chooses() {
    let options = [(1_u8, "One".to_string()), (2, "Two".to_string())];
    let mut h = harness((2_u8, options), |ui, (current, options)| {
        let _ = helios_ui::choice(ui, "test.choice", "Number shown", current, options);
    });
    let node = h.get_by_label("Number shown");
    assert_eq!(node.accesskit_node().role(), Role::ComboBox);
    node.click();
    h.run();
    h.get_by_label("One").click();
    h.run();
    assert_eq!(h.state().0, 1);
}

#[test]
fn details_show_each_name_with_its_value() {
    let rows = vec![
        ("Kind".to_string(), "import".to_string()),
        ("Elapsed".to_string(), "42 s".to_string()),
    ];
    let h = harness(rows, |ui, rows| {
        helios_ui::details(ui, "test.details", rows);
    });
    for label in ["Kind", "import", "Elapsed", "42 s"] {
        h.get_by_label(label);
    }
    let x = |label: &str| h.get_by_label(label).rect().min.x;
    assert!(x("Kind") < x("import"), "the value after its name");
    assert!(
        (x("import") - x("42 s")).abs() < 0.5,
        "the values in one column"
    );
}

#[test]
fn progress_is_a_bar_when_the_work_counts_and_a_spinner_when_it_does_not() {
    for place in Progress::ALL {
        let h = harness(place, |ui, place| {
            let _ = helios_ui::progress(ui, *place, "Import 412 photos", Some(0.25));
        });
        let node = h.get_by_label("Import 412 photos");
        let a = node.accesskit_node();
        assert_eq!(a.role(), Role::ProgressIndicator, "{place:?}");
        assert_eq!(a.numeric_value(), Some(25.0), "{place:?}");
        let h = harness(place, |ui, place| {
            let _ = helios_ui::progress(ui, *place, "Empty the trash", None);
        });
        let a = h.get_by_label("Empty the trash").accesskit_node();
        assert_eq!(a.numeric_value(), None, "{place:?}: no fake bar");
    }
}

#[test]
fn a_scroll_shows_its_content_and_a_tail_section_dialog_or_form_stays_bounded() {
    for kind in Scroll::ALL {
        let h = harness(kind, |ui, kind| {
            let rect = helios_ui::scroll(ui, "test.scroll", *kind, |ui| {
                for i in 0..100 {
                    ui.label(format!("line {i}"));
                }
                ui.min_rect()
            });
            assert!(rect.height() > 360.0, "the content keeps its height");
            if matches!(
                *kind,
                Scroll::Tail | Scroll::Section | Scroll::Dialog | Scroll::Form
            ) {
                assert!(ui.min_rect().height() < 300.0, "{kind:?} is bounded");
            }
        });
        let shown = if matches!(kind, Scroll::Tail | Scroll::Log) {
            "line 99"
        } else {
            "line 0"
        };
        assert!(h.query_by_label(shown).is_some(), "{kind:?}");
    }
}
