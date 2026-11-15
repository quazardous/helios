//! Notifications: one call for every notice a screen raises, so the
//! record of it (the log, the alerts) can be added in one place.

use egui_pop::api::AppHandle;
use egui_pop::api::toasts::{TOASTS, Toast};

/// A notice's level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Notice {
    Info,
    Success,
    Warning,
    Error,
}

impl Notice {
    pub const ALL: [Notice; 4] = [
        Notice::Info,
        Notice::Success,
        Notice::Warning,
        Notice::Error,
    ];

    /// The toast egui-pop shows for it.
    pub fn toast(self, text: &str) -> Toast {
        match self {
            Notice::Info => Toast::info(text),
            Notice::Success => Toast::success(text),
            Notice::Warning => Toast::warning(text),
            Notice::Error => Toast::error(text),
        }
    }
}

/// Raises `text` at `level`: a toast of egui-pop's toasts service. Its
/// id, or `None` when the application has no toasts.
pub fn notify(app: &AppHandle, level: Notice, text: &str) -> Option<u64> {
    app.service(&TOASTS).map(|t| t.notify(level.toast(text)))
}
