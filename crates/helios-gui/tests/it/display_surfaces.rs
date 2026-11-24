//! Several display surfaces (`display_surfaces`): a photo is shown on the
//! Image or on Compare through one bus request, `photo.show`. A double
//! click (Enter) in a gallery goes through it, and the strategy the user
//! picked chooses the surface: the primary one, the one focused last, the
//! one in the gallery's window, or ask. The photo menu's "Show in ▸" runs
//! the same request an agent does, with the same answer and effect, and
//! the surface a double click reaches wears a target mark. Compare opens
//! as many times as asked ("New surface", "New window", the always-new
//! strategy), each with a comparison of its own.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use helios_gui::dialogs::MenuItem;
use helios_gui::display_surfaces::{
    COMPARE, IMAGE, NEW, NEW_WINDOW, SHOW_COMMAND, STRATEGY_SETTING,
};
use helios_gui::library::Entry;
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

fn harness() -> Harness<'static, Shell> {
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries());
    let mut h = headless::harness(shell, vec2(1600.0, 1000.0));
    h.run_steps(3);
    h
}

fn set_strategy(h: &mut Harness<'_, Shell>, word: &str) {
    use egui_pop::model::values::{Change, Set, Value as PopValue};
    let store = h.state().app().handle().store();
    store
        .lock()
        .unwrap()
        .set(
            Set::new(
                &format!("settings/{STRATEGY_SETTING}"),
                Change::To(PopValue::Choice(word.into())),
                egui_pop::model::commands::Origin::User,
            )
            .untracked(),
        )
        .unwrap();
    h.run_steps(2);
}

/// Runs `photo.show`: a shell command, done in the next frame.
fn run(h: &mut Harness<'_, Shell>, args: Value, origin: Origin) {
    let _ = h.state_mut().run(SHOW_COMMAND, args, origin);
    h.run_steps(2);
}

/// Where the last `photo.show` went: the photo the Image is asked to
/// open, and the photos Compare shows.
fn shown(h: &mut Harness<'_, Shell>) -> (Option<String>, Vec<String>) {
    let image = h.state_mut().take_show_in_image();
    let compare = h.state().comparison().photos().to_vec();
    (image, compare)
}

/// Compare opened beside the Image, docked at the end side.
fn open_compare(h: &mut Harness<'_, Shell>) {
    h.state_mut()
        .run(
            "workbench.panel.open",
            json!({ "panel": COMPARE, "place": "end_side" }),
            Origin::User,
        )
        .unwrap();
    h.run_steps(3);
}

/// A double click's route: what the shell's `show_activated` sends, from
/// the gallery `from`.
fn double_click(h: &mut Harness<'_, Shell>, photo: &str, from: &str) {
    run(
        h,
        json!({ "photo": photo, "origin": "double_click", "from": from }),
        Origin::User,
    );
}

/// Compares exactly `images`, the effects read since cleared.
fn compare_only(h: &mut Harness<'_, Shell>, images: &[&str]) {
    h.state_mut()
        .run("compare.set", json!({ "images": images }), Origin::User)
        .unwrap();
    h.run_steps(2);
    shown(h);
}

