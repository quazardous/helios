//! The panels following the photo (the scopes, the navigator) say when
//! what they show is not current: another photo selected, they are stale
//! at once; once that has lasted the delay, the scopes' content is dimmed
//! with a busy mark on its corner and its tab; the new photo's data
//! landed, the mark clears. The panels draw none of it: egui-pop's chrome
//! does, from `panel.state` on the bus.

use std::time::Duration;

use egui::vec2;
use egui_kittest::Harness;
use egui_pop::api::bus::{PanelState, mark_id};
use egui_pop::core::{indicator, motion};
use egui_pop::theme::Tokens;
use helios_core::{Graph, Image, ModuleRef, NodeId, Params, Region};
use helios_gui::{FOLLOWS_PHOTO, Shell, headless, helios_panels};
use helios_shell::commands::Origin;

const SCOPES: &str = "helios.scopes";

/// A flat grey through an exposure: a photo's network.
fn network(level: f32) -> (Graph, NodeId) {
    let mut g = Graph::new();
    let src = g.add_source(
        "source",
        Image::from_fn(Region::sized(64, 48), 4, |_, _, c| match c {
            0..=2 => level,
            _ => 1.0,
        }),
    );
    let exp = g
        .add_module(
            "exposure",
            ModuleRef::new(helios_module_exposure::Exposure),
            &Params::new(),
        )
        .unwrap();
    g.connect(src, "image", exp, "input").unwrap();
    (g, exp)
}

/// Frames until the preview asked has landed and is drawn.
fn settle(h: &mut Harness<'_, Shell>) {
    for _ in 0..3 {
        h.step();
        assert!(
            h.state().preview().wait(Duration::from_secs(30)),
            "the preview renders"
        );
    }
    h.step();
    h.step();
}

fn state(h: &Harness<'_, Shell>, panel: &str) -> PanelState {
    h.state().app().handle().panel_state(panel)
}

/// The scopes' mark (on its content, or on its tab), when shown.
fn mark(h: &Harness<'_, Shell>, on_tab: bool) -> Option<indicator::Record> {
    indicator::find(&h.ctx, &mark_id(SCOPES, on_tab)).filter(|r| r.shown)
}

#[test]
fn the_scopes_are_marked_stale_until_the_new_photo_s_data_lands() {
    let (graph, node) = network(0.2);
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_view_source(graph, node, "output").unwrap();
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    let clock = motion::freeze(&h.ctx, 0.0);
    h.state_mut()
        .run("helios.scopes.focus", serde_json::Value::Null, Origin::User)
        .expect("the scopes shown");
    settle(&mut h);
    for panel in FOLLOWS_PHOTO {
        assert_eq!(state(&h, panel), PanelState::Ready, "{panel}");
    }
    let (first, _) = h.state().preview().latest().expect("the first preview");
    assert!(mark(&h, false).is_none());

    // Another photo selected: stale at once, before any frame.
    h.state_mut().open_from_library(Some("b"));
    for panel in FOLLOWS_PHOTO {
        assert_eq!(state(&h, panel), PanelState::Stale, "{panel}");
    }
    h.step();
    assert!(mark(&h, false).is_none(), "nothing within the delay");
    // Its network is slow to come: past the delay, the mark shows.
    let t = Tokens::of(&h.ctx);
    let (delay, min) = (
        f64::from(t.motion.spinner_delay),
        f64::from(t.motion.spinner_min),
    );
    clock.set(delay + 0.05);
    h.step();
    h.step();
    let shown = mark(&h, false).expect("the busy mark on the scopes");
    assert_eq!(shown.state, "busy");
    assert_eq!(mark(&h, true).expect("on the tab").state, "busy");

    // The new photo's network: its preview lands, the mark clears.
    let (graph, node) = network(0.7);
    h.state_mut()
        .set_view_source(graph, node, "output")
        .unwrap();
    settle(&mut h);
    let (now, _) = h.state().preview().latest().expect("the new preview");
    assert!(now > first, "the new data landed");
    for panel in FOLLOWS_PHOTO {
        assert_eq!(state(&h, panel), PanelState::Ready, "{panel}");
    }
    clock.set(delay + 0.05 + min + 0.05);
    h.step();
    h.step();
    assert!(mark(&h, false).is_none(), "cleared with the new data");
    assert!(mark(&h, true).is_none(), "on the tab too");
}
