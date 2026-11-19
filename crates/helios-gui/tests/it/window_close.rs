//! Helios's own windows on egui-pop's bus (ADR 0029, slice 8): the
//! Library and Activity focus windows and the import window each open as
//! their owner's `pop.window.open` request and listen on `pop.window.close` (a
//! local listener, after `pop.window.registry`, before `pop.window.place`), so
//! whatever closes their window (a command, the title bar or the
//! compositor, the main window's close) leaves their state closed in the
//! same call, with no pass of the main window in between.

use egui::{ViewportId, vec2};
use egui_kittest::Harness;
use egui_pop::api::bus::{Reason, WindowKind, WindowRequest};
use egui_pop::api::windows::WindowId;
use helios_gui::shell::{ACTIVITY_WINDOW, LIBRARY_WINDOW, LIBRARY_WINDOW_COMMAND};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

fn harness() -> Harness<'static, Shell> {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.ctx.set_embed_viewports(false);
    h.state_mut().draw_frame(true);
    h.run();
    h
}

/// The windows the main window's last pass declared.
fn declared(h: &Harness<'static, Shell>) -> Vec<ViewportId> {
    h.output()
        .viewport_output
        .iter()
        .filter(|(v, o)| **v != ViewportId::ROOT && o.viewport_ui_cb.is_some())
        .map(|(v, _)| *v)
        .collect()
}

/// One of Helios's windows: how it opens and whether its state is open.
struct Window {
    id: &'static str,
    owner: &'static str,
    open: fn(&mut Harness<'static, Shell>),
    is_open: fn(&Harness<'static, Shell>) -> bool,
}

const WINDOWS: &[Window] = &[
    Window {
        id: LIBRARY_WINDOW,
        owner: "helios.library",
        open: |h| {
            h.state_mut()
                .run(LIBRARY_WINDOW_COMMAND, Value::Null, Origin::User)
                .unwrap();
        },
        is_open: |h| h.state().library_window_state()["open"] == true,
    },
    Window {
        id: ACTIVITY_WINDOW,
        owner: "helios.activity",
        open: |h| {
            h.state_mut()
                .run("activity.show", Value::Null, Origin::User)
                .unwrap();
        },
        is_open: |h| h.state().activity_focus_state()["open"] == true,
    },
    Window {
        id: "import",
        owner: "helios.import-window",
        open: |h| {
            h.state_mut()
                .run("library.importWindow", json!({}), Origin::User)
                .unwrap();
        },
        is_open: |h| h.state().import_window_open(),
    },
];

/// Each window, closed by a command, by the person (its title bar,
/// Escape) and by the system (the compositor), has its state closed
/// inside the close itself: no pass of the main window needed.
#[test]
fn each_window_closed_any_way_leaves_its_state_closed_in_the_same_call() {
    let mut h = harness();
    let app = h.state().app().handle().clone();
    for w in WINDOWS {
        for reason in [None, Some(Reason::User), Some(Reason::System)] {
            (w.open)(&mut h);
            h.run_steps(3);
            assert!((w.is_open)(&h), "{}: not open", w.id);
            assert_eq!(declared(&h).len(), 1, "{}: its own window", w.id);
            let id = WindowId::new(w.id);
            let opened = app
                .windows_list()
                .into_iter()
                .find(|r| r.id == w.id)
                .unwrap_or_else(|| panic!("{}: not in the registry", w.id));
            assert_eq!(opened.owner, w.owner, "{}: its owner", w.id);
            match reason {
                None => app.close_window(&id),
                Some(reason) => {
                    assert!(app.request_window_close(WindowRequest::new(
                        w.id,
                        WindowKind::App,
                        w.owner,
                        reason,
                    )));
                }
            }
            assert!(
                !(w.is_open)(&h),
                "{}: still open after its close ({reason:?})",
                w.id
            );
            h.run_steps(3);
            assert!(declared(&h).is_empty(), "{}: its window is gone", w.id);
            assert!(!(w.is_open)(&h), "{}: opened again", w.id);
        }
    }
}

/// The listeners of `pop.window.close` in their order, Helios's marked
/// local: a reviewed dump.
#[test]
fn helios_windows_listen_on_window_close_in_a_declared_order() {
    let h = harness();
    let order = h.state().window_close_order();
    for owner in [
        "helios.library",
        "helios.activity",
        "helios.calibrate",
        "helios.import-window",
    ] {
        let line = order
            .lines()
            .find(|l| l.contains(&format!("{owner}.close (owner {owner})")))
            .unwrap_or_else(|| panic!("no {owner} listener:\n{order}"));
        assert!(
            line.ends_with("local; after pop.window.registry; before pop.window.place"),
            "{line}"
        );
    }
    let at = |id: &str| order.find(id).unwrap_or_else(|| panic!("{id}:\n{order}"));
    assert!(at("pop.window.registry (") < at("helios.import-window.close"));
    assert!(at("helios.import-window.close") < at("pop.window.place ("));
}
