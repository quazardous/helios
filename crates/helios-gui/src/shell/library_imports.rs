//! The Library's import sessions (`docs/design/library.md`, the Imports
//! section): every import of the import history
//! (`helios_library::import_history`) is a virtual collection of the
//! photos it brought in, nothing copied or moved.
//!
//! - **Listed** in the Library's Imports section, newest first, read
//!   through the node (`import.history.list`, the history reader the
//!   import window uses), again every [`READ_EVERY`] and at once when an
//!   import ends.
//! - **Shown** by [`IMPORT_SESSION_COMMAND`] (`id`; `select`): the
//!   Library scoped to the session's `import:` term, with the other
//!   filters; the History entries of the import window offer it too
//!   ("Show in Library").
//! - **After an import** the user started, with [`SHOW_IMPORT_SETTING`]
//!   on (the default), the Library shows its session, its photos
//!   selected; never for automatic imports, nor while the user edits a
//!   photo, when the import's notice offers it instead.
//! - **Run Workshop…** ([`RUN_WORKSHOP_COMMAND`]): `workshop.run` on the
//!   Library's selection, whatever it comes from.

use std::sync::PoisonError;
use std::time::{Duration, Instant};

use egui_pop::api::toasts::{TOASTS, Toast};
use egui_pop::model::ids::CommandId;
use helios_core::Message;
use helios_library::import_history::Record;
use helios_shell::commands::Origin;
use serde_json::{Value, json};

use super::Shell;
use crate::library::{IMPORT_SESSION_COMMAND, ImportSession, RUN_WORKSHOP_COMMAND};

/// The setting: after an import the user started, the Library shows its
/// session with its photos selected (on by default).
pub const SHOW_IMPORT_SETTING: &str = "import.show_in_library";

/// How often the Library's import sessions are read again.
const READ_EVERY: Duration = Duration::from_secs(10);

/// The commands, with their titles (category Library).
pub(super) const COMMANDS: &[(&str, &str)] = &[(
    IMPORT_SESSION_COMMAND,
    "Show an import session in the Library (`id`: an import of `import.history.list`; null: the whole library), its photos all selected with `select: true`; it works with the other filters, nothing is copied or moved. Without `id`, says the session shown",
)];

/// The command running a workshop on the selection, with its title
/// (refused with the switch's key while the workshops are off).
pub(super) const RUN_WORKSHOP: (&str, &str) = (
    RUN_WORKSHOP_COMMAND,
    "Run Workshop… on the Library's selection (the workshop applied, on the photos selected, whatever the selection comes from: an import session, an album, a filter)",
);

/// What the shell keeps of the sessions.
#[derive(Default)]
pub(super) struct Sessions {
    /// When the sessions were last read; none: at once.
    read_at: Option<Instant>,
}

/// The sessions of the history's records, newest first: those that
/// brought photos in or could not start are left out.
pub fn sessions_of(records: &[Record]) -> Vec<ImportSession> {
    records
        .iter()
        .filter(|r| r.error.is_none())
        .map(|r| ImportSession {
            id: r.id.clone(),
            when: r.when,
            source: r.source_label(),
            count: if r.images.is_empty() {
                r.counts.added + r.counts.located
            } else {
                r.images.len() as u64
            },
        })
        .collect()
}

impl Shell {
    /// Runs one of [`COMMANDS`] or [`RUN_WORKSHOP`].
    pub(super) fn library_imports_command(
        &mut self,
        id: &str,
        args: &Value,
        origin: &Origin,
    ) -> Result<Value, String> {
        match id {
            IMPORT_SESSION_COMMAND => self.show_import_session(args),
            RUN_WORKSHOP_COMMAND => {
                // Whatever the selection comes from.
                self.workshop_command("workshop.run", &json!({ "set": "selection" }), origin)
            }
            other => Err(format!("no library command `{other}`")),
        }
    }

    /// `library.importSession`: the Library scoped to the import session
    /// `id` (null: none), shown, its photos selected with `select`;
    /// answers the session shown.
    fn show_import_session(&mut self, args: &Value) -> Result<Value, String> {
        if let Some(id) = args.get("id") {
            let id = match id {
                Value::Null => None,
                Value::String(id) if !id.is_empty() => Some(id.clone()),
                other => return Err(format!("`id`: {other} is not an import of the history")),
            };
            let select = args["select"].as_bool().unwrap_or(false);
            {
                let mut lib = self.focus.library.borrow_mut();
                lib.show_import(id.as_deref(), select);
                lib.record_filter();
            }
            if id.is_some() {
                // Shown where the Library is (a window without the place
                // keeps its own).
                let _ = crate::contexts::go(
                    self.core.app.handle(),
                    crate::contexts::LIBRARY,
                    Value::Null,
                    &egui_pop::model::commands::Origin::User,
                );
            }
        }
        Ok(json!({ "id": self.focus.library.borrow().import_session() }))
    }