#[test]
fn each_double_click_strategy_routes_the_photo_as_set() {
    let mut h = harness();
    open_compare(&mut h);

    // Primary (the default): the Image, as before surfaces.
    double_click(&mut h, "b", "helios.vault");
    assert_eq!(shown(&mut h), (Some("b".to_string()), vec![]));
    assert_eq!(h.state().library().double_click_target(), Some(IMAGE));

    // Same window: a gallery of the main window shows on the Image
    // there (the rule across windows: `display_surfaces`'s unit test,
    // headless runs keeping one window).
    set_strategy(&mut h, "same_window");
    double_click(&mut h, "a", "helios.vault");
    assert_eq!(shown(&mut h), (Some("a".to_string()), vec![]));

    // Last focused: Compare once the focus went to it.
    set_strategy(&mut h, "last_focused");
    h.state_mut()
        .run("helios.compare.focus", json!(null), Origin::User)
        .unwrap();
    h.run_steps(3);
    double_click(&mut h, "d", "helios.vault");
    let (image, compare) = shown(&mut h);
    assert_eq!(image, None);
    assert_eq!(compare, ["d"], "Compare as a loupe: a comparison of one");

    // Ask: nothing shown until a surface is chosen; the choice runs
    // `photo.show` with the surface, for the photo asked about.
    set_strategy(&mut h, "ask");
    double_click(&mut h, "b", "helios.vault");
    assert_eq!(shown(&mut h), (None, vec!["d".to_string()]), "asked first");
    assert_eq!(h.state().library().double_click_target(), None);
    run(&mut h, json!({ "surface": IMAGE }), Origin::User);
    assert_eq!(shown(&mut h).0.as_deref(), Some("b"));
}

#[test]
fn an_agents_photo_show_equals_the_show_in_menus() {
    let mut h = harness();
    open_compare(&mut h);
    let ctx = h.ctx.clone();
    let items = h.state().library().show_in_items(&ctx, "c");
    let shows: Vec<Value> = items
        .iter()
        .filter_map(|i| match i {
            MenuItem::Command { id, args, .. } if id == SHOW_COMMAND => Some(args.clone()),
            _ => None,
        })
        .collect();
    let surfaces: Vec<&str> = shows.iter().filter_map(|a| a["surface"].as_str()).collect();
    assert_eq!(surfaces, [IMAGE, COMPARE], "both open surfaces offered");
    for args in shows {
        compare_only(&mut h, &["a"]);
        run(&mut h, args.clone(), Origin::User);
        let by_menu = shown(&mut h);
        compare_only(&mut h, &["a"]);
        run(&mut h, args.clone(), Origin::Agent("test".into()));
        let by_agent = shown(&mut h);
        assert_eq!(by_menu, by_agent, "{args}");
        assert_ne!(by_menu, (None, vec!["a".to_string()]), "{args}: shown");
    }
    // A surface that is not one shows nothing.
    compare_only(&mut h, &["a"]);
    run(
        &mut h,
        json!({ "photo": "c", "surface": "helios.graph" }),
        Origin::Agent("test".into()),
    );
    assert_eq!(shown(&mut h), (None, vec!["a".to_string()]));
}

/// The Compares open, the declared one first.
fn compares(h: &Harness<'_, Shell>) -> Vec<String> {
    let app = h.state().app().handle();
    app.panel_instances(&egui_pop::model::ids::PanelId::from_static(COMPARE))
        .into_iter()
        .map(|p| p.to_string())
        .collect()
}

