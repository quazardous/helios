//! Full screen and zen mode: in full screen the title bar stays with
//! Helios's global items (jobs, decisions waiting, the profile), each
//! opening what it is about; zen mode (`Ctrl+K Z`) leaves the image
//! alone, and Escape brings the bars and areas back.

use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_pop::api::jobs_centre::PANEL_JOBS;
use egui_pop::core::frame;
use egui_pop::core::indicator;
use egui_pop::model::ids::PanelId;
use helios_core::jobs::{Progress, Start};
use helios_gui::caption::{DECISIONS_MARK, JOBS_MARK, PROFILE};
use helios_gui::{Shell, headless, helios_panels};
use helios_library::stacks::Decision;
use serde_json::json;

fn framed() -> Shell {
    let mut shell = Shell::new(helios_panels(), None);
    shell.draw_frame(true);
    shell.set_title("photos");
    shell
}

fn full_screen(h: &mut Harness<'_, Shell>, on: bool) {
    h.input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .fullscreen = Some(on);
    h.run();
}

/// A stack suggestion waiting for an answer.
fn proposal(id: &str) -> Decision {
    serde_json::from_value(json!({
        "id": id,
        "kind": "stacks.suggest",
        "status": "proposed",
        "selection": ["a", "b"],
        "left_out": [],
        "resolver": "rules",
        "proposal": {
            "groups": [{
                "members": ["a", "b"],
                "best": "b",
                "kind": "burst",
                "why": "taken together, look alike"
            }],
            "model": { "name": "rules", "version": "1" },
            "rationale": "",
            "at": 0
        },
        "answer": null
    }))
    .unwrap()
}

#[test]
fn full_screen_keeps_the_bar_with_helios_s_global_items() {
    let mut h = headless::harness(framed(), vec2(1280.0, 800.0));
    let job = h
        .state()
        .jobs()
        .start(Start::new("export-1", "Export a.png", "export").total(4));
    job.progress(Progress::new().counts(1, 4));
    h.state_mut()
        .stack_review_mut()
        .set_decisions(vec![proposal("stacks-1"), proposal("stacks-2")]);
    h.run();
    full_screen(&mut h, true);

    // egui-pop's bar stays, its way out in place of maximize.
    assert!(h.query_by_label("Exit full screen").is_some());
    assert!(h.query_by_label("Minimize").is_none());
    assert!(h.query_by_label("Window menu").is_some());
    let bar = frame::regions(&h.ctx)
        .and_then(|r| r.caption)
        .expect("the bar publishes its caption in full screen");
    let ppp = h.ctx.pixels_per_point();
    let bar_bottom = (bar.y + bar.h) as f32 / ppp;

    // The jobs going on, the decisions waiting and the profile, in it.
    let jobs = indicator::find(&h.ctx, JOBS_MARK).expect("the jobs' mark");
    assert_eq!(jobs.kind, "pip");
    assert!(
        jobs.name.as_deref().unwrap_or_default().contains('1'),
        "{jobs:?}"
    );
    let decisions = indicator::find(&h.ctx, DECISIONS_MARK).expect("the decisions' count");
    assert_eq!(decisions.value, Some(2.0), "{decisions:?}");
    let profile = h.state().target_rect(PROFILE).expect("the profile's item");
    assert!(
        profile.bottom() <= bar_bottom + 1.0,
        "{profile:?} in the bar"
    );
    assert!(h.query_by_label("default").is_some());

    // Each opens what it is about: the Jobs panel, the Stacks panel,
    // the Profiles menu.
    let app = h.state().app().handle().clone();
    assert!(!app.panel_visible(&PANEL_JOBS));
    h.get_by_label("Show the jobs").click();
    h.run();
    h.run();
    assert!(app.panel_visible(&PANEL_JOBS));
    h.get_by_label_contains("Decisions waiting").click();
    h.run();
    h.run();
    assert!(app.panel_visible(&PanelId::from_static(helios_gui::stack_review::PANEL)));
    h.get_by_label("default").click();
    h.run();
    h.run();
    assert!(
        h.query_by_label_contains("Export the profile").is_some(),
        "the Profiles menu is open"
    );
    h.key_press(Key::Escape);
    h.run();

    // Nothing to say, nothing shown: the jobs' mark goes with the job.
    job.finished();
    h.state_mut().stack_review_mut().set_decisions(Vec::new());
    h.run();
    h.run();
    // (Read from the accessibility tree: the indicators collector keeps
    // the last marks drawn while no mark is drawn at all.)
    assert!(h.query_by_label("Show the jobs").is_none());
    assert!(h.query_by_label_contains("Decisions waiting").is_none());
    full_screen(&mut h, false);
    assert!(h.query_by_label("Exit full screen").is_none());
}

#[test]
fn ctrl_k_z_enters_zen_mode_and_escape_leaves_it() {
    let mut h = headless::harness(framed(), vec2(1280.0, 800.0));
    h.run();
    let app = h.state().app().handle().clone();
    assert!(
        app.keys_for(&egui_pop::api::workbench::CMD_ZEN)
            .iter()
            .any(|k| k == "Ctrl+K Z")
    );
    assert!(h.query_by_label("Window menu").is_some());
    assert!(h.query_by_label("default").is_some());

    h.key_press_modifiers(Modifiers::CTRL, Key::K);
    h.run();
    h.key_press(Key::Z);
    h.run();
    h.run();
    assert!(app.zen(), "Ctrl+K Z enters zen mode");
    // The title bar and its items are gone; no caption published.
    assert!(h.query_by_label("Window menu").is_none());
    assert!(h.query_by_label("default").is_none());
    assert!(frame::regions(&h.ctx).is_none_or(|r| r.caption.is_none()));

    h.key_press(Key::Escape);
    h.run();
    h.run();
    assert!(!app.zen(), "Escape leaves zen mode");
    assert!(h.query_by_label("Window menu").is_some());
    assert!(h.query_by_label("default").is_some());
}
