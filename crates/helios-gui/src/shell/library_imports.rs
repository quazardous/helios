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
//! - **A set of photos** by [`SHOW_COMMAND`] (`ids`): the Library on
//!   exactly those, all selected (a job's photos: the Activity focus's
//!   "Show in Library").
//! - **Run Workshop…** ([`RUN_WORKSHOP_COMMAND`]): `workshop.run` on the
//!   Library's selection, whatever it comes from.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::PoisonError;
use std::time::{Duration, Instant};

use egui_pop::api::app::AppHandle;
use egui_pop::api::toasts::Toast;
use egui_pop::model::ids::CommandId;
use helios_core::Message;
use helios_library::import_history::Record;
use helios_shell::commands::Origin;
use serde_json::{Value, json};

use super::Shell;
use crate::library::{IMPORT_SESSION_COMMAND, ImportSession, RUN_WORKSHOP_COMMAND, SHOW_COMMAND};

/// The setting: after an import the user started, the Library shows its
/// session with its photos selected (on by default).
pub const SHOW_IMPORT_SETTING: &str = "import.show_in_library";

/// How often the Library's import sessions are read again.
const READ_EVERY: Duration = Duration::from_secs(10);

/// The commands, with their titles (category Library).
pub(super) const COMMANDS: &[(&str, &str)] = &[
    (
        IMPORT_SESSION_COMMAND,
        "Show an import session in the Library (`id`: an import of `import.history.list`; null: the whole library), its photos all selected with `select: true`; it works with the other filters, nothing is copied or moved. Without `id`, says the session shown",
    ),
    (
        SHOW_COMMAND,
        "Show exactly these photos in the Library (`ids`: image ids, e.g. a job's), the filter cleared and all of them selected; the next filter set shows by it again. Without `ids`, says how many the set shown holds",
    ),
];

/// The command running a workshop on the selection, with its title
/// (refused with the switch's key while the workshops are off).
pub(super) const RUN_WORKSHOP: (&str, &str) = (RUN_WORKSHOP_COMMAND, "Run Workshop on Selection…");

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
            // Photos, as the Library counts them: a RAW+JPEG pair is one.
            count: r.photos(),
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
            SHOW_COMMAND => self.show_photos(args),
            RUN_WORKSHOP_COMMAND => {
                // Whatever the selection comes from.
                self.workshop_command("workshop.run", &json!({ "set": "selection" }), origin)
            }
            other => Err(format!("no library command `{other}`")),
        }
    }

    /// `library.show`: the Library on exactly the photos `ids`, all
    /// selected, shown where the Library is; answers how many it shows.
    fn show_photos(&mut self, args: &Value) -> Result<Value, String> {
        if let Some(ids) = args.get("ids") {
            let ids: Vec<String> = ids
                .as_array()
                .ok_or_else(|| format!("`ids`: {ids} is not a list of image ids"))?
                .iter()
                .map(|id| {
                    id.as_str()
                        .map(String::from)
                        .ok_or_else(|| format!("`ids`: {id} is not an image id"))
                })
                .collect::<Result<_, _>>()?;
            {
                let mut lib = self.focus.library.borrow_mut();
                lib.show_ids(&ids);
            }
            let _ = crate::contexts::go(
                self.core.app.handle(),
                crate::contexts::LIBRARY,
                Value::Null,
                &egui_pop::model::commands::Origin::User,
            );
        }
        let lib = self.focus.library.borrow();
        Ok(json!({ "set": lib.shown_set(), "shown": lib.shown().len() }))
    }

    /// `library.importSession`: the Library scoped to the import session
    /// `id` (null: none), shown, its photos selected with `select`;
    /// answers the session shown.
    fn show_import_session(&mut self, args: &Value) -> Result<Value, String> {
        show_import_session(&self.focus.library, self.core.app.handle(), args)
    }

    /// Whether the Library shows the session of an import the user
    /// started, once it ends ([`SHOW_IMPORT_SETTING`], on by default).
    pub fn shows_imports_in_library(&self) -> bool {
        shows_imports_in_library(self.core.app.handle())
    }

    /// Each frame: the Library's import sessions read again when they are
    /// older than [`READ_EVERY`] (or an import ended).
    pub(super) fn follow_import_sessions(&mut self) {
        if self
            .focus
            .imports
            .borrow()
            .sessions
            .read_at
            .is_some_and(|at| at.elapsed() < READ_EVERY)
        {
            return;
        }
        // Nothing to read without a library.
        let Ok(reader) = self.history_reader() else {
            return;
        };
        self.focus.imports.borrow_mut().sessions.read_at = Some(Instant::now());
        // A history that cannot be read lists nothing.
        let sessions = reader().map(|r| sessions_of(&r)).unwrap_or_default();
        self.focus.library.borrow_mut().set_imports(sessions);
    }

    /// `library.import-session`, the shell's listener of the import's
    /// points: an import ended, the sessions are read again; one the
    /// user started is shown in the Library, or offered while the user
    /// edits a photo. Whether its photos are to be selected goes to
    /// `select` (read by `library.select-imported`).
    pub(super) fn listen_import_session(&self) -> Result<(), String> {
        let ended = SessionEnded {
            imports: self.focus.imports.clone(),
            library: self.focus.library.clone(),
            teller: self.teller(),
        };
        crate::import_end::listen_local(
            &*self.core.app.handle().bus(),
            crate::import_end::SESSION,
            move |end, r| r.select = ended.heard(&end.request, &end.line()),
        )
    }

    /// An import the user started here was handed to the node: its
    /// request, as the end line names it.
    pub(super) fn user_import_started(&mut self, request: Value) {
        self.focus.imports.borrow_mut().user_imports.push(request);
    }
}

