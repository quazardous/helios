//! Every clickable control says what it does: walked through the
//! accessibility tree of the main window (the Library panel with a photo
//! under the cursor, its marks bar shown), the Library window (a photo
//! under its own cursor too: each Library view keeps its selection), the
//! import window and the Activity focus, a control without a name, or named by
//! a symbol alone ("+", "↓", "×"), fails the test with its place in the
//! tree. An icon-only control carries a tooltip naming what it does and
//! the same words as its accessible name.

use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use helios_gui::library::{Entry, WINDOW_GALLERY};
use helios_gui::shell::LIBRARY_WINDOW_COMMAND;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::Origin;
use serde_json::{Value, json};

/// The roles a pointer or a key acts on.
const CLICKABLE: &[Role] = &[
    Role::Button,
    Role::DefaultButton,
    Role::CheckBox,
    Role::RadioButton,
    Role::Switch,
    Role::Slider,
    Role::SpinButton,
    Role::ComboBox,
    Role::Link,
    Role::MenuItem,
    Role::MenuItemCheckBox,
    Role::MenuItemRadio,
    Role::Tab,
    Role::ListBoxOption,
    Role::DisclosureTriangle,
    Role::ColorWell,
];

/// Controls named by a symbol on purpose, each with its reason. Empty:
/// keep it so; a new entry says why the symbol is the clearest name.
const ALLOWED: &[(&str, &str)] = &[];

/// A name a reader understands: it holds at least one letter.
fn says_something(name: &str) -> bool {
    name.chars().any(char::is_alphabetic)
}

/// The clickable nodes of `h`'s last pass with no name or a name of
/// symbols alone, each with the named nodes above it.
fn unclear<S>(h: &Harness<'_, S>) -> Vec<String> {
    let mut out = Vec::new();
    for n in h.root().children_recursive() {
        let a = n.accesskit_node();
        if !CLICKABLE.contains(&a.role()) {
            continue;
        }
        let name = a.label().unwrap_or_default();
        if !a.data().labelled_by().is_empty() || says_something(&name) {
            continue;
        }
        if ALLOWED.iter().any(|(allowed, _)| *allowed == name) {
            continue;
        }
        let mut path = vec![format!("{:?} {name:?}", a.role())];
        let mut at = a.parent();
        while let Some(p) = at {
            if let Some(l) = p.label().filter(|l| !l.is_empty()) {
                path.push(format!("{:?} {l:?}", p.role()));
            }
            at = p.parent();
        }
        path.reverse();
        out.push(path.join(" > "));
    }
    out
}

#[track_caller]
fn assert_clear<S>(h: &Harness<'_, S>, what: &str) {
    let found = unclear(h);
    assert!(
        found.is_empty(),
        "{what}: clickable controls without a name that says what they do:\n  {}\n\n\
         accessibility tree:\n{}",
        found.join("\n  "),
        egui_pop_test::a11y_tree(h)
    );
}

/// Three photos, the first under the cursor of each Library view (the
/// main panel's and the Library window's, each with its own selection):
/// the marks bar shows in both, as once a user clicked a photo there.
fn shell() -> Shell {
    let entries = ["a", "b", "c"]
        .iter()
        .map(|id| Entry {
            id: (*id).to_string(),
            name: format!("{id}.jpg"),
            ..Entry::default()
        })
        .collect();
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_library(entries);
    shell.library_mut().select(0);
    shell
        .library_mut()
        .with_gallery(WINDOW_GALLERY, |lib| lib.select(0));
    shell
}

fn run(h: &mut Harness<'_, Shell>, command: &str, args: Value) {
    h.state_mut().run(command, args, Origin::User).unwrap();
    h.run_steps(4);
}

#[test]
fn the_main_window_s_controls_say_what_they_do() {
    let mut h = headless::harness(shell(), vec2(1600.0, 1000.0));
    h.run_steps(4);
    assert_clear(&h, "the main window");
}

#[test]
fn the_library_window_s_controls_say_what_they_do() {
    let mut h = headless::harness(shell(), vec2(1600.0, 1000.0));
    h.run_steps(2);
    run(&mut h, LIBRARY_WINDOW_COMMAND, Value::Null);
    // The marks bar is among the controls walked: without a photo under
    // the window's cursor it is not drawn, and its names go unchecked.
    h.get_by_role_and_label(Role::Window, "Library")
        .get_by_role_and_label(Role::CheckBox, "Red label");
    assert_clear(&h, "the Library window");
}

