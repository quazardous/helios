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
