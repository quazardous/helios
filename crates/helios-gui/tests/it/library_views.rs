//! Several Library views (`library_views`): the Results panel opens as
//! many times as asked, each new one a view of its own, with its own
//! filter and cursor. Moving in one leaves the others as they are; a
//! restart gives each back with its filter and cursor; closing one
//! leaves the others; an agent's `library.view.open` opens one as the
//! View menu's "New Library" does.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::model::ids::PanelId;
use egui_pop::model::layout::Layout;
use egui_pop::views::library::filter::Filter;
use helios_gui::library::Entry;
use helios_gui::library_views::{
    CLOSE_COMMAND, LINK_COMMAND, OPEN_COMMAND, SELECTION_COMMAND, close_command, open_views,
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

/// A command's answer: the shell queues it for its frame, then its
/// history has the result.
fn answer(h: &mut Harness<'static, Shell>, id: &str, args: Value) -> Result<Value, String> {
    let out = h.state_mut().run(id, args, Origin::Agent("test".into()));
    h.run_steps(3);
    match out {
        Ok(v) if v == json!({ "queued": true }) => {
            h.state()
                .history()
                .into_iter()
                .rev()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id} never ran"))
                .result
        }
        other => other,
    }
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
            );
        });
        lib.with_gallery(&second, |l| l.select_ids(&["d".to_string()]));
    }
    h.run_steps(3);
    assert_eq!(payload(&h, &first)["selection"], json!(["a", "c", "e"]));
    assert_eq!(payload(&h, &first)["cursor"], "c");
    // A lone cursor is its own selection: not written twice.
    assert_eq!(payload(&h, &second).get("selection"), None);
    let before = answer(&mut h, SELECTION_COMMAND, json!({})).unwrap();
    assert_eq!(
        before["views"].as_array().map(Vec::len),
        Some(3),
        "{before}"
    );
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
    let after = answer(&mut h, SELECTION_COMMAND, json!({})).unwrap();
    assert_eq!(after, before, "the agent's snapshot reads back the same");
    let one = answer(&mut h, SELECTION_COMMAND, json!({ "view": first })).unwrap();
    assert_eq!(one, before["views"][1]);
    assert!(
        answer(
            &mut h,
            SELECTION_COMMAND,
            json!({ "view": "helios.vault#99" })
        )
        .is_err(),
        "no such view"
    );
}

/// S7 part 2: the main view's selection and cursor are kept with the
/// layout (the main Results panel's payload) and read back the same
/// after a restart.
#[test]
fn the_main_view_s_selection_survives_a_restart() {
    let mut h = harness(None);
    h.state().library_mut().with_gallery("main", |l| {
        l.select_kept(&["b".to_string(), "d".to_string()], Some("d"));
    });
    h.run_steps(3);
    let before = answer(&mut h, SELECTION_COMMAND, json!({ "view": "main" })).unwrap();
    assert_eq!(
        before,
        json!({ "view": "main", "selection": ["b", "d"], "cursor": "d" })
    );
    let saved = h.state().app().handle().layout().to_json();
    drop(h);
    let mut h = harness(Some(Layout::from_json(&saved).unwrap()));
    h.run_steps(3);
    let after = answer(&mut h, SELECTION_COMMAND, json!({ "view": "main" })).unwrap();
    assert_eq!(
        after, before,
        "the main view's selection reads back the same"
    );
}