/// `library.importSession` on `library`: the Library scoped to the
/// import session `id` (null: none), shown, its photos selected with
/// `select`; answers the session shown.
fn show_import_session(
    library: &RefCell<crate::library::Library>,
    app: &AppHandle,
    args: &Value,
) -> Result<Value, String> {
    if let Some(id) = args.get("id") {
        let id = match id {
            Value::Null => None,
            Value::String(id) if !id.is_empty() => Some(id.clone()),
            other => return Err(format!("`id`: {other} is not an import of the history")),
        };
        let select = args["select"].as_bool().unwrap_or(false);
        {
            let mut lib = library.borrow_mut();
            lib.show_import(id.as_deref(), select);
            lib.record_filter();
        }
        if id.is_some() {
            // Shown where the Library is (a window without the place
            // keeps its own).
            let _ = crate::contexts::go(
                app,
                crate::contexts::LIBRARY,
                Value::Null,
                &egui_pop::model::commands::Origin::User,
            );
        }
    }
    Ok(json!({ "id": library.borrow().import_session() }))
}

/// Whether the Library shows the session of an import the user started,
/// once it ends ([`SHOW_IMPORT_SETTING`], on by default).
fn shows_imports_in_library(app: &AppHandle) -> bool {
    let store = app.store();
    let store = store.lock().unwrap_or_else(PoisonError::into_inner);
    store
        .get(&format!("settings/{SHOW_IMPORT_SETTING}"))
        .and_then(|e| e.value.as_bool())
        .unwrap_or(true)
}

/// What `library.import-session` holds of the shell.
struct SessionEnded {
    imports: Rc<RefCell<super::import::Imports>>,
    library: Rc<RefCell<crate::library::Library>>,
    teller: super::said::Teller,
}

impl SessionEnded {
    /// An import ended (its request, as asked, and its end line): the
    /// sessions are read again; one the user started is shown in the
    /// Library, or offered while the user edits a photo. Whether its
    /// photos are to be selected.
    fn heard(&self, request: &Value, line: &Value) -> bool {
        let started_here = {
            let mut imports = self.imports.borrow_mut();
            imports.sessions.read_at = None;
            let started = &mut imports.user_imports;
            match started.iter().position(|r| r == request) {
                Some(at) => {
                    started.remove(at);
                    true
                }
                None => false,
            }
        };
        let app = &self.teller.app;
        let by_user = started_here || request["origin"] == crate::persist::WHO;
        if !by_user || !shows_imports_in_library(app) || line.get("ended").is_none() {
            return false;
        }
        let Some(history) = line["ended"]["history"].as_str().map(String::from) else {
            // No session to show: its photos are selected all the same.
            return true;
        };
        if crate::contexts::darkroom(app).is_some() {
            self.offer(&history);
            return false;
        }
        if let Err(e) = show_import_session(
            &self.library,
            app,
            &json!({ "id": history, "select": true }),
        ) {
            self.teller.notify(
                crate::dialogs::Level::Warning,
                Message::from(format!("{IMPORT_SESSION_COMMAND}: {e}")),
            );
        }
        true
    }

    /// While the user edits a photo: a notice offering the import's
    /// session ("Show in Library").
    fn offer(&self, history: &str) {
        let message = Message::new(
            "gui-import-session-ended",
            "The import ended: its photos are in the Library",
        );
        let label = self
            .teller
            .text(Message::new("gui-import-session-show", "Show in Library"));
        let text = self.teller.text(message);
        if let Ok(id) = CommandId::new(IMPORT_SESSION_COMMAND) {
            self.teller.toast(Toast::info(&text).action(
                &label,
                id,
                json!({ "id": history, "select": true }),
            ));
        }
        self.teller.note(text);
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

    /// A card of two RAW+JPEG pairs, a lone raw and a lone JPEG, its files
    /// handed one by one: one session of four photos named by the card,
    /// whether the listing kept its photos or left them out.
    #[test]
    fn a_session_counts_a_pair_once_and_is_named_by_the_card() {
        let dcim = std::path::Path::new("/tmp/journey/card/DCIM/100HELIO");
        let mut r = Record {
            id: "a".into(),
            when: 10,
            sources: [
                "IMG_0001.JPG",
                "IMG_0001.NEF",
                "IMG_0002.JPG",
                "IMG_0002.PEF",
                "IMG_0003.RW2",
                "IMG_0004.JPG",
            ]
            .iter()
            .map(|n| dcim.join(n))
            .collect(),
            counts: helios_library::import_history::Counts {
                files: 6,
                added: 6,
                paired: 2,
                ..Default::default()
            },
            ..Record::default()
        };
        let sessions = sessions_of(std::slice::from_ref(&r));
        assert_eq!(
            (sessions[0].count, sessions[0].source.as_str()),
            (4, "card")
        );
        assert!(
            sessions[0].line().ends_with(" · card · 4"),
            "{}",
            sessions[0].line()
        );
        r.images = vec!["1".into(), "2".into(), "3".into(), "4".into()];
        assert_eq!(sessions_of(&[r])[0].count, 4);
    }
}
