//! Several display surfaces (`display_surfaces`): a photo is shown on the
//! Image or on Compare through one bus request, `photo.show`. A double
//! click (Enter) in a gallery goes through it, and the strategy the user
//! picked chooses the surface: the primary one, the one focused last, the
//! one in the gallery's window, or ask. The photo menu's "Show in ▸" runs
//! the same request an agent does, with the same answer and effect, and
//! the surface a double click reaches wears a target mark.

use egui::vec2;
use egui_kittest::Harness;
use egui_pop::model::ids::PanelId;
use egui_pop::model::layout::Auxiliary;
use helios_gui::dialogs::MenuItem;
use helios_gui::display_surfaces::{COMPARE, IMAGE, SHOW_COMMAND, STRATEGY_SETTING};
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

fn run(h: &mut Harness<'_, Shell>, args: Value, origin: Origin) -> Value {
    let out = h.state_mut().run(SHOW_COMMAND, args, origin).unwrap();
    h.run_steps(2);
    out
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
fn double_click(h: &mut Harness<'_, Shell>, photo: &str, from: &str) -> Value {
    run(
        h,
        json!({ "photo": photo, "origin": "double_click", "from": from }),
        Origin::User,
    )
}

#[test]
fn each_double_click_strategy_routes_the_photo_as_set() {
    let mut h = harness();
    open_compare(&mut h);

    // Primary (the default): the Image, as before surfaces.
    let out = double_click(&mut h, "b", "helios.vault");
    assert_eq!(out["surface"], IMAGE);
    assert_eq!(shown(&mut h).0.as_deref(), Some("b"));
    assert_eq!(h.state().library().double_click_target(), Some(IMAGE));

    // Last focused: Compare once the focus went to it.
    set_strategy(&mut h, "last_focused");
    let out = double_click(&mut h, "c", "helios.vault");
    assert_eq!(out["surface"], IMAGE, "no surface focused yet: the primary");
    shown(&mut h);
    h.state_mut()
        .run("helios.compare.focus", json!(null), Origin::User)
        .unwrap();
    h.run_steps(3);
    assert_eq!(h.state().library().double_click_target(), Some(COMPARE));
    let out = double_click(&mut h, "d", "helios.vault");
    assert_eq!(out["surface"], COMPARE);
    let (image, compare) = shown(&mut h);
    assert_eq!(image, None);
    assert_eq!(compare, ["d"], "Compare as a loupe: a comparison of one");

    // Same window: Compare in a window of its own with a second gallery;
    // a double click there shows on Compare, one in the main window on
    // the Image.
    set_strategy(&mut h, "same_window");
    let mut layout = h.state().app().handle().layout();
    let compare = PanelId::new(COMPARE).unwrap();
    layout.remove(&compare);
    layout.hidden.retain(|p| p != &compare);
    layout.auxiliary.push(Auxiliary::window(
        compare.clone(),
        "main",
        [0.0, 0.0, 800.0, 600.0],
    ));
    let gallery = PanelId::new("helios.vault#2").unwrap();
    layout.join_window(&gallery, &compare).unwrap();
    h.state_mut().set_layout(layout);
    h.run_steps(3);
    assert_eq!(
        double_click(&mut h, "e", "helios.vault#2")["surface"],
        COMPARE
    );
    assert_eq!(shown(&mut h), (None, vec!["e".to_string()]));
    assert_eq!(double_click(&mut h, "a", "helios.vault")["surface"], IMAGE);
    assert_eq!(shown(&mut h).0.as_deref(), Some("a"));

    // Ask: nothing shown until a surface is chosen; the choice runs
    // `photo.show` with the surface, for the photo asked about.
    set_strategy(&mut h, "ask");
    let out = double_click(&mut h, "b", "helios.vault");
    assert_eq!(out["asked"], true);
    assert_eq!(shown(&mut h).0, None);
    assert_eq!(h.state().library().double_click_target(), None);
    let out = run(&mut h, json!({ "surface": IMAGE }), Origin::User);
    assert_eq!(out, json!({ "photo": "b", "surface": IMAGE }));
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
        let by_menu = run(&mut h, args.clone(), Origin::User);
        let menu_effect = shown(&mut h);
        let by_agent = run(&mut h, args.clone(), Origin::Agent("test".into()));
        let agent_effect = shown(&mut h);
        assert_eq!(by_menu, by_agent, "{args}");
        assert_eq!(menu_effect, agent_effect, "{args}");
    }
    // A surface that is not one is refused.
    assert!(
        h.state_mut()
            .run(
                SHOW_COMMAND,
                json!({ "photo": "c", "surface": "helios.graph" }),
                Origin::Agent("test".into())
            )
            .is_err()
    );
}
