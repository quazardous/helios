//! The centre stays a drop target once every tab was moved out of it: the
//! Library dragged from the centre to a side bar, then back onto the empty
//! centre, docks there again.

use egui::{Pos2, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::model::ids::PanelId;
use egui_pop::model::layout::{Layout, Node};
use egui_pop::model::panels::Area;
use helios_gui::{Shell, headless, helios_panels};

fn id(p: &str) -> PanelId {
    PanelId::new(p).unwrap()
}

fn tab(h: &Harness<'_, Shell>, title: &str) -> Rect {
    h.get_by_role_and_label(egui::accesskit::Role::Button, title)
        .rect()
}

fn area(h: &Harness<'_, Shell>, area: Area) -> Rect {
    h.state()
        .app()
        .handle()
        .area_rect(area)
        .expect("the area is drawn")
}

fn tabs_in(h: &Harness<'_, Shell>, area: Area) -> Option<Vec<PanelId>> {
    h.state()
        .layout()
        .areas
        .into_iter()
        .find(|(a, _)| *a == area)
        .map(|(_, n)| match n {
            Node::Tabs { panels, .. } => panels,
            other => panic!("{other:?}"),
        })
}

/// Presses on `from`, drags to `to` a frame at a time, holds, lets go.
fn drag_and_drop(h: &mut Harness<'_, Shell>, from: Pos2, to: Pos2) {
    let mut send = |e: egui::Event| {
        h.input_mut().events.push(e);
        h.step();
    };
    send(egui::Event::PointerMoved(from));
    send(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    for i in 1..=8 {
        send(egui::Event::PointerMoved(
            from + (to - from) * (i as f32 / 8.0),
        ));
    }
    for _ in 0..4 {
        h.step();
    }
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
}

#[test]
fn the_library_dragged_out_of_the_centre_and_back_docks_there() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    // The library layout: the Library alone in the centre, the Stacks at
    // the end side.
    let specs = h.state().panels().to_vec();
    let layout = Layout {
        areas: vec![
            (Area::Center, Node::tab(id("helios.vault"))),
            (Area::EndSide, Node::tab(id("helios.stacks"))),
        ],
        hidden: specs
            .iter()
            .filter(|s| !["helios.vault", "helios.stacks"].contains(&s.id.as_str()))
            .map(|s| s.id.clone())
            .collect(),
        ..Layout::default()
    };
    h.state_mut().set_layout(layout);
    h.run();
    assert_eq!(tabs_in(&h, Area::Center), Some(vec![id("helios.vault")]));
    // Out to the end side bar, joining the Stacks.
    let from = tab(&h, "Library").center();
    let to = area(&h, Area::EndSide).center();
    drag_and_drop(&mut h, from, to);
    assert_eq!(tabs_in(&h, Area::Center), None, "the centre is empty");
    assert_eq!(
        tabs_in(&h, Area::EndSide),
        Some(vec![id("helios.stacks"), id("helios.vault")])
    );
    // Back onto the empty centre.
    let centre = area(&h, Area::Center);
    let from = tab(&h, "Library").center();
    drag_and_drop(&mut h, from, centre.center());
    assert_eq!(
        tabs_in(&h, Area::Center),
        Some(vec![id("helios.vault")]),
        "docked in the centre again"
    );
}