/// S7 part 2 (G7/G8): a view linked to the main one follows its
/// selection; an unlinked one keeps its own. The toggle is found by its
/// accesskit name and turns the link on; the link is kept through a
/// restart.
#[test]
fn a_linked_view_follows_the_main_selection_and_an_unlinked_one_does_not() {
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
    let (linked, own) = (views[0].clone(), views[1].clone());
    h.state()
        .library_mut()
        .with_gallery(&own, |l| l.select_ids(&["a".to_string()]));
    let out = answer(
        &mut h,
        LINK_COMMAND,
        json!({ "view": linked, "linked": true }),
    )
    .unwrap();
    assert_eq!(out, json!({ "view": linked, "linked": true }));
    assert_eq!(payload(&h, &linked)["link"], "main");

    h.state().library_mut().with_gallery("main", |l| {
        l.select_kept(&["c".to_string(), "e".to_string()], Some("e"));
    });
    h.run_steps(3);
    let read = |h: &mut Harness<'static, Shell>, v: &str| {
        answer(h, SELECTION_COMMAND, json!({ "view": v })).unwrap()
    };
    assert_eq!(read(&mut h, &linked)["selection"], json!(["c", "e"]));
    assert_eq!(read(&mut h, &linked)["cursor"], "e");
    assert_eq!(
        read(&mut h, &own)["selection"],
        json!(["a"]),
        "unlinked: its own"
    );

    // Unlinked from its toggle (by its accesskit name), it keeps what it has.
    let toggles: Vec<_> = h
        .query_all_by_label("Follow the main view's selection")
        .collect();
    assert_eq!(toggles.len(), 2, "one toggle per view");
    drop(toggles);
    answer(&mut h, LINK_COMMAND, json!({ "view": linked })).unwrap();
    assert_eq!(payload(&h, &linked).get("link"), None);
    h.state()
        .library_mut()
        .with_gallery("main", |l| l.select_ids(&["b".to_string()]));
    h.run_steps(3);
    assert_eq!(read(&mut h, &linked)["selection"], json!(["c", "e"]));

    // Linked again from the toggle: kept through a restart.
    h.get_all_by_label("Follow the main view's selection")
        .next()
        .unwrap()
        .click();
    h.run_steps(3);
    let first_linked = open_views(h.state().app().handle())
        .into_iter()
        .filter(|v| payload(&h, v)["link"] == "main")
        .count();
    assert_eq!(first_linked, 1, "the toggle linked one view");
    let saved = h.state().app().handle().layout().to_json();
    drop(h);
    let mut h = harness(Some(Layout::from_json(&saved).unwrap()));
    h.run_steps(3);
    let linked_after = open_views(h.state().app().handle())
        .into_iter()
        .filter(|v| payload(&h, v)["link"] == "main")
        .count();
    assert_eq!(linked_after, 1, "the link is kept with the layout");
}

/// G8: the culling strip, a Library view in one row under Compare. It
/// opens linked to the main view, so it shows the main selection; found
/// by its accesskit name and focused, its arrows move its cursor, and
/// Space adds the photo under it to the comparison, then takes it out.
#[test]
fn the_culling_strip_follows_the_main_view_its_arrows_move_and_space_edits_compare() {
    use egui::Key;
    let mut h = harness(None);
    answer(&mut h, "compare.set", json!({ "images": ["a", "b"] })).unwrap();
    let out = answer(&mut h, OPEN_COMMAND, json!({ "strip": true })).unwrap();
    assert_eq!(out["strip"], true);
    let strip = out["view"].as_str().unwrap().to_string();
    assert_eq!(payload(&h, &strip)["strip"], true);
    assert_eq!(payload(&h, &strip)["link"], "main", "linked when it opens");

    // The strip shows the main view's selection.
    h.state()
        .library_mut()
        .with_gallery("main", |l| l.select_ids(&["c".to_string()]));
    h.run_steps(3);
    let read = |h: &mut Harness<'static, Shell>| {
        answer(h, SELECTION_COMMAND, json!({ "view": strip })).unwrap()
    };
    assert_eq!(read(&mut h)["cursor"], "c");

    // Focused, an arrow moves its cursor, not the main view's.
    h.get_by_label("Culling filmstrip").click();
    h.run_steps(2);
    assert!(
        h.get_by_label("Culling filmstrip").is_focused(),
        "a click gives the strip the keyboard"
    );
    assert_eq!(read(&mut h)["cursor"], "c");
    h.key_press(Key::ArrowRight);
    h.run_steps(3);
    assert_eq!(read(&mut h)["cursor"], "d", "the arrow moved its cursor");
    assert_eq!(view(&h, "main").1.as_deref(), Some("c"), "not the main one");

    // Space adds the photo under the cursor to Compare, then takes it out.
    let compared = |h: &mut Harness<'static, Shell>| {
        answer(h, "compare.active", json!({})).unwrap()["images"].clone()
    };
    h.key_press(Key::Space);
    h.run_steps(3);
    assert_eq!(compared(&mut h), json!(["a", "b", "d"]));
    h.key_press(Key::Space);
    h.run_steps(3);
    assert_eq!(compared(&mut h), json!(["a", "b"]));

    // Its layout is kept with the layout.
    let saved = h.state().app().handle().layout().to_json();
    drop(h);
    let h = harness(Some(Layout::from_json(&saved).unwrap()));
    assert_eq!(payload(&h, &strip)["strip"], true);
}
