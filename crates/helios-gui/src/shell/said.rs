//! What the shell's local listeners of the bus say, without the shell at
//! hand.
//!
//! A local listener (`listen_local`, run on the interface's thread inside
//! the emit) holds `Rc` handles, not `&mut Shell`. A [`Teller`] gives it
//! the shell's words (its language), its toasts, and the record of what
//! was said: the notes and notices it leaves in [`Said`], which the shell
//! takes into its own ([`super::Shell::absorb_said`]) once the emit
//! returns. What the person sees (the toast) is shown by the listener
//! itself.

use std::cell::RefCell;
use std::rc::Rc;

use egui_pop::api::app::AppHandle;
use egui_pop::api::toasts::{TOASTS, Toast};
use egui_pop::model::ids::CommandId;
use helios_core::Message;
use serde_json::json;

use crate::dialogs::{Level, Notice};

/// The notes and notices local listeners left, until the shell takes
/// them in.
#[derive(Default)]
pub(super) struct Said {
    pub(super) notes: Vec<String>,
    /// Their `at` set as the shell takes them in.
    pub(super) notices: Vec<Notice>,
}

/// The shell's words, toasts and record, for a local listener.
#[derive(Clone)]
pub(super) struct Teller {
    pub(super) i18n: egui_pop::i18n::I18n,
    pub(super) app: AppHandle,
    pub(super) said: Rc<RefCell<Said>>,
}

impl Teller {
    /// A message in the shell's language, else its English.
    pub(super) fn text(&self, m: impl Into<Message>) -> String {
        helios_i18n::message(&self.i18n, &m.into())
    }

    /// Shows `toast` where the shell's toasts go.
    pub(super) fn toast(&self, toast: Toast) {
        if let Some(toasts) = self.app.service(&TOASTS) {
            toasts.notify(toast);
        }
    }

    /// `text` kept in the shell's notes.
    pub(super) fn note(&self, text: String) {
        self.said.borrow_mut().notes.push(text);
    }

    /// `notice` kept (and its text in the notes).
    pub(super) fn notice(&self, notice: Notice) {
        let mut said = self.said.borrow_mut();
        said.notes.push(notice.text.clone());
        said.notices.push(notice);
    }

    /// As [`super::Shell::notify_unlogged`]: a notice of `level`, a toast
    /// unless it is plain information; a failure's toast offers its
    /// details (the last failure's).
    pub(super) fn notify_unlogged(&self, level: Level, message: Message) {
        let report = (level == Level::Error)
            .then(|| crate::report::Context::default().cause(message.english.clone()));
        let text = self.text(message.clone());
        let details = report
            .as_ref()
            .map(|_| self.text(Message::new("gui-notice-details", "Details")));
        let toast = match level {
            Level::Info => None,
            Level::Warning => Some(Toast::warning(&text)),
            Level::Error => Some(Toast::error(&text)),
        };
        if let Some(toast) = toast {
            self.toast(match &details {
                Some(label) => toast.action(
                    label,
                    CommandId::from_static(super::NOTICE_DETAILS),
                    json!({}),
                ),
                None => toast,
            });
        }
        self.notice(Notice {
            level,
            text,
            key: message.key,
            action: details.map(|label| (label, super::NOTICE_DETAILS.to_string())),
            at: 0.0,
            report,
        });
    }

    /// As [`super::Shell::notify`]: written to the log file too.
    pub(super) fn notify(&self, level: Level, message: Message) {
        crate::log_panel::write(level, &message);
        self.notify_unlogged(level, message);
    }
}
