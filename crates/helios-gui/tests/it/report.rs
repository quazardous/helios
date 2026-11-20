//! Help > Report a problem: the report egui-pop's recorder attaches
//! (`help.report.bundle`), with Helios's parts. Image ids are aliased
//! `photo-1`, `photo-2`… the same way all through one report; the
//! settings part lists only those changed; the report is shown in a
//! dialog before anything is copied or opened.

use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use egui_pop::api::about::CMD_REPORT_SEND;
use egui_pop::api::dialogs::{DIALOGS, Dialog};
use egui_pop::model::values::{Change, Set, Value as PopValue};
use helios_gui::library::Entry;
use helios_gui::{Shell, display, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::{Value as Json, json};

fn id(c: char) -> String {
    c.to_string().repeat(64)
}

/// A shell whose library holds three photos, the first selected.
fn harness() -> Harness<'static, Shell> {
    let shell = Shell::new(helios_panels(), None);
    {
        let mut lib = shell.library_mut();
        lib.set_entries(
            ['a', 'b', 'c']
                .into_iter()
                .map(|c| Entry {
                    id: id(c),
                    name: format!("Jane {c}.CR2"),
                    ..Entry::default()
                })
                .collect(),
        );
        lib.select(0);
    }
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

fn bundle(h: &mut Harness<'static, Shell>) -> Json {
    h.state_mut()
        .run("help.report.bundle", Json::Null, Origin::User)
        .expect("the report's bundle")
}

/// The data of the journal's entries named `name`.
fn data_of(b: &Json, name: &str) -> Vec<Json> {
    b["journal"]
        .as_array()
        .expect("a journal")
        .iter()
        .filter(|e| e["name"] == name)
        .map(|e| e["data"].clone())
        .collect()
}

fn set_wide_gamut(h: &mut Harness<'static, Shell>, on: bool) {
    let store = h.state().app().handle().store();
    store
        .lock()
        .unwrap()
        .set(
            Set::new(
                &format!("settings/{}", display::SETTING_WIDE_GAMUT),
                Change::To(PopValue::Bool(on)),
                egui_pop::model::commands::Origin::User,
            )
            .untracked(),
        )
        .unwrap();
    h.step();
}

#[test]
fn image_ids_keep_one_alias_all_through_the_report() {
    let mut h = harness();
    let handle = h.state().app().handle().clone();
    for c in ['a', 'b', 'a'] {
        handle.publish("test", None, "test.photo", json!({ "image": id(c) }));
    }
    h.run();
    let b = bundle(&mut h);
    let text = b.to_string();
    for c in ['a', 'b', 'c'] {
        assert!(!text.contains(&id(c)), "an image id left in {text}");
    }
    let seen: Vec<Json> = data_of(&b, "test.photo")
        .into_iter()
        .map(|d| d["image"].clone())
        .collect();
    assert_eq!(seen.len(), 3, "{text}");
    let first = seen[0].as_str().expect("an alias").to_string();
    assert!(first.starts_with("photo-"), "{first}");
    assert_eq!(seen[2], seen[0], "the same photo, the same alias");
    assert_ne!(seen[1], seen[0], "another photo, another alias");
    // The library part names the open photo by the journal's alias.
    assert_eq!(b["context"]["library"]["open"], json!(first));
    assert_eq!(b["context"]["library"]["photos"], 3);
    assert!(!text.contains("Jane"), "{text}");
    // A second report gives the same aliases.
    let again = bundle(&mut h);
    let seen_again: Vec<Json> = data_of(&again, "test.photo")
        .into_iter()
        .map(|d| d["image"].clone())
        .collect();
    assert_eq!(seen_again[..3], seen[..]);
    assert_eq!(again["context"]["library"]["open"], json!(first));
}

#[test]
fn the_settings_part_lists_only_the_changed_settings() {
    let mut h = harness();
    let setting = display::SETTING_WIDE_GAMUT;
    let key = setting.as_str();
    let before = bundle(&mut h)["context"]["settings"].clone();
    let before = before.as_object().expect("the settings part").clone();
    assert!(!before.contains_key(key), "{before:?}");
    set_wide_gamut(&mut h, true);
    let changed = bundle(&mut h)["context"]["settings"].clone();
    let changed = changed.as_object().unwrap().clone();
    assert_eq!(
        changed.get(key),
        Some(&json!({ "bool": true })),
        "a value as egui-pop writes it: {changed:?}"
    );
    let mut rest = changed.clone();
    rest.remove(key);
    assert_eq!(rest, before, "nothing else is said changed");
    // Back to its default: gone again.
    set_wide_gamut(&mut h, false);
    let after = bundle(&mut h)["context"]["settings"].clone();
    assert_eq!(after.as_object().unwrap(), &before);
}

#[test]
fn the_report_is_shown_before_anything_is_copied_or_opened() {
    let mut h = harness();
    let out = h
        .state_mut()
        .run("help.report", Json::Null, Origin::User)
        .expect("the report opens");
    assert!(out["dialog"].as_u64().is_some(), "{out}");
    h.step();
    let sent = |h: &Harness<'static, Shell>| {
        h.output().platform_output.commands.iter().any(|c| {
            matches!(
                c,
                egui::OutputCommand::OpenUrl(_) | egui::OutputCommand::CopyText(_)
            )
        })
    };
    assert!(
        !sent(&h),
        "nothing copied nor opened before the user says so"
    );
    let dialogs = h.state().app().handle().service(&DIALOGS).expect("dialogs");
    match dialogs.current() {
        Some((_, Dialog::Custom(c))) => {
            assert_eq!(c.command, Some(CMD_REPORT_SEND));
        }
        other => panic!("not the report's dialog: {other:?}"),
    }
    h.run();
    assert!(
        h.query_by_label_contains("Nothing has been sent").is_some(),
        "the dialog says nothing was sent"
    );
    // Its button copies the report and opens the issue.
    h.get_by_role_and_label(Role::Button, "Copy and open the issue")
        .click_accesskit();
    h.step();
    let commands = &h.output().platform_output.commands;
    let copied = commands.iter().find_map(|c| match c {
        egui::OutputCommand::CopyText(t) => Some(t.clone()),
        _ => None,
    });
    let opened = commands.iter().find_map(|c| match c {
        egui::OutputCommand::OpenUrl(u) => Some(u.url.clone()),
        _ => None,
    });
    let copied = copied.expect("the report copied");
    assert!(copied.contains("\"open\": \"photo-"), "{copied}");
    assert!(!copied.contains(&id('a')), "{copied}");
    assert!(
        opened.is_some_and(|u| u.contains("/issues/new")),
        "the issue opened"
    );
}
