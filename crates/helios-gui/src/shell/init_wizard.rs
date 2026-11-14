//! The first-run setup (`helios_init`): the steps modules declare, drawn
//! by the Settings app's wizard kit (egui-pop's `admin::wizard`), which
//! fills the Settings app's whole page while it runs: one step per page,
//! Back / Next / Finish, the stepper as the progress line, Skip ("Later")
//! on an optional step only. The wizard is declared from the init
//! registry ([`crate::config::init_wizard`]); each step's fields are the
//! settings it fills.
//!
//! This is a [`Host`] for [`helios_init::ensure`]: the steps say what
//! they ask, the host draws them, the outcome is read from the settings.
//!
//! - **First run**: once the library is known, when a required step is
//!   not done (the first launch, or a folder later cleared), every step
//!   is asked ([`Shell::set_first_run`] turns the check on).
//! - **"Set up Helios…"** (the Helios menu, [`COMMAND`]): every step
//!   again, done or not.
//! - **Partial runs**: before the import window opens, the steps import
//!   requires that are not done ([`Shell::ensure_for_import`]); the window
//!   opens when they are, and says why it cannot import when they were
//!   left.
//! - [`FOLDER_STEP`] (`init.libraryFolder`): the library folder step
//!   alone, the health warning's fix; with `path`, sets it at once.
//! - [`APPLY`] (`init.apply`): the wizard's finish, the draft's settings
//!   written through their keeper (the library folder made if missing,
//!   checked writable with room left).
//! - [`CHOOSE`] (`init.chooseFolder`): the folder step's "Choose…", the
//!   folder chooser parented to the window the wizard is in, its answer
//!   written into the draft.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::admin::ADMIN;
use egui_pop::api::files::{self, FILES, FileRequest};
use egui_pop::api::host::Pending;
use helios_core::Message;
use helios_init::{
    IMPORT, LIBRARY_FOLDER, LIBRARY_FOLDER_SETTING, Outcome, Plan, Registry, Reply, Request,
};
use serde_json::{Map, Value, json};

use super::Shell;

/// The wizard's id in the Settings app's manifest.
pub const WIZARD: &str = "init.setup";

/// "Set up Helios…": every step, done or not.
pub const COMMAND: &str = "helios.setUp";

/// The library folder step alone (`path`: set at once): the health
/// warning's fix (`helios_health::checks::CHOOSE_LIBRARY_FOLDER`).
pub const FOLDER_STEP: &str = "init.libraryFolder";

/// The wizard's finish: the draft's settings written.
pub const APPLY: &str = "init.apply";

/// The folder step's "Choose…": the folder chooser into the draft.
pub const CHOOSE: &str = "init.chooseFolder";

/// The `validate` hook of the library folder field.
pub const CHECK_FOLDER: &str = "init.libraryFolder.check";

/// What asked for a run, told its outcome.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Then {
    /// The first run, or "Set up Helios…": nothing more.
    Nothing,
    /// The import window, with its arguments: opened once done, told why
    /// it cannot import when left.
    Import(Value),
}

/// The run being drawn.
struct Running {
    reply: Reply,
    /// What the Settings app said it last finished when the run opened:
    /// a new answer of this wizard means its finish ran.
    finished_before: Option<Value>,
}

/// The setup's state in the shell.
#[derive(Default)]
pub(crate) struct InitState {
    /// The first-run check is on (the desktop's), and was made.
    first_run: bool,
    checked: bool,
    running: Option<Running>,
    /// The outcomes heard, with what asked for them.
    heard: Arc<Mutex<Vec<(Then, Outcome)>>>,
    /// The folder chooser asked by "Choose…".
    choosing: Option<Pending>,
}

/// The settings the steps read, as the desktop holds them.
struct DesktopValues {
    folder: Option<PathBuf>,
}

impl helios_init::Values for DesktopValues {
    fn value(&self, key: &str) -> Option<Value> {
        match key {
            LIBRARY_FOLDER_SETTING => self
                .folder
                .as_ref()
                .map(|p| Value::String(p.display().to_string())),
            _ => None,
        }
    }
}

/// The host: the Settings app's wizard, opened on the plan's first step.
struct SettingsHost<'a> {
    app: &'a egui_pop::api::app::AppHandle,
    folder: Option<PathBuf>,
    running: Option<Running>,
    refused: Option<String>,
}

