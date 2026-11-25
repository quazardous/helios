//! Several Library views (`library_views`): the Results panel opens as
//! many times as asked, each new one a view of its own, with its own
//! filter and cursor. Moving in one leaves the others as they are; a
//! restart gives each back with its filter and cursor; closing one
//! leaves the others; an agent's `library.view.open` opens one as the
//! View menu's "New Library" does.

use egui::vec2;
use egui_kittest::Harness;
use egui_pop::model::ids::PanelId;
use egui_pop::model::layout::Layout;
use egui_pop::views::library::filter::Filter;
use helios_gui::library::Entry;
use helios_gui::library_views::{
    CLOSE_COMMAND, OPEN_COMMAND, SELECTION_COMMAND, close_command, open_views,
};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

/// Photos a to e.
fn entries() -> Vec<Entry> {
    ["a", "b", "c", "d", "e"]
        .iter()
        .map(|id| Entry {
            id: (*id).to_string(),
            name: format!("{id}.jpg"),
            ..Entry::default()
        })
        .collect()
}

fn harness(layout: Option<Layout>) -> Harness<'static, Shell> {
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries());
    if let Some(layout) = layout {
        shell.set_layout(layout);
    }
    let mut h = headless::harness(shell, vec2(1600.0, 1000.0));
    h.run_steps(3);
    h
}

fn run(h: &mut Harness<'_, Shell>, command: &str, args: Value, origin: Origin) -> Value {
    let out = h.state_mut().run(command, args, origin).unwrap();
    h.run_steps(3);
    out
}

/// A gallery's filter text and cursor.
fn view(h: &Harness<'_, Shell>, id: &str) -> (String, Option<String>) {
    h.state()
        .library_mut()
        .with_gallery(id, |l| (l.filter().text.clone(), l.cursor()))
}

fn payload(h: &Harness<'_, Shell>, id: &str) -> Value {
    h.state()
        .app()
        .handle()
        .instance_payload(&PanelId::new(id).unwrap())
}

#[test]
fn two_library_views_keep_their_own_filter_and_cursor_through_a_move_a_restart_and_a_close() {
    let mut h = harness(None);
    // One as the View menu's "New Library" opens it, one by an agent;
    // each in an area of its own, so both draw.
    let menu = run(
        &mut h,
        "workbench.panel.open",
        json!({ "panel": "helios.vault", "instance": "new", "place": "center" }),
        Origin::User,
    );
    let first = menu["instance"].as_str().unwrap().to_string();
    run(
        &mut h,
        OPEN_COMMAND,
        json!({ "place": "end_side" }),
        Origin::Agent("test".into()),
    );
    // The agent's answer names it (`view`); here, the one it added.
    let views = open_views(h.state().app().handle());
    let second = views.last().unwrap().clone();
    assert_ne!(first, second);
    assert_eq!(
        open_views(h.state().app().handle()),
        vec![first.clone(), second.clone()],
        "the agent's view is one as the menu's"
    );
    // Each view draws a gallery of its own: different filters and
    // cursors.
    {
        let mut lib = h.state().library_mut();
        lib.with_gallery(&first, |l| {
            l.set_filter(Filter::parse("folder:/one"));
            l.select_ids(&["b".to_string()]);
        });
        lib.with_gallery(&second, |l| {
            l.set_filter(Filter::parse("folder:/two"));
            l.select_ids(&["d".to_string()]);
        });
    }
    h.run_steps(3);
    assert_eq!(view(&h, &first), ("folder:/one".into(), Some("b".into())));
    assert_eq!(view(&h, &second), ("folder:/two".into(), Some("d".into())));
    // Kept with the layout, in each view's own state.
    assert_eq!(payload(&h, &first)["cursor"], "b");
    assert_eq!(payload(&h, &second)["cursor"], "d");
    // The main view is none of them.
    assert_eq!(view(&h, helios_gui::library::MAIN_GALLERY).1, None);

    // Moving in the first leaves the second as it was.
    h.state()
        .library_mut()
        .with_gallery(&first, |l| l.select_ids(&["c".to_string()]));
    h.run_steps(3);
    assert_eq!(view(&h, &first).1.as_deref(), Some("c"));
    assert_eq!(view(&h, &second), ("folder:/two".into(), Some("d".into())));
    assert_eq!(payload(&h, &first)["cursor"], "c");
    assert_eq!(payload(&h, &second)["cursor"], "d");

    // A restart: the layout as saved, in a new desktop.
    let saved = h.state().app().handle().layout().to_json();
    drop(h);
    let mut h = harness(Some(Layout::from_json(&saved).unwrap()));
    h.run_steps(3);
    assert_eq!(
        open_views(h.state().app().handle()),
        vec![first.clone(), second.clone()]
    );
    assert_eq!(view(&h, &first), ("folder:/one".into(), Some("c".into())));
    assert_eq!(view(&h, &second), ("folder:/two".into(), Some("d".into())));

    // Closing the first leaves the second as it was; the first's state
    // goes with it.
    run(
        &mut h,
        CLOSE_COMMAND,
        json!({ "view": first }),
        Origin::Agent("test".into()),
    );
    assert_eq!(open_views(h.state().app().handle()), vec![second.clone()]);
    assert_eq!(view(&h, &second), ("folder:/two".into(), Some("d".into())));
    assert_eq!(payload(&h, &first), Value::Null);
    assert!(!h.state().library().has_gallery(&first), "forgotten");
    // The main explorer is not a view to close.
    let app = h.state().app().handle().clone();
    assert!(close_command(&app, &json!({ "view": "helios.vault" })).is_err());
    assert!(
        close_command(&app, &json!({ "view": first })).is_err(),
        "closed already"
    );
}

