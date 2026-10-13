//! Panels as palettes (egui-pop's floating panels): the Masks panel and
//! the scopes float over the image and dock back in their area.

use egui::vec2;
use egui_pop::model::ids::PanelId;
use egui_pop::model::panels::Area;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

fn id(p: &str) -> PanelId {
    PanelId::new(p).unwrap()
}

#[test]
fn the_masks_panel_and_the_scopes_float_and_dock_back() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let agent = Origin::Agent("a1".into());
    for panel in helios_gui::FLOATABLE {
        let spec = h
            .state()
            .panels()
            .iter()
            .find(|s| s.id.as_str() == panel)
            .cloned()
            .unwrap();
        assert!(spec.floatable, "{panel}");
        h.state_mut()
            .run(
                &format!("{panel}.float"),
                json!({ "pos": [300.0, 40.0] }),
                agent.clone(),
            )
            .unwrap();
        h.run();
        let layout = h.state().layout();
        let floating = layout.floating_of(&id(panel)).expect("it floats");
        assert_eq!(floating.pos, [300.0, 40.0]);
        assert_eq!(layout.area_of(&id(panel)), None, "{panel} left its area");

        h.state_mut()
            .run(&format!("{panel}.dock"), json!(null), agent.clone())
            .unwrap();
        h.run();
        let layout = h.state().layout();
        assert!(layout.floating_of(&id(panel)).is_none(), "{panel}");
        assert_eq!(layout.area_of(&id(panel)), Some(spec.area));
    }
    // The other panels do not float.
    let refused = h.state_mut().run("helios.params.float", json!(null), agent);
    assert!(refused.is_err());
    assert_eq!(
        h.state().layout().area_of(&id("helios.params")),
        Some(Area::EndSide)
    );
}
