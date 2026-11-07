//! The health check on the desktop: a check that finds a problem shows
//! the status bar's badge; the badge opens the Health page listing it;
//! "Ignore" hides it (under "Ignored (1)") and clears the badge; "Show
//! again" brings it back.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_pop::core::indicator::{self, Tone};
use helios_core::Message;
use helios_gui::shell::HEALTH_BADGE;
use helios_gui::{Shell, headless, helios_panels};
use helios_health::runner::{Runner, Schedule};
use helios_health::{Check, Context, Finding, Findings, Level, Registry};

/// A check that always finds the same problem.
struct Broken;

impl Check for Broken {
    fn id(&self) -> &'static str {
        "test.broken"
    }

    fn title(&self) -> Message {
        Message::plain("Broken")
    }

    fn run(&self, _: &Context) -> Vec<Finding> {
        vec![Finding::new(
            "test.broken",
            "",
            Level::Problem,
            Message::plain("The flux capacitor is unplugged"),
            Message::plain("Plug it back in"),
        )]
    }
}

fn shell() -> Shell {
    let mut registry = Registry::default();
    registry.add(Arc::new(Broken));
    let mut shell = Shell::new(helios_panels(), None);
    shell.set_health(Runner::new(
        registry,
        Context::default,
        Findings::default(),
        // At once, then not again during the test.
        Schedule::new(Instant::now(), Duration::ZERO, Duration::from_hours(1)),
    ));
    shell
}

fn badge(h: &Harness<'_, Shell>) -> Option<indicator::Record> {
    indicator::find(&h.ctx, HEALTH_BADGE).filter(|r| r.shown)
}

/// Steps frames until `done` holds, for ten seconds at most.
fn until(h: &mut Harness<'_, Shell>, what: &str, done: impl Fn(&Harness<'_, Shell>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("never got there: {what}");
}

#[test]
fn a_problem_shows_the_badge_the_page_lists_it_and_ignore_clears_it() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    until(&mut h, "the badge", |h| badge(h).is_some());
    let b = badge(&h).unwrap();
    assert!(
        b.name
            .as_deref()
            .is_some_and(|n| n.contains("Health check:"))
    );
    assert_eq!(b.value, Some(1.0));
    assert_eq!(b.tone, Tone::Danger, "a problem");
    assert!(!h.state().health_page_open(), "never a modal");

    // The badge opens the page, which lists the problem.
    let [x, y, w, hh] = b.rect;
    let centre = egui::pos2(x + w / 2.0, y + hh / 2.0);
    h.hover_at(centre);
    h.step();
    h.drag_at(centre);
    h.step();
    h.drop_at(centre);
    until(&mut h, "the page", |h| h.state().health_page_open());
    until(&mut h, "the finding", |h| {
        h.query_by_label("The flux capacitor is unplugged")
            .is_some()
    });
    assert!(h.query_by_label("Plug it back in").is_some());
    assert!(h.query_by_label_contains("Problems (1)").is_some());

    // Ignore: gone from the list, and the badge goes.
    h.get_by_label("Ignore").click();
    until(&mut h, "the badge gone", |h| badge(h).is_none());
    assert!(h.query_by_label_contains("Ignored (1)").is_some());
    assert!(h.query_by_label_contains("Problems (1)").is_none());

    // Undone: back, with its badge.
    h.get_by_label("Show again").click();
    until(&mut h, "the badge back", |h| badge(h).is_some());
}

#[test]
fn the_health_page_is_a_command_and_check_now_runs_again() {
    let mut h = headless::harness(shell(), vec2(1280.0, 800.0));
    until(&mut h, "a first run", |h| {
        h.state()
            .health()
            .is_some_and(|r| r.findings().ran() && !r.running())
    });
    h.state_mut()
        .run(
            helios_gui::shell::HEALTH_COMMAND,
            serde_json::Value::Null,
            helios_shell::commands::Origin::User,
        )
        .unwrap();
    until(&mut h, "the page", |h| {
        h.query_by_label("Check now").is_some()
    });
    h.state_mut()
        .run(
            helios_gui::shell::HEALTH_CHECK_NOW,
            serde_json::Value::Null,
            helios_shell::commands::Origin::User,
        )
        .unwrap();
    until(&mut h, "a second run", |h| {
        h.state().health().is_some_and(|r| r.runs() == 2)
    });
}