/// S7: each view's selection (several photos) and cursor are kept with
/// the layout; what `library.view.selection` says before a restart, it
/// says after it.
#[test]
fn each_view_s_selection_and_cursor_read_back_the_same_after_a_restart() {
    let mut h = harness(None);
    for place in ["center", "end_side"] {
        run(
            &mut h,
            OPEN_COMMAND,
            json!({ "place": place }),
            Origin::Agent("test".into()),
        );
    }
    let views = open_views(h.state().app().handle());
    let (first, second) = (views[0].clone(), views[1].clone());
    {
        let mut lib = h.state().library_mut();
        // Three photos selected, the cursor on the middle one.
        lib.with_gallery(&first, |l| {
            l.select_kept(
                &["a".to_string(), "c".to_string(), "e".to_string()],
                Some("c"),
            )
        });
        lib.with_gallery(&second, |l| l.select_ids(&["d".to_string()]));
    }
    h.run_steps(3);
    assert_eq!(payload(&h, &first)["selection"], json!(["a", "c", "e"]));
    assert_eq!(payload(&h, &first)["cursor"], "c");
    // A lone cursor is its own selection: not written twice.
    assert_eq!(payload(&h, &second).get("selection"), None);
    let agent = || Origin::Agent("test".into());
    let before = run(&mut h, SELECTION_COMMAND, json!({}), agent());
    assert_eq!(
        before["views"][1],
        json!({ "view": first, "selection": ["a", "c", "e"], "cursor": "c" })
    );
    assert_eq!(
        before["views"][2],
        json!({ "view": second, "selection": ["d"], "cursor": "d" })
    );
    assert_eq!(before["views"][0]["view"], "main");

    let saved = h.state().app().handle().layout().to_json();
    drop(h);
    let mut h = harness(Some(Layout::from_json(&saved).unwrap()));
    h.run_steps(3);
    let after = run(&mut h, SELECTION_COMMAND, json!({}), agent());
    assert_eq!(after, before, "the agent's snapshot reads back the same");
    let one = run(&mut h, SELECTION_COMMAND, json!({ "view": first }), agent());
    assert_eq!(one, before["views"][1]);
    assert!(
        h.state_mut()
            .run(
                SELECTION_COMMAND,
                json!({ "view": "helios.vault#99" }),
                agent()
            )
            .is_err(),
        "no such view"
    );
}