#[test]
fn the_import_window_s_controls_say_what_they_do() {
    let mut h = headless::harness(shell(), vec2(1600.0, 1000.0));
    h.run_steps(2);
    run(&mut h, "library.importWindow", Value::Null);
    assert_clear(&h, "the import window");
}

#[test]
fn the_activity_focus_s_controls_say_what_they_do() {
    let mut h = headless::harness(shell(), vec2(1600.0, 1000.0));
    h.run_steps(2);
    run(&mut h, "activity.show", json!({}));
    assert_clear(&h, "the Activity focus");
}

#[test]
fn a_name_of_symbols_alone_says_nothing() {
    assert!(!says_something("+"));
    assert!(!says_something("↓"));
    assert!(!says_something(" × "));
    assert!(!says_something(""));
    assert!(says_something("Red label (6)"));
    assert!(says_something("Étoile"));
}

/// Hovers the node named `name` until a tooltip holding `tip` shows.
#[track_caller]
fn tooltip_of(h: &mut Harness<'_, Shell>, role: Role, name: &str, tip: &str) {
    let at = h
        .get_by_role_and_label(Role::Window, "Library")
        .get_by_role_and_label(role, name)
        .rect()
        .center();
    // Away first: the tooltip shown before goes.
    h.hover_at(egui::pos2(1.0, 999.0));
    h.run_steps(30);
    h.hover_at(at);
    for _ in 0..60 {
        h.step();
        if h.query_by_label(tip).is_some() {
            return;
        }
    }
    panic!(
        "no tooltip {tip:?} over {name:?}:\n{}",
        egui_pop_test::a11y_tree(h)
    );
}

#[test]
fn the_marks_bar_s_buttons_say_what_they_do_and_their_keys() {
    // In the Library window, wide enough for the whole marks bar (the
    // main window's panel may cut its end).
    let mut h = headless::harness(shell(), vec2(1600.0, 1000.0));
    h.run_steps(2);
    run(&mut h, LIBRARY_WINDOW_COMMAND, Value::Null);
    tooltip_of(&mut h, Role::CheckBox, "Red label", "Red label (6)");
    tooltip_of(&mut h, Role::CheckBox, "Purple label", "Purple label (F5)");
    tooltip_of(&mut h, Role::CheckBox, "Pick", "Flag as pick (P)");
    tooltip_of(&mut h, Role::CheckBox, "Reject", "Reject (X)");
}

/// The longest menu entry a reader takes in at a glance.
const MENU_TITLE_MAX: usize = 60;

/// Every command title of the menus in `items`, submenus walked.
fn menu_titles(items: &Value, out: &mut Vec<String>) {
    for item in items.as_array().into_iter().flatten() {
        if let Some(title) = item["title"].as_str() {
            out.push(title.to_string());
        }
        if item.is_array() {
            menu_titles(item, out);
        }
        menu_titles(&item["items"], out);
    }
}

/// The menu titles of `h` that read as a developer's notes: brackets,
/// backticks, or longer than [`MENU_TITLE_MAX`].
fn unclear_titles(h: &mut Harness<'_, Shell>) -> Vec<String> {
    let mut titles = Vec::new();
    for menu in [
        egui_pop::api::app::MENU_MAIN,
        egui_pop::api::app::MENU_FILE,
        egui_pop::api::app::MENU_EDIT,
        egui_pop::api::app::MENU_VIEW,
        egui_pop::api::app::MENU_HELP,
    ] {
        menu_titles(&h.state_mut().menu_items(menu.as_str()), &mut titles);
    }
    titles.sort();
    titles.dedup();
    titles
        .into_iter()
        .filter(|t| t.contains(['(', '`']) || t.chars().count() > MENU_TITLE_MAX)
        .collect()
}

#[test]
fn menu_entries_are_titles_not_notes_in_english_and_french() {
    let mut h = headless::harness(shell(), vec2(1600.0, 1000.0));
    h.run_steps(2);
    let english = unclear_titles(&mut h);
    let app = h.state().app().handle().clone();
    egui_pop::i18n::plugin::set_language(&app, "fr").unwrap();
    h.run_steps(2);
    let french = unclear_titles(&mut h);
    assert!(
        english.is_empty() && french.is_empty(),
        "menu entries with brackets, backticks or over {MENU_TITLE_MAX} characters:\n  \
         English: {english:#?}\n  French: {french:#?}"
    );
}