impl helios_init::Host for SettingsHost<'_> {
    fn run(&mut self, plan: Plan, reply: Reply) {
        let Some(admin) = self.app.service(&ADMIN) else {
            self.refused = Some("no Settings app".into());
            return;
        };
        let start = plan.steps.get(plan.start).map(|s| s.id.clone());
        let suggested = self
            .folder
            .clone()
            .or_else(helios_library::import::suggested_library_folder)
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let values = [(LIBRARY_FOLDER_SETTING.to_owned(), json!(suggested))].into();
        let finished_before = admin.last_finished();
        match admin.open_wizard(self.app, WIZARD, start.as_deref(), values) {
            Ok(_) => {
                self.running = Some(Running {
                    reply,
                    finished_before,
                });
            }
            Err(e) => self.refused = Some(e),
        }
    }
}

impl Shell {
    /// Turns the first-run check on: once the library is known, the setup
    /// opens when a required step is not done. The desktop's; tests turn
    /// it on when they look at it.
    pub fn set_first_run(&mut self, on: bool) {
        self.core.init.first_run = on;
        self.core.init.checked = false;
    }

    /// The library folder, as the import's account holds it; `None`
    /// without a library.
    fn init_folder(&self) -> Option<Option<PathBuf>> {
        let library = self.import.get().accounts.library.clone()?;
        Some(
            library
                .get()
                .inspect_err(|e| log::warn!("init: library folder: {e}"))
                .ok()
                .flatten()
                .filter(|p| !p.as_os_str().is_empty()),
        )
    }

    /// Makes sure the steps `request` asks are done, the wizard opened on
    /// them when not; `then` hears the outcome. `always`: opened even when
    /// they are done (the menu's, the health fix's).
    fn init_ensure(
        &mut self,
        request: &Request,
        then: Then,
        always: bool,
    ) -> Result<Value, String> {
        if self.core.init.running.is_some() {
            return Err("the setup is already open".into());
        }
        let folder = self.init_folder().flatten();
        let values = DesktopValues {
            folder: folder.clone(),
        };
        let registry = Registry::builtin();
        let heard = self.core.init.heard.clone();
        let done = {
            let then = then.clone();
            move |o: Outcome| {
                heard
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((then, o));
            }
        };
        let app = self.core.app.handle().clone();
        let mut host = SettingsHost {
            app: &app,
            folder,
            running: None,
            refused: None,
        };
        if always {
            // Every step asked, done or not: read over no value.
            let none = DesktopValues { folder: None };
            helios_init::ensure(&registry, &none, request, &mut host, done)?;
        } else {
            helios_init::ensure(&registry, &values, request, &mut host, done)?;
        }
        if let Some(e) = host.refused {
            return Err(e);
        }
        let opened = host.running.is_some();
        self.core.init.running = host.running;
        Ok(json!({ "opened": opened }))
    }

    /// `helios.setUp`: every step.
    pub(super) fn init_setup(&mut self) -> Result<Value, String> {
        self.init_ensure(&Request::all(), Then::Nothing, true)
    }

    /// `init.libraryFolder`: sets the folder to `path`, or opens the
    /// library folder step alone.
    pub(super) fn init_library_folder(&mut self, args: &Value) -> Result<Value, String> {
        if args["path"].as_str().is_some_and(|p| !p.is_empty()) {
            return self.init_apply(&json!({ LIBRARY_FOLDER_SETTING: args["path"] }));
        }
        let request = Request::steps(&[LIBRARY_FOLDER]).starting_at(LIBRARY_FOLDER);
        self.init_ensure(&request, Then::Nothing, true)
    }

    /// Opening the import window: the steps import requires first, in a
    /// partial run on the first one missing; the window opens when they
    /// are done, or left (then saying why it cannot import yet). `false`:
    /// nothing is missing, open it now.
    pub(super) fn ensure_for_import(&mut self, args: &Value) -> bool {
        let Some(folder) = self.init_folder() else {
            return false;
        };
        let registry = Registry::builtin();
        let missing = registry
            .missing(IMPORT, &DesktopValues { folder })
            .unwrap_or_default();
        let Some(first) = missing.first() else {
            self.import.get_mut().not_set_up = None;
            return false;
        };
        let steps: Vec<&str> = missing.iter().map(String::as_str).collect();
        let request = Request::steps(&steps).starting_at(first);
        match self.init_ensure(&request, Then::Import(args.clone()), false) {
            Ok(_) => true,
            Err(e) => {
                log::warn!("init: {e}");
                false
            }
        }
    }