/// Whether a widget (a tab) carries `label`.
fn labelled(h: &Harness<'_, Shell>, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

#[test]
fn new_surface_and_new_window_open_compares_of_their_own() {
    let mut h = harness();
    open_compare(&mut h);
    compare_only(&mut h, &["a"]);
    let ctx = h.ctx.clone();
    // "Show in ▸" offers a new surface and a new window.
    let items = h.state().library().show_in_items(&ctx, "c");
    let surfaces: Vec<String> = items
        .iter()
        .filter_map(|i| match i {
            MenuItem::Command { id, args, .. } if id == SHOW_COMMAND => {
                args["surface"].as_str().map(String::from)
            }
            _ => None,
        })
        .collect();
    assert_eq!(surfaces, [IMAGE, COMPARE, NEW, NEW_WINDOW]);

    // New surface: a second Compare, comparing the photo alone; the
    // first keeps its own.
    run(
        &mut h,
        json!({ "photo": "c", "surface": NEW }),
        Origin::User,
    );
    h.run_steps(3);
    assert_eq!(compares(&h), [COMPARE, "helios.compare#2"]);
    assert_eq!(h.state().compared_in("helios.compare#2"), ["c"]);
    assert_eq!(shown(&mut h), (None, vec!["a".to_string()]));
    assert!(labelled(&h, "Compare 2"), "the instance's tab titled");

    // `photo.show` with the instance as `surface` targets it alone.
    run(
        &mut h,
        json!({ "photo": "d", "surface": "helios.compare#2" }),
        Origin::Agent("test".into()),
    );
    assert_eq!(h.state().compared_in("helios.compare#2"), ["d"]);
    assert_eq!(shown(&mut h), (None, vec!["a".to_string()]));
    // And the `compare.*` commands too.
    h.state_mut()
        .run(
            "compare.add",
            json!({ "images": ["e"], "surface": "helios.compare#2" }),
            Origin::Agent("test".into()),
        )
        .unwrap();
    h.run_steps(2);
    assert_eq!(h.state().compared_in("helios.compare#2"), ["d", "e"]);
    assert_eq!(h.state().comparison().photos(), ["a"]);

    // New window: a third Compare, detached in a window of its own.
    run(
        &mut h,
        json!({ "photo": "b", "surface": NEW_WINDOW }),
        Origin::User,
    );
    h.run_steps(3);
    assert_eq!(
        compares(&h),
        [COMPARE, "helios.compare#2", "helios.compare#3"]
    );
    assert_eq!(h.state().compared_in("helios.compare#3"), ["b"]);
    let layout = h.state().app().handle().layout();
    assert!(
        helios_gui::display_surfaces::window_of(&layout, "helios.compare#3").is_some(),
        "in a window of its own"
    );
    assert!(helios_gui::display_surfaces::window_of(&layout, "helios.compare#2").is_none());
    // Kept with the layout: its instance's payload names its photos.
    let kept = layout
        .instance(&egui_pop::model::ids::PanelId::new("helios.compare#2").unwrap())
        .map(|i| i.payload.clone());
    assert_eq!(kept, Some(json!({ "images": ["d", "e"] })));
}

#[test]
fn the_always_new_strategy_opens_a_fresh_compare_each_double_click() {
    let mut h = harness();
    set_strategy(&mut h, "always_new");
    double_click(&mut h, "a", "helios.vault");
    h.run_steps(3);
    double_click(&mut h, "b", "helios.vault");
    h.run_steps(3);
    let open = compares(&h);
    assert_eq!(open.len(), 2, "{open:?}");
    let [first, second] = [&open[0], &open[1]];
    assert_eq!(h.state().compared_in(first), ["a"]);
    assert_eq!(h.state().compared_in(second), ["b"]);
    assert!(labelled(&h, "Compare 3"));
    // Nothing on the Image, and no target mark (each is a new one).
    assert_eq!(shown(&mut h).0, None);
    assert_eq!(h.state().library().double_click_target(), None);
}

#[test]
fn the_target_mark_moves_to_the_compare_focused_last() {
    let mut h = harness();
    open_compare(&mut h);
    run(
        &mut h,
        json!({ "photo": "c", "surface": NEW }),
        Origin::User,
    );
    h.run_steps(3);
    set_strategy(&mut h, "last_focused");
    // The new Compare had the focus last: it wears the mark, and a
    // double click shows there.
    assert_eq!(
        h.state().library().double_click_target(),
        Some("helios.compare#2")
    );
    double_click(&mut h, "e", "helios.vault");
    assert_eq!(h.state().compared_in("helios.compare#2"), ["e"]);
    // The first Compare focused: the mark moves to it.
    h.state()
        .app()
        .handle()
        .focus_panel(&egui_pop::model::ids::PanelId::from_static(COMPARE));
    h.run_steps(3);
    assert_eq!(h.state().library().double_click_target(), Some(COMPARE));
    // Closed, the second Compare is no target and keeps no comparison.
    h.state().app().handle().set_panel_visible(
        &egui_pop::model::ids::PanelId::new("helios.compare#2").unwrap(),
        false,
    );
    h.run_steps(3);
    assert!(h.state().compared_in("helios.compare#2").is_empty());
}
