//! What the window showed, in the log: when a window goes wrong (empty,
//! black, on the wrong page), the log says where it was and what was
//! asked just before. One info line for:
//!
//! - each transition of the context stack, and the stack restored at
//!   start (egui-pop's `context.*` events);
//! - the commands that change what holds the screen or the Library under
//!   it (`health.fix`, the pairing, the setup, the Settings app, the
//!   places), with their answer or their error;
//! - the Settings app and its wizard opening and closing, with the focus
//!   screen's flag.
//!
//! The lines come from egui-pop's journal, read once a frame: nothing is
//! logged per frame.

use egui_pop::api::admin::{self, ADMIN};
use egui_pop::api::app::AppHandle;
use serde_json::Value;

/// The longest answer written to the log.
const MAX: usize = 600;

/// The commands whose answer is logged: by id, or by prefix (ending with
/// a dot).
const COMMANDS: &[&str] = &[
    "health.fix",
    "library.files.pair",
    "library.files.unpair",
    "init.",
    "admin.open",
    "admin.close",
    "admin.wizard.open",
    "admin.wizard.cancel",
    "admin.wizard.finish",
    "view.",
    "workbench.arrangement.",
    "workbench.zen",
];

fn watched(command: &str) -> bool {
    COMMANDS.iter().any(|c| {
        if c.ends_with('.') {
            command.starts_with(c)
        } else {
            command == *c
        }
    })
}

fn short(data: &Value) -> String {
    let mut text = data.to_string();
    if text.len() > MAX {
        let mut end = MAX;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

/// The log line of an event of egui-pop's journal, if it has one, and
/// whether it is a failure (a warning).
pub(super) fn event_line(name: &str, data: &Value) -> Option<(bool, String)> {
    if name.starts_with("context.") {
        return Some((
            name == "context.refused",
            format!("{name}: {}", short(data)),
        ));
    }
    let command = data["command"].as_str()?;
    if !watched(command) {
        return None;
    }
    match name {
        "command.done" => Some((false, format!("{command}: {}", short(&data["result"])))),
        "command.failed" => Some((true, format!("{command} failed: {}", short(&data["error"])))),
        _ => None,
    }
}

/// What holds the screen besides the places: the Settings app, its
/// wizard, the focus screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Screen {
    settings_open: bool,
    wizard: Option<String>,
    focus_screen: bool,
}

impl Screen {
    fn of(app: &AppHandle) -> Screen {
        let a = app.service(&ADMIN);
        Screen {
            settings_open: a.as_ref().is_some_and(|a| a.is_open()),
            wizard: a
                .as_ref()
                .and_then(|a| a.wizard_flow())
                .map(|f| f.wizard.clone()),
            focus_screen: admin::focus_screen(app),
        }
    }

    /// Looks at the screen now; logs it when it changed.
    pub(super) fn follow(&mut self, app: &AppHandle) {
        let now = Screen::of(app);
        if now != *self {
            log::info!(
                target: "helios",
                "screen: the Settings app is {}, wizard: {}, focus screen: {} (in its own window: {})",
                if now.settings_open { "open" } else { "closed" },
                now.wizard.as_deref().unwrap_or("none"),
                now.focus_screen,
                admin::in_window(app),
            );
            *self = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_stack_and_the_watched_commands_have_a_line() {
        let (warn, line) = event_line(
            "context.changed",
            &json!({ "places": [{ "frame": "helios.library" }] }),
        )
        .unwrap();
        assert!(!warn);
        assert!(line.starts_with("context.changed: "), "{line}");
        assert!(line.contains("helios.library"));
        assert!(event_line("context.refused", &json!({})).unwrap().0);

        let done = json!({ "command": "health.fix", "result": { "id": "library.raw-jpeg-pairs", "command": "library.files.pair" } });
        let (warn, line) = event_line("command.done", &done).unwrap();
        assert!(!warn);
        assert!(line.starts_with("health.fix: "), "{line}");
        assert!(line.contains("library.raw-jpeg-pairs"));

        let failed = json!({ "command": "library.files.pair", "error": "no RAW+JPEG pair" });
        let (warn, line) = event_line("command.failed", &failed).unwrap();
        assert!(warn);
        assert_eq!(line, "library.files.pair failed: \"no RAW+JPEG pair\"");

        assert!(
            event_line(
                "command.done",
                &json!({ "command": "view.library", "result": null })
            )
            .is_some()
        );
        assert!(
            event_line(
                "command.done",
                &json!({ "command": "edit.copy", "result": null })
            )
            .is_none()
        );
        assert!(event_line("command.dispatched", &done).is_none());
        assert!(event_line("toast.shown", &json!({})).is_none());
    }

    #[test]
    fn a_long_answer_is_cut() {
        let long = json!({ "command": "view.library", "result": "é".repeat(1000) });
        let (_, line) = event_line("command.done", &long).unwrap();
        assert!(line.len() < MAX + 40, "{}", line.len());
        assert!(line.ends_with('…'));
    }
}
