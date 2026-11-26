//! Panels as palettes (egui-pop's floating panels): the Masks panel and
//! the scopes float over the image and dock back in their area; any
//! other panel floats when asked, from its tab menu too, and a whole tab
//! group floats at once (Float group).

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
    // The other panels float too (egui-pop: every panel may float unless
    // it opts out; a palette only starts floating), and dock back home.
    let params = id("helios.params");
    assert_eq!(h.state().layout().area_of(&params), Some(Area::EndSide));
    h.state_mut()
        .run(
            "helios.params.float",
            json!({ "pos": [200.0, 60.0] }),
            agent.clone(),
        )
        .unwrap();
    h.run();
    let layout = h.state().layout();
    assert_eq!(
        layout.floating_of(&params).expect("it floats").pos,
        [200.0, 60.0]
    );
    assert_eq!(layout.area_of(&params), None);
    h.state_mut()
        .run("helios.params.dock", json!(null), agent)
        .unwrap();
    h.run();
    let layout = h.state().layout();
    assert!(layout.floating_of(&params).is_none());
    assert_eq!(layout.area_of(&params), Some(Area::EndSide));
}

/// Opens a docked tab's menu, as a right click on it: the tab named by
/// its panel's title (its label may add the panel's kind after a comma).
fn tab_menu(h: &mut egui_kittest::Harness<'_, Shell>, title: &str) {
    use egui_kittest::kittest::{NodeT as _, Queryable as _};
    let prefix = format!("{title}, ");
    h.get_all_by_role(egui::accesskit::Role::Tab)
        .find(|t| {
            t.accesskit_node()
                .label()
                .is_some_and(|l| l == title || l.starts_with(&prefix))
        })
        .unwrap_or_else(|| panic!("a tab {title}"))
        .click_secondary();
    h.run();
}

/// Every docked Helios panel's tab menu offers Float, enabled: Helios
/// opts no panel out of floating (the owner saw Float on Masks, Scopes
/// and the Navigator only, #5219).
#[test]
fn every_docked_panel_menu_offers_float() {
    use egui_kittest::kittest::{NodeT as _, Queryable as _};
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let docked: Vec<(PanelId, String)> = h
        .state()
        .panels()
        .iter()
        .filter(|s| h.state().layout().area_of(&s.id).is_some())
        .map(|s| (s.id.clone(), s.title.clone()))
        .collect();
    assert!(docked.len() >= 5, "{docked:?}");
    for (panel, title) in docked {
        let shown = h
            .state()
            .panels()
            .iter()
            .find(|s| s.id == panel)
            .map(|s| s.floatable);
        assert_eq!(shown, Some(true), "{panel} may float");
        tab_menu(&mut h, &title);
        let float = h.get_by_label("Float");
        assert!(!float.accesskit_node().is_disabled(), "{panel}: Float on");
        h.key_press(egui::Key::Escape);
        h.run();
    }
}

/// Float group floats the bottom panel's whole tab group (Filmstrip,
/// Queue, Log) as one palette with all its tabs; docking it back restores
/// the group as it was.
#[test]
fn float_group_floats_the_bottom_group_and_dock_back_restores_it() {
    use egui_kittest::kittest::Queryable as _;
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let strip = id("helios.filmstrip");
    let (before, shown) = h.state().layout().group_of(&strip).expect("docked");
    assert!(
        before.len() >= 2,
        "the Filmstrip shares its group: {before:?}"
    );
    tab_menu(&mut h, "Filmstrip");
    h.get_by_label("Float group").click();
    h.run();
    h.run();
    let layout = h.state().layout();
    let g = layout.floating_of(&strip).expect("the group floats");
    assert_eq!(g.panels().cloned().collect::<Vec<_>>(), before);
    for p in &before {
        assert_eq!(layout.area_of(p), None, "{p} left its area");
    }
    // Dock back, as the floating group's menu says it: every tab where it
    // was, in order.
    h.state_mut()
        .run(
            "workbench.panel.dock_back",
            json!({ "panel": "helios.filmstrip" }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    h.run();
    let layout = h.state().layout();
    assert!(layout.floating_of(&strip).is_none());
    assert_eq!(layout.area_of(&strip), Some(Area::Bottom));
    assert_eq!(layout.group_of(&strip), Some((before, shown)));
}
