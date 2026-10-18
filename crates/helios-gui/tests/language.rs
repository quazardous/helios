//! The desktop follows the language setting: Helios's panel titles and
//! egui-pop's own menus switch to French and back while it runs.

use egui::vec2;
use egui_kittest::kittest::Queryable;
use helios_gui::{Shell, headless, helios_panels};

#[test]
fn the_desktop_switches_to_french_and_back() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    h.get_by_label("View");
    h.get_by_label("Help");
    let graph = |h: &egui_kittest::Harness<'_, Shell>| {
        egui_pop::core::strings::panel_title(&h.ctx, "helios.graph", "Graph")
    };
    assert_eq!(graph(&h), "Graph");
    let app = h.state().app().handle().clone();
    egui_pop::i18n::plugin::set_language(&app, "fr").unwrap();
    h.run();
    h.run();
    h.get_by_label("Affichage");
    h.get_by_label("Aide");
    assert_eq!(graph(&h), "Graphe");
    let changed = app
        .events_since(0)
        .into_iter()
        .find(|e| e.name == "locale.changed")
        .expect("the change is published");
    assert_eq!(changed.data["to"], "fr");
    egui_pop::i18n::plugin::set_language(&app, "en").unwrap();
    h.run();
    h.run();
    h.get_by_label("View");
    assert_eq!(graph(&h), "Graph");
}

/// The library's file details (size, day modified) follow the language.
#[test]
fn file_sizes_and_dates_follow_the_language() {
    use helios_gui::library::{Entry, file_details_in};

    let utc = chrono::Utc;

    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let entry = Entry {
        name: "a.cr2".into(),
        bytes: Some(25 * 1024 * 1024 + 300 * 1024),
        // 2026-10-12, at noon UTC.
        modified: Some(1_791_806_400),
        ..Entry::default()
    };
    assert_eq!(
        file_details_in(&h.ctx, &entry, &utc).as_deref(),
        Some("25.3 MiB · modified Oct 12, 2026")
    );
    let app = h.state().app().handle().clone();
    egui_pop::i18n::plugin::set_language(&app, "fr").unwrap();
    h.run();
    h.run();
    assert_eq!(
        file_details_in(&h.ctx, &entry, &utc).as_deref(),
        Some("25,3 Mio · modifié le 12 oct. 2026")
    );
    let small = Entry {
        bytes: Some(512),
        ..Entry::default()
    };
    assert_eq!(
        file_details_in(&h.ctx, &small, &utc).as_deref(),
        Some("512 o")
    );
}

/// The day a file was modified is the one of the time zone shown: late
/// on the 12th in UTC is already the 13th in Paris, and the evening
/// before in Honolulu.
#[test]
fn file_dates_are_in_the_time_zone() {
    use chrono::FixedOffset;
    use helios_gui::library::{Entry, file_details_in};

    let h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    let entry = Entry {
        // 2026-10-12, 23:30 UTC.
        modified: Some(1_791_847_800),
        ..Entry::default()
    };
    let day = |tz: &FixedOffset| file_details_in(&h.ctx, &entry, tz);
    let paris = FixedOffset::east_opt(2 * 3600).unwrap();
    let honolulu = FixedOffset::west_opt(10 * 3600).unwrap();
    assert_eq!(
        day(&FixedOffset::east_opt(0).unwrap()).as_deref(),
        Some("Oct 12, 2026")
    );
    assert_eq!(day(&paris).as_deref(), Some("Oct 13, 2026"));
    assert_eq!(day(&honolulu).as_deref(), Some("Oct 12, 2026"));
    let early = Entry {
        // 2026-10-13, 00:30 UTC.
        modified: Some(1_791_851_400),
        ..Entry::default()
    };
    assert_eq!(
        file_details_in(&h.ctx, &early, &honolulu).as_deref(),
        Some("Oct 12, 2026")
    );
}

/// French on: a step whose GPU version failed is told in a French toast,
/// once per module, its failure translated too.
#[test]
fn a_gpu_fallback_is_told_in_french() {
    use egui_pop::api::toasts::TOASTS;
    use helios_core::NodeId;
    use helios_core::engine::GpuFellBack;
    use helios_core::module::Failure;

    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let app = h.state().app().handle().clone();
    egui_pop::i18n::plugin::set_language(&app, "fr").unwrap();
    h.run();
    h.run();
    let engine = h.state().viewport().engine().clone();
    let fell = GpuFellBack::new(
        NodeId(1),
        "e1",
        "exposure",
        Failure::OutOfMemory,
        "budget spent",
    );
    assert_eq!(
        fell.message.english,
        "exposure ran on the CPU: its GPU version failed (out of memory: budget spent)."
    );
    engine.bus().publish("engine", None, &fell);
    engine.bus().publish("engine", None, &fell);
    h.run();
    let french = "exposure a été calculé sur le CPU : sa version GPU a échoué \
                  (mémoire épuisée : budget spent).";
    let told: Vec<_> = h
        .state()
        .notices()
        .iter()
        .filter(|n| n.key == "engine-gpu-fell-back")
        .collect();
    assert_eq!(told.len(), 1, "once per module");
    assert_eq!(told[0].text, french);
    let toasts = app.service(&TOASTS).expect("egui-pop's toasts").shown();
    assert!(toasts.iter().any(|t| t.toast.text == french), "{toasts:?}");
}

/// French on: Helios's commands in the menu bar show their French titles,
/// without the hint of their arguments the palette keeps.
#[test]
fn menu_entries_show_french_titles_without_their_argument_hint() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let app = h.state().app().handle().clone();
    egui_pop::i18n::plugin::set_language(&app, "fr").unwrap();
    h.run();
    h.run();
    h.get_by_label("Fichier").click();
    h.run();
    h.get_by_label("Ouvrir un espace de travail dans une nouvelle fenêtre");
    assert!(h.query_by_label_contains("(`path`)").is_none());
    assert!(h.query_by_label_contains("Open a workspace").is_none());
}
