//! A dialog's Copy button gives a bug report: the dialog's text, then its
//! details (Helios's version, the system, the command that failed, the
//! error), never a secret nor the home folder's path.
//!
//! The environment is part of what is checked (a client id set there, the
//! home folder), so the failing command's check runs in a child process
//! of this test binary (no `std::env::set_var` in a test process).

use std::process::Command;

use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use helios_gui::dialogs::Confirm;
use helios_gui::{Shell, headless, helios_panels};
use helios_shell::commands::{CommandSpec, Origin};
use serde_json::json;

const CHILD: &str = "a_failing_command_in_an_environment_holding_a_secret";
const CLIENT_ID: &str = "HELIOS_TESTING_PHOTOS_CLIENT_ID";
const SECRET: &str = "4242-client-abcdef.apps.example";
const HOME: &str = "/tmp/helios-dialog-details-home";

fn harness(shell: Shell) -> Harness<'static, Shell> {
    let mut h = headless::harness(shell, vec2(1280.0, 800.0));
    h.run();
    h
}

/// Clicks the dialog's Copy button; returns what it put on the clipboard.
fn copy(h: &mut Harness<'static, Shell>) -> String {
    h.get_by_role_and_label(Role::Button, "Copy")
        .click_accesskit();
    h.step();
    let copied = h
        .output()
        .platform_output
        .commands
        .iter()
        .find_map(|c| match c {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        });
    h.run();
    copied.expect("the Copy button copies")
}

#[test]
fn a_confirmation_copies_its_text_and_hidden_details() {
    let mut shell = Shell::new(helios_panels(), None);
    shell.confirm(Confirm {
        title: "Move the log".into(),
        detail: "The log goes to the editors.".into(),
        verb: "Move the log".into(),
        danger: false,
        command: "helios.log.moveTo".into(),
        args: json!({ "area": "center" }),
    });
    let mut h = harness(shell);
    let copied = copy(&mut h);
    let (text, details) = copied.split_once("\n\n---\n").expect("details follow");
    assert_eq!(text, "Move the log\n\nThe log goes to the editors.");
    assert!(
        details.contains(&format!("helios: {}", env!("HELIOS_VERSION"))),
        "{details}"
    );
    assert!(details.contains("command: helios.log.moveTo"), "{details}");
    // Hidden: nowhere on screen.
    assert!(h.query_by_label("Details").is_none());
    assert!(h.query_by_label_contains("helios.log.moveTo").is_none());
}

#[test]
fn a_failing_command_opens_an_error_whose_copy_is_a_bug_report() {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            CHILD,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CLIENT_ID, SECRET)
        .env("HOME", HOME)
        .output()
        .expect("the test binary runs again");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{log}");
    // The child did run (an unknown name would pass with 0 tests).
    assert!(log.contains("1 passed"), "{log}");
}

/// Run by the test above, with a client id and a home folder in its
/// environment.
#[test]
#[ignore = "run by a_failing_command_opens_an_error_whose_copy_is_a_bug_report"]
fn a_failing_command_in_an_environment_holding_a_secret() {
    assert_eq!(std::env::var(CLIENT_ID).as_deref(), Ok(SECRET));
    let mut shell = Shell::new(helios_panels(), None);
    shell
        .register(
            CommandSpec::new("testing.fail", "Fail", "Testing"),
            |_, _| {
                Err(format!(
                    "{HOME}/Pictures/a.raw: refused for client {SECRET}"
                ))
            },
        )
        .unwrap();
    let mut h = harness(shell);
    assert!(
        h.state_mut()
            .run("testing.fail", json!(null), Origin::User)
            .is_err()
    );
    h.run();
    // The toast offers the details: an error dialog.
    h.get_by_role_and_label(Role::Button, "Details")
        .click_accesskit();
    for _ in 0..3 {
        h.run();
    }
    assert!(h.state().target_rect("helios.dialog").is_ok());
    let copied = copy(&mut h);
    assert!(copied.starts_with("Something went wrong\n\n"), "{copied}");
    let (_, details) = copied.split_once("\n\n---\n").expect("details follow");
    assert!(
        details.contains(&format!("helios: {}", env!("HELIOS_VERSION"))),
        "{details}"
    );
    assert!(details.contains("os: "), "{details}");
    assert!(details.contains("command: testing.fail"), "{details}");
    assert!(details.contains("error: ~/Pictures/a.raw"), "{details}");
    assert!(!copied.contains(SECRET), "{copied}");
    assert!(!details.contains(HOME), "{details}");
}
