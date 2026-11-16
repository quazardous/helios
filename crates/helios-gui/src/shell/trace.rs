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
//! logged per frame. The Settings app opening or closing with its own
//! window is the exception: it is logged by a listener on the bus's
//! `window.open` and `window.close` (egui-pop ADR 0029), in the call that
//! opens or closes the window, so the log never says the app is open
//! while its window is gone, nor closed while its window is up.

use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::admin::{self, ADMIN, Admin};
use egui_pop::api::app::AppHandle;
use egui_pop::api::bus::egui_pop_hooks::ListenOpts;
use egui_pop::api::bus::egui_pop_hooks::Point;
use egui_pop::api::bus::{WindowClose, WindowOpen, WindowRequest};
use serde_json::Value;

/// Helios's listener on `window.open` and `window.close`: the Settings
/// app's window opening or closing logs what holds the screen, right
/// after `admin.window` has opened or closed the app.
pub(super) const LISTENER: &str = "helios.screen";

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
struct Held {
    settings_open: bool,
    wizard: Option<String>,
    focused: bool,
}

impl Held {
    fn of(admin: Option<&Admin>) -> Held {
        Held {
            settings_open: admin.is_some_and(Admin::is_open),
            wizard: admin.and_then(Admin::wizard_flow).map(|f| f.wizard.clone()),
            focused: admin.is_some_and(Admin::focus_screen),
        }
    }

    /// Logs `now` when it differs from `self`, and keeps it; whether it
    /// did.
    fn change(&mut self, now: Held, in_window: bool) -> bool {
        if now == *self {
            return false;
        }
        log::info!(
            target: "helios",
            "screen: the Settings app is {}, wizard: {}, focus screen: {} (in its own window: {})",
            if now.settings_open { "open" } else { "closed" },
            now.wizard.as_deref().unwrap_or("none"),
            now.focused,
            in_window,
        );
        *self = now;
        true
    }
}

/// What holds the screen, logged when it changes: by [`LISTENER`] when
/// the Settings app's window opens or closes, in the same call as
/// egui-pop's `window.opened` and `window.closed`, and looked at once a
/// frame ([`Screen::follow`]) for what no window says (the app opening
/// or closing over the main window, its wizard, the focus screen).
#[derive(Clone, Debug, Default)]
pub(super) struct Screen {
    held: Arc<Mutex<Held>>,
    listening: bool,
}

impl Screen {
    /// Looks at the screen now; logs it when it changed. The first call
    /// puts [`LISTENER`] on the application's bus.
    pub(super) fn follow(&mut self, app: &AppHandle) {
        if !self.listening {
            self.listening = true;
            self.listen(app);
        }
        let admin = app.service(&ADMIN);
        let now = Held::of(admin.as_deref());
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.change(now, admin::in_window(app));
    }

    /// [`LISTENER`] on `window.open` and `window.close`, after
    /// `admin.window` (which opens or closes the Settings app with its
    /// window, whoever asked).
    fn listen(&self, app: &AppHandle) {
        let Some(admin) = app.service(&ADMIN) else {
            return;
        };
        let opened = self.listen_on::<WindowOpen>(app, admin.clone());
        let closed = self.listen_on::<WindowClose>(app, admin);
        for l in [opened, closed] {
            if let Err(e) = l {
                log::error!(target: "helios", "{e}");
            }
        }
    }

    fn listen_on<P>(&self, app: &AppHandle, admin: Arc<Admin>) -> Result<(), String>
    where
        P: Point<Input = WindowRequest, Value = Vec<String>>,
    {
        let held = self.held.clone();
        app.bus()
            .listen::<P>(
                LISTENER,
                ListenOpts::new()
                    .owner("helios")
                    .after(admin::WINDOW_LISTENER),
                move |req: &WindowRequest, did: &mut Vec<String>| {
                    if req.window != admin::APP_WINDOW {
                        return Ok(());
                    }
                    let now = Held::of(Some(&admin));
                    let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
                    if held.change(now, true) {
                        did.push(format!(
                            "{LISTENER}: the Settings app is {}",
                            if held.settings_open { "open" } else { "closed" }
                        ));
                    }
                    Ok(())
                },
            )
            .map_err(|e| e.to_string())
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
