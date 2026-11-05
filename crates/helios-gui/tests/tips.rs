//! Helios's tips: registered from their files, every one naming commands
//! the desktop declares, in Help > Tips…, shown with the keys in force,
//! and translated in French.

use egui::vec2;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_pop::api::tips::TIPS;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::json;

#[test]
fn helios_s_tips_are_registered_and_name_what_the_desktop_declares() {
    let shell = Shell::new(helios_panels(), None);
    let tips = shell.app().handle().service(&TIPS).expect("tips connected");
    let ours: Vec<_> = tips
        .all()
        .into_iter()
        .filter(|t| t.origin_name() == helios_gui::tips::PLUGIN.as_str())
        .collect();
    assert_eq!(ours.len(), helios_gui::tips::FILES.len());
    // Outside the desktop's window (tests, pictures, tools), tips only
    // come when asked.
    assert!(!tips.auto());
    shell.tips_by_themselves(true);
    assert!(tips.auto());
    // A renamed command or a broken surface fails here.
    egui_pop_test::assert_tips_clean(shell.app(), &[]);
}

#[test]
fn the_help_menu_has_tips() {
    let mut shell = Shell::new(helios_panels(), None);
    shell.draw_frame(true);
    let items = shell.menu_items("app.menu.help");
    let tips = items
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "tips.browse")
        .expect("Help has egui-pop's tips browser");
    assert_eq!(tips["title"], json!("Tips…"));
}

#[test]
fn a_tip_shows_the_key_in_force() {
    let mut h = headless::harness(Shell::new(helios_panels(), None), vec2(1280.0, 800.0));
    h.run();
    let shown = h
        .state_mut()
        .run(
            "tips.show",
            json!({ "tip": "helios.palette" }),
            Origin::User,
        )
        .unwrap();
    assert_eq!(shown["shown"], json!("helios.palette"));
    h.run();
    let current = h
        .state_mut()
        .run("tips.current", json!(null), Origin::User)
        .unwrap();
    assert_eq!(current["title"], json!("Find any command"));
    let text = current["text"].as_str().unwrap();
    // The palette's key, as the keymap binds it, in code.
    assert!(text.starts_with('`'), "{text}");
    assert!(!text.contains('{') && !text.contains('$'), "{text}");
    assert!(current["card"].is_array(), "the card is drawn: {current}");
    h.state_mut()
        .run("tips.notNow", json!(null), Origin::User)
        .unwrap();
    h.run();
    let none = h
        .state_mut()
        .run("tips.current", json!(null), Origin::User)
        .unwrap();
    assert!(none.is_null());
}

#[test]
fn every_tip_of_helios_is_translated_in_french() {
    let i = helios_i18n::I18n::with_egui_pop("fr");
    i.add_catalogues(helios_i18n::CATALOGUES).unwrap();
    let french = i.keys("fr");
    for (file, _) in helios_gui::tips::FILES {
        let id = file.trim_end_matches(".md").replace('.', "-");
        for attr in ["title", "text"] {
            let key = format!("tip-{id}.{attr}");
            assert!(french.contains(&key), "{key} has no French");
        }
    }
    let args = [("key", helios_i18n::FluentValue::from("`F1`"))];
    assert_eq!(
        i.get("tip-helios-palette.text", &args).unwrap(),
        "`F1` ouvre la palette de commandes : tapez quelques lettres de ce que vous voulez faire, puis Entrée pour le lancer."
    );
}

/// The tip that came by itself on the desktop closes with the cross on
/// its card, and stays closed.
#[test]
fn the_cross_closes_a_tip_that_came_by_itself() {
    let shell = Shell::new(helios_panels(), None);
    shell.tips_by_themselves(true);
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    let current = |h: &mut egui_kittest::Harness<'_, Shell>| {
        h.state_mut()
            .run("tips.current", json!(null), Origin::User)
            .unwrap()
    };
    // Steps of a quarter of a second, past the first tip's moment.
    let past_first = ((egui_pop::api::tips::FIRST_AFTER + 1.0) * 4.0) as usize;
    for _ in 0..past_first {
        h.step();
    }
    h.run();
    let shown = current(&mut h);
    assert!(shown["card"].is_array(), "no tip came by itself: {shown}");
    assert_no_id_clash(&h);
    h.get_by_label("Not now").click();
    h.run();
    assert!(
        current(&mut h).is_null(),
        "the cross did not close the card"
    );
    let title = shown["title"].as_str().unwrap();
    assert!(h.query_by_label(title).is_none());
    // Tips still come by themselves: the card does not come back.
    for _ in 0..past_first {
        h.step();
    }
    h.run();
    assert!(current(&mut h).is_null(), "the card came back");
}

/// egui's id clash warnings (in debug builds) the last pass drew, with
/// the widgets under each: two widgets sharing an id can swallow each
/// other's clicks.
fn assert_no_id_clash(h: &egui_kittest::Harness<'_, Shell>) {
    let shapes: Vec<&egui::Shape> = h.output().shapes.iter().map(|c| &c.shape).collect();
    let mut clashes = Vec::new();
    for (i, shape) in shapes.iter().enumerate() {
        let egui::Shape::Text(text) = shape else {
            continue;
        };
        if !text.galley.text().contains('🔥') {
            continue;
        }
        // The clashing widget's outline is drawn just before its text.
        let rect = i
            .checked_sub(1)
            .and_then(|j| match shapes[j] {
                egui::Shape::Rect(r) => Some(r.rect),
                _ => None,
            })
            .unwrap_or(egui::Rect::from_min_size(text.pos, egui::Vec2::ZERO));
        let under: Vec<String> = h
            .root()
            .children_recursive()
            .filter(|n| n.rect().contains(rect.center()))
            .map(|n| {
                format!(
                    "{:?} {:?}",
                    n.accesskit_node().role(),
                    n.accesskit_node().label()
                )
            })
            .collect();
        clashes.push(format!(
            "{} at {rect:?}, under: {under:?}",
            text.galley.text()
        ));
    }
    assert!(clashes.is_empty(), "id clashes:\n{}", clashes.join("\n"));
}
