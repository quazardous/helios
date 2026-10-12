//! The shell's windows, dialogs, menus and notices: one set (egui-pop's
//! components), drawn the same way, answered by a human or an agent alike.

use egui::{Key, vec2};
use egui_kittest::kittest::{NodeT, Queryable};
use helios_gui::dialogs::{
    Choice, Confirm, Field, Form, Level, MenuItem, Picker, Request, request,
};
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::{CommandSpec, Origin};
use helios_values::{Descriptor, PathKind};
use serde_json::json;

fn harness(shell: Shell) -> egui_kittest::Harness<'static, Shell> {
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

fn move_log(danger: bool) -> Confirm {
    Confirm {
        title: "Move the log".into(),
        detail: "The log goes to the editors.".into(),
        verb: "Move the log".into(),
        danger,
        command: "helios.log.moveTo".into(),
        args: json!({ "area": "editors" }),
    }
}

fn log_area(shell: &Shell) -> String {
    let layout = shell.layout();
    let (area, _) = layout
        .areas
        .iter()
        .find(|(_, n)| serde_json::to_string(n).unwrap().contains("helios.log"))
        .unwrap();
    format!("{area:?}")
}

#[test]
fn a_destructive_confirmation_starts_on_cancel_and_enter_keeps_it_safe() {
    let mut shell = Shell::new(helios_panels(), None);
    shell.confirm(move_log(true));
    let mut h = harness(shell);
    assert!(h.state().target_rect("helios.dialog").is_ok());
    let current = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::Agent("a1".into()))
        .unwrap();
    assert_eq!(current["kind"], json!("confirm"));
    assert_eq!(current["verb"], json!("Move the log"));
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(
        log_area(h.state()),
        "BottomPanel",
        "Enter on a danger dialog cancels"
    );
    assert!(h.state().target_rect("helios.dialog").is_err(), "closed");
}

