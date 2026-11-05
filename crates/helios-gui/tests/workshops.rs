//! Workshops behind their switch (`interface.experimental.workshops`):
//! off, no Workshops panel; on, the panel is declared, closed until
//! opened, and says it is experimental.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_pop::model::ids::PanelId;
use helios_gui::{Experimental, Shell, headless, helios_panels_with, workshops};
use helios_shell::commands::Origin;
use serde_json::json;

fn harness(on: bool) -> Harness<'static, Shell> {
    let panels = helios_panels_with(Experimental { workshops: on });
    let mut h = headless::harness(Shell::new(panels, None), vec2(1280.0, 800.0));
    h.run();
    h
}

fn declared(h: &Harness<'_, Shell>) -> bool {
    h.state()
        .panels()
        .iter()
        .any(|p| p.id.as_str() == workshops::PANEL)
}

#[test]
fn off_there_is_no_workshops_panel() {
    let mut h = harness(false);
    assert!(!declared(&h));
    assert!(
        h.state_mut()
            .run("helios.workshops.focus", json!(null), Origin::User)
            .is_err()
    );
}

#[test]
fn on_the_panel_opens_and_says_it_is_experimental() {
    let mut h = harness(true);
    assert!(declared(&h));
    let id = PanelId::new(workshops::PANEL).unwrap();
    assert!(!h.state().app().handle().panel_visible(&id));
    h.state_mut()
        .run("helios.workshops.focus", json!(null), Origin::User)
        .unwrap();
    h.run();
    assert!(h.state().app().handle().panel_visible(&id));
    h.get_by_label_contains("Experimental: may change");
}
