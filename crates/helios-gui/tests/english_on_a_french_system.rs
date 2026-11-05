//! Tests read English labels on any machine: a shell built with the
//! user's settings, as many tests build it, speaks English even when the
//! system's language is French; only the desktop asks for the system's
//! (`ShellBuilder::system_language`).
//!
//! The system's language is read from the environment, so the check runs
//! in a child process of this test binary with a French environment
//! (no `std::env::set_var` in a test process).

use std::process::Command;

use egui::vec2;
use egui_kittest::kittest::Queryable;
use helios_gui::{Shell, headless, helios_panels};

const CHILD: &str = "in_a_french_environment";

#[test]
fn the_harness_speaks_english_on_a_french_system() {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            CHILD,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("LANG", "fr_FR.UTF-8")
        .env("LC_ALL", "fr_FR.UTF-8")
        .env("LC_MESSAGES", "fr_FR.UTF-8")
        .env("LANGUAGE", "fr")
        .env(helios_i18n::ENV, "fr")
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

/// Run by the test above, in a French environment.
#[test]
#[ignore = "run by the_harness_speaks_english_on_a_french_system"]
fn in_a_french_environment() {
    let dir = std::env::temp_dir().join(format!("helios-french-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // The environment does say French: the desktop's shell speaks it.
    let desktop = Shell::builder(helios_panels(), Some(dir.join("desktop")))
        .system_language()
        .build();
    let mut h = headless::harness(desktop, vec2(1280.0, 800.0));
    h.run();
    h.run();
    h.get_by_label("Affichage");
    drop(h);
    // A shell built as tests build it speaks English, settings or not.
    for settings in [Some(dir.join("tests")), None] {
        let shell = Shell::builder(helios_panels(), settings).build();
        let mut h = headless::harness(shell, vec2(1280.0, 800.0));
        h.run();
        h.run();
        h.get_by_label("View");
        h.get_by_label("Help");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