#[test]
fn an_agent_answers_a_dialog_by_command_with_its_origin() {
    let mut shell = Shell::new(helios_panels(), None);
    shell.confirm(move_log(false));
    let mut h = harness(shell);
    h.state_mut()
        .run(
            "dialog.answer",
            json!({ "answer": "primary" }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    h.run();
    assert_eq!(log_area(h.state()), "Editors");
    let run = h
        .state()
        .history()
        .iter()
        .find(|r| r.id == "helios.log.moveTo")
        .unwrap()
        .clone();
    assert_eq!(run.origin, Origin::Agent("a1".into()));
    assert_eq!(
        h.state_mut()
            .run("dialog.current", json!(null), Origin::User)
            .unwrap(),
        json!(null)
    );
}

#[test]
fn enter_runs_the_primary_action_of_a_safe_confirmation() {
    let mut shell = Shell::new(helios_panels(), None);
    shell.confirm(move_log(false));
    let mut h = harness(shell);
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(log_area(h.state()), "Editors");
}

#[test]
fn a_form_is_built_from_descriptors_and_answered_with_values() {
    let mut shell = Shell::new(helios_panels(), None);
    shell.form(Form {
        title: "Move the log".into(),
        fields: vec![Field {
            key: "area".into(),
            descriptor: Descriptor::choice(
                "area",
                &["editors", "primary_side_bar", "bottom_panel"],
                "bottom_panel",
            ),
            value: helios_core::Value::Choice("bottom_panel".into()),
        }],
        verb: "Move".into(),
        command: "helios.log.moveTo".into(),
    });
    let mut h = harness(shell);
    let current = h
        .state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap();
    assert_eq!(current["values"], json!({ "area": "bottom_panel" }));
    // A wrong value is refused and the form stays.
    h.state_mut()
        .run(
            "dialog.answer",
            json!({ "values": { "area": 3 } }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    h.run();
    assert!(h.state().target_rect("helios.dialog").is_ok());
    h.state_mut()
        .run(
            "dialog.answer",
            json!({ "values": { "area": "primary_side_bar" } }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    h.run();
    assert_eq!(log_area(h.state()), "PrimarySideBar");
}

#[test]
fn a_picker_runs_its_command_with_the_value_chosen() {
    let mut shell = Shell::new(helios_panels(), None);
    shell.pick(Picker {
        title: "Move the log to".into(),
        choices: ["editors", "primary_side_bar", "secondary_side_bar"]
            .iter()
            .map(|a| Choice {
                value: a.to_string(),
                label: a.replace('_', " "),
                detail: None,
            })
            .collect(),
        command: "helios.log.moveTo".into(),
        arg: "area".into(),
    });
    let mut h = harness(shell);
    // egui-pop's choice has no search yet: one clicks a label.
    h.get_by_label("secondary side bar").click();
    h.run();
    assert_eq!(log_area(h.state()), "SecondarySideBar");

    // An agent chooses by value.
    let mut shell = Shell::new(helios_panels(), None);
    shell.pick(Picker {
        title: "Move the log to".into(),
        choices: ["editors", "primary_side_bar"]
            .iter()
            .map(|a| Choice {
                value: a.to_string(),
                label: a.replace('_', " "),
                detail: None,
            })
            .collect(),
        command: "helios.log.moveTo".into(),
        arg: "area".into(),
    });
    let mut h = harness(shell);
    h.state_mut()
        .run(
            "dialog.answer",
            json!({ "choice": "primary_side_bar" }),
            Origin::Agent("a1".into()),
        )
        .unwrap();
    h.run();
    assert_eq!(log_area(h.state()), "PrimarySideBar");
}

#[test]
fn a_menu_lists_commands_with_their_keys_and_greys_what_does_not_apply() {
    let mut shell = Shell::new(helios_panels(), None);
    shell
        .register(
            CommandSpec::new("test.palette_only", "Only with the palette", "Test")
                .when("paletteVisible"),
            |_, _| Ok(json!(null)),
        )
        .unwrap();
    shell.open_menu(
        egui::pos2(200.0, 200.0),
        vec![
            MenuItem::command("test.palette_only"),
            MenuItem::Separator,
            MenuItem::command("workbench.fullscreen"),
            MenuItem::Command {
                id: "helios.log.moveTo".into(),
                args: json!({ "area": "editors" }),
            },
        ],
    );
    let mut h = harness(shell);
    assert!(h.state().target_rect("helios.menu").is_ok());
    // The palette-only entry is greyed; the others show their keys.
    assert!(
        h.get_by_label("Only with the palette")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        !h.get_by_label("Move Log to an area")
            .accesskit_node()
            .is_disabled()
    );
    h.get_by_label("Move Log to an area").click();
    h.run();
    assert_eq!(log_area(h.state()), "Editors");
    assert!(h.state().target_rect("helios.menu").is_err(), "closed");
}

#[test]
fn warnings_become_toasts_information_stays_in_the_status_bar() {
    let mut shell = Shell::new(helios_panels(), None);
    shell.notify(Level::Info, "saved");
    let mut h = harness(shell);
    assert!(h.state().target_rect("helios.toasts").is_err());
    h.state_mut()
        .notify(Level::Warning, "the vault is read-only");
    h.run();
    assert!(h.state().target_rect("helios.toasts").is_ok());
    assert_eq!(h.state().notices().len(), 2);
}

#[test]
fn the_yes_policy_confirms_at_once_and_says_so() {
    let mut shell = Shell::new(helios_panels(), None);
    shell
        .run(
            "dialog.assumeYes",
            json!({ "on": true }),
            Origin::Script("cli".into()),
        )
        .unwrap();
    let mut h = harness(shell);
    h.state_mut().confirm(move_log(true));
    h.run();
    assert_eq!(log_area(h.state()), "Editors");
    assert_eq!(
        h.state().notes.last().map(String::as_str),
        Some("Move the log: confirmed by policy")
    );
}

#[test]
fn a_panel_asks_the_shell_and_never_draws_a_window_itself() {
    let mut shell = Shell::new(helios_panels(), None);
    let mut asked = false;
    shell.set_content("helios.params", move |ui| {
        if ui.button("Reset all").clicked() || !asked {
            asked = true;
            request(ui.ctx(), Request::Confirm(move_log(true)));
        }
    });
    let mut h = harness(shell);
    h.run();
    assert!(h.state().target_rect("helios.dialog").is_ok());
}

/// Pictures of each kind in each theme, for `docs/gui.md`; written to
/// `docs/gui/` when `HELIOS_DOC_PICTURES` is set.
#[test]
fn pictures_of_each_kind() {
    let out = std::env::var_os("HELIOS_DOC_PICTURES").map(std::path::PathBuf::from);
    for theme in ["helios-dark", "helios-light"] {
        for kind in ["confirm", "form", "menu", "system", "toast"] {
            let mut shell = Shell::new(helios_panels(), None);
            shell.set_theme(theme, std::collections::BTreeMap::default());
            let target = match kind {
                "system" => {
                    shell.draw_frame(true);
                    shell.set_title("photos");
                    shell
                        .run("workbench.systemMenu", json!(null), Origin::User)
                        .unwrap();
                    "helios.menu"
                }
                "confirm" => {
                    shell.confirm(Confirm {
                        title: "Delete 3 images?".into(),
                        detail: "Their files stay on disk; edits and ratings are lost.".into(),
                        verb: "Delete 3 images".into(),
                        danger: true,
                        command: "workbench.fullscreen".into(),
                        args: json!(null),
                    });
                    "helios.dialog"
                }
                "form" => {
                    shell.form(Form {
                        title: "Export".into(),
                        fields: vec![
                            Field {
                                key: "quality".into(),
                                descriptor: Descriptor::int("quality", 1, 100, 92),
                                value: helios_core::Value::Int(92),
                            },
                            Field {
                                key: "format".into(),
                                descriptor: Descriptor::choice(
                                    "format",
                                    &["jpeg", "tiff", "png"],
                                    "jpeg",
                                ),
                                value: helios_core::Value::Choice("jpeg".into()),
                            },
                            Field {
                                key: "metadata".into(),
                                descriptor: Descriptor::bool("keep metadata", true),
                                value: helios_core::Value::Bool(true),
                            },
                        ],
                        verb: "Export".into(),
                        command: "workbench.fullscreen".into(),
                    });
                    "helios.dialog"
                }
                "menu" => {
                    shell.open_menu(
                        egui::pos2(300.0, 200.0),
                        vec![
                            MenuItem::command("app.palette"),
                            MenuItem::command("app.fullscreen"),
                            MenuItem::Separator,
                            MenuItem::command("helios.log.toggle"),
                        ],
                    );
                    "helios.menu"
                }
                _ => {
                    shell.notify(
                        Level::Warning,
                        "The vault is read-only: changes are kept in memory.",
                    );
                    "helios.toasts"
                }
            };
            let mut h = headless::harness(shell, vec2(1280.0, 800.0));
            h.run();
            let picture = headless::snapshot(&mut h, target).unwrap();
            assert!(
                picture.width() > 100,
                "{kind} in {theme}: {}",
                picture.width()
            );
            if let Some(dir) = &out {
                let path = dir.join(format!("{kind}-{theme}.png"));
                helios_gui::snapshot::save(&picture, &path).unwrap();
            }
        }
    }
}

/// A tree of folders of its own for a test.
fn tree(name: &str, dirs: &[&str], files: &[&str]) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("helios-browse-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for d in dirs {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::create_dir_all(&root).unwrap();
    for f in files {
        std::fs::write(root.join(f), "").unwrap();
    }
    root
}

fn current(h: &mut egui_kittest::Harness<'static, Shell>) -> serde_json::Value {
    h.state_mut()
        .run("dialog.current", json!(null), Origin::User)
        .unwrap()
}

fn click(h: &mut egui_kittest::Harness<'static, Shell>, label: &str) {
    h.get_by_label(label).click();
    h.run();
}

fn path_form(key: &str, descriptor: Descriptor, value: &std::path::Path) -> Form {
    Form {
        title: "Choose".into(),
        fields: vec![Field {
            key: key.into(),
            descriptor,
            value: helios_core::Value::Text(value.display().to_string()),
        }],
        verb: "Use".into(),
        command: "workbench.fullscreen".into(),
    }
}

#[test]
fn browse_walks_folders_in_the_own_picker_and_fills_the_field() {
    let root = tree("dirs", &["a/inner", "b"], &["notes.txt"]);
    let mut shell = Shell::new(helios_panels(), None);
    shell.form(path_form(
        "dir",
        Descriptor::path("folder", PathKind::Dir, &[]),
        &root,
    ));
    let mut h = harness(shell);
    click(&mut h, "Browse…");
    // The folder's subfolders, no files when choosing a folder.
    assert!(h.query_by_label("a/").is_some() && h.query_by_label("b/").is_some());
    assert!(h.query_by_label("notes.txt").is_none());
    click(&mut h, "a/");
    assert!(h.query_by_label("inner/").is_some(), "in a");
    click(&mut h, "Up");
    click(&mut h, "b/");
    click(&mut h, "Choose this folder");
    let form = current(&mut h);
    assert_eq!(form["kind"], json!("form"));
    assert_eq!(
        form["values"]["dir"],
        json!(root.join("b").display().to_string())
    );
    assert!(
        h.query_by_label("Choose this folder").is_none(),
        "the picker closed"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_file_field_lists_accepted_files_and_closing_the_picker_keeps_the_form() {
    let root = tree("files", &[], &["photo.jpg", "photo.xmp"]);
    let mut shell = Shell::new(helios_panels(), None);
    shell.form(path_form(
        "sidecar",
        Descriptor::path("sidecar", PathKind::File, &["xmp"]),
        &root,
    ));
    let mut h = harness(shell);
    click(&mut h, "Browse…");
    assert!(h.query_by_label("photo.jpg").is_none(), "not accepted");
    click(&mut h, "Close the picker");
    let form = current(&mut h);
    assert_eq!(form["kind"], json!("form"), "the form stays");
    click(&mut h, "Browse…");
    click(&mut h, "photo.xmp");
    let form = current(&mut h);
    assert_eq!(
        form["values"]["sidecar"],
        json!(root.join("photo.xmp").display().to_string())
    );
    let _ = std::fs::remove_dir_all(&root);
}