    /// Whether the Library shows the session of an import the user
    /// started, once it ends ([`SHOW_IMPORT_SETTING`], on by default).
    pub fn shows_imports_in_library(&self) -> bool {
        let store = self.core.app.handle().store();
        let store = store.lock().unwrap_or_else(PoisonError::into_inner);
        store
            .get(&format!("settings/{SHOW_IMPORT_SETTING}"))
            .and_then(|e| e.value.as_bool())
            .unwrap_or(true)
    }

    /// Each frame: the Library's import sessions read again when they are
    /// older than [`READ_EVERY`] (or an import ended).
    pub(super) fn follow_import_sessions(&mut self) {
        if self
            .focus
            .import_sessions
            .read_at
            .is_some_and(|at| at.elapsed() < READ_EVERY)
        {
            return;
        }
        // Nothing to read without a library.
        let Ok(reader) = self.history_reader() else {
            return;
        };
        self.focus.import_sessions.read_at = Some(Instant::now());
        // A history that cannot be read lists nothing.
        let sessions = reader().map(|r| sessions_of(&r)).unwrap_or_default();
        self.focus.library.borrow_mut().set_imports(sessions);
    }

    /// An import ended (its request, as asked, and its end line): the
    /// sessions are read again; one the user started is shown in the
    /// Library with its photos selected, or offered while the user edits
    /// a photo. Whether the Library changes its selection.
    pub(super) fn import_session_ended(&mut self, request: &Value, line: &Value) -> bool {
        self.focus.import_sessions.read_at = None;
        let started_here = {
            let started = &mut self.focus.user_imports;
            match started.iter().position(|r| r == request) {
                Some(at) => {
                    started.remove(at);
                    true
                }
                None => false,
            }
        };
        let by_user = started_here || request["origin"] == crate::persist::WHO;
        if !by_user || !self.shows_imports_in_library() || line.get("ended").is_none() {
            return false;
        }
        let Some(history) = line["ended"]["history"].as_str().map(String::from) else {
            // No session to show: its photos are selected all the same.
            return true;
        };
        if crate::contexts::darkroom(self.core.app.handle()).is_some() {
            self.offer_import_session(&history);
            return false;
        }
        if let Err(e) = self.show_import_session(&json!({ "id": history, "select": true })) {
            self.warn(format!("{IMPORT_SESSION_COMMAND}: {e}"));
        }
        true
    }

    /// While the user edits a photo: a notice offering the import's
    /// session ("Show in Library").
    fn offer_import_session(&mut self, history: &str) {
        let message = Message::new(
            "gui-import-session-ended",
            "The import ended: its photos are in the Library",
        );
        let label = self.text(Message::new("gui-import-session-show", "Show in Library"));
        let text = self.text(message);
        if let (Ok(id), Some(toasts)) = (
            CommandId::new(IMPORT_SESSION_COMMAND),
            self.core.app.handle().service(&TOASTS),
        ) {
            toasts.notify(Toast::info(&text).action(
                &label,
                id,
                json!({ "id": history, "select": true }),
            ));
        }
        self.notes.push(text);
    }

    /// An import the user started here was handed to the node: its
    /// request, as the end line names it.
    pub(super) fn user_import_started(&mut self, request: Value) {
        self.focus.user_imports.push(request);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_counts_its_photos_and_a_failed_import_is_none() {
        let ok = Record {
            id: "a".into(),
            when: 10,
            sources: vec!["/media/EOS_DIGITAL".into()],
            counts: helios_library::import_history::Counts {
                added: 3,
                located: 1,
                ..Default::default()
            },
            ..Record::default()
        };
        let failed = Record {
            id: "b".into(),
            error: Some("gone".into()),
            ..Record::default()
        };
        let sessions = sessions_of(&[ok, failed]);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].count, 4);
        assert_eq!(sessions[0].source, "EOS_DIGITAL");
        assert!(sessions[0].line().ends_with(" · EOS_DIGITAL · 4"));
    }
}