    /// `init.apply`: the draft's settings written through their keeper,
    /// then the run ended with what they are now.
    pub(super) fn init_apply(&mut self, args: &Value) -> Result<Value, String> {
        let empty = Map::new();
        let values = args.as_object().unwrap_or(&empty);
        let mut written = Vec::new();
        for (key, value) in values {
            match key.as_str() {
                LIBRARY_FOLDER_SETTING => {
                    let path = value
                        .as_str()
                        .filter(|p| !p.trim().is_empty())
                        .ok_or("`library.folder`: a folder's path")?;
                    let dir = PathBuf::from(path);
                    if let Err(p) = helios_library::import::check_library_folder(&dir) {
                        let said = self.text(p.message());
                        self.notify(crate::dialogs::Level::Error, Message::plain(said.clone()));
                        self.init_end();
                        return Err(said);
                    }
                    self.set_library_folder(&dir)?;
                    written.push(key.clone());
                }
                _ => log::warn!("init: `{key}` is not a setting a step fills here"),
            }
        }
        self.init_end();
        Ok(json!({ "written": written }))
    }

    /// Ends the run, if one is open: its caller hears the outcome read
    /// from the settings.
    fn init_end(&mut self) {
        let Some(run) = self.core.init.running.take() else {
            return;
        };
        let values = DesktopValues {
            folder: self.init_folder().flatten(),
        };
        run.reply.end(&values);
    }

    /// `init.chooseFolder`: the folder chooser, its answer written into
    /// the wizard's draft.
    pub(super) fn init_choose_folder(&mut self) -> Result<Value, String> {
        let app = self.core.app.handle();
        let f = app.service(&FILES).ok_or("no folder chooser")?;
        let title = self.text(Message::new(
            "gui-pick-choose-folder",
            "Choose the library folder…",
        ));
        let mut request = FileRequest::folder(&title).purpose("helios.library.folder");
        if let Some(dir) = helios_library::import::suggested_library_folder()
            .as_deref()
            .and_then(Path::parent)
            .filter(|d| d.is_dir())
        {
            request = request.directory(dir);
        }
        self.core.init.choosing = Some(f.choose(app, request));
        Ok(json!({ "asked": true }))
    }

    /// Each frame: the first-run check, the chooser's answer, a run the
    /// wizard left (cancelled), and the outcomes heard.
    pub(super) fn init_tick(&mut self) {
        let app = self.core.app.handle().clone();
        // The folder chosen, into the draft.
        if let Some(result) = self.core.init.choosing.as_ref().and_then(Pending::take) {
            self.core.init.choosing = None;
            if let Ok(files::Chosen::Paths(paths)) = files::chosen(&result)
                && let Some(p) = paths.into_iter().next()
                && let Some(admin) = app.service(&ADMIN)
            {
                let set = [(
                    LIBRARY_FOLDER_SETTING.to_owned(),
                    json!(p.display().to_string()),
                )];
                if let Err(e) = admin.wizard_set(&app, set.into()) {
                    log::warn!("init: {e}");
                }
            }
        }
        // The first run: once the library is known.
        if self.core.init.first_run && !self.core.init.checked {
            if let Some(folder) = self.init_folder() {
                self.core.init.checked = true;
                let missing = Registry::builtin()
                    .status(&DesktopValues { folder })
                    .is_ok_and(|s| s.iter().any(|s| s.required && !s.done));
                if missing && let Err(e) = self.init_ensure(&Request::all(), Then::Nothing, false) {
                    log::warn!("init: the first run: {e}");
                }
            }
        }
        // The wizard closed without its finish: cancelled.
        if let Some(run) = &self.core.init.running
            && let Some(admin) = app.service(&ADMIN)
            && admin.wizard_flow().is_none_or(|f| f.wizard != WIZARD)
        {
            let finished = admin.last_finished();
            let ours = finished != run.finished_before
                && finished.as_ref().is_some_and(|f| f["finished"] == WIZARD);
            // Finished: `init.apply` is on its way and ends the run.
            if !ours {
                self.init_end();
            }
        }
        let heard = std::mem::take(
            &mut *self
                .core
                .init
                .heard
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for (then, outcome) in heard {
            if let Then::Import(args) = then {
                let why = match outcome {
                    Outcome::Done => None,
                    Outcome::Cancelled { .. } => Some(Message::new(
                        "gui-import-no-library-folder",
                        "Choose a library folder first: imports copy photos into it (Helios menu › Set up Helios…).",
                    )),
                };
                self.import.get_mut().not_set_up = why;
                self.open_import_window(&args);
            }
        }
    }
}
