//! The desktop shell: an egui-pop application (`egui_pop::api::App`) with
//! Helios's plugins, around Helios's own frame, areas and status bar.
//!
//! egui-pop draws what every application shares, from declarations: the
//! menu bar and menus, the command palette, dialogs, toasts, the settings
//! window with its shortcuts editor, the window's place and frame, and
//! the keys, resolved by its keymap. Helios declares its commands there
//! and runs them on its own kernel (see `host`), opens its system menu
//! from the frame, draws its status bar, and gives its panels to
//! egui-pop's workbench (areas, auxiliary windows, arrangements, see
//! `workbench`).
//!
//! Everything the user can do goes through a command, so a key, the
//! palette, a menu, a script and an agent reach the same actions.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use egui::{Context as Egui, Id, Rect, Ui};
use egui_pop::api::app::OverlayKind;
use egui_pop::api::dialogs::{self, Choose, ChooseOption, DIALOGS, Dialogs};
use egui_pop::api::frame::{self, FramePlugin, SETTING_FRAME};
use egui_pop::api::host::{Reply, Resolver};
use egui_pop::api::toasts::{TOASTS, Toast};
use egui_pop::api::{App, AppBuilder, AppHandle, DefaultPlugins, palette};
use egui_pop::core::frame::{FrameRequest, HitTable};
use egui_pop::core::scale;
use egui_pop::model::ids::PanelId;
use egui_pop::model::ids::{CommandId, MenuId, OwnerId};
use egui_pop::model::keymap::{DocBinding, KeymapDoc, Layer};
use egui_pop::model::layout::Layout;
use egui_pop::model::menus::{Item, MenuItem as PopItem, Shown, show_items};
use egui_pop::model::panels::{Area, PanelSpec};
use helios_core::{Bus, Message};
use helios_shell::commands::{CommandSpec, Origin, Registry, Run};
use image::RgbaImage;
use serde_json::{Value, json};

use crate::dialogs::{Answer, Level, MenuItem, Notice};
use crate::host::{HeliosHost, Later, Workspace, to_helios, to_pop, workspace_key};
use crate::plugins::{
    self, Declarations, ListSource, SYSTEM, SYSTEM_PROFILES, SYSTEM_RECENT, SYSTEM_THEMES,
};
use crate::snapshot::{self, Recorder};
use crate::viewport::ZoomTo;
use crate::workbench::Contents;

pub use crate::workbench::Content;
pub use export::{ExportAsk, OPEN_FOLDER, folder_path, settings_of};
pub use health::{
    BADGE as HEALTH_BADGE, CHECK_NOW as HEALTH_CHECK_NOW, COMMAND as HEALTH_COMMAND,
    FINDINGS as HEALTH_FINDINGS, FIX as HEALTH_FIX, IGNORE as HEALTH_IGNORE,
    OTHERS as HEALTH_OTHERS, PAGE as HEALTH_PAGE, findings_answer as health_findings_answer,
    owner_key as health_owner_key,
};
pub use import_auto::Trigger as ImportTrigger;
pub use import_choices::Choices;
pub use import_sources::{
    DOCUMENT as IMPORT_SOURCES, Honours, ReadSources, SavedSource, WriteSources,
    write_in_background as write_import_sources_in_background,
};
pub use macro_edit::{SELECTED as GRAPH_SELECTED, selected_nodes};
pub use storage::{
    BACKUP_NOW as BACKUP_NOW_COMMAND, COMMAND as STORAGE_COMMAND, StorageLink, VaultStorage,
    role_text,
};
pub use workshop_generalise::{Generalised, choice_word};
pub use workspace_edit::STATE as WORKSPACE_STATE;

mod accounts_admin;
mod core;
mod export;
mod focus;
pub use accounts_admin::{
    ACCOUNT_CONNECTORS as ACCOUNT_CONNECTORS_COMMAND, ACCOUNTS as ACCOUNTS_COMMAND,
    ADD_SOURCE as ADD_SOURCE_COMMAND, DISCONNECT as DISCONNECT_COMMAND,
    OAUTH_CLIENTS as OAUTH_CLIENTS_COMMAND, RECONNECT as RECONNECT_COMMAND,
    RENAME as RENAME_COMMAND, SET_OAUTH_CLIENT as SET_OAUTH_CLIENT_COMMAND,
    SIGN_OUT as SIGN_OUT_COMMAND,
};
mod calibrate_focus;
mod camera_look;
pub use calibrate_focus::{COMMAND as CALIBRATE_COMMAND, WINDOW as CALIBRATE_WINDOW};
mod health;
mod import;
mod import_accounts;
mod import_browsers;
mod import_chosen;
pub use import_accounts::LibraryFolder;
pub use import_accounts::{KnownRemote, RemoteImport};
mod import_auto;
mod import_choices;
mod import_detail;
mod import_history;
mod import_profiles;
mod import_sources;
mod import_window;
mod import_wizard;
pub(crate) mod init_wizard;
mod lifetime;
pub use import_history::{Reader as ImportHistoryReader, reader_of as import_history_of};
pub use import_window::{FORGET_FOLDER, FORGET_VOLUME};
pub use lifetime::{Closing, LastWindow};
mod storage;
mod storage_admin;
pub use storage_admin::{
    MARK_VOLUME as MARK_VOLUME_COMMAND, SET_STRATEGY as SET_STRATEGY_COMMAND,
    SET_UP_STRATEGY as SET_UP_STRATEGY_COMMAND, STRATEGY as BACKUP_STRATEGY_COMMAND,
    VOLUME_ROLE as VOLUME_ROLE_COMMAND, VOLUME_SET as VOLUME_SET_COMMAND,
    VOLUMES as VOLUMES_COMMAND, snapshot_rows,
};

/// Where an import's lines are sent: its arguments and a line.
type ImportLines = std::sync::mpsc::Sender<(Value, Value)>;
mod macro_edit;
mod network_edit;
mod workshop_design;
mod workshop_edit;
mod workshop_generalise;
pub use workshop_run::{ReviewFn as WorkshopReviewFn, RunFn as WorkshopRunFn};
mod workshop_run;
mod workshop_session;
mod workspace_edit;
mod workspace_live;

/// The command the host registers to run the imports the shell hands
/// over ([`Shell::take_imports`]); only with a library open.
pub const IMPORT_COMMAND: &str = "library.import";

/// Opens a failure's details as an error dialog, from its toast.
pub const NOTICE_DETAILS: &str = "notice.details";

/// The `when` of a command of the main window (the library's selection,
/// the image shown): greyed in another window's menus, which say where
/// it runs (docs/design/windows.md, section 7).
pub const IN_MAIN_WINDOW: &str = "appWindow == 'main'";

/// The screen's own scale (physical pixels per point, the interface zoom
/// left out) from which it is `hidpi` for profile variants.
const HIDPI: f32 = 1.5;

/// About's one-line description of Helios, as the README says it.
fn about_description() -> Message {
    Message::new(
        "gui-about-description",
        "A client/server, AI-first image processing framework.",
    )
}

/// What Help > About Helios shows (egui-pop's `about::AboutPlugin`):
/// Cargo's version, licence, authors and repository, and the logo. The
/// description is translated once the shell runs (`follow_language`).
/// The repository's issue list turns on egui-pop's `help.report` (Help >
/// Report a problem): a new issue, prefilled with the version and the
/// platform.
fn about_info() -> egui_pop::api::about::AppInfo {
    let mut info = egui_pop::api::about::AppInfo::new("Helios", env!("HELIOS_VERSION"))
        .description(&about_description().english)
        .licence(env!("CARGO_PKG_LICENSE"))
        .repository(env!("CARGO_PKG_REPOSITORY"))
        .issues(concat!(env!("CARGO_PKG_REPOSITORY"), "/issues"))
        .icon(|ui, side| {
            let logo = crate::plugins::logo(ui.ctx());
            ui.add(egui::Image::new((logo.id(), egui::vec2(side, side))));
        });
    for author in env!("CARGO_PKG_AUTHORS")
        .split(':')
        .filter(|a| !a.is_empty())
    {
        info = info.author(author);
    }
    info
}

/// What commands ask the shell to do; they are queued and applied at the
/// next frame, where the shell's state is at hand. The command shows as
/// running until then (see `host::Later`).
enum Action {
    Palette,
    /// Quick open's list of modules, images and workspaces.
    QuickOpen,
    /// Goes to what quick open offers.
    QuickOpenGo(crate::quick_open::Item),
    Theme(String),
    ThemeExport(PathBuf),
    /// Where, which parts, only this machine's view.
    ProfileExport(PathBuf, Vec<helios_profile::Part>, bool),
    ProfileImport(PathBuf),
    ProfileApply,
    ProfileUse(String),
    ScopeMode(crate::scopes::Mode),
    ScopeStops(bool),
    /// A decision answered, as whom.
    Decision(crate::decisions::Request, Origin),
    /// A decision answered with plain JSON values (`decision.decide`).
    DecisionDecide(String, Value, Origin),
    /// The decisions waiting (`decision.list`).
    DecisionList,
    Rate(i64),
    Reject,
    /// Pick, reject or unflag (culling).
    Flag(egui_pop::views::library::Flag),
    Label(String),
    Step(i64),
    /// Culling's auto-advance: on, off, or toggled.
    AutoAdvance(Option<bool>),
    /// Expand or collapse the stack under the library's cursor.
    ToggleStack,
    /// A selection or batch command of the library (`library.select.*`,
    /// `library.marks.*`) with its arguments.
    LibraryBatch(&'static str, Value),
    /// Import a card or folder: as the workspace's import source says,
    /// or asked (`library.importSource`'s arguments).
    ImportSource(Value, Origin),
    /// Open the import window (`library.importWindow`'s arguments).
    ImportWindow(Value),
    /// Pause or resume the automatic imports
    /// (`library.importAutomatic.pause`'s arguments).
    PauseAutomaticImports(Value),
    /// Open the Calibration focus (`calibrate.open`'s arguments).
    Calibrate(Value),
    /// `import.addSource` or `import.reconnect`, with its arguments.
    Account(&'static str, Value),
    /// Open the Health page (`help.health`).
    Health,
    /// Run the health checks now (`health.checkNow`).
    HealthCheckNow,
    /// Run a finding's fix (`health.fix`'s arguments).
    HealthFix(Value),
    /// Ignore a finding or show it again (`health.ignore`'s arguments).
    HealthIgnore(Value),
    /// Open the Storage page (`library.storage`'s arguments).
    Storage(Value),
    /// Files and folders to import (`library.importFiles`'s arguments: a
    /// paste of files, a drop on the window).
    ImportFiles(Value, Origin),
    /// Import through an importer plugin (`library.importWith`'s
    /// arguments).
    ImportWith(Value, Origin),
    /// List the import history (`import.history.list`'s arguments).
    ImportHistory(Value),
    /// Run an import of the history again (`import.history.again`'s
    /// arguments), as whom.
    ImportAgain(Value, Origin),
    /// Export photos (`library.export`'s arguments), as whom.
    Export(Value, Origin),
    /// Show a folder in the file manager (`library.openFolder`'s
    /// arguments).
    OpenFolder(Value),
    /// Choose the library folder (`library.chooseFolder`'s arguments).
    ChooseLibraryFolder(Value),
    /// The setup's commands (`init_wizard`): every step, the folder step
    /// (its arguments), the wizard's finish (the draft), the folder
    /// chooser into the draft.
    InitSetUp,
    InitLibraryFolder(Value),
    InitApply(Value),
    InitChooseFolder,
    Fullscreen,
    Minimize,
    Maximize,
    /// Quit Helios, from any window (`lifetime`).
    Quit,
    /// Quit, the question about running jobs answered.
    QuitConfirmed,
    /// A notice's details as an error dialog (`notice.details`).
    NoticeDetails(Option<usize>),
    /// Bring the main window forward, or back.
    MainWindow,
    Detach(String),
    /// A floatable panel floats over the interface, at a place or
    /// cascaded.
    Float(String, Option<[f32; 2]>),
    MoveTo(String, Area),
    /// Folds an area to its strip, or unfolds it.
    Fold(Area),
    Zoom(crate::viewport::ZoomTo),
    /// Before and after side by side, or not.
    Compare,
    /// The mask display set to a mode, or moved along its cycle.
    MaskDisplay(Option<crate::mask_display::Mode>, Origin),
    /// A module (the selected one by default) bypassed, brought back, or
    /// toggled, as whom.
    Bypass(Option<String>, Option<bool>, Origin),
    /// Goes to a place of the context stack at the workspace's top: the
    /// library, or the workspace's network.
    Place(crate::contexts::FrameId),
    /// Compares photos side by side, in the Compare place (`images`, else
    /// the selection: two or more).
    ComparePhotos(Value),
    /// Sorts the library (`by`, `descending`), as whom; without `by`,
    /// says how it is sorted.
    Sort(Value, Origin),
    /// Scopes the library to a folder (`path`; null: none), as whom;
    /// without `path`, says the folder.
    Folder(Value, Origin),
    /// Scopes the library to a collection (`id`; null: none), as whom;
    /// without `id`, says the collection.
    Collection(Value, Origin),
    /// An album command (its id) with its arguments.
    CollectionEdit(&'static str, Value),
    /// A recent-filters command (its id) with its arguments
    /// (`crate::recent_filters`).
    RecentFilters(&'static str, Value),
    /// Scans a folder for the image map (`path`).
    ScanFolder(Value),
    /// Moves the comparison's active photo by a step.
    CompareStep(i64),
    /// Zoom and pan synchronised across the compared photos, or not
    /// (toggled when `None`).
    CompareSync(Option<bool>),
    /// Enters a sub-workspace (by id; without one, the one under the
    /// place shown that holds the selected node), as whom.
    EnterSubworkspace(Option<String>, Origin),
    /// Culling on, off, or toggled.
    Culling(Option<bool>),
    /// A drawn-mask command, as whom.
    Mask(crate::masks::Command, Origin),
    /// Rename a node of the Masks panel (its selected one by default) in
    /// place.
    RenameInTree(Option<String>),
    /// A parameter copied, its text the value as shown or the path.
    CopyParam(String, crate::clipboard::param::Text, Origin),
    /// The clipboard pasted onto a parameter (or only said, a dry run).
    PasteParam(String, bool, Origin),
    /// One camera look fitted from the Library's selection (photos of one
    /// camera), as a job: `look.camera.fitSelection` on its images.
    FitCameraLook(Value, Origin),
    /// The files of the images the library acts on, copied for other
    /// applications, as whom (the user only).
    CopyFiles(Origin),
    /// Files or an image another application copied, pasted into the
    /// library (or only said, a dry run), as whom.
    PasteFiles(bool, Origin),
    /// The image the viewport shows, rendered and copied for other
    /// applications, as whom (the user only).
    CopyImage(Origin),
    SystemMenu,
    /// A command, its arguments, as whom.
    Run(String, Value, Origin),
    Keybindings,
    /// A keymap package's keys added over the defaults (`true`), or taken
    /// away.
    KeymapPackage(String, bool),
    OpenWorkspace(Option<PathBuf>),
    /// A workspace from a template (its id), in a directory, the
    /// template's parameters answered (`parameter.<id>`); what is
    /// missing is asked.
    NewWorkspace(Option<String>, Option<PathBuf>, Value),
    /// A command of the workspace's edits, its arguments, as whom.
    WorkspaceEdit(String, Value, Origin),
    Answer(Answer, Origin),
    AssumeYes(bool),
    Dock(String),
    Snapshot {
        target: String,
        path: PathBuf,
    },
    RecordStart {
        target: String,
        path: PathBuf,
        fps: u32,
    },
    RecordStop,
}

type Queue = Arc<Mutex<Vec<(Action, Option<Resolver>)>>>;

/// How a command turns its arguments into an action.
type ActionOf = Box<dyn Fn(&Value, &Origin) -> Result<Action, String> + Send + Sync>;

/// A picture asked of the window, waiting for egui's screenshot.
enum Shot {
    Save { target: String, path: PathBuf },
    Frame,
}

/// What a window (eframe) or a test harness draws with: the process's one
/// GPU device (`helios_gpu::shared`), which the engine computes on too.
///
/// The window is handed the engine's device rather than the engine the
/// window's: processes without a window (the node, exports, batches) open
/// it the same way, it exists before any window does (a render may start
/// first), and device loss is handled where the device is owned. Letting
/// eframe open its own would make a second instance and device, which the
/// Vulkan loader before 1.4.350 crashes on (see `helios_gpu::shared`).
pub fn wgpu_setup(gpu: &helios_gpu::Gpu) -> eframe::egui_wgpu::WgpuSetup {
    let h = gpu.handles();
    eframe::egui_wgpu::WgpuSetup::Existing(eframe::egui_wgpu::WgpuSetupExisting {
        instance: h.instance,
        adapter: h.adapter,
        device: h.device,
        queue: h.queue,
    })
}

/// Names a widget's rectangle for this frame, so it can be pictured by
/// that name (`gui.snapshot {target: name}`). The rectangle is in the
/// points of the window it is drawn in, which is kept with it.
pub fn name_rect(ctx: &Egui, name: &str, rect: Rect) {
    let window = ctx.viewport_id();
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<BTreeMap<String, (Rect, egui::ViewportId)>>(Id::unique(
            "helios.named",
        ))
        .insert(name.to_string(), (rect, window));
    });
}

/// What commands are wired to, before the shell exists: declaring them
/// comes before building the application that reads them.
struct Wiring<'a> {
    registry: &'a Arc<Mutex<Registry>>,
    queue: Queue,
    later: &'a Later,
    dialogs: Arc<OnceLock<Dialogs>>,
    resolved_view: Arc<Mutex<Value>>,
    values: Arc<Mutex<helios_values::Store>>,
    themes: &'a crate::theme::Themes,
    declarations: &'a Rc<RefCell<Declarations>>,
}

/// What the shell keeps of its wiring until egui-pop's application is
/// built.
struct Parts {
    registry: Arc<Mutex<Registry>>,
    lists: Rc<RefCell<ListSource>>,
    declarations: Rc<RefCell<Declarations>>,
    panels: Vec<PanelSpec>,
    contents: Contents,
    queue: Queue,
    themes: crate::theme::Themes,
    resolved_view: Arc<Mutex<Value>>,
    dialogs: Arc<OnceLock<Dialogs>>,
    values: Arc<Mutex<helios_values::Store>>,
    snapshots: crate::edit_snapshots::Shared,
    workspace: Workspace,
    /// The catalogues of the application's translation plugin.
    i18n: egui_pop::i18n::I18n,
    opens: crate::platform::Opens,
    jobs: helios_core::jobs::Jobs,
    library_history: crate::library_history::LibraryHistory,
    status: crate::jobs::Status,
    relays: crate::jobs::Relays,
    /// What checks a restored darkroom's image is still there.
    images: crate::contexts::Images,
    /// The workspace's sub-workspaces, for their place.
    subworkspaces: crate::contexts::Subworkspaces,
    /// Where the agents' collectors find the library and the viewport.
    agent_state: crate::agent_state::Late,
    /// The controls' context: the Parameters panel sets its box and hand,
    /// the `controls.*` commands read and set it.
    controls: helios_controls::commands::SharedContexts,
    /// The plugins installed, for Settings > Plugins: set by
    /// [`ShellBuilder::plugins`] before the application is built.
    plugin_settings: crate::config::plugins::Shared,
}

/// A shell whose egui-pop application is still to build: in a test
/// harness's context, or in a window by egui-pop's `run_native_with`.
pub struct ShellBuilder {
    app: AppBuilder,
    /// The language plugin, added last as the application is built.
    i18n: egui_pop::i18n::I18nPlugin,
    parts: Parts,
}

impl ShellBuilder {
    /// The language setting's default is the system's (`HELIOS_LANG`
    /// first), as on the desktop; otherwise the shell speaks English.
    #[must_use]
    pub fn system_language(mut self) -> Self {
        self.i18n = self.i18n.language(egui_pop::i18n::plugin::SYSTEM);
        self
    }

    /// Opens a workspace directory: its layout is the one egui-pop loads
    /// as the application starts and saves as it closes, under the
    /// interface's settings (`workspaces/<key>/layout`).
    #[must_use]
    pub fn workspace(self, dir: &Path) -> Self {
        self.parts.workspace.set(Some(workspace_key(dir)));
        self
    }

    /// Says whether an image of the vault is still there, by id: a
    /// darkroom restored on an image that is gone is left at start.
    #[must_use]
    pub fn images(self, check: crate::contexts::ImageCheck) -> Self {
        *self
            .parts
            .images
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(check);
        self
    }

    /// Runs `f` when the application closes, before the `app.exit` step
    /// (which saves the interface's settings).
    #[must_use]
    pub fn on_exit(mut self, f: impl FnMut(&AppHandle) + 'static) -> Self {
        self.app = self.app.on_exit(f);
        self
    }

    /// The desktop's platform services (see `platform::desktop`): the
    /// single instance this launch is.
    #[must_use]
    pub fn desktop(mut self, instance: egui_pop::platform::instance::Instance) -> Self {
        self.parts.opens.listen();
        self.app = crate::platform::desktop(self.app, instance);
        self
    }

    /// The plugins installed: each importer an entry under File > Import
    /// from, each plugin's catalogues added to the interface's (its
    /// importers' labels translated there), each a page of Settings >
    /// Plugins with its settings. A catalogue refused is said
    /// in the log and left out.
    #[must_use]
    pub fn plugins(self, broker: &helios_plugin::Broker) -> Self {
        let entries = crate::importers::entries(broker);
        for id in broker.ids() {
            let Some(manifest) = broker.manifest(id) else {
                continue;
            };
            let Some(dir) = broker.dir(id) else {
                continue;
            };
            let commands: Vec<String> = manifest.commands.iter().map(|c| c.id.clone()).collect();
            let added = helios_i18n::plugins::read(dir).and_then(|catalogues| {
                helios_i18n::plugins::add(&self.parts.i18n, id, &commands, &catalogues)
            });
            if let Err(e) = added {
                log::warn!("plugin `{id}`'s catalogues: {e}");
            }
        }
        self.parts.lists.borrow_mut().importers = entries;
        *crate::config::lock(&self.parts.plugin_settings) =
            crate::config::plugins::PluginSettings::of(broker);
        self
    }

    /// The window manager's focus mode, read beforehand (else egui-pop
    /// reads it as the window opens, a child process on Linux).
    #[must_use]
    pub fn system_focus(mut self, system: egui_pop::core::focus_mode::SystemFocus) -> Self {
        self.app = self.app.system_focus(system);
        self
    }

    /// The shell, its application built on a context of its own (tests,
    /// tools, pictures; a harness draws it in its own).
    pub fn build(self) -> Shell {
        let app = self
            .app
            .plugin(self.i18n)
            .build(&Egui::default())
            .unwrap_or_else(|e| panic!("Helios's application builds: {e}"));
        Shell::assemble(self.parts, app)
    }

    /// Opens the window (egui-pop's `run_native_with`): the window's
    /// place from the last run, the application built on the window's
    /// context, then `make` gives eframe the application around the
    /// shell, which calls [`Shell::exit`] when the window closes.
    pub fn run_native(
        self,
        title: &str,
        options: eframe::NativeOptions,
        make: impl FnOnce(Shell, &eframe::CreationContext<'_>) -> Box<dyn eframe::App> + 'static,
    ) -> eframe::Result {
        let parts = self.parts;
        self.app
            .plugin(self.i18n)
            .run_native_with(title, options, move |app, cc| {
                Ok(make(Shell::assemble(parts, app), cc))
            })
    }
}

/// The Settings window's id (egui-pop's Settings app in its window mode):
/// `window: "settings"` for agents, `appWindow == 'settings'` in `when`
/// clauses, its size and place kept under it.
pub const SETTINGS_WINDOW: &str = egui_pop::api::admin::APP_WINDOW;

/// Settings open in an OS window of their own (docs/design/windows.md,
/// slice 5): egui-pop's Settings app in its window mode, an application
/// window as the import window is, with Helios's title bar, its own size
/// and place, closing alone. The person may still pop them in over the
/// main window ("Pop in", remembered); only a mode never chosen is set.
fn settings_in_their_own_window(app: &egui_pop::api::app::AppHandle) {
    use egui_pop::api::admin::WINDOW;
    use egui_pop::model::ids::OwnerId;
    use egui_pop::model::values::{Change, Set, Value as Setting};
    let store = app.store();
    let mut store = lock(&store);
    let path = WINDOW.path();
    if store.get(&path).is_none_or(|e| e.origin.is_some()) {
        return;
    }
    let set = Set::new(
        path.as_str(),
        Change::To(Setting::Bool(true)),
        egui_pop::model::commands::Origin::User,
    )
    .by(&OwnerId::FRAMEWORK)
    .untracked();
    if let Err(e) = store.set(set) {
        log::warn!("{}: {e:?}", WINDOW.as_str());
    }
}

/// What a step of egui-pop's gave: its result, or why it was vetoed;
/// nothing when it did not run.
fn step_result(o: Option<egui_pop::api::steps::Outcome>) -> Result<Value, String> {
    use egui_pop::api::steps::Outcome;
    match o {
        None => Ok(Value::Null),
        Some(Outcome::Done(r)) => r,
        Some(Outcome::Vetoed { by, reason }) => Err(format!("stopped by `{by}`: {reason}")),
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The desktop shell: the process's [`Core`](core::Core), what every
/// window shares, the [`Focus`](focus::Focus) of its main window and the
/// import focus.
///
/// The split follows Helios's windows (docs/design/windows.md): one OS
/// window per focus, all of them in one process around one core. The main
/// window (the root viewport) is always the first focus; the import focus
/// opens in an OS window of its own (`import_window`), through egui-pop's
/// application windows.
pub struct Shell {
    /// What every window of the process shares.
    core: core::Core,
    /// What belongs to the main window.
    focus: focus::Focus,
    /// The import focus, in an OS window of its own.
    import: import_window::ImportFocus,
    /// The Calibration focus, in an OS window of its own.
    calibration: calibrate_focus::CalibrationFocus,
    /// The application's life, apart from its windows'.
    lifetime: lifetime::Lifetime,
    /// What the node's endpoint asks of the windows' commands.
    window_asks: Option<crate::window_commands::Asks>,
    /// The window group the shell's toasts go to while it answers, in
    /// the main window's pass, what another window asked (the import
    /// window's Import, Forget, Storage…); `None`: the window being drawn
    /// or commanded, as egui-pop says.
    toasts_in: Option<egui::ViewportId>,
    /// What the shell has to say: command results, layout repairs.
    pub notes: Vec<String>,
}

impl Shell {
    /// A shell over declared panels, in a saved layout (repaired) or the
    /// default one.
    pub fn new(panels: Vec<PanelSpec>, layout: Option<Layout>) -> Shell {
        Shell::build(panels, layout, None)
    }

    /// As [`Shell::new`], keeping the interface's settings (window place,
    /// focus mode, the user's keys) in `ui_settings`.
    pub fn build(
        panels: Vec<PanelSpec>,
        layout: Option<Layout>,
        ui_settings: Option<PathBuf>,
    ) -> Shell {
        let mut shell = Shell::builder(panels, ui_settings).build();
        if let Some(layout) = layout {
            shell.set_layout(layout);
        }
        shell
    }

    /// A shell to build: its commands and plugins declared, egui-pop's
    /// application not built yet ([`ShellBuilder::build`], or
    /// [`ShellBuilder::run_native`] in a window).
    pub fn builder(panels: Vec<PanelSpec>, ui_settings: Option<PathBuf>) -> ShellBuilder {
        // Relative dates in smart albums ("today") count by the local
        // clock, as cameras set to local time write capture times.
        helios_vault::rules::set_local_offset(i64::from(
            chrono::Local::now().offset().local_minus_utc(),
        ));
        let registry = Arc::new(Mutex::new(Registry::new()));
        let bus = Arc::new(Bus::new());
        // Background work reports on the shell's bus, which egui-pop
        // reads as its journal (its jobs centre).
        let jobs = helios_core::jobs::Jobs::new(bus.clone());
        let status = crate::jobs::Status::default();
        let relays = crate::jobs::Relays::default();
        // Helios's values: what widgets and agents edit, and what Edit >
        // Undo walks back through the host.
        let values: Arc<Mutex<helios_values::Store>> = Arc::default();
        let agent_state = crate::agent_state::Late::default();
        let controls = helios_controls::commands::SharedContexts::default();
        // The open image's history snapshots: taken through the history
        // owner, pinned and kept by the vault's link (`persist`).
        let snapshots = crate::edit_snapshots::Shared::default();
        let i18n_plugin = egui_pop::i18n::I18nPlugin::new()
            .catalogues(helios_i18n::CATALOGUES)
            .env(helios_i18n::ENV)
            // English unless the desktop asks for the system's language
            // (`ShellBuilder::system_language`): tests, pictures and
            // tools read the same labels on any machine, with or
            // without the user's settings.
            .language("en");
        let i18n = i18n_plugin.i18n();
        let library_history = crate::library_history::LibraryHistory::default();
        let host = HeliosHost::new(registry.clone(), bus, Arc::default())
            .with_values(values.clone(), snapshots.clone())
            .with_library(library_history.clone())
            .with_i18n(i18n.clone());
        let later = host.later();
        let workspace = host.workspace();
        let images = crate::contexts::Images::default();
        let subworkspaces = crate::contexts::Subworkspaces::default();
        let contents = Contents::default();
        let themes = crate::theme::themes();
        let declarations = Rc::new(RefCell::new(Declarations::default()));
        let plugin_settings = crate::config::plugins::Shared::default();
        let lists = Rc::new(RefCell::new(ListSource {
            profiles_dir: config_dir().join("profiles"),
            importers: Vec::new(),
        }));
        // Commands are declared before the application is built: its
        // plugins read the registry when they connect.
        let wiring = Wiring {
            registry: &registry,
            queue: Arc::default(),
            later: &later,
            dialogs: Arc::default(),
            resolved_view: Arc::new(Mutex::new(Value::Null)),
            values,
            themes: &themes,
            declarations: &declarations,
        };
        Shell::declare_commands(&wiring, &panels);
        // The param commands first: the menus take their titles.
        helios_values::register(&mut lock(&registry), wiring.values.clone())
            .expect("param commands are declared once");
        helios_controls::commands::register_with(
            &mut lock(&registry),
            wiring.values.clone(),
            crate::controls::registry().clone(),
            crate::controls::steps().clone(),
            controls.clone(),
        )
        .expect("controls commands are declared once");
        Shell::declare_menus(&wiring);
        let opens = crate::platform::Opens::default();
        let Wiring {
            queue,
            dialogs,
            resolved_view,
            values,
            ..
        } = wiring;
        let mut builder = App::builder()
            .name("Helios")
            .host(host)
            // The import focus opens an OS window of its own: on Wayland,
            // a present mode that never stalls every window behind one.
            .multi_window(true)
            // On macOS the menus are in the system's menu bar
            // (`crate::macos`), not drawn in the window.
            .menu_bar(!cfg!(target_os = "macos"))
            .themes(themes.clone())
            .theme(crate::theme::ThemeSpec::new(crate::theme::DEFAULT))
            .plugins(DefaultPlugins)
            // Photoshop's habits by default: the wheel over a slider scrolls the panel
            // and a slider takes it once clicked or focused (darktable's
            // pitfall of values changed by a scroll). A saved choice is
            // kept; the first run offers darktable's profile
            // (`pointer_choice`), and Settings › Input picks any.
            .pointer_profile(crate::pointer_choice::DEFAULT_PROFILE)
            .plugin(plugins::Commands {
                registry: registry.clone(),
                declarations: declarations.clone(),
            })
            .plugin(plugins::Lists {
                source: lists.clone(),
                i18n: i18n.clone(),
            })
            .plugin(plugins::History {
                values: values.clone(),
                i18n: i18n.clone(),
            })
            // egui-pop's History panel (the end side bar, closed until
            // asked for) and the agents' `history.*` commands, over
            // Helios's history owner (`crate::history::HeliosHistory`).
            .plugin(egui_pop::api::history_view::HistoryPlugin::default())
            // The Window menu and Close window: every window has them
            // (docs/design/windows.md, section 7). Its switcher (Switch
            // to window…) is not offered (`plugins::WindowMenu`).
            .plugin(egui_pop::api::windows::WindowsPlugin)
            .plugin(plugins::WindowMenu)
            // Copy and paste: egui-pop's clipboard (its default owner,
            // the Edit commands, the agents' `clipboard.*`), Helios's
            // types and sanitiser over it.
            .plugin(egui_pop::api::clipboard::ClipboardPlugin)
            .plugin(crate::clipboard::Types)
            // egui-pop's data views (the Masks panel's tree).
            .plugin(egui_pop::views::ViewsPlugin)
            // The photo library's steps and commands (the library panels).
            .plugin(egui_pop::views::library::LibraryPlugin)
            // The image tools' steps: the histogram's drag, curves, colour
            // wheels, the navigator.
            .plugin(egui_pop::imaging::ImagingPlugin)
            // The window's frame: Helios's own on the desktop, the
            // system's elsewhere (tests, pictures, tools) until asked.
            // On the desktop its title bar holds VS Code's layout
            // controls: Customize layout and a toggle per area.
            .plugin(if ui_settings.is_some() {
                FramePlugin::own().layout_controls(true)
            } else {
                FramePlugin::system()
            })
            .plugin(plugins::Frame {
                status: status.clone(),
            })
            // The jobs centre, its status bar item and each job's log
            // console as a terminal.
            .plugin(crate::jobs::plugin(jobs.clone(), relays.clone()))
            .plugin(egui_pop::editors::JobConsolesPlugin)
            .plugin(crate::display::Display)
            // egui-pop's Settings app (full pages, families on top; `Ctrl+,`
            // and File > Settings), with Helios's scopes and its own
            // settings (`helios-config`) in them. In `DefaultPlugins` too:
            // named here, it takes the group's place.
            .plugin(egui_pop::api::admin::AdminPlugin)
            .plugin(crate::config::BridgePlugin {
                i18n: Some(i18n.clone()),
                plugins: plugin_settings.clone(),
            })
            // egui-pop's theme editor, as a page of the Settings app, on
            // the theme in use.
            .plugin(egui_pop::api::theme_editor::ThemeEditorPlugin)
            .plugin(crate::theme_editor::Page)
            .plugin(crate::platform::Platform::new(opens.clone()))
            .plugin(crate::workbench::Panels {
                specs: panels.clone(),
                contents: contents.clone(),
            })
            // Places and modes (egui-pop's context stack), kept beside
            // the workspace's layout.
            .plugin(crate::contexts::stack(
                workspace.clone(),
                images.clone(),
                subworkspaces.clone(),
            ))
            .plugin(crate::contexts::Frames(
                workspace.clone(),
                subworkspaces.clone(),
            ))
            // egui-pop's status bar: the breadcrumb and the mode chips,
            // then the status items (Helios's by place, the jobs'); no
            // activity bar.
            .plugin(egui_pop::api::strips::StripsPlugin::status_only())
            // Help > About Helios.
            .plugin(egui_pop::api::about::AboutPlugin::new(about_info()))
            // Help > Tips… and the tips' card (egui-pop's), with Helios's
            // tips; they come by themselves once the desktop asks
            // (`Shell::tips_by_themselves`).
            .plugin(egui_pop::api::tips::TipsPlugin)
            .plugin(crate::tips::Tips)
            // What agents and scenarios read: the library, the values,
            // the jobs (`helios.library`, `helios.values`, `helios.jobs`).
            .plugin(crate::agent_state::AgentState {
                late: agent_state.clone(),
                values: values.clone(),
            });
        if let Some(dir) = ui_settings {
            builder = builder.persist(dir);
        }
        ShellBuilder {
            app: builder,
            i18n: i18n_plugin,
            parts: Parts {
                registry,
                lists,
                declarations,
                panels,
                contents,
                queue,
                themes,
                resolved_view,
                dialogs,
                values,
                snapshots,
                workspace,
                i18n,
                opens,
                jobs,
                library_history,
                status,
                relays,
                images,
                subworkspaces,
                agent_state,
                controls,
                plugin_settings,
            },
        }
    }

    /// The shell around egui-pop's application, once built.
    fn assemble(parts: Parts, app: App) -> Shell {
        let Parts {
            registry,
            lists,
            declarations,
            panels,
            contents,
            queue,
            themes,
            resolved_view,
            dialogs,
            values,
            snapshots,
            workspace,
            i18n,
            opens,
            jobs,
            library_history,
            status,
            relays,
            images: _,
            subworkspaces,
            agent_state,
            controls,
            plugin_settings,
        } = parts;
        if let Some(d) = app.handle().service(&DIALOGS) {
            let _ = dialogs.set((*d).clone());
        }
        // The viewport engine and its render cache: one per process, every
        // window's viewport renders on it.
        let engine = crate::viewport::engine();
        let viewport = focus::viewport_on(&engine);
        let library: Rc<RefCell<crate::library::Library>> = Rc::default();
        agent_state.set(library.clone(), viewport.clone());
        let system: crate::clipboard::system::Shared = Rc::new(RefCell::new(Box::new(
            crate::clipboard::system::Arboard::default(),
        )));
        let clip = crate::clipboard::Endpoints::register(
            app.handle(),
            values.clone(),
            viewport.clone(),
            library.clone(),
            system.clone(),
        );
        // The main window, the root viewport: always the first focus.
        let focus = focus::Focus::new(focus::Seed {
            window: egui::ViewportId::ROOT,
            contents,
            workspace,
            subworkspaces,
            library,
            viewport,
            controls,
        });
        let core = core::Core {
            app,
            registry,
            lists,
            declarations,
            panels,
            queue,
            seen: 0,
            theme: crate::theme::resolve(&themes, crate::theme::DEFAULT, &BTreeMap::new()),
            themes,
            theme_name: crate::theme::DEFAULT.into(),
            theme_overrides: BTreeMap::new(),
            installed: None,
            profiles_dir: config_dir().join("profiles"),
            profile_name: "default".into(),
            pending_import: None,
            machine: helios_profile::Machine::here(&config_dir().join("machine.toml")),
            loaded: None,
            resolved_view,
            stack_review: Rc::default(),
            duplicates: Rc::default(),
            imports: Vec::new(),
            no_library: None,
            health: health::HealthPage::new(plugin_settings),
            init: init_wizard::InitState::default(),
            storage: storage::StoragePage::default(),
            import_lines: std::sync::mpsc::channel(),
            exports: Vec::new(),
            workshop_runner: None,
            workshop_reviewer: None,
            export_lines: std::sync::mpsc::channel(),
            log: Rc::default(),
            image_files: BTreeMap::new(),
            dialogs,
            assume_yes: false,
            notices: Vec::new(),
            now: 0.0,
            engine,
            gpu: None,
            gpu_info: None,
            clip,
            system,
            copying: None,
            values,
            snapshots,
            still: false,
            i18n,
            opens,
            engine_seen: 0,
            fell_back: BTreeSet::new(),
            about_generation: None,
            jobs,
            library_history,
            status,
            ended: crate::jobs::Ended::default(),
            relays,
            forgotten: crate::jobs::Forgotten::default(),
            pointer_choice: false,
        };
        let mut shell = Shell {
            core,
            focus,
            import: import_window::ImportFocus::new(),
            calibration: calibrate_focus::CalibrationFocus::default(),
            lifetime: lifetime::Lifetime::default(),
            window_asks: None,
            toasts_in: None,
            notes: Vec::new(),
        };
        if let Err(e) = crate::pointer_choice::declare(shell.core.app.handle()) {
            shell.notes.push(format!("pointer choice: {e}"));
        }
        for d in shell.core.app.handle().diagnostics() {
            shell.notes.push(format!("{}: {}", d.code, d.message));
        }
        // The built-in templates' text, each under its own keys, once
        // the translation plugin has loaded Helios's catalogues.
        if let Err(e) = crate::templates::add_texts(&shell.core.i18n, &crate::templates::builtin())
        {
            shell.notes.push(e);
        }
        shell.core.seen = shell.core.app.handle().last_event();
        shell.core.ended = crate::jobs::Ended::after(shell.core.seen);
        shell.core.forgotten = crate::jobs::Forgotten::after(shell.core.seen);
        {
            let mut store = lock(&shell.core.values);
            crate::viewport::declare(&mut store);
            crate::mask_display::declare(&mut store);
            crate::masks::declare(&mut store);
            crate::calibrate::declare(&mut store);
            crate::graph::declare(&mut store);
            store.declare(
                crate::params::SELECTION,
                // Which node one looks at is not an edit: never undone.
                helios_values::Descriptor::text("selected node", "").transient(),
            );
            store.declare(
                macro_edit::SELECTED,
                // The nodes selected, several at once: never undone.
                helios_values::Descriptor::text("selected nodes", "[]").transient(),
            );
        }
        let has =
            |shell: &Shell, panel: &str| shell.core.panels.iter().any(|p| p.id.as_str() == panel);
        if has(&shell, "helios.params") {
            shell
                .focus
                .params
                .borrow_mut()
                .set_app(shell.core.app.handle().clone());
            let (params, viewport, values) = (
                shell.focus.params.clone(),
                shell.focus.viewport.clone(),
                shell.core.values.clone(),
            );
            shell.set_content("helios.params", move |ui| {
                let (selection, members) = {
                    let store = lock(&values);
                    let selection =
                        store
                            .get(crate::params::SELECTION)
                            .and_then(|e| match &e.value {
                                helios_core::Value::Text(t) if !t.is_empty() => Some(t.clone()),
                                _ => None,
                            });
                    // Which member each of a mask group's per-port values
                    // is for: the viewport's network says.
                    let members = viewport
                        .borrow()
                        .graph()
                        .map(|g| crate::mask_tree::members_by_port(g, &store))
                        .unwrap_or_default();
                    (selection, members)
                };
                params
                    .borrow_mut()
                    .panel_ui(ui, &values, selection.as_deref(), &members);
            });
        }
        if has(&shell, crate::workshops::PANEL) {
            let (panel, params, values) = (
                shell.focus.workshops.clone(),
                shell.focus.params.clone(),
                shell.core.values.clone(),
            );
            shell.set_content(crate::workshops::PANEL, move |ui| {
                panel.borrow_mut().ui(ui, &values, &mut params.borrow_mut());
            });
        }
        if has(&shell, crate::queue::PANEL) {
            let (panel, app) = (
                shell.focus.queue_panel.clone(),
                shell.core.app.handle().clone(),
            );
            shell.set_content(crate::queue::PANEL, move |ui| {
                panel.borrow_mut().ui(ui, &app);
            });
        }
        if has(&shell, "helios.viewport") {
            let (viewport, values, app) = (
                shell.focus.viewport.clone(),
                shell.core.values.clone(),
                shell.core.app.handle().clone(),
            );
            shell.set_content("helios.viewport", move |ui| {
                let area = ui.max_rect();
                let leave = {
                    let mut viewport = viewport.borrow_mut();
                    viewport.ui(ui, &values);
                    viewport.take_leave()
                };
                // While the image has the focus, the Edit commands copy
                // its mask's shapes and paste into it.
                if let Some(id) = crate::clipboard::focused_within(ui.ctx(), area) {
                    crate::clipboard::mask::focus(&app, crate::clipboard::MASK_IMAGE_ENDPOINT, id);
                }
                // The darktable profile's double click on the image: back
                // where the darkroom was opened from.
                if leave {
                    crate::contexts::leave_darkroom(&app, &egui_pop::model::commands::Origin::User);
                }
            });
        }
        if has(&shell, crate::graph::PANEL) {
            shell
                .focus
                .graph
                .borrow_mut()
                .set_app(shell.core.app.handle().clone());
            let (graph, viewport, values) = (
                shell.focus.graph.clone(),
                shell.focus.viewport.clone(),
                shell.core.values.clone(),
            );
            shell.set_content(crate::graph::PANEL, move |ui| {
                graph.borrow_mut().ui(ui, &values, &viewport);
            });
        }
        if has(&shell, crate::modules::PANEL) {
            let (viewport, values) = (shell.focus.viewport.clone(), shell.core.values.clone());
            let workshops = has(&shell, crate::workshops::PANEL);
            shell.set_content(crate::modules::PANEL, move |ui| {
                crate::modules::ui(ui, &viewport, &values, workshops);
            });
        }
        if has(&shell, crate::mask_tree::PANEL) {
            shell
                .focus
                .mask_tree
                .borrow_mut()
                .set_app(shell.core.app.handle().clone());
            let (tree, viewport, values, app) = (
                shell.focus.mask_tree.clone(),
                shell.focus.viewport.clone(),
                shell.core.values.clone(),
                shell.core.app.handle().clone(),
            );
            shell.set_content(crate::mask_tree::PANEL, move |ui| {
                let area = ui.max_rect();
                tree.borrow_mut().ui(ui, &viewport, &values);
                // While a row has the focus, the Edit commands copy the
                // mask's shapes (the selected one) and paste into it.
                if let Some(id) = crate::clipboard::focused_within(ui.ctx(), area) {
                    crate::clipboard::mask::focus(&app, crate::clipboard::MASKS_ENDPOINT, id);
                }
            });
        }
        if has(&shell, crate::navigator::PANEL) {
            let mut panel = crate::navigator::NavigatorPanel::default();
            let (viewport, preview, values, app) = (
                shell.focus.viewport.clone(),
                shell.focus.preview.clone(),
                shell.core.values.clone(),
                shell.core.app.handle().clone(),
            );
            shell.set_content(crate::navigator::PANEL, move |ui| {
                panel.ui(ui, &viewport, &preview, &values, &app);
            });
        }
        if has(&shell, "helios.scopes") {
            let (scopes, preview, values, app) = (
                shell.focus.scopes.clone(),
                shell.focus.preview.clone(),
                shell.core.values.clone(),
                shell.core.app.handle().clone(),
            );
            shell.set_content("helios.scopes", move |ui| {
                let mut scopes = scopes.borrow_mut();
                scopes.follow(&preview);
                scopes.ui_with(ui, Some((&app, &*values)));
            });
        }
        shell
            .focus
            .library
            .borrow_mut()
            .set_app(shell.core.app.handle().clone());
        if has(&shell, crate::log_panel::PANEL) {
            let log = shell.core.log.clone();
            shell.set_content(crate::log_panel::PANEL, move |ui| {
                log.borrow_mut().ui(ui);
            });
        }
        if has(&shell, crate::duplicates::PANEL) {
            let duplicates = shell.core.duplicates.clone();
            shell.set_content(crate::duplicates::PANEL, move |ui| {
                duplicates.borrow_mut().ui(ui);
            });
        }
        if has(&shell, crate::stack_review::PANEL) {
            let (review, library) = (shell.core.stack_review.clone(), shell.focus.library.clone());
            shell.set_content(crate::stack_review::PANEL, move |ui| {
                let show = review.borrow_mut().ui(ui, &library.borrow());
                if let Some(ids) = show {
                    library.borrow_mut().select_ids(&ids);
                }
            });
        }
        if has(&shell, crate::decisions::PANEL) {
            let decisions = shell.focus.decisions.clone();
            shell.set_content(crate::decisions::PANEL, move |ui| {
                decisions.borrow_mut().ui(ui);
            });
        }
        for (panel, view) in [
            ("helios.vault", 0),
            ("helios.filmstrip", 1),
            ("helios.compare", 2),
        ] {
            if has(&shell, panel) {
                let library = shell.focus.library.clone();
                let comparison = shell.focus.comparison.clone();
                shell.set_content(panel, move |ui| {
                    let mut library = library.borrow_mut();
                    match view {
                        0 => library.grid(ui),
                        1 => library.filmstrip(ui),
                        _ => library.compare_ui(ui, &mut comparison.borrow_mut()),
                    }
                });
            }
        }
        // A new layout's palettes open together at the end edge, not
        // cascaded over the image; a loaded layout keeps its own.
        let mut layout = shell.core.app.handle().layout();
        if let Some(lead) = crate::workbench::gather_palettes(&mut layout) {
            let notes = shell.core.app.handle().set_layout(layout);
            shell.notes.extend(notes);
            shell.focus.palettes_to_place = Some(lead);
        }
        settings_in_their_own_window(shell.core.app.handle());
        shell
    }

    /// A long job ended: said on the desktop when no Helios
    /// window has the focus and the `notifications.jobs` setting is on
    /// (see `platform::job_finished`); whether it was.
    pub fn job_finished(&self, end: &crate::platform::JobEnd) -> bool {
        let app = self.core.app.handle();
        let focused = crate::platform::window_focused(&app.egui());
        crate::platform::job_finished(app, end, focused, |m| self.text(m))
    }

    /// What later launches and `helios:` links asked to open since the
    /// last call (the window opens them).
    pub fn take_opens(&self) -> Vec<crate::platform::Open> {
        self.core.opens.take()
    }

    /// The egui-pop application (its handle: services, menus, history).
    pub fn app(&self) -> &App {
        &self.core.app
    }

    /// The parameters' clipboard endpoint (a command or a test pins the
    /// parameter it copies or pastes onto).
    pub fn clip_param(&self) -> &crate::clipboard::param::Endpoint {
        &self.core.clip.param
    }

    /// The graph's clipboard endpoint (a test pins the nodes it copies).
    pub fn clip_graph(&self) -> &crate::clipboard::nodes::Endpoint {
        &self.core.clip.graph
    }

    /// The masks' clipboard endpoint.
    pub fn clip_masks(&self) -> &crate::clipboard::mask::Endpoint {
        &self.core.clip.masks
    }

    /// Replaces the system clipboard's files and images (a test's
    /// [`crate::clipboard::system::Memory`]).
    pub fn set_system_clipboard(
        &mut self,
        backend: Box<dyn crate::clipboard::system::SystemClipboard>,
    ) {
        *self.core.system.borrow_mut() = backend;
    }

    /// Waits for the image "Copy the image" renders and puts it on the
    /// system clipboard (`None`: none was being rendered); a test's.
    pub fn finish_copying(&mut self, timeout: std::time::Duration) -> Option<Result<(), String>> {
        let rx = self.core.copying.take()?;
        let got = rx.recv_timeout(timeout).map_err(|e| e.to_string());
        Some(
            got.and_then(|r| r)
                .and_then(|image| self.put_copied(&image)),
        )
    }

    /// The rendered image, once it came: put on the system clipboard.
    fn copied_image(&mut self) {
        let Some(rx) = &self.core.copying else { return };
        let got = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the render stopped".to_string())
            }
        };
        self.core.copying = None;
        if let Err(e) = got.and_then(|image| self.put_copied(&image)) {
            self.warn(
                Message::new(
                    "gui-copy-image-failed",
                    "the image was not copied: { $error }",
                )
                .with("error", e),
            );
        }
    }

    fn put_copied(&mut self, image: &image::RgbaImage) -> Result<(), String> {
        self.core.system.borrow_mut().set_image(image)?;
        self.note(
            Message::new(
                "gui-copied-image",
                "copied the image ({ $width } × { $height })",
            )
            .with("width", image.width())
            .with("height", image.height()),
        );
        Ok(())
    }

    /// Renders the image the viewport shows on a thread of its own, for
    /// the system clipboard: the frame that sees it done puts it there.
    fn copy_image(&mut self) -> Result<Value, String> {
        if self.core.copying.is_some() {
            return Err("the image is being copied already".into());
        }
        let (engine, shown) = {
            let v = self.focus.viewport.borrow();
            (v.engine().clone(), v.shown().ok_or("no image is open")?)
        };
        let size = [shown.extent.width, shown.extent.height];
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("helios-copy-image".into())
            .spawn(move || {
                let _ = tx.send(crate::clipboard::rendered(&engine, &shown));
            })
            .map_err(|e| e.to_string())?;
        self.core.copying = Some(rx);
        Ok(json!({ "rendering": size }))
    }

    /// Saves the interface's settings and the user's keys (when built
    /// with a place for them).
    pub fn save(&self) -> Result<(), String> {
        self.core.app.save().map_err(|e| e.to_string())
    }

    /// The application closes (egui-pop's `App::exit`): the builder's
    /// `on_exit` functions, then the `app.exit` step, whose default saves
    /// the interface's settings, the user's keys and the layout.
    pub fn exit(&mut self) -> Result<(), String> {
        use egui_pop::api::steps::Outcome;
        // Agents write the workspace's files again from now on.
        self.release_workspace();
        match self.core.app.exit() {
            Outcome::Done(r) => r.map(|_| ()),
            Outcome::Vetoed { by, reason } => Err(format!("exit stopped by `{by}`: {reason}")),
        }
    }

    /// Declares a command on Helios's registry, and to the interface at
    /// once (egui-pop's `App::declare`): its menus, palette entry and keys
    /// follow, no plugin reconnected.
    /// The interface's texts, for a command's answer written on another
    /// thread.
    pub fn texts(&self) -> impl Fn(Message) -> String + Send + Sync + 'static {
        let i18n = self.core.i18n.clone();
        move |m| helios_i18n::message(&i18n, &m)
    }

    pub fn register(
        &mut self,
        spec: CommandSpec,
        handler: impl Fn(&Value, &Origin) -> Result<Value, String> + Send + Sync + 'static,
    ) -> Result<(), String> {
        let declared = plugins::declared(&spec, &self.core.declarations.borrow())
            .map_err(|e| e.to_string())?;
        lock(&self.core.registry).register(spec, handler)?;
        self.core
            .app
            .declare(&OwnerId::from(&plugins::COMMANDS), declared)
            .map_err(|e| e.to_string())
    }

    /// Every command Helios declares, by id.
    pub fn specs(&self) -> Vec<CommandSpec> {
        lock(&self.core.registry).specs().cloned().collect()
    }

    /// The commands egui-pop's workbench declares for Helios's panels
    /// with texts of their own (`<panel>.detach`, `<panel>.moveTo`...),
    /// run in the interface. A panel's `focus` and `toggle` commands are
    /// left out: they read texts egui-pop shares between every panel.
    pub fn panel_commands(&self) -> Vec<CommandSpec> {
        let owner = egui_pop::model::ids::OwnerId::from(&crate::workbench::PANELS);
        let ctx = self.core.app.handle().egui();
        self.core
            .app
            .handle()
            .commands()
            .into_iter()
            .filter(|(c, o)| {
                *o == owner && egui_pop::core::strings::panel_command(&ctx, c.id.as_str()).is_none()
            })
            .map(|(c, _)| CommandSpec::new(c.id.as_str(), &c.title, &c.category))
            .collect()
    }

    /// The panels declared.
    pub fn panels(&self) -> &[PanelSpec] {
        &self.core.panels
    }

    /// The panels a workspace template may name: Helios's, and those
    /// egui-pop's plugins declare (the jobs centre, `egui_pop.jobs`).
    pub fn template_panels(&self) -> Vec<PanelSpec> {
        let mut all = self.core.panels.clone();
        for (spec, owner) in self.core.app.handle().declared_panels() {
            if owner != OwnerId::FRAMEWORK && !all.iter().any(|p| p.id == spec.id) {
                all.push(spec);
            }
        }
        all
    }

    /// The last command runs, Helios's and the interface's, oldest first.
    pub fn history(&self) -> Vec<Run> {
        self.core
            .app
            .handle()
            .history()
            .into_iter()
            .map(|r| Run {
                id: r.id.to_string(),
                args: r.args,
                origin: to_helios(&r.origin),
                result: r.result,
            })
            .collect()
    }

    /// The image the scopes measure (working data, before the display
    /// transform): set by the viewport once it exists.
    pub fn set_scope_image(&mut self, image: helios_core::Image) {
        self.focus.scopes.borrow_mut().set_image(image);
    }

    /// The images the library shows (the vault's, in its order); the
    /// selection follows its image when it is still there.
    pub fn set_library(&mut self, entries: Vec<crate::library::Entry>) {
        self.focus.library.borrow_mut().set_entries(entries);
        // Their backups, read once for the whole Library.
        self.refresh_backups();
    }

    /// Where the library's images are on disk, by id: quick open opens
    /// them from there, and a copy of images names them.
    pub fn set_image_files(&mut self, files: impl IntoIterator<Item = (String, PathBuf)>) {
        self.core.image_files = files.into_iter().collect();
        self.focus.library.borrow_mut().set_files(
            self.core
                .image_files
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
    }

    /// Where thumbnails are sent, by image id, from any thread.
    pub fn library_thumbnails(&self) -> crate::library::ThumbnailSink {
        self.focus.library.borrow().thumbnails()
    }

    /// What the user changed in the library since the last call, for the
    /// host to apply to the vault.
    pub fn take_library_changes(&mut self) -> Vec<crate::library::Change> {
        self.focus.library.borrow_mut().take_changes()
    }

    /// The process's engine and its cache, which every window's viewport
    /// renders on, for renders made beside them (the Compare panel's head
    /// versions).
    pub fn render_engine(&self) -> Arc<helios_core::Engine> {
        self.core.engine.clone()
    }

    /// An image of the library is being opened in the viewport (`None`:
    /// its opening failed): until its render is complete, the viewport
    /// draws its large thumbnail (the cache's 1024 px entry), asked of
    /// the thumbnail pool now.
    pub fn open_from_library(&mut self, id: Option<&str>) {
        self.focus
            .viewport
            .borrow_mut()
            .await_stand_in(id.is_some());
        self.focus.opening = id.map(str::to_string);
        if let Some(id) = id {
            self.focus.library.borrow_mut().ask_large(id);
        }
    }

    /// Hands the viewport the large thumbnail of the image being opened,
    /// once it arrived.
    fn follow_opening(&mut self, ctx: &Egui) {
        let Some(id) = &self.focus.opening else {
            return;
        };
        let mut viewport = self.focus.viewport.borrow_mut();
        if !viewport.wants_stand_in() {
            self.focus.opening = None;
            return;
        }
        if let Some(texture) = self.focus.library.borrow_mut().take_large(ctx, id) {
            viewport.set_stand_in(texture);
            self.focus.opening = None;
        }
    }

    /// The library's state, to read.
    pub fn library(&self) -> std::cell::Ref<'_, crate::library::Library> {
        self.focus.library.borrow()
    }

    /// The rule editor open on a smart album's rules, if any.
    pub fn rule_editor(&self) -> Option<&crate::rule_editor::RuleEditor> {
        self.focus.rule_editor.as_ref()
    }

    /// The library's state, to show what it holds.
    pub fn library_mut(&self) -> std::cell::RefMut<'_, crate::library::Library> {
        self.focus.library.borrow_mut()
    }

    /// The window's comparison: its photos, the active one, its zoom and
    /// pan.
    pub fn comparison(&self) -> std::cell::Ref<'_, crate::comparison::Comparison> {
        self.focus.comparison.borrow()
    }

    /// Where an import's lines go, with its arguments (`library.import`'s),
    /// from any thread: its duplicates fill the Duplicates panel.
    pub fn duplicates_sink(&self) -> std::sync::mpsc::Sender<(Value, Value)> {
        self.core.duplicates.borrow().sink()
    }

    /// The duplicates imports found, to read.
    pub fn duplicates(&self) -> std::cell::Ref<'_, crate::duplicates::Duplicates> {
        self.core.duplicates.borrow()
    }

    /// The duplicates imports found, to answer them.
    pub fn duplicates_mut(&self) -> std::cell::RefMut<'_, crate::duplicates::Duplicates> {
        self.core.duplicates.borrow_mut()
    }

    /// The Stacks panel's proposals, to read.
    pub fn stack_review(&self) -> std::cell::Ref<'_, crate::stack_review::Review> {
        self.core.stack_review.borrow()
    }

    /// The Stacks panel's proposals, to show the vault's and answer them.
    pub fn stack_review_mut(&self) -> std::cell::RefMut<'_, crate::stack_review::Review> {
        self.core.stack_review.borrow_mut()
    }

    /// Shows a node's output in the viewport; the parameters of the
    /// graph's nodes become values (`node/<node>/<param>`), and a change to
    /// them renders again.
    pub fn set_view_source(
        &mut self,
        graph: helios_core::Graph,
        node: helios_core::NodeId,
        port: &str,
    ) -> Result<(), String> {
        {
            let mut store = lock(&self.core.values);
            // Another image: its values take the paths of the one before,
            // so that one's steps must not be undone into them, nor its
            // snapshots restored into them.
            store.clear_history();
            crate::edit_snapshots::lock(&self.core.snapshots).clear();
            for id in graph.ids() {
                helios_values::bind_node(&mut store, &graph, id)?;
            }
            store.take_dirty();
        }
        let mut viewport = self.focus.viewport.borrow_mut();
        viewport.set_source(graph, node, port)?;
        // Another image: its own decisions, the decision nodes of its
        // network waiting for a person until an edit's book says more.
        let mut decisions = self.focus.decisions.borrow_mut();
        decisions.set_book(helios_decision::Book::new());
        if let Some(g) = viewport.graph() {
            decisions.adopt(g);
        }
        decisions.refresh(viewport.graph());
        Ok(())
    }

    /// The open image's decisions and their cards, to read.
    pub fn decisions(&self) -> std::cell::Ref<'_, crate::decisions::Decisions> {
        self.focus.decisions.borrow()
    }

    /// The Decisions panel, to act as the user would (its requests are
    /// answered at the next frame).
    pub fn decisions_mut(&self) -> std::cell::RefMut<'_, crate::decisions::Decisions> {
        self.focus.decisions.borrow_mut()
    }

    /// The open image's edit's decisions (a loaded edit's book): their
    /// answers are applied to the graph shown, which renders again; the
    /// decision nodes the book does not know wait for a person.
    pub fn set_decisions(&mut self, book: helios_decision::Book) -> Result<(), String> {
        let mut viewport = self.focus.viewport.borrow_mut();
        let mut decisions = self.focus.decisions.borrow_mut();
        decisions.set_book(book);
        if let Some(replayed) = viewport.edit_graph(|graph| {
            decisions.adopt(graph);
            decisions.book().replay(graph).map(|_| ())
        }) {
            replayed?;
        }
        decisions.refresh(viewport.graph());
        Ok(())
    }

    /// Answers a decision of the open image through its book, as
    /// `origin` (recorded as who decided): the graph shown takes the
    /// answer and renders again, its card goes. The panel's buttons and
    /// the `decision.*` commands come here.
    pub fn answer_decision(
        &mut self,
        request: crate::decisions::Request,
        origin: &Origin,
    ) -> Result<Value, String> {
        use crate::decisions::Request;
        let who = match origin {
            Origin::User => "user".to_string(),
            Origin::Agent(id) | Origin::Script(id) | Origin::Remote(id) => id.clone(),
            Origin::Midi => "midi".to_string(),
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut viewport = self.focus.viewport.borrow_mut();
        let region = viewport.shown().map(|s| s.extent);
        let engine = viewport.engine().clone();
        let mut decisions = self.focus.decisions.borrow_mut();
        decisions.refresh(viewport.graph());
        let above: Vec<String> = match &request {
            Request::AcceptAbove { confidence } => decisions
                .above(*confidence)
                .into_iter()
                .map(String::from)
                .collect(),
            _ => Vec::new(),
        };
        let answered = viewport
            .edit_graph(|graph| {
                let book = decisions.book_mut();
                match request {
                    Request::Accept { node, option } => book
                        .accept(graph, &node, &who, option, now)
                        .map(|v| json!({ "node": node, "version": v })),
                    Request::Decide { node, values } => {
                        let inputs = match region {
                            Some(r) => helios_decision::input_keys(&engine, graph, &node, r)?,
                            None => Vec::new(),
                        };
                        book.decide(graph, &node, &who, values, inputs, now)
                            .map(|v| json!({ "node": node, "version": v }))
                    }
                    Request::AcceptAbove { .. } => {
                        let (mut accepted, mut failed) = (Vec::new(), Vec::new());
                        for node in above {
                            match book.accept(graph, &node, &who, 0, now) {
                                Ok(_) => accepted.push(node),
                                Err(e) => failed.push(json!({ "node": node, "error": e })),
                            }
                        }
                        Ok(json!({ "accepted": accepted, "failed": failed }))
                    }
                }
            })
            .ok_or_else(|| "no image is open: no decision to answer".to_string())?;
        decisions.refresh(viewport.graph());
        answered
    }

    /// Answers what the Decisions panel asked during the last frame, and
    /// makes its cards again from the book (an agent's answer removes
    /// its card here).
    fn follow_decisions(&mut self) {
        let requests = self.focus.decisions.borrow_mut().take_requests();
        for r in requests {
            if let Err(e) = self.answer_decision(r, &Origin::User) {
                self.warn(e);
            }
        }
        let viewport = self.focus.viewport.borrow();
        self.focus.decisions.borrow_mut().refresh(viewport.graph());
    }

    /// Gives the viewport the GPU path it draws with, on the render state
    /// the window (or a test harness) draws with: the process's one device
    /// ([`wgpu_setup`]), never one of the window's own. Installed on the
    /// next frame, in its context. The viewport's engine computes on that
    /// device from then on, and hands its results to the window there
    /// (device tiles: nothing read back to be uploaded again).
    pub fn attach_gpu(&mut self, rs: &eframe::egui_wgpu::RenderState) {
        debug_assert!(
            helios_gpu::shared::owns(&rs.instance),
            "the window opened a wgpu instance of its own: hand it wgpu_setup()"
        );
        self.core.gpu = Some(rs.clone());
        match helios_gpu::shared::opened() {
            Some((_, gpu)) if gpu.handles().device == rs.device => {
                self.focus.viewport.borrow().attach_gpu(gpu);
            }
            _ => log::warn!(
                "the window draws on another device than the process's GPU: \
                 the viewport computes on the CPU"
            ),
        }
    }

    /// The viewport's GPU path, once installed: `None` draws on the CPU.
    pub fn viewport_gpu(&self) -> Option<egui_pop::viewport::gpu::GpuInfo> {
        self.core.gpu_info
    }

    /// Follows the Wide-gamut preview setting with the image's own surface
    /// on Wayland: installed once the viewport's GPU path is (on `rs`, the
    /// main window's render state, with `window`'s handles: eframe's
    /// frame) the first frame the setting is on, taken down when it goes
    /// off. Called after [`Shell::ui`]; viewports follow on the next frame.
    pub fn follow_image_surface(
        &mut self,
        ctx: &Egui,
        rs: &eframe::egui_wgpu::RenderState,
        window: &(
             impl egui_pop::viewport::image_surface::raw_window_handle::HasWindowHandle
             + egui_pop::viewport::image_surface::raw_window_handle::HasDisplayHandle
         ),
    ) {
        use crate::display::SurfaceStep;
        let on = crate::display::wide_gamut(self.core.app.handle());
        match self
            .focus
            .image_surface
            .step(on, self.core.gpu_info.is_some())
        {
            SurfaceStep::Install => {
                let offer = crate::display::install_image_surface(ctx, rs, window);
                if let Some(why) = offer.unavailable {
                    log::info!(target: "helios", "no image surface, the image stays sRGB: {why}");
                }
            }
            SurfaceStep::Down => crate::display::take_down_image_surface(ctx, self.focus.window),
            SurfaceStep::Keep | SurfaceStep::Up => {}
        }
    }

    /// What the viewport's render threads did.
    pub fn viewport_stats(&self) -> Arc<crate::engine_source::Stats> {
        self.focus.viewport.borrow().stats.clone()
    }

    /// Every value widgets and agents edit (`param.*` commands): the
    /// process's one store, shared by every window.
    pub fn values(&self) -> &Arc<Mutex<helios_values::Store>> {
        &self.core.values
    }

    /// The open image's history snapshots (`crate::edit_snapshots`).
    pub fn snapshots(&self) -> &crate::edit_snapshots::Shared {
        &self.core.snapshots
    }

    /// The viewport: its egui-pop state, its readout, its scene.
    pub fn viewport(&self) -> std::cell::Ref<'_, crate::viewport::Viewport> {
        self.focus.viewport.borrow()
    }

    /// The Masks panel: its tree's state, the network it shows.
    pub fn mask_tree(&self) -> std::cell::Ref<'_, crate::mask_tree::MaskTree> {
        self.focus.mask_tree.borrow()
    }

    /// The viewport, to set its hooks.
    pub fn viewport_mut(&self) -> std::cell::RefMut<'_, crate::viewport::Viewport> {
        self.focus.viewport.borrow_mut()
    }

    /// The preview of what the viewport shows (the pipeline's output,
    /// small), which the scopes measure and the navigator draws.
    pub fn preview(&self) -> &crate::preview::Preview {
        &self.focus.preview
    }

    /// The image the scopes measure now.
    pub fn scope_image(&self) -> Option<helios_core::Image> {
        self.focus.scopes.borrow().image.clone()
    }

    /// The float data the viewport shows, before the display transform.
    pub fn viewport_scene(&self) -> Option<helios_core::Image> {
        self.focus.viewport.borrow().scene()
    }

    /// The layout as it stands, to be saved with the workspace.
    pub fn layout(&self) -> Layout {
        self.core.app.handle().layout()
    }

    /// Gives a panel its content; panels without one show a placeholder.
    pub fn set_content(&mut self, panel: &str, content: impl FnMut(&mut Ui) + 'static) {
        self.focus
            .contents
            .borrow_mut()
            .insert(panel.into(), Box::new(content));
    }

    /// Switches to a theme, with the user's overrides; problems (an
    /// unknown base, a wrong token) fall back and are noted, never fatal.
    pub fn set_theme(&mut self, name: &str, overrides: BTreeMap<String, serde_json::Value>) {
        self.core.theme_name = name.into();
        self.core.theme_overrides = overrides;
        // An unknown theme falls back to Helios's default, not to the
        // egui-pop base egui-pop itself would pick.
        let known = self.core.themes.get(name).is_some();
        if !known {
            self.warn(
                Message::new(
                    "gui-theme-unknown",
                    "theme { $name }: no theme `{ $name }`: { $default } is used instead",
                )
                .with("name", name)
                .with("default", crate::theme::DEFAULT),
            );
        }
        let resolved = if known { name } else { crate::theme::DEFAULT };
        self.core.theme =
            crate::theme::resolve(&self.core.themes, resolved, &self.core.theme_overrides);
        let mut spec = crate::theme::ThemeSpec::new(resolved);
        spec.overrides.clone_from(&self.core.theme_overrides);
        // What the user chose across themes stays: the text size, the
        // fonts (Settings › Appearance › Fonts) and the accent colour.
        let service = self.core.app.handle().theme();
        let kept = service.spec();
        spec.text_size = kept.text_size;
        spec.font_ui = kept.font_ui;
        spec.font_mono = kept.font_mono;
        spec.accent = kept.accent;
        service.set(spec);
        let problems: Vec<String> = self
            .core
            .theme
            .resolved()
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        for p in problems {
            self.warn(
                Message::new("gui-theme-problem", "theme { $name }: { $problem }")
                    .with("name", name)
                    .with("problem", p),
            );
        }
    }

    /// Measures what Helios's status bar items say, and publishes what
    /// changed.
    fn status_items(&mut self, ctx: &egui::Context) {
        let (images, picked, rejected) = self.focus.library.borrow().counts();
        let zoom = self
            .focus
            .viewport
            .borrow()
            .state()
            .mapping()
            .map(|m| m.zoom);
        let history = lock(&self.core.values).undo_steps().len();
        let measure = crate::status_items::Measure {
            images,
            picked,
            rejected,
            zoom,
            history,
        };
        self.focus
            .status_items
            .frame(self.core.app.handle(), ctx, &measure);
    }

    /// Takes a theme installed from elsewhere (the theme editor) as the
    /// one in use: its base and overrides become what a profile saves and
    /// what `theme.exportMerged` writes. The theme the shell installed
    /// itself, or an unknown name it fell back from, changes nothing.
    fn follow_theme(&mut self, spec: &crate::theme::ThemeSpec) {
        let ours = if self.core.themes.get(&self.core.theme_name).is_some() {
            self.core.theme_name.as_str()
        } else {
            crate::theme::DEFAULT
        };
        if spec.base == ours && spec.overrides == self.core.theme_overrides {
            return;
        }
        self.core.theme_name.clone_from(&spec.base);
        self.core.theme_overrides.clone_from(&spec.overrides);
        self.core.theme =
            crate::theme::resolve(&self.core.themes, &spec.base, &self.core.theme_overrides);
    }

    /// The name of the theme in use, as chosen.
    pub fn theme_name(&self) -> &str {
        &self.core.theme_name
    }

    /// The theme in use.
    pub fn theme(&self) -> &crate::theme::Tokens {
        &self.core.theme
    }

    /// Binds keys to a command in the user's keymap (what a profile
    /// carries), from the next frame.
    pub fn bind_user_keys(&mut self, keys: &str, command: &str) -> Result<(), String> {
        let id = CommandId::new(command).map_err(|e| e.to_string())?;
        self.core.app.handle().rebind(id, Some(keys.to_string()));
        self.core.app.process_requests();
        Ok(())
    }

    /// What the shell holds as a profile: theme, user keys, layout. The
    /// keys are the user's layer as egui-pop exports it
    /// (`AppHandle::export_keymap`): each binding with its `when`, and the
    /// default keys a rebinding took away (`-<command>`, no keys).
    pub fn profile(&self) -> helios_profile::Profile {
        use helios_profile::{Binding, Profile, System, ThemeChoice, portable_keys};
        let keymap = self
            .core
            .app
            .handle()
            .export_keymap(Layer::User)
            .bindings
            .into_iter()
            // A profile binds commands; keys unbound from every command
            // (no command) have no place in it yet.
            .filter_map(|b| {
                Some(Binding {
                    keys: portable_keys(&b.keys.to_lowercase(), System::current()),
                    command: b.command?,
                    when: b.when,
                })
            })
            .collect();
        Profile {
            name: self.core.profile_name.clone(),
            keymap: Some(keymap),
            theme: Some(ThemeChoice {
                name: self.core.theme_name.clone(),
                overrides: self.core.theme_overrides.clone(),
            }),
            layout: serde_json::from_str(&self.layout().to_json()).ok(),
            ..Profile::default()
        }
    }

    /// Applies a profile's parts the shell holds, resolved for this
    /// machine (its variants for this system, screen and machine applied);
    /// the others stay. `profile.resolved` says which values a variant set.
    pub fn apply_profile(&mut self, p: &helios_profile::Profile) {
        let machine = self.machine_now();
        let resolved = p.resolve(&machine);
        if !resolved.from.is_empty() {
            let from: Vec<String> = resolved
                .from
                .iter()
                .map(|f| {
                    let removed = if f.removed { " (removed)" } else { "" };
                    format!("{:?} {}{removed} from {}", f.part, f.key, f.selector)
                })
                .collect();
            self.note(
                Message::new(
                    "gui-profile-variants",
                    "profile { $name }: { $count } values from variants: { $variants }",
                )
                .with("name", &p.name)
                .with("count", from.len())
                .with("variants", from.join(", ")),
            );
        }
        *lock(&self.core.resolved_view) = json!({
            "profile": p.name,
            "machine": machine,
            "from": resolved.from,
        });
        self.apply_resolved(&resolved.profile);
        self.core.loaded = Some((p.clone(), resolved));
    }

    /// This machine, its screen as of the last frame.
    fn machine_now(&self) -> helios_profile::Machine {
        let mut m = self.core.machine.clone();
        m.hidpi |= self.focus.native_scale.unwrap_or(1.0) >= HIDPI;
        m
    }

    /// Sets this machine as profile variants see it (the host reads it
    /// from `machine.toml` in the config directory); applies at the next
    /// profile.
    pub fn set_machine(&mut self, machine: helios_profile::Machine) {
        self.core.machine = machine;
    }

    /// Applies a resolved profile's parts the shell holds.
    fn apply_resolved(&mut self, p: &helios_profile::Profile) {
        use helios_profile::{System, local_keys};
        self.core.profile_name.clone_from(&p.name);
        if let Some(t) = &p.theme {
            self.set_theme(&t.name, t.overrides.clone());
        }
        if let Some(keys) = &p.keymap {
            // The whole user layer, replaced through egui-pop's `keys.load`
            // step: scopes and `when` clauses kept, what changed said.
            let doc = KeymapDoc {
                bindings: keys
                    .iter()
                    .map(|b| DocBinding {
                        keys: local_keys(&b.keys, System::current()),
                        command: Some(b.command.clone()),
                        when: b.when.clone(),
                        ..DocBinding::default()
                    })
                    .collect(),
            };
            match self.core.app.load_keymap(&doc, Layer::User) {
                Ok(diff) => {
                    let changed = diff.added.len() + diff.removed.len() + diff.changed.len();
                    if changed > 0 {
                        self.note(
                            Message::new(
                                "gui-profile-keys-changed",
                                "profile { $name }: { $count } key bindings changed",
                            )
                            .with("name", &p.name)
                            .with("count", changed),
                        );
                    }
                    for s in diff.skipped {
                        self.warn(
                            Message::new(
                                "gui-profile-keys-skipped",
                                "profile { $name }: keys skipped: { $keys }",
                            )
                            .with("name", &p.name)
                            .with("keys", s),
                        );
                    }
                    for c in diff.conflicts {
                        self.warn(
                            Message::new(
                                "gui-profile-keys-conflict",
                                "profile { $name }: keys in conflict: { $keys }",
                            )
                            .with("name", &p.name)
                            .with("keys", c),
                        );
                    }
                }
                Err(e) => self.warn(
                    Message::new(
                        "gui-profile-keys-error",
                        "profile { $name }: keys: { $error }",
                    )
                    .with("name", &p.name)
                    .with("error", e),
                ),
            }
        }
        if let Some(layout) = &p.layout {
            match Layout::from_json(&layout.to_string()) {
                Ok(l) => self.set_layout(l),
                Err(e) => self.warn(
                    Message::new(
                        "gui-profile-layout-error",
                        "profile { $name }: layout: { $error }",
                    )
                    .with("name", &p.name)
                    .with("error", e),
                ),
            }
        }
    }

    /// Reads the workspace's sub-workspaces again when its state changed
    /// (an edit, an undo, an agent, another workspace opened), then
    /// leaves a sub-workspace shown that is gone, with a toast.
    fn follow_subworkspaces(&mut self) {
        let (seq, list) = {
            let store = lock(&self.core.values);
            let entry = store.get(WORKSPACE_STATE);
            let seq = entry.map(|e| e.seq);
            if self.focus.subworkspaces_seen == (true, seq) {
                (seq, None)
            } else {
                let list = entry.and_then(|e| match &e.value {
                    helios_core::Value::Text(t) => serde_json::from_str::<Value>(t)
                        .ok()
                        .and_then(|j| {
                            serde_json::from_value(j["template"]["subworkspaces"].clone()).ok()
                        })
                        .or(Some(Vec::new())),
                    _ => None,
                });
                (seq, Some(list))
            }
        };
        if let Some(list) = list {
            self.focus.subworkspaces_seen = (true, seq);
            self.focus.subworkspaces.set(list);
        }
        if let Some(id) =
            crate::contexts::leave_gone(self.core.app.handle(), &self.focus.subworkspaces)
        {
            self.warn(
                Message::new(
                    "gui-subworkspace-gone",
                    "The sub-workspace { $id } is gone: back to where it was entered from",
                )
                .with("id", id),
            );
        }
    }

    /// The open workspace's key (see `host::workspace_key`), if any.
    pub fn workspace(&self) -> Option<String> {
        self.focus.workspace.get()
    }

    /// Switches to a workspace directory: the layout as it stands is
    /// saved to the workspace left, then the one `dir` saved is loaded
    /// (egui-pop's `layout.save` and `layout.load` steps). A workspace
    /// with no layout saved yet keeps the layout as it is.
    pub fn switch_workspace(&mut self, dir: &Path) -> Result<(), String> {
        step_result(self.core.app.save_layout())?;
        self.focus.workspace.set(Some(workspace_key(dir)));
        self.load_layout()?;
        // Its sub-workspaces are read once its state is: until then a
        // restored one is kept, and left then if it is gone.
        self.focus.subworkspaces.set(None);
        self.focus.subworkspaces_seen = (false, None);
        // Its places and modes, kept beside its layout.
        egui_pop::api::contexts::reload(self.core.app.handle());
        Ok(())
    }

    /// Loads the open workspace's layout again (egui-pop's `layout.load`
    /// step), repaired to today's panels; the repairs are noted. With no
    /// layout saved yet the layout stays as it is.
    pub fn load_layout(&mut self) -> Result<(), String> {
        let loaded = step_result(self.core.app.load_layout())?;
        let repaired = loaded["repaired"].as_array().into_iter().flatten();
        self.notes
            .extend(repaired.filter_map(|n| n.as_str().map(String::from)));
        Ok(())
    }

    /// Arranges the panels as a layout says, repaired to today's panels
    /// (egui-pop's workbench); the repairs are noted. A layout saved by
    /// Helios before egui-pop's workbench reads the same
    /// (`Layout::from_json` migrates its area names).
    pub fn set_layout(&mut self, layout: Layout) {
        let notes = self.core.app.handle().set_layout(layout);
        self.notes.extend(notes);
    }

    /// Where profiles are read and written.
    pub fn set_profiles_dir(&mut self, dir: PathBuf) {
        self.core.lists.borrow_mut().profiles_dir.clone_from(&dir);
        self.core.profiles_dir = dir;
    }

    pub fn profile_name(&self) -> &str {
        &self.core.profile_name
    }

    fn profile_file(&self, name: &str) -> PathBuf {
        self.core
            .profiles_dir
            .join(format!("{name}.helios-profile"))
    }

    fn read_profile(path: &std::path::Path) -> Result<helios_profile::Profile, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        helios_profile::Profile::import(&text)
    }

    /// Uses a named profile, the parts it leaves out from the Default
    /// profile (`default.helios-profile`, when there is one).
    pub fn use_profile(&mut self, name: &str) -> Result<(), String> {
        let default = Self::read_profile(&self.profile_file("default")).unwrap_or_default();
        let p = if name == "default" {
            default.clone()
        } else {
            Self::read_profile(&self.profile_file(name))?
        };
        let mut p = p.over(&default);
        p.name = name.to_string();
        self.apply_profile(&p);
        Ok(())
    }

    /// Adds a theme (a user's file, a plugin's) to those that can be
    /// chosen; it is offered from the next start.
    pub fn add_theme(&mut self, theme: egui_pop::tokens::TokenFile) {
        self.core.themes.add(theme);
    }

    /// The workspace's name, for the window's title.
    pub fn set_title(&mut self, title: &str) {
        self.focus.title = title.into();
        // The Settings header names the main window's focus by it.
        crate::config::name_workspace(self.core.app.handle(), Some(title));
    }

    /// The window's title: the workspace, then Helios. The frame's title
    /// bar shows it.
    pub fn window_title(&self) -> String {
        if self.focus.title.is_empty() {
            "Helios".into()
        } else {
            format!("{} — Helios", self.focus.title)
        }
    }

    /// Draws the window's frame (title bar, buttons, resize edges) instead
    /// of the system's, or gives it back: egui-pop's `window.frame`
    /// setting, which the frame follows from the next frame.
    pub fn draw_frame(&mut self, on: bool) {
        use egui_pop::model::values::{Change, Set, Value as PopValue};
        let path = format!("settings/{SETTING_FRAME}");
        let choice = if on { "own" } else { "system" };
        let set = Set::new(
            &path,
            Change::To(PopValue::Choice(choice.into())),
            egui_pop::model::commands::Origin::User,
        )
        .untracked();
        let done = lock(&self.core.app.handle().store()).set(set).map(|_| ());
        if let Err(e) = done {
            self.warn(
                Message::new("gui-window-frame-error", "window frame: { $error }")
                    .with("error", e.to_string()),
            );
        }
    }

    /// The panel that has the focus.
    pub fn focused(&self) -> Option<String> {
        self.core
            .app
            .handle()
            .focused_panel()
            .map(|p| p.to_string())
    }

    pub fn palette_open(&self) -> bool {
        palette::is_open(self.core.app.handle())
    }

    /// Where a target was drawn at the last frame: `window`, a panel id or
    /// a name given by [`name_rect`]; `helios.dialog`, `helios.toasts` and
    /// `helios.menu` for what egui-pop and the shell show over the rest.
    pub fn target_rect(&self, target: &str) -> Result<Rect, String> {
        self.focus
            .rects
            .get(target)
            .copied()
            .ok_or_else(|| format!("nothing named `{target}` is shown"))
    }

    /// The window a target was drawn in at the last frame: the main one
    /// (the focus's window, `ViewportId::ROOT`) unless its panel is
    /// detached.
    pub fn target_window(&self, target: &str) -> egui::ViewportId {
        self.focus
            .rect_windows
            .get(target)
            .copied()
            .unwrap_or(self.focus.window)
    }

    /// The main window's physical pixels per point (a detached panel's
    /// window has its own: `ui.ctx().pixels_per_point()` where it draws).
    pub fn pixels_per_point(&self) -> f32 {
        self.focus.pixels_per_point
    }

    /// Runs a command as `origin`, in the context of the last frame. A
    /// command whose work waits for the next frame answers
    /// `{"queued": true}`; its result goes to the history and the bus.
    pub fn run(&mut self, id: &str, args: Value, origin: Origin) -> Result<Value, String> {
        let result = match CommandId::new(id) {
            Ok(c) => match self.core.app.run(&c, args, to_pop(&origin)) {
                Reply::Done(r) => r,
                Reply::Later(_) => Ok(json!({ "queued": true })),
            },
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = &result {
            let report = crate::report::Context::command(id).cause(e.clone());
            self.notify_failure(Level::Warning, e.clone(), report);
        }
        result
    }

    /// Answers the node endpoint's requests for the windows' commands at
    /// each frame (`window_commands::Bridge::attach`).
    pub fn serve_window_commands(&mut self, asks: crate::window_commands::Asks) {
        self.window_asks = Some(asks);
    }

    /// Runs a command as `origin` in a given context: what a window's
    /// native menu chose runs in that window's (`appWindow`), what it
    /// opens opening there.
    pub fn run_in(
        &mut self,
        id: &str,
        args: Value,
        origin: Origin,
        cx: &egui_pop::model::when::WhenContext,
    ) -> Result<Value, String> {
        let result = match CommandId::new(id) {
            Ok(c) => match self.core.app.run_in(&c, args, to_pop(&origin), cx) {
                Reply::Done(r) => r,
                Reply::Later(_) => Ok(json!({ "queued": true })),
            },
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = &result {
            let report = crate::report::Context::command(id).cause(e.clone());
            self.notify_failure(Level::Warning, e.clone(), report);
        }
        result
    }

    /// Opens the Settings on a page (`admin.open` on `path`: Storage, the
    /// health check). Popped in, the Settings are drawn over the main
    /// window only: asked from another window (the import window's
    /// Storage…), the main window comes to the front, as it does when the
    /// import window closes. In their own window, that window shows.
    pub(super) fn open_settings_page(&mut self, path: &str) -> Result<Value, String> {
        let asked = self.core.app.handle().current_window_group();
        let out = self.run(
            egui_pop::api::admin::CMD_OPEN.as_str(),
            json!({ "path": path }),
            Origin::User,
        )?;
        let app = self.core.app.handle();
        if asked != egui::ViewportId::ROOT && !egui_pop::api::admin::in_window(app) {
            let ctx = app.egui();
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        Ok(out)
    }

    fn declare_commands(w: &Wiring<'_>, panels: &[PanelSpec]) {
        let mut registry = lock(w.registry);
        let view = w.dialogs.clone();
        registry
            .register(
                CommandSpec::new(
                    "dialog.current",
                    "Describe the dialog waiting for an answer",
                    "Window",
                ),
                move |_, _| Ok(view.get().map_or(Value::Null, Dialogs::describe)),
            )
            .expect("shell commands are declared once");
        let view = w.resolved_view.clone();
        registry
            .register(
                CommandSpec::new(
                    "profile.resolved",
                    "Describe the profile in use as resolved for this machine: which values came from which variant",
                    "Preferences",
                ),
                move |_, _| Ok(lock(&view).clone()),
            )
            .expect("shell commands are declared once");
        let (q, later) = (w.queue.clone(), w.later.clone());
        let push = move |a: Action| lock(&q).push((a, later.take()));
        let mut add = |spec: CommandSpec, action: ActionOf| {
            let push = push.clone();
            registry
                .register(spec, move |args, origin| {
                    push(action(args, origin)?);
                    Ok(json!({"queued": true}))
                })
                .expect("shell commands are declared once");
        };
        add(
            CommandSpec::new(
                "dialog.answer",
                "Answer the dialog (`answer`: primary or cancel; `values` for a form; `choice` for a picker)",
                "Window",
            ),
            Box::new(|args, origin| {
                let answer = if let Some(c) = args.get("choice") {
                    Answer::Choice(match c {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                } else if let Some(v) = args.get("values") {
                    Answer::Values(v.clone())
                } else {
                    match args.get("answer").and_then(Value::as_str) {
                        Some("primary") => Answer::Primary,
                        Some("cancel") => Answer::Cancel,
                        _ => return Err("`answer` is primary or cancel".into()),
                    }
                };
                Ok(Action::Answer(answer, origin.clone()))
            }),
        );
        add(
            CommandSpec::new(
                "dialog.assumeYes",
                "Answer confirmations yes by policy (`on`)",
                "Window",
            ),
            Box::new(|a, _| {
                Ok(Action::AssumeYes(
                    a.get("on").and_then(Value::as_bool).unwrap_or(true),
                ))
            }),
        );
        add(
            CommandSpec::new(
                NOTICE_DETAILS,
                "Show a failure's details (`notice`: its index; the last failure by default)",
                "Window",
            ),
            Box::new(|a, _| {
                Ok(Action::NoticeDetails(
                    a.get("notice").and_then(Value::as_u64).map(|i| i as usize),
                ))
            }),
        );
        add(
            CommandSpec::new("workbench.systemMenu", "Show the Helios menu", "Window"),
            Box::new(|_, _| Ok(Action::SystemMenu)),
        );
        add(
            CommandSpec::new("workbench.run", "Run a command by id (`command`)", "Window"),
            Box::new(|a, origin| {
                a.get("command")
                    .and_then(Value::as_str)
                    .map(|c| Action::Run(c.into(), Value::Null, origin.clone()))
                    .ok_or_else(|| "`command` is missing".to_string())
            }),
        );
        add(
            CommandSpec::new("workbench.keybindings", "Keyboard shortcuts", "Preferences"),
            Box::new(|_, _| Ok(Action::Keybindings)),
        );
        add(
            CommandSpec::new("workbench.quit", "Quit Helios", "Window"),
            Box::new(|_, _| Ok(Action::Quit)),
        );
        add(
            CommandSpec::new(
                lifetime::QUIT_CONFIRMED,
                "Quit Helios, cancelling the running jobs",
                "Window",
            ),
            Box::new(|_, _| Ok(Action::QuitConfirmed)),
        );
        add(
            CommandSpec::new(lifetime::MAIN_WINDOW, "Main window", "Window"),
            Box::new(|_, _| Ok(Action::MainWindow)),
        );
        add(
            CommandSpec::new(
                "workspace.open",
                "Open a workspace in a new window (`path`)",
                "Workspace",
            ),
            Box::new(|a, _| {
                Ok(Action::OpenWorkspace(
                    a.get("path")
                        .and_then(Value::as_str)
                        .filter(|p| !p.is_empty())
                        .map(PathBuf::from),
                ))
            }),
        );
        add(
            CommandSpec::new(
                "workspace.new",
                "New workspace from a template (`template`, `path`)",
                "Workspace",
            ),
            Box::new(|a, _| {
                let text = |key: &str| {
                    a.get(key)
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                };
                Ok(Action::NewWorkspace(
                    text("template"),
                    text("path").map(PathBuf::from),
                    a.clone(),
                ))
            }),
        );
        for (id, title) in workspace_edit::COMMANDS
            .iter()
            .chain(network_edit::COMMANDS)
            .chain(macro_edit::COMMANDS)
        {
            let id = *id;
            add(
                CommandSpec::new(id, title, "Workspace"),
                Box::new(move |a, origin| {
                    Ok(Action::WorkspaceEdit(id.into(), a.clone(), origin.clone()))
                }),
            );
        }
        // Experimental: only with their panel (the switch on).
        if panels
            .iter()
            .any(|p| p.id.as_str() == crate::workshops::PANEL)
        {
            for (id, title) in workshop_edit::COMMANDS {
                let id = *id;
                add(
                    CommandSpec::new(id, title, crate::workshops::CATEGORY),
                    Box::new(move |a, origin| {
                        Ok(Action::WorkspaceEdit(id.into(), a.clone(), origin.clone()))
                    }),
                );
            }
        }
        add(
            CommandSpec::new("workbench.palette", "Show all commands", "View"),
            Box::new(|_, _| Ok(Action::Palette)),
        );
        add(
            CommandSpec::new(
                crate::quick_open::COMMAND,
                "Go to a module, a sub-workspace, an image or a workspace",
                "View",
            ),
            Box::new(|_, _| Ok(Action::QuickOpen)),
        );
        add(
            CommandSpec::new(
                crate::quick_open::GO,
                "Go to what quick open offers (`item`: module:<node>, subworkspace:<id>, image:<id> or workspace:<path>)",
                "View",
            ),
            Box::new(|a, _| {
                let item = a
                    .get("item")
                    .and_then(Value::as_str)
                    .ok_or("`item` is missing")?;
                crate::quick_open::Item::parse(item).map(Action::QuickOpenGo)
            }),
        );
        let string = |args: &Value, key: &str| -> Result<String, String> {
            args.get(key)
                .and_then(Value::as_str)
                .map(String::from)
                .ok_or_else(|| format!("`{key}` is missing"))
        };
        add(
            CommandSpec::new(
                "gui.snapshot",
                "Take a picture of a panel or a widget",
                "Developer",
            ),
            Box::new(move |a, _| {
                let target = string(a, "target").unwrap_or_else(|_| "window".into());
                let path =
                    string(a, "path").map_or_else(|_| default_path(&target, "png"), Into::into);
                Ok(Action::Snapshot { target, path })
            }),
        );
        add(
            CommandSpec::new("gui.record.start", "Start recording a video", "Developer"),
            Box::new(move |a, _| {
                let target = string(a, "target").unwrap_or_else(|_| "window".into());
                let path =
                    string(a, "path").map_or_else(|_| default_path(&target, "mp4"), Into::into);
                let fps = a.get("fps").and_then(Value::as_u64).unwrap_or(30) as u32;
                Ok(Action::RecordStart { target, path, fps })
            }),
        );
        add(
            CommandSpec::new("gui.record.stop", "Stop recording", "Developer"),
            Box::new(|_, _| Ok(Action::RecordStop)),
        );
        // `<panel>.focus` and `<panel>.toggle` are egui-pop's, declared
        // with each panel.
        for p in panels {
            let id = p.id.to_string();
            let move_to = id.clone();
            add(
                CommandSpec::new(
                    &format!("{id}.moveTo"),
                    &format!("Move {} to an area (`area`)", p.title),
                    "View",
                ),
                Box::new(move |a, _| {
                    let name = a
                        .get("area")
                        .and_then(Value::as_str)
                        .ok_or("`area` is missing: center, start_side, end_side or bottom")?;
                    let area = crate::workbench::area_named(name)
                        .ok_or_else(|| format!("`area`: no area `{name}`"))?;
                    Ok(Action::MoveTo(move_to.clone(), area))
                }),
            );
            let detach = id.clone();
            add(
                CommandSpec::new(
                    &format!("{id}.detach"),
                    &format!("Move {} to a new window", p.title),
                    "View",
                ),
                Box::new(move |_, _| Ok(Action::Detach(detach.clone()))),
            );
            if p.floatable {
                let float = id.clone();
                add(
                    CommandSpec::new(
                        &format!("{id}.float"),
                        &format!("Float {} over the interface (`pos`: [x, y])", p.title),
                        "View",
                    ),
                    Box::new(move |a, _| {
                        let pos = match a.get("pos") {
                            None | Some(Value::Null) => None,
                            Some(v) => Some(
                                serde_json::from_value::<[f32; 2]>(v.clone())
                                    .map_err(|e| format!("`pos`: {e}"))?,
                            ),
                        };
                        Ok(Action::Float(float.clone(), pos))
                    }),
                );
            }
            add(
                CommandSpec::new(
                    &format!("{id}.dock"),
                    &format!("Dock {} back in its area", p.title),
                    "View",
                ),
                Box::new(move |_, _| Ok(Action::Dock(id.clone()))),
            );
        }
        for (id, title, action) in [
            (
                "workbench.fullscreen",
                "Toggle full screen",
                (|| Action::Fullscreen) as fn() -> Action,
            ),
            ("window.minimize", "Minimize the window", || {
                Action::Minimize
            }),
            ("window.maximize", "Maximize or restore the window", || {
                Action::Maximize
            }),
            ("view.endSideBar.toggle", "Fold the end side bar", || {
                Action::Fold(Area::EndSide)
            }),
            ("view.bottomPanel.toggle", "Fold the bottom panel", || {
                Action::Fold(Area::Bottom)
            }),
        ] {
            add(
                CommandSpec::new(id, title, "View"),
                Box::new(move |_, _| Ok(action())),
            );
        }
        let names = crate::theme::choosable(w.themes);
        for name in names {
            let title = name
                .trim_start_matches("helios-")
                .split('-')
                .map(|w| {
                    let mut c = w.chars();
                    c.next().map_or_else(String::new, |f| {
                        f.to_uppercase().collect::<String>() + c.as_str()
                    })
                })
                .collect::<Vec<_>>()
                .join(" ");
            let picked = name.clone();
            add(
                CommandSpec::new(
                    &format!("theme.{name}"),
                    &format!("Theme: {title}"),
                    "Preferences",
                ),
                Box::new(move |_, _| Ok(Action::Theme(picked.clone()))),
            );
        }
        add(
            CommandSpec::new("theme.select", "Use a theme by name", "Preferences"),
            Box::new(move |a, _| {
                a.get("name")
                    .and_then(Value::as_str)
                    .map(|n| Action::Theme(n.to_string()))
                    .ok_or_else(|| "`name` is missing".to_string())
            }),
        );
        add(
            CommandSpec::new(
                "theme.exportMerged",
                "Export the theme in use, overrides merged",
                "Preferences",
            ),
            Box::new(move |a, _| {
                // By default next to the user's settings, where themes are read.
                let path = a.get("path").and_then(Value::as_str).map_or_else(
                    || config_dir().join("themes/exported.tokens.json"),
                    PathBuf::from,
                );
                Ok(Action::ThemeExport(path))
            }),
        );
        for (id, title, mode) in [
            (
                "scopes.histogram",
                "Scopes: histogram",
                crate::scopes::Mode::Histogram,
            ),
            (
                "scopes.waveform",
                "Scopes: waveform",
                crate::scopes::Mode::Waveform,
            ),
            (
                "scopes.parade",
                "Scopes: RGB parade",
                crate::scopes::Mode::Parade,
            ),
        ] {
            add(
                CommandSpec::new(id, title, "View"),
                Box::new(move |_, _| Ok(Action::ScopeMode(mode))),
            );
        }
        add(
            CommandSpec::new(
                "decision.list",
                "List the open image's decisions waiting for an answer: what each asks, the model's proposal, how sure it is and why",
                "Decisions",
            ),
            Box::new(|_, _| Ok(Action::DecisionList)),
        );
        add(
            CommandSpec::new(
                "decision.accept",
                "Accept a decision's proposal (`node`; `option`, 0 for the model's best)",
                "Decisions",
            ),
            Box::new(|a, origin| {
                let node = a
                    .get("node")
                    .and_then(Value::as_str)
                    .ok_or("`node` is missing")?;
                let option = a.get("option").and_then(Value::as_u64).unwrap_or(0);
                Ok(Action::Decision(
                    crate::decisions::Request::Accept {
                        node: node.into(),
                        option: usize::try_from(option).map_err(|e| e.to_string())?,
                    },
                    origin.clone(),
                ))
            }),
        );
        add(
            CommandSpec::new(
                "decision.decide",
                "Answer a decision, or correct its proposal (`node`; `values`: the answer's parameters, as plain values)",
                "Decisions",
            ),
            Box::new(|a, origin| {
                let node = a
                    .get("node")
                    .and_then(Value::as_str)
                    .ok_or("`node` is missing")?;
                let values = a.get("values").cloned().ok_or("`values` is missing")?;
                Ok(Action::DecisionDecide(node.into(), values, origin.clone()))
            }),
        );
        add(
            CommandSpec::new(
                "decision.acceptAbove",
                "Accept every proposal the model is at least this sure of (`confidence`, 0 to 1)",
                "Decisions",
            ),
            Box::new(|a, origin| {
                let confidence = a
                    .get("confidence")
                    .and_then(Value::as_f64)
                    .ok_or("`confidence` is missing")?;
                Ok(Action::Decision(
                    crate::decisions::Request::AcceptAbove { confidence },
                    origin.clone(),
                ))
            }),
        );
        add(
            CommandSpec::new(
                "vault.rate",
                "Rate the selected image (`stars`, 0 to 5)",
                "Library",
            ),
            Box::new(|a, _| {
                a.get("stars")
                    .and_then(Value::as_i64)
                    .map(Action::Rate)
                    .ok_or_else(|| "`stars` is missing".to_string())
            }),
        );
        for n in 0..=5 {
            let title = format!("Rate the selected image {n} stars");
            add(
                CommandSpec::new(&format!("vault.rate.{n}"), &title, "Library"),
                Box::new(move |_, _| Ok(Action::Rate(n))),
            );
        }
        add(
            CommandSpec::new(
                "vault.reject",
                "Reject the selected image, or take it back",
                "Library",
            ),
            Box::new(|_, _| Ok(Action::Reject)),
        );
        for label in crate::library::LABELS {
            let title = format!("Toggle the {label} label on the selected image");
            add(
                CommandSpec::new(&format!("vault.label.{label}"), &title, "Library"),
                Box::new(move |_, _| Ok(Action::Label(label.into()))),
            );
        }
        {
            use egui_pop::views::library::Flag;
            for (id, title, flag) in [
                ("vault.flag.pick", "Pick the selected images", Flag::Pick),
                (
                    "vault.flag.reject",
                    "Reject the selected images",
                    Flag::Reject,
                ),
                (
                    "vault.flag.none",
                    "Take the pick or reject off the selected images",
                    Flag::None,
                ),
            ] {
                add(
                    CommandSpec::new(id, title, "Library"),
                    Box::new(move |_, _| Ok(Action::Flag(flag))),
                );
            }
        }
        add(
            CommandSpec::new(
                "view.library",
                "Go to the library: its photos to browse, rate and cull",
                "View",
            ),
            Box::new(|_, _| Ok(Action::Place(crate::contexts::LIBRARY))),
        );
        add(
            CommandSpec::new(
                "view.network",
                "Go to the workspace's network, to edit it",
                "View",
            ),
            Box::new(|_, _| Ok(Action::Place(crate::contexts::WORKSPACE))),
        );
        add(
            CommandSpec::new(
                crate::comparison::COMMAND,
                "Compare photos side by side (`images`: ids, else the selection; two to four)",
                "Library",
            ),
            Box::new(|a, _| Ok(Action::ComparePhotos(a.clone()))),
        );
        add(
            CommandSpec::new(
                crate::library::SORT_COMMAND,
                "Sort the library (`by`: captured, imported, name, rating, edited, size or focus; \
                 `descending`); kept with the workspace. Without `by`, says how it is sorted",
                "Library",
            ),
            Box::new(|a, origin| Ok(Action::Sort(a.clone(), origin.clone()))),
        );
        add(
            CommandSpec::new(
                crate::library::FOLDER_COMMAND,
                "Scope the library to a folder and those below it (`path`, absolute; null: the whole library); kept with the workspace. Without `path`, says the folder",
                "Library",
            ),
            Box::new(|a, origin| Ok(Action::Folder(a.clone(), origin.clone()))),
        );
        add(
            CommandSpec::new(
                crate::library::COLLECTION_COMMAND,
                "Scope the library to an album (`id`; null: the whole library); kept with the workspace. Without `id`, says the album",
                "Library",
            ),
            Box::new(|a, origin| Ok(Action::Collection(a.clone(), origin.clone()))),
        );
        for (id, title) in [
            (
                crate::library::COLLECTION_CREATE_COMMAND,
                "New album (`name`; `images`: ids, else the selection), or with `kind: smart` a smart album (`rules`: a tree of groups (`match`: all, any or none; `rules`) and conditions (`field`, `op`, `value`), or the form's keys), or with `kind: set` an album set (`set`: the set it goes in); without `name`, asks it. Answers its `id`",
            ),
            (
                crate::library::COLLECTION_RENAME_COMMAND,
                "Rename an album (`id`, `name`); without `name`, asks it",
            ),
            (
                crate::library::COLLECTION_DELETE_COMMAND,
                "Delete an album or a set (`id`); its photos stay in the library. A set's albums move up into the set it was in, or with `members: true` are deleted too",
            ),
            (
                crate::library::COLLECTION_MOVE_COMMAND,
                "Move an album, a smart album or a set (`id`) into a set (`set`; null: the top); a set never goes inside itself. Without `set`, asks it",
            ),
            (
                crate::library::COLLECTION_ADD_COMMAND,
                "Add photos to an album (`id`; `images`: ids, else the selection)",
            ),
            (
                crate::library::COLLECTION_REMOVE_COMMAND,
                "Remove photos from an album (`id`; `images`: ids, else the selection); they stay in the library",
            ),
            (
                crate::library::COLLECTION_RULES_COMMAND,
                "Set a smart album's rules (`id`; `rules`, a tree, or the form's keys); without them, opens the rule editor on them. Unknown fields and invalid rules are refused with the reason",
            ),
            (
                crate::library::COLLECTION_LIST_COMMAND,
                "List the albums: `id`, `name`, `kind` (album, smart or set), the `set` it is in, how many photos each holds (a set: its albums' together), a smart album's `rules`; and the quick collection's `quick` count",
            ),
            (
                crate::library::QUICK_TOGGLE_COMMAND,
                "Put photos in the quick collection (`images`: ids, else the selection), or take them out when it holds them all; scope the library to it with the collection `quick`",
            ),
            (
                crate::library::QUICK_CLEAR_COMMAND,
                "Empty the quick collection; its photos stay in the library",
            ),
            (
                crate::library::QUICK_SAVE_COMMAND,
                "Save the quick collection as an album (`name`; `clear`: empty it too); without `name`, asks it. Answers the album's `id`",
            ),
        ] {
            add(
                CommandSpec::new(id, title, "Library"),
                Box::new(move |a, _| Ok(Action::CollectionEdit(id, a.clone()))),
            );
        }
        for (id, title) in [
            (
                crate::recent_filters::SWITCH_COMMAND,
                "Show the recent and pinned filters and the smart albums, to apply one to the library",
            ),
            (
                crate::recent_filters::RECENT_COMMAND,
                "List the library's `pinned` and `recent` filters (newest first) and its `smart` albums, each with its `query` and `filter`",
            ),
            (
                crate::recent_filters::APPLY_COMMAND,
                "Apply a filter to the library (`query`, typed as in the search field, or `filter` as listed); it becomes the first recent filter",
            ),
            (
                crate::recent_filters::PIN_COMMAND,
                "Pin a filter (`query` or `filter`, else the filter in force), or unpin it with `pinned: false`",
            ),
        ] {
            add(
                CommandSpec::new(id, title, "Library"),
                Box::new(move |a, _| Ok(Action::RecentFilters(id, a.clone()))),
            );
        }
        add(
            CommandSpec::new(
                crate::library::COLLECTION_RULES_EDIT_COMMAND,
                "Edit the rules in the open rule editor, one undoable edit at a time: `edit` (add_condition, add_group, remove, move with `to`, match with `match`: all, any or none, field with `field`, op with `op`, value with `value`, `high` (between's second bound, or within's count) and `unit` (days, weeks or months), undo, redo, apply, cancel) at `at`, a path of indexes from the top group ([]). A capture date takes a day (YYYY-MM-DD) or, with `op: within`, today, this_year or last. Answers the draft, each condition's fault and the rules when valid",
                "Library",
            ),
            Box::new(|a, _| {
                Ok(Action::CollectionEdit(
                    crate::library::COLLECTION_RULES_EDIT_COMMAND,
                    a.clone(),
                ))
            }),
        );
        add(
            CommandSpec::new(
                crate::library::SCAN_FOLDER_COMMAND,
                "Scan a folder and those below it for new or changed files (`path`), as the image map scans a volume",
                "Library",
            ),
            Box::new(|a, _| Ok(Action::ScanFolder(a.clone()))),
        );
        for (id, title, step) in [
            ("compare.next", "Compare: make the next photo active", 1),
            (
                "compare.previous",
                "Compare: make the previous photo active",
                -1,
            ),
        ] {
            add(
                CommandSpec::new(id, title, "Library"),
                Box::new(move |_, _| Ok(Action::CompareStep(step))),
            );
        }
        add(
            CommandSpec::new(
                "compare.sync",
                "Compare: zoom and pan synchronised across the photos (`on`; toggled when absent)",
                "Library",
            ),
            Box::new(|a, _| Ok(Action::CompareSync(a.get("on").and_then(Value::as_bool)))),
        );
        add(
            CommandSpec::new(
                "subworkspace.enter",
                "Enter a sub-workspace: its nodes only, shown and edited (`id`; without it, the one holding the selected node)",
                "View",
            ),
            Box::new(|a, origin| {
                let id = a.get("id").and_then(Value::as_str).map(String::from);
                Ok(Action::EnterSubworkspace(id, origin.clone()))
            }),
        );
        add(
            CommandSpec::new(
                "view.culling",
                "Culling: flag, rate and label from anywhere, the image first (`on`; toggled when absent)",
                "View",
            ),
            Box::new(|a, _| Ok(Action::Culling(a.get("on").and_then(Value::as_bool)))),
        );
        add(
            CommandSpec::new(
                "vault.autoAdvance",
                "Culling: move to the next image after each mark (`on`; toggled when absent)",
                "Library",
            ),
            Box::new(|a, _| Ok(Action::AutoAdvance(a.get("on").and_then(Value::as_bool)))),
        );
        add(
            CommandSpec::new(
                "vault.stack.toggle",
                "Expand or collapse the stack of the selected image",
                "Library",
            ),
            Box::new(|_, _| Ok(Action::ToggleStack)),
        );
        for (id, title) in [
            (
                crate::library::SELECT_NONE_COMMAND,
                "Select no photo, the cursor gone too",
            ),
            (
                crate::library::SELECT_INVERT_COMMAND,
                "Invert the selection: the photos shown that were not selected",
            ),
            (
                crate::library::SELECT_SIMILAR_COMMAND,
                "Select the photos shown like the selected ones (`by`: day, taken the same day, the default; or stack, in the same stack)",
            ),
            (
                crate::library::MARKS_COPY_COMMAND,
                "Copy a photo's marks: rating, colour labels, tags and title (`image`: an id, else the photo under the cursor). Answers them",
            ),
            (
                crate::library::REMOVE_COMMAND,
                "Remove from Library: take the photos out (`images`: ids, else the selection), their files untouched; Ctrl+Z puts them back",
            ),
            (
                crate::library::TRASH_COMMAND,
                "Move to Trash…: the photos' files to the system trash (`images`: ids, else the selection), asked first unless `confirmed`; refused on cards and backup volumes, the photos taken out of the Library; the system trash undoes it, not Ctrl+Z",
            ),
            (
                crate::library::DELETE_REJECTED_COMMAND,
                "Delete Rejected…: the rejected photos shown to the system trash (`images`: of them, those named), asked first unless `confirmed`",
            ),
            (
                crate::library::MARKS_PASTE_COMMAND,
                "Paste the copied marks (`images`: ids, else the selection; `marks`: some of rating, labels, tags, title, all by default; `mode`: merge, the default, adds labels and tags, overwrite makes each mark the copy's)",
            ),
        ] {
            add(
                CommandSpec::new(id, title, "Library"),
                Box::new(move |a, _| Ok(Action::LibraryBatch(id, a.clone()))),
            );
        }
        add(
            CommandSpec::new(
                "library.importSource",
                "Import photos… (`path`: a card or folder, asked when absent; `copy`, `darktable`, `remember` when the workspace has no import source)",
                "Library",
            ),
            Box::new(|a, origin| Ok(Action::ImportSource(a.clone(), origin.clone()))),
        );
        add(
            CommandSpec::new(
                storage::COMMAND,
                "Storage… (the Settings app at Storage › Volumes; `volume`: the volume to mark and scroll to)",
                "Library",
            ),
            Box::new(|a, _| Ok(Action::Storage(a.clone()))),
        );
        add(
            CommandSpec::new(health::COMMAND, "Health check…", "Help"),
            Box::new(|_, _| Ok(Action::Health)),
        );
        add(
            CommandSpec::new(
                health::CHECK_NOW,
                "Run the health checks now (Help > Health check)",
                "Help",
            ),
            Box::new(|_, _| Ok(Action::HealthCheckNow)),
        );
        add(
            CommandSpec::new(
                health::FIX,
                "Run a health finding's fix, then the checks again (`id`)",
                "Help",
            ),
            Box::new(|a, _| Ok(Action::HealthFix(a.clone()))),
        );
        add(
            CommandSpec::new(
                health::IGNORE,
                "Ignore a health finding, or show it again (`id`, `on`: true by default)",
                "Help",
            ),
            Box::new(|a, _| Ok(Action::HealthIgnore(a.clone()))),
        );
        add(
            CommandSpec::new(
                import_window::COMMAND,
                "Import photos… (`source`: the card or folder the import window opens on)",
                "Library",
            ),
            Box::new(|a, _| Ok(Action::ImportWindow(a.clone()))),
        );
        add(
            CommandSpec::new(
                import_auto::PAUSE,
                "Pause automatic imports, or resume them (`on`: true pauses, false resumes; without it, toggles)",
                "Library",
            ),
            Box::new(|a, _| Ok(Action::PauseAutomaticImports(a.clone()))),
        );
        add(
            CommandSpec::new(
                calibrate_focus::COMMAND,
                "Calibrate camera colour… (`path`: a chart shot; `corners`: its four corner patches' centres; `camera`: the profiles listed)",
                "Calibration",
            ),
            Box::new(|a, _| Ok(Action::Calibrate(a.clone()))),
        );
        for (id, about) in [
            (
                accounts_admin::ADD_SOURCE,
                "Add a source that needs connecting: opens the import window and runs the connector's connection steps (`connector`)",
            ),
            (
                accounts_admin::RECONNECT,
                "Connect an import account again, from the step that failed (`account`)",
            ),
            (
                accounts_admin::DISCONNECT,
                "Disconnect an import account (`account`): its token revoked where the service allows, its secrets forgotten; the saved sources using it are kept with it, not connected (`sources`: \"keep\") or forgotten with it (\"forget\"); an account no source uses is forgotten",
            ),
        ] {
            add(
                CommandSpec::new(id, about, "Library"),
                Box::new(move |a, _| Ok(Action::Account(id, a.clone()))),
            );
        }
        add(
            CommandSpec::new(
                "library.importFiles",
                "Import files and folders (`paths`), placed as the workspace's import source says (`copy`: into the library folder or not, `into`, `layout`, `rename`; `duplicates`; `ask`: asked without an import source; `darktable`; `camera_look`: false leaves the cameras' default looks off the raws)",
                "Library",
            ),
            Box::new(|a, origin| Ok(Action::ImportFiles(a.clone(), origin.clone()))),
        );
        add(
            CommandSpec::new(
                crate::importers::COMMAND,
                "Import with an importer plugin (`plugin`, `importer`; `source`: the file or folder it reads, asked when absent; `copy`, `darktable`)",
                "Library",
            ),
            Box::new(|a, origin| Ok(Action::ImportWith(a.clone(), origin.clone()))),
        );
        add(
            CommandSpec::new(
                import_history::LIST,
                "List the import history, newest first (`limit`): each import's sources, choices, counts, when and who asked",
                "Library",
            )
            .read_only(),
            Box::new(|a, _| Ok(Action::ImportHistory(a.clone()))),
        );
        add(
            CommandSpec::new(
                import_history::AGAIN,
                "Import again (`id`: an import of `import.history.list`): the same sources with the same choices; photos the library has are skipped as duplicates",
                "Library",
            ),
            Box::new(|a, origin| Ok(Action::ImportAgain(a.clone(), origin.clone()))),
        );
        // The main window's commands, greyed in the other windows' menus
        // (docs/design/windows.md, section 7).
        add(
            CommandSpec::new(
                "library.export",
                "Export… (`images`: ids, else `scope`: `selected` or `shown`, the selection by default, else every image shown; `format`: jpeg, png or tiff; `quality`, `bits`, `long_edge`, `space`, `folder`, `pattern`, `clash`: number, overwrite or skip; `remember`; asked when `folder` is absent or `ask` is true)",
                "Library",
            )
            .when(IN_MAIN_WINDOW),
            Box::new(|a, origin| Ok(Action::Export(a.clone(), origin.clone()))),
        );
        add(
            CommandSpec::new(
                export::OPEN_FOLDER,
                "Open a folder in the file manager (`path`)",
                "Library",
            ),
            Box::new(|a, _| Ok(Action::OpenFolder(a.clone()))),
        );
        add(
            CommandSpec::new(init_wizard::COMMAND, "Set up Helios…", "Helios"),
            Box::new(|_, _| Ok(Action::InitSetUp)),
        );
        add(
            CommandSpec::new(
                init_wizard::FOLDER_STEP,
                "Choose the library folder in the setup (`path`: set at once)",
                "Library",
            ),
            Box::new(|a, _| Ok(Action::InitLibraryFolder(a.clone()))),
        );
        add(
            CommandSpec::new(
                init_wizard::APPLY,
                "Write the setup's settings (`library.folder`: the library folder)",
                "Helios",
            ),
            Box::new(|a, _| Ok(Action::InitApply(a.clone()))),
        );
        add(
            CommandSpec::new(
                init_wizard::CHOOSE,
                "Choose the library folder for the setup",
                "Library",
            ),
            Box::new(|_, _| Ok(Action::InitChooseFolder)),
        );
        add(
            CommandSpec::new(
                "library.chooseFolder",
                "Choose the library folder imports copy photos into (`path`; asked through the folder chooser when absent)",
                "Library",
            ),
            Box::new(|a, _| Ok(Action::ChooseLibraryFolder(a.clone()))),
        );
        add(
            CommandSpec::new("vault.next", "Select the next image", "Library"),
            Box::new(|_, _| Ok(Action::Step(1))),
        );
        add(
            CommandSpec::new("vault.previous", "Select the previous image", "Library"),
            Box::new(|_, _| Ok(Action::Step(-1))),
        );
        // The system clipboard beyond text: files and images with other
        // applications (`crate::clipboard::system`).
        add(
            CommandSpec::new(
                "library.fitCameraLook",
                "Fit a camera look from the selection: one look from photos of one camera, fitted to their embedded JPEGs and written into each (`images`: ids, else the selection; `default`: keep it as the camera's default too)",
                "Library",
            )
            .when(IN_MAIN_WINDOW),
            Box::new(|a, origin| Ok(Action::FitCameraLook(a.clone(), origin.clone()))),
        );
        add(
            CommandSpec::new(
                "library.copyFiles",
                "Copy files: the selected images' files, for a file manager or another application",
                "Library",
            )
            .when(IN_MAIN_WINDOW),
            Box::new(|_, origin| Ok(Action::CopyFiles(origin.clone()))),
        );
        add(
            CommandSpec::new(
                "library.pasteFiles",
                "Paste files or an image another application copied into the library, imported once confirmed (`dry_run`: only say what would be imported)",
                "Library",
            ),
            Box::new(|a, origin| {
                let dry_run = a.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
                Ok(Action::PasteFiles(dry_run, origin.clone()))
            }),
        );
        add(
            CommandSpec::new(
                "viewport.copyImage",
                "Copy the image: the edited image rendered whole, for another application (as PNG)",
                "View",
            )
            .when(IN_MAIN_WINDOW),
            Box::new(|_, origin| Ok(Action::CopyImage(origin.clone()))),
        );
        // A parameter copied: the item is the parameter with its type,
        // its text the value as shown (`+0.30 EV`) or the path; either
        // pastes back (`param.paste`).
        for (id, title, text) in [
            (
                "param.copyPath",
                "Copy a value's path (`path`)",
                crate::clipboard::param::Text::Path,
            ),
            (
                "param.copyValue",
                "Copy a value (`path`)",
                crate::clipboard::param::Text::Value,
            ),
        ] {
            let values = w.values.clone();
            add(
                CommandSpec::new(id, title, "Values"),
                Box::new(move |a, origin| {
                    let path = string(a, "path")?;
                    if lock(&values).get(&path).is_none() {
                        return Err(format!("no value at `{path}`"));
                    }
                    Ok(Action::CopyParam(path, text, origin.clone()))
                }),
            );
        }
        add(
            CommandSpec::new(
                "param.paste",
                "Paste onto a value (`path`; `dry_run` says what it would set)",
                "Values",
            ),
            Box::new(move |a, origin| {
                let dry_run = a.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
                Ok(Action::PasteParam(
                    string(a, "path")?,
                    dry_run,
                    origin.clone(),
                ))
            }),
        );
        for (id, title, zoom) in [
            ("viewport.zoom.fit", "Image: fit to the view", ZoomTo::Fit),
            (
                "viewport.zoom.fill",
                "Image: fill the view (cropping the longer side)",
                ZoomTo::Fill,
            ),
            (
                "viewport.zoom.100",
                "Image: actual pixels (100 %, 1 image pixel = 1 screen pixel)",
                ZoomTo::Actual,
            ),
            ("viewport.zoom.in", "Image: zoom in", ZoomTo::In),
            ("viewport.zoom.out", "Image: zoom out", ZoomTo::Out),
        ] {
            add(
                CommandSpec::new(id, title, "View"),
                Box::new(move |_, _| Ok(Action::Zoom(zoom))),
            );
        }
        add(
            CommandSpec::new(
                "viewport.compare",
                "Image: before and after side by side",
                "View",
            ),
            Box::new(|_, _| Ok(Action::Compare)),
        );
        add(
            CommandSpec::new(
                crate::mask_display::COMMAND,
                "Image: show the mask (cycles overlay, grey, off; or `mode`)",
                "View",
            ),
            Box::new(|a, origin| {
                let mode = match a.get("mode") {
                    None | Some(Value::Null) => None,
                    Some(m) => Some(
                        m.as_str()
                            .and_then(crate::mask_display::Mode::parse)
                            .ok_or_else(|| format!("`mode`: {m} is not off, overlay or grey"))?,
                    ),
                };
                Ok(Action::MaskDisplay(mode, origin.clone()))
            }),
        );
        add(
            CommandSpec::new(
                crate::mask_display::COMMAND_OFF,
                "Image: stop showing the mask",
                "View",
            ),
            Box::new(|_, origin| {
                Ok(Action::MaskDisplay(
                    Some(crate::mask_display::Mode::Off),
                    origin.clone(),
                ))
            }),
        );
        add(
            CommandSpec::new(
                crate::modules::COMMAND,
                "Bypass a module or bring it back (`node`: the selected one by default; `on`: toggled by default)",
                "Node graph",
            ),
            Box::new(|a, origin| {
                let node = match a.get("node") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(n)) => Some(n.clone()),
                    Some(n) => return Err(format!("`node`: {n} is not a node's name")),
                };
                let on = match a.get("on") {
                    None | Some(Value::Null) => None,
                    Some(Value::Bool(b)) => Some(*b),
                    Some(v) => return Err(format!("`on`: {v} is not true or false")),
                };
                Ok(Action::Bypass(node, on, origin.clone()))
            }),
        );
        Shell::declare_mask_commands(&mut add);
        Shell::declare_member_commands(&mut add);
        add(
            CommandSpec::new("scopes.stops", "Scopes: in stops (log)", "View"),
            Box::new(|_, _| Ok(Action::ScopeStops(true))),
        );
        add(
            CommandSpec::new("scopes.linear", "Scopes: linear", "View"),
            Box::new(|_, _| Ok(Action::ScopeStops(false))),
        );
        add(
            CommandSpec::new(
                "profile.export",
                "Export the profile (all parts, or `parts`; `this_machine` flattens its variants)",
                "Preferences",
            ),
            Box::new(move |a, _| {
                let this_machine = a
                    .get("this_machine")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let parts = match a.get("parts") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(v) => {
                        serde_json::from_value(v.clone()).map_err(|e| format!("`parts`: {e}"))?
                    }
                };
                let path = a.get("path").and_then(Value::as_str).map_or_else(
                    || config_dir().join("profiles/exported.helios-profile"),
                    PathBuf::from,
                );
                Ok(Action::ProfileExport(path, parts, this_machine))
            }),
        );
        add(
            CommandSpec::new(
                "profile.import",
                "Read a profile and show what it would change",
                "Preferences",
            ),
            Box::new(move |a, _| {
                a.get("path")
                    .and_then(Value::as_str)
                    .map(|p| Action::ProfileImport(PathBuf::from(p)))
                    .ok_or_else(|| "`path` is missing".to_string())
            }),
        );
        add(
            CommandSpec::new(
                "profile.import.apply",
                "Apply the profile just read",
                "Preferences",
            ),
            Box::new(|_, _| Ok(Action::ProfileApply)),
        );
        for (id, title, on) in [
            (
                "keymap.package",
                "Add a keymap package's keys over the defaults (`name`: darktable-classic)",
                true,
            ),
            (
                "keymap.package.remove",
                "Take a keymap package's keys away (`name`)",
                false,
            ),
        ] {
            add(
                CommandSpec::new(id, title, "Preferences"),
                Box::new(move |a, _| {
                    a.get("name")
                        .and_then(Value::as_str)
                        .map(|n| Action::KeymapPackage(n.to_string(), on))
                        .ok_or_else(|| "`name` is missing".to_string())
                }),
            );
        }
        add(
            CommandSpec::new("profile.use", "Use a saved profile by name", "Preferences"),
            Box::new(move |a, _| {
                a.get("name")
                    .and_then(Value::as_str)
                    .map(|n| Action::ProfileUse(n.to_string()))
                    .ok_or_else(|| "`name` is missing".to_string())
            }),
        );
        drop(registry);
        let mut d = w.declarations.borrow_mut();
        // The default map, every key with its `when` (see `keys`).
        d.keys.extend(crate::keys::defaults());
        // egui-pop's palette is the palette; Helios's name for it stays
        // for scripts.
        d.hidden.insert("workbench.palette".into());
        d.hidden.insert(crate::quick_open::GO.into());
        d.icons.extend(
            crate::plugins::COMMAND_ICONS
                .iter()
                .map(|(id, icon)| ((*id).to_string(), (*icon).to_string())),
        );
        for p in panels {
            for (verb, icon) in crate::plugins::PANEL_COMMAND_ICONS {
                d.icons
                    .insert(format!("{}.{verb}", p.id), (*icon).to_string());
            }
        }
        for name in crate::theme::choosable(w.themes) {
            d.checked
                .insert(format!("theme.{name}"), format!("heliosTheme == '{name}'"));
            d.icons.insert(format!("theme.{name}"), "theme".into());
        }
    }

    /// The drawn-mask commands (`mask.*`): what the viewport's handles,
    /// clicks and keys do, for the palette and agents too.
    fn declare_mask_commands(add: &mut impl FnMut(CommandSpec, ActionOf)) {
        use crate::masks::Command;
        fn point(args: &Value) -> Result<[f64; 2], String> {
            let n = |k: &str| args.get(k).and_then(Value::as_f64);
            match (n("x"), n("y")) {
                (Some(x), Some(y)) => Ok([x, y]),
                _ => Err("`x` and `y` (image pixels) are missing".into()),
            }
        }
        fn index(args: &Value, key: &str) -> Option<usize> {
            args.get(key)
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
        }
        fn params(args: &Value) -> Result<helios_core::Params, String> {
            let mut p = helios_core::Params::new();
            let Some(map) = args.get("params").and_then(Value::as_object) else {
                return Ok(p);
            };
            for (k, v) in map {
                let v = match v {
                    Value::Number(n) => helios_core::Value::Float(n.as_f64().unwrap_or(0.0)),
                    Value::Bool(b) => helios_core::Value::Bool(*b),
                    Value::String(s) => helios_core::Value::Choice(s.clone()),
                    other => return Err(format!("`params.{k}`: {other} is not a value")),
                };
                p.set(k, v);
            }
            Ok(p)
        }
        let text =
            |args: &Value, key: &str| args.get(key).and_then(Value::as_str).map(String::from);
        add(
            CommandSpec::new(
                "mask.select",
                "Masks: select a mask, a shape of it, a path point (`mask`, `shape`, `point`)",
                "Masks",
            ),
            Box::new(move |a, origin| {
                Ok(Action::Mask(
                    Command::Select {
                        mask: text(a, "mask"),
                        shape: text(a, "shape"),
                        point: a.get("point").and_then(Value::as_i64),
                    },
                    origin.clone(),
                ))
            }),
        );
        for (kind, title) in [
            ("circle", "Masks: add a circle at the view's centre"),
            ("ellipse", "Masks: add an ellipse at the view's centre"),
            ("gradient", "Masks: add a gradient across the view's centre"),
            (
                "path",
                "Masks: draw a path (click its points; with `corners`, made at once)",
            ),
            (
                "parametric",
                "Masks: add a parametric member, reading the masked module's input",
            ),
        ] {
            add(
                CommandSpec::new(&format!("mask.add.{kind}"), title, "Masks"),
                Box::new(move |a, origin| {
                    let corners = match a.get("corners") {
                        None | Some(Value::Null) => Vec::new(),
                        Some(v) => serde_json::from_value(v.clone())
                            .map_err(|e| format!("`corners` ([[x, y], ...]): {e}"))?,
                    };
                    Ok(Action::Mask(
                        Command::Add {
                            kind: kind.into(),
                            params: params(a)?,
                            corners,
                        },
                        origin.clone(),
                    ))
                }),
            );
        }
        add(
            CommandSpec::new(
                "mask.paste",
                "Masks: paste copied shapes into the mask edited, scaled to the image (`clip`: a `helios.mask` payload)",
                "Masks",
            ),
            Box::new(|a, origin| {
                Ok(Action::Mask(
                    Command::Paste {
                        clip: a.get("clip").cloned().unwrap_or(Value::Null),
                    },
                    origin.clone(),
                ))
            }),
        );
        add(
            CommandSpec::new(
                "mask.path.close",
                "Masks: close the path being drawn",
                "Masks",
            ),
            Box::new(|_, origin| Ok(Action::Mask(Command::Close, origin.clone()))),
        );
        add(
            CommandSpec::new("mask.cancel", "Masks: stop drawing the path", "Masks"),
            Box::new(|_, origin| Ok(Action::Mask(Command::Cancel, origin.clone()))),
        );
        add(
            CommandSpec::new(
                "mask.point.add",
                "Masks: add a point to the selected path, on its edge nearest `x`, `y`",
                "Masks",
            ),
            Box::new(|a, origin| {
                Ok(Action::Mask(
                    Command::PointAdd { at: point(a)? },
                    origin.clone(),
                ))
            }),
        );
        add(
            CommandSpec::new(
                "mask.point.remove",
                "Masks: remove the selected path point (or `point`)",
                "Masks",
            ),
            Box::new(|a, origin| {
                Ok(Action::Mask(
                    Command::PointRemove {
                        point: index(a, "point"),
                    },
                    origin.clone(),
                ))
            }),
        );
        add(
            CommandSpec::new(
                "mask.point.smooth",
                "Masks: make the selected path point (or `point`) smooth, or sharp with `smooth: false`",
                "Masks",
            ),
            Box::new(|a, origin| {
                Ok(Action::Mask(
                    Command::PointSmooth {
                        point: index(a, "point"),
                        smooth: a.get("smooth").and_then(Value::as_bool).unwrap_or(true),
                    },
                    origin.clone(),
                ))
            }),
        );
        add(
            CommandSpec::new(
                "mask.handle.move",
                "Masks: move a shape's handle (`handle`: centre, radius, feather, corner.2...) to `x`, `y`, one undo step",
                "Masks",
            ),
            Box::new(move |a, origin| {
                Ok(Action::Mask(
                    Command::Move {
                        shape: text(a, "shape"),
                        handle: text(a, "handle").ok_or("`handle` is missing")?,
                        to: point(a)?,
                    },
                    origin.clone(),
                ))
            }),
        );
    }

    /// The commands on the members of a mask (`mask.member.*`): what the
    /// Masks panel does, for the palette, its context menu and agents.
    fn declare_member_commands(add: &mut impl FnMut(CommandSpec, ActionOf)) {
        use crate::masks::Command;
        let text =
            |args: &Value, key: &str| args.get(key).and_then(Value::as_str).map(String::from);
        // `member`, else the row of the Masks panel its menu opened on.
        let member = crate::mask_tree::row_member;
        add(
            CommandSpec::new(
                "mask.member.move",
                "Masks: move a member (`member`, the selected one by default) to `index` of `group` (its own, last, by default); rewires the network, no undo step",
                "Masks",
            ),
            Box::new(move |a, origin| {
                Ok(Action::Mask(
                    Command::MemberMove {
                        member: member(a),
                        group: text(a, "group"),
                        index: a
                            .get("index")
                            .and_then(Value::as_u64)
                            .and_then(|n| usize::try_from(n).ok()),
                    },
                    origin.clone(),
                ))
            }),
        );
        add(
            CommandSpec::new(
                "mask.member.delete",
                "Masks: take a member (`member`, the selected one by default) out of the mask; rewires the network, no undo step",
                "Masks",
            ),
            Box::new(move |a, origin| {
                Ok(Action::Mask(
                    Command::MemberDelete {
                        member: member(a),
                        // Answered yes: a group with members goes unasked.
                        confirmed: a.get("confirmed").and_then(Value::as_bool) == Some(true),
                    },
                    origin.clone(),
                ))
            }),
        );
        add(
            CommandSpec::new(
                "mask.member.invert",
                "Masks: invert a member (`member`, the selected one by default), or back",
                "Masks",
            ),
            Box::new(move |a, origin| {
                Ok(Action::Mask(
                    Command::MemberInvert { member: member(a) },
                    origin.clone(),
                ))
            }),
        );
        add(
            CommandSpec::new(
                "mask.member.opacity",
                "Masks: set a member's (`member`, the selected one by default) `opacity`, 0 to 1; without it, off or back to full",
                "Masks",
            ),
            Box::new(move |a, origin| {
                Ok(Action::Mask(
                    Command::MemberOpacity {
                        member: member(a),
                        opacity: a.get("opacity").and_then(Value::as_f64),
                    },
                    origin.clone(),
                ))
            }),
        );
        add(
            CommandSpec::new(
                "mask.member.rename",
                "Masks: name a member (`member`, the selected one by default) `label`; without it, rename in the Masks panel",
                "Masks",
            ),
            Box::new(move |a, origin| match text(a, "label") {
                Some(label) => Ok(Action::Mask(
                    Command::MemberRename {
                        member: member(a),
                        label,
                    },
                    origin.clone(),
                )),
                None => Ok(Action::RenameInTree(member(a))),
            }),
        );
        add(
            CommandSpec::new(
                "mask.member.lock",
                "Masks: lock a member (`member`, the selected one by default) against moves and edits, or unlock it; `locked` sets it",
                "Masks",
            ),
            Box::new(move |a, origin| {
                Ok(Action::Mask(
                    Command::MemberLock {
                        member: member(a),
                        locked: a.get("locked").and_then(Value::as_bool),
                    },
                    origin.clone(),
                ))
            }),
        );
    }

    /// The menus Helios contributes: its system menu, and its commands in
    /// egui-pop's menu bar.
    fn declare_menus(w: &Wiring<'_>) {
        use crate::mask_tree::{ROW_MENU, ROW_MENU_ADD};
        use egui_pop::api::app::{MENU_EDIT, MENU_FILE, MENU_HELP};
        use egui_pop::api::windows::MENU_WINDOW;
        // No label: egui-pop shows the command's title in the interface's
        // language, without its argument hint.
        let command = |menu: MenuId, group: &str, order: i32, id: &str| {
            PopItem::command(
                menu,
                group,
                order,
                CommandId::new(id).expect("Helios's command ids are valid"),
            )
        };
        let mut items = vec![
            command(SYSTEM, "1_workspace", 0, "workspace.open"),
            PopItem::submenu(SYSTEM, "1_workspace", 1, SYSTEM_RECENT, "Recent workspaces"),
            PopItem::submenu(SYSTEM, "2_preferences", 0, SYSTEM_PROFILES, "Profiles"),
            PopItem::submenu(SYSTEM, "2_preferences", 1, SYSTEM_THEMES, "Themes"),
            command(SYSTEM, "2_preferences", 2, "workbench.keybindings"),
            // The first-run setup, again.
            command(SYSTEM, "2_preferences", 3, init_wizard::COMMAND),
            // egui-pop's About (`about::AboutPlugin`), also in Help.
            command(SYSTEM, "4_about", 0, "about"),
            command(SYSTEM, "9_quit", 0, "workbench.quit"),
            command(SYSTEM_PROFILES, "2", 0, "profile.export"),
            command(SYSTEM_PROFILES, "2", 1, "profile.import"),
            command(SYSTEM_THEMES, "2", 0, "theme.exportMerged"),
            // A parameter's context menu: its widget gives the path.
            command(plugins::PARAM, "1", 0, "param.reset"),
            command(plugins::PARAM, "2", 0, "param.copyPath"),
            command(plugins::PARAM, "2", 1, "param.copyValue"),
            command(plugins::PARAM, "2", 2, "param.paste"),
            // The same commands in the menu bar.
            // Importing, always within reach (Ctrl+Shift+I, as Lightroom).
            command(MENU_FILE, "1_import", 0, import_window::COMMAND),
            // File > Close window (Ctrl+W), in every window: the import
            // window's own File entry once, now beside Quit everywhere.
            command(MENU_FILE, "9_close", 0, "window.close"),
            // Window: Helios's windows, open or not, to bring one forward.
            command(MENU_WINDOW, "1_open", 0, lifetime::MAIN_WINDOW),
            command(MENU_WINDOW, "1_open", 1, import_window::COMMAND),
            command(
                MENU_WINDOW,
                "1_open",
                2,
                egui_pop::api::admin::CMD_OPEN.as_str(),
            ),
            // File > Storage: the disks and what each is for.
            command(MENU_FILE, "1_import", 3, storage::COMMAND),
            // Help > Health check.
            command(MENU_HELP, "1_health", 0, health::COMMAND),
            // An entry per importer plugin, the submenu shown only when
            // one is installed (`crate::importers`).
            PopItem::submenu(
                MENU_FILE,
                "1_import",
                1,
                crate::importers::MENU,
                "Import from",
            )
            .when(crate::importers::CONTEXT, false),
            // Exporting, beside it (Ctrl+Shift+E, as Lightroom).
            command(MENU_FILE, "1_import", 2, "library.export"),
            command(MENU_FILE, "1_workspace", 0, "workspace.open"),
            // The workspace's own edits.
            command(MENU_FILE, "2_edit", 0, "workspace.edit"),
            command(MENU_FILE, "2_edit", 1, "workspace.library"),
            command(MENU_FILE, "2_edit", 2, "workspace.importParts"),
            command(MENU_FILE, "2_edit", 3, "workspace.saveAsTemplate"),
            command(MENU_FILE, "2_edit", 4, "workspace.reset"),
            // With other applications, after egui-pop's Cut, Copy, Paste
            // and Paste special.
            command(MENU_EDIT, "1_clipboard", 10, "library.copyFiles"),
            command(MENU_EDIT, "1_clipboard", 11, "viewport.copyImage"),
            command(MENU_EDIT, "1_clipboard", 12, "library.pasteFiles"),
        ];
        // A row's context menu in the Masks panel: its commands act on
        // that row's member, the tree giving its key. Its entries are
        // lists (`plugins::Lists`), for their short, translated labels.
        items.push(PopItem::submenu(ROW_MENU, "2_add", 0, ROW_MENU_ADD, "Add"));
        let select = CommandId::from_static("theme.select");
        for (i, name) in crate::theme::choosable(w.themes).iter().enumerate() {
            items.push(
                PopItem::command(SYSTEM_THEMES, "1", i as i32, select.clone())
                    .args(json!({ "name": name }))
                    .label(name)
                    .checked(&format!("heliosTheme == '{name}'")),
            );
        }
        w.declarations.borrow_mut().menus = items;
    }

    // --- Dialogs, menus and notices ---

    /// A message in the shell's language, else its English.
    pub fn text(&self, m: impl Into<Message>) -> String {
        helios_i18n::message(&self.core.i18n, &m.into())
    }

    /// Shows a notice, in the shell's language: the status bar always, a
    /// toast unless it is plain information. Kept in `notes` too, and
    /// written to the log file in English (target `helios`).
    /// An error offers its details (see [`Shell::notify_failure`]).
    pub fn notify(&mut self, level: Level, text: impl Into<Message>) {
        let message = text.into();
        crate::log_panel::write(level, &message);
        self.notify_unlogged(level, message);
    }

    /// [`Shell::notify`] for a message the Log panel has already taken,
    /// and so written to the log file.
    pub(crate) fn notify_unlogged(&mut self, level: Level, message: Message) {
        let report = (level == Level::Error)
            .then(|| crate::report::Context::default().cause(message.english.clone()));
        self.notify_with(level, message, report);
    }

    /// Shows a notice about a failure: its toast offers "Details", an
    /// error dialog whose Copy button gives a bug report (`report`).
    pub fn notify_failure(
        &mut self,
        level: Level,
        text: impl Into<Message>,
        report: crate::report::Context,
    ) {
        let message = text.into();
        crate::log_panel::write(level, &message);
        self.notify_with(level, message, Some(report));
    }

    fn notify_with(
        &mut self,
        level: Level,
        message: Message,
        report: Option<crate::report::Context>,
    ) {
        let text = self.text(message.clone());
        self.notes.push(text.clone());
        let index = self.core.notices.len();
        let details = report
            .as_ref()
            .map(|_| self.text(Message::new("gui-notice-details", "Details")));
        let toast = match level {
            Level::Info => None,
            Level::Warning => Some(Toast::warning(&text)),
            Level::Error => Some(Toast::error(&text)),
        };
        let toast = match (toast, &details) {
            (Some(t), Some(label)) => Some(t.action(
                label,
                CommandId::from_static(NOTICE_DETAILS),
                json!({ "notice": index }),
            )),
            (t, _) => t,
        };
        if let (Some(t), Some(toasts)) = (toast, self.core.app.handle().service(&TOASTS)) {
            match self.toasts_in {
                Some(group) => toasts.notify_in(group, t),
                None => toasts.notify(t),
            };
        }
        self.core.notices.push(Notice {
            level,
            text,
            key: message.key,
            action: details.map(|label| (label, NOTICE_DETAILS.to_string())),
            at: self.core.now,
            report,
        });
    }

    /// Opens a notice's details (the last failure's by default) as an
    /// error dialog: its text, and the report its Copy button adds.
    fn notice_details(&mut self, index: Option<usize>) {
        let notice = match index {
            Some(i) => self.core.notices.get(i),
            None => self.core.notices.iter().rev().find(|n| n.report.is_some()),
        };
        let Some((text, Some(report))) = notice.map(|n| (n.text.clone(), n.report.clone())) else {
            return self.warn(Message::new(
                "gui-notice-no-details",
                "No failure to show the details of",
            ));
        };
        let title = self.text(Message::new("gui-notice-failed", "Something went wrong"));
        // Copied whole: no secret, the home folder written `~`, as in
        // the details.
        let system = crate::report::System::current();
        let text = crate::report::redact(&text, &system);
        self.open_dialog(dialogs::Info::error(&title, &text).details(
            crate::report::details_with(&report, &system),
            dialogs::DetailsMode::Auto,
        ));
    }

    /// Says each snapshot restored since the last frame: in the status
    /// bar and as a toast offering Undo (the restore is one step).
    pub fn follow_snapshots(&mut self) {
        let restored = crate::edit_snapshots::lock(&self.core.snapshots).take_restored();
        if restored.is_empty() {
            return;
        }
        let undo = self.text(Message::new("gui-history-undo", "Undo"));
        for name in restored {
            let message = Message::new("gui-history-snapshot-restored", "Restored “{ $name }”")
                .with("name", name);
            let text = self.text(message.clone());
            if let Some(toasts) = self.core.app.handle().service(&TOASTS) {
                toasts.notify(Toast::success(&text).action(
                    &undo,
                    egui_pop::api::app::CMD_UNDO,
                    Value::Null,
                ));
            }
            self.notes.push(text.clone());
            self.core.notices.push(Notice {
                level: Level::Info,
                text,
                key: message.key,
                action: Some((undo.clone(), egui_pop::api::app::CMD_UNDO.to_string())),
                at: self.core.now,
                report: None,
            });
        }
    }

    fn note(&mut self, text: impl Into<Message>) {
        self.notify(Level::Info, text);
    }

    fn warn(&mut self, text: impl Into<Message>) {
        self.notify(Level::Warning, text);
    }

    /// What the viewport's engine says a user should know: a step whose
    /// GPU version failed (once per module), a GPU lost. Shown as toasts.
    fn engine_notices(&mut self) {
        let engine = self.core.engine.clone();
        for e in engine.bus().since(self.core.engine_seen) {
            self.core.engine_seen = e.seq;
            let message = || serde_json::from_value::<Message>(e.payload["message"].clone()).ok();
            match e.kind {
                "engine.gpu_fell_back" => {
                    let module = e.payload["module"].as_str().unwrap_or_default().to_string();
                    if self.core.fell_back.insert(module)
                        && let Some(m) = message()
                    {
                        self.notify(Level::Warning, m);
                    }
                }
                "engine.gpu_lost" => {
                    if let Some(m) = message() {
                        let level = if e.payload["recovered"] == true {
                            Level::Warning
                        } else {
                            Level::Error
                        };
                        self.notify(level, m);
                    }
                }
                _ => {}
            }
        }
    }

    fn open_dialog(&mut self, dialog: impl Into<dialogs::Dialog>) {
        match self.core.dialogs.get() {
            Some(d) => {
                // A dialog, as egui-pop's spec with its defaults.
                d.open(Into::<dialogs::Dialog>::into(dialog));
            }
            None => self.warn(Message::new("gui-no-dialogs", "no dialogs to show this in")),
        }
    }

    /// Asks before running a command. Under the yes policy it runs at once,
    /// and says so.
    pub fn confirm(&mut self, c: crate::dialogs::Confirm) {
        if self.core.assume_yes {
            self.notify(
                Level::Warning,
                Message::new("gui-confirmed-by-policy", "{ $verb }: confirmed by policy")
                    .with("verb", &c.verb),
            );
            let _ = self.run(&c.command, c.args, Origin::Script("policy".into()));
            return;
        }
        let command = match CommandId::new(c.command.as_str()) {
            Ok(id) => id,
            Err(e) => return self.warn(e.to_string()),
        };
        let report = crate::report::Context::command(command.as_str());
        let mut d = dialogs::Confirm::new(&c.title, &c.verb, command)
            .detail(&c.detail)
            .args(c.args)
            .details(
                crate::report::details(&report),
                dialogs::DetailsMode::Hidden,
            );
        if c.danger {
            d = d.danger();
        }
        self.open_dialog(d);
    }

    /// Asks for values, then runs the form's command with them.
    pub fn form(&mut self, f: crate::dialogs::Form) {
        let command = match CommandId::new(f.command.as_str()) {
            Ok(id) => id,
            Err(e) => return self.warn(e.to_string()),
        };
        let mut form = dialogs::Form::new(&f.title, &f.verb, command);
        for field in &f.fields {
            form = form.field_at(
                &field.key,
                crate::dialogs::descriptor(&field.descriptor),
                crate::dialogs::value(&field.value),
            );
        }
        self.open_dialog(form);
    }

    /// Asks for a choice (filtered as one types), then runs the picker's
    /// command with the value chosen as its `arg`.
    pub fn pick(&mut self, p: crate::dialogs::Picker) {
        let command = match CommandId::new(p.command.as_str()) {
            Ok(id) => id,
            Err(e) => return self.warn(e.to_string()),
        };
        let options = p
            .choices
            .iter()
            .map(|c| {
                let o = ChooseOption::new(&c.label, json!(c.value));
                match &c.detail {
                    Some(d) => o.hint(d),
                    None => o,
                }
            })
            .collect();
        self.open_dialog(Choose::with_options(&p.title, options, command).value_key(&p.arg));
    }

    /// Quick open: the network's modules, the library's images and the
    /// recent workspaces in one filtered list (see `quick_open`).
    fn quick_open(&mut self) {
        let modules = self
            .focus
            .viewport
            .borrow()
            .graph()
            .map(|g| {
                g.ids()
                    .filter_map(|id| {
                        let name = g.name(id).ok()?.to_string();
                        let module = g.manifest(id).ok()?.map(|m| m.name);
                        Some((name, module))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let images = self
            .focus
            .library
            .borrow()
            .entries
            .iter()
            .map(|e| (e.id.clone(), e.name.clone()))
            .collect();
        let subworkspaces = self
            .workspace_state()
            .map(|st| {
                st.bundle
                    .template
                    .subworkspaces
                    .iter()
                    .map(|s| (s.id.clone(), s.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let sources = crate::quick_open::Sources {
            modules,
            subworkspaces,
            images,
            workspaces: recent_workspaces(),
        };
        let choices = crate::quick_open::choices(&sources, |m| self.text(m));
        let title = self.text(Message::new(
            "gui-quick-open-title",
            "Go to a module, a sub-workspace, an image or a workspace",
        ));
        self.pick(crate::dialogs::Picker {
            title,
            choices,
            command: crate::quick_open::GO.into(),
            arg: "item".into(),
        });
    }

    /// Goes to what quick open offered: a module selected (the Parameters
    /// panel brought forward), an image opened in the viewport, a
    /// workspace opened in its window.
    fn quick_open_go(&mut self, item: crate::quick_open::Item) -> Result<Value, String> {
        use crate::quick_open::Item;
        match item {
            Item::Module(node) => {
                let known = self
                    .focus
                    .viewport
                    .borrow()
                    .graph()
                    .is_some_and(|g| g.find(&node).is_some());
                if !known {
                    return Err(format!("no module `{node}` in the network shown"));
                }
                lock(&self.core.values)
                    .set(helios_values::Set::new(
                        crate::params::SELECTION,
                        helios_values::Change::To(helios_core::Value::Text(node.clone())),
                        Origin::User,
                    ))
                    .map_err(|e| e.to_string())?;
                if self
                    .core
                    .panels
                    .iter()
                    .any(|p| p.id.as_str() == "helios.params")
                {
                    self.run("helios.params.focus", Value::Null, Origin::User)?;
                }
                Ok(json!({ "module": node }))
            }
            Item::Subworkspace(id) => {
                crate::contexts::enter_subworkspace(
                    self.core.app.handle(),
                    &self.focus.subworkspaces,
                    &id,
                    &egui_pop::model::commands::Origin::User,
                )?;
                Ok(json!({ "subworkspace": id }))
            }
            Item::Image(id) => {
                let path = self
                    .core
                    .image_files
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| format!("image `{id}` has no file at hand"))?;
                self.core
                    .opens
                    .push(crate::platform::Open::Image(path.clone()));
                Ok(json!({ "image": id, "path": path }))
            }
            Item::File(path) => {
                if !path.is_file() {
                    return Err(format!("no image file at {}", path.display()));
                }
                self.core
                    .opens
                    .push(crate::platform::Open::Image(path.clone()));
                Ok(json!({ "path": path }))
            }
            Item::Workspace(dir) => {
                let ctx = self.core.app.handle().egui();
                self.apply(&ctx, Action::OpenWorkspace(Some(dir.clone())))?;
                Ok(json!({ "workspace": dir }))
            }
        }
    }

    /// Opens a menu of commands at a position.
    pub fn open_menu(&mut self, at: egui::Pos2, items: Vec<MenuItem>) {
        egui_pop::api::menus::close_popup(self.core.app.handle());
        self.focus.menu = Some((at, items));
    }

    /// A declared menu as data, as it would open now: each entry's
    /// command, title, keys, whether it applies and whether it is ticked;
    /// submenus nested.
    pub fn menu_items(&mut self, menu: &str) -> Value {
        fn rows(h: &AppHandle, menu: &MenuId, cx: &egui_pop::model::when::WhenContext) -> Value {
            let shown = show_items(&h.menu(menu), cx);
            Value::Array(
                shown
                    .into_iter()
                    .map(|s| match s {
                        Shown::Separator => json!({ "kind": "separator" }),
                        Shown::Item {
                            item: Item::Submenu { menu, label },
                            ..
                        } => json!({ "kind": "submenu", "label": label, "items": rows(h, &menu, cx) }),
                        Shown::Item {
                            item: Item::Command { id, args, label },
                            enabled,
                            checked,
                            ..
                        } => {
                            let title = label.unwrap_or_else(|| {
                                h.command(&id)
                                    .map_or_else(|| id.to_string(), |s| h.command_menu_title(&s))
                            });
                            json!({
                                "kind": "command",
                                "id": id,
                                "args": args,
                                "title": title,
                                "keys": h.keys_for(&id).first(),
                                "enabled": enabled && h.applies(&id, cx),
                                "checked": checked,
                            })
                        }
                        Shown::Item {
                            item: Item::List { name },
                            enabled,
                            ..
                        } => Value::Array(
                            h.list_items(&name, cx)
                                .into_iter()
                                .map(|e| {
                                    json!({
                                        "kind": "command",
                                        "id": e.command,
                                        "args": e.args,
                                        "title": e.label,
                                        "keys": h.keys_for(&e.command).first(),
                                        "enabled": enabled && e.enabled && h.applies(&e.command, cx),
                                        "checked": e.checked,
                                    })
                                })
                                .collect(),
                        ),
                    })
                    .flat_map(|v| match v {
                        // A list's entries take its place.
                        Value::Array(entries) => entries,
                        one => vec![one],
                    })
                    .collect(),
            )
        }
        self.set_context(&self.core.app.handle().egui());
        let h = self.core.app.handle().clone();
        let cx = h.when_context();
        match MenuId::new(menu) {
            Ok(id) => rows(&h, &id, &cx),
            Err(_) => Value::Array(Vec::new()),
        }
    }

    /// The notices so far, oldest first.
    pub fn notices(&self) -> &[Notice] {
        &self.core.notices
    }

    /// Answers the dialog shown, as `origin` (applied at the next frame).
    fn answer(&mut self, answer: Answer, origin: &Origin) -> Result<(), String> {
        let d = self.core.dialogs.get().ok_or("no dialogs")?.clone();
        let (_, dialog) = d.current().ok_or("no dialog to answer")?;
        let answer = match (answer, &dialog) {
            (Answer::Cancel, _) => dialogs::Answer::Cancel,
            (Answer::Choice(c), dialogs::Dialog::Choose(choose)) => {
                // By value, else by label, else by index.
                match choose.options.iter().find(|o| o.value == json!(c)) {
                    Some(o) => dialogs::Answer::Value {
                        value: o.value.clone(),
                    },
                    None => dialogs::Answer::Choose {
                        index: choose
                            .options
                            .iter()
                            .position(|o| o.label == c)
                            .or_else(|| {
                                c.parse::<usize>()
                                    .ok()
                                    .filter(|i| *i < choose.options.len())
                            })
                            .ok_or_else(|| format!("no choice `{c}`"))?,
                    },
                }
            }
            (Answer::Choice(_), _) => return Err("this dialog has no choices".into()),
            (_, dialogs::Dialog::Choose(_)) => return Err("answer a choice with `choice`".into()),
            (Answer::Primary, _) => dialogs::Answer::Accept {
                values: serde_json::Map::new(),
            },
            (Answer::Values(v), _) => dialogs::Answer::Accept {
                values: v.as_object().cloned().ok_or("`values` is an object")?,
            },
        };
        d.answer(answer, to_pop(origin))
    }

    /// Lets tips come by themselves (`on`), as cards: the desktop's
    /// window does; a shell in tests, pictures or tools shows them only
    /// when asked (`tips.show`).
    pub fn tips_by_themselves(&self, on: bool) {
        if let Some(tips) = self.core.app.handle().service(&egui_pop::api::tips::TIPS) {
            tips.set_auto(on);
        }
    }

    /// Translates what Helios gives About again once the language
    /// changes.
    fn follow_language(&mut self) {
        let generation = self.core.i18n.generation();
        if self.core.about_generation == Some(generation) {
            return;
        }
        self.core.about_generation = Some(generation);
        let description = self.text(about_description());
        if let Some(about) = self.core.app.handle().service(&egui_pop::api::about::ABOUT) {
            about.update(|i| i.description = description);
        }
    }

    /// Sets the context keys commands and keys are tested against.
    fn set_context(&self, ctx: &Egui) {
        let h = self.core.app.handle();
        let flag = |on: bool| if on { "true" } else { "false" };
        h.set_context(
            "focusedPanel",
            h.focused_panel().as_ref().map_or("", PanelId::as_str),
        );
        h.set_context("paletteVisible", flag(palette::is_open(h)));
        h.set_context(
            "fullScreen",
            flag(ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))),
        );
        {
            // What the mask keys need: the tool, a path point selected.
            let store = lock(&self.core.values);
            h.set_context("maskTool", &crate::masks::tool(&store));
            h.set_context("maskPoint", flag(crate::masks::point(&store).is_some()));
            h.set_context("maskDisplay", crate::mask_display::mode(&store).name());
        }
        h.set_context("heliosTheme", &self.core.theme_name);
        h.set_context("libraryOpen", flag(self.library_open()));
        h.set_context("heliosProfile", &self.core.profile_name);
        h.set_context(
            crate::importers::CONTEXT,
            flag(!self.core.lists.borrow().importers.is_empty()),
        );
    }

    /// Adds a keymap package's keys to the user's layer, before the
    /// user's own (which keep winning), or takes them away; the user's
    /// other keys stay.
    fn keymap_package(&mut self, name: &str, on: bool) -> Result<Value, String> {
        let package = crate::keymaps::package(name).ok_or_else(|| {
            let names: Vec<String> = crate::keymaps::packages()
                .into_iter()
                .map(|p| p.name)
                .collect();
            format!(
                "no keymap package `{name}` (there are: {})",
                names.join(", ")
            )
        })?;
        let ours = package.bindings();
        let mut doc = self.core.app.handle().export_keymap(Layer::User);
        doc.bindings
            .retain(|b| !ours.iter().any(|o| crate::keymaps::same(o, b)));
        if on {
            doc.bindings.splice(0..0, ours);
        }
        let diff = self.core.app.load_keymap(&doc, Layer::User)?;
        Ok(json!({
            "package": package.name,
            "on": on,
            "added": diff.added.len(),
            "removed": diff.removed.len(),
            "skipped": diff.skipped,
            "conflicts": diff.conflicts,
        }))
    }

    /// Loads a workspace template's keys into egui-pop's workspace keymap
    /// layer: over the defaults, under the user's own keys.
    pub fn apply_template_tools(&mut self, name: &str, tools: &helios_template::format::Tools) {
        use helios_profile::{System, local_keys};
        let doc = KeymapDoc {
            bindings: tools
                .keymap
                .iter()
                .map(|b| DocBinding {
                    keys: local_keys(&b.keys, System::current()),
                    command: Some(b.command.clone()),
                    when: b.when.clone(),
                    ..DocBinding::default()
                })
                .collect(),
        };
        if let Err(e) = self.core.app.load_keymap(&doc, Layer::Workspace) {
            self.warn(
                Message::new(
                    "gui-template-keys-error",
                    "template { $name }: keys: { $error }",
                )
                .with("name", name)
                .with("error", e),
            );
        }
    }

    /// What a template sets for the library: its import source (the
    /// Library panel then offers to import from it), the images it opens
    /// on (its query, as the search field's filter) and culling's
    /// auto-advance.
    pub fn apply_template_library(&mut self, library: Option<&helios_template::LibraryDesc>) {
        let Some(l) = library else {
            return;
        };
        self.focus.import_source.clone_from(&l.import);
        self.focus.open_in_compare = l.compare;
        let mut lib = self.focus.library.borrow_mut();
        lib.auto_advance = l.auto_advance;
        lib.import_offered = l.import.is_some();
        if let Some(sort) = crate::library::sort_of(l.sort.as_ref()) {
            lib.set_sort(sort);
        }
        if let Some(q) = &l.query {
            lib.set_filter(crate::library::filter_of(q));
        }
        if let Some(folder) = &l.folder {
            lib.set_folder(Some(folder));
        }
        if let Some(collection) = &l.collection {
            lib.set_collection(Some(collection));
        }
    }

    /// `library.sort`: sorts this window's library, and keeps the sort
    /// with the workspace (one undo step) when one is open; answers the
    /// sort in force.
    fn sort_library(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        let current = self.focus.library.borrow().sort();
        if let Some(by) = args.get("by").filter(|v| !v.is_null()) {
            let name = by.as_str().ok_or("`by`: not a sort's name")?;
            let by = helios_vault::Sort::parse(name)
                .filter(|s| helios_template::format::SORTS.contains(&s.name()))
                .ok_or_else(|| {
                    format!(
                        "`by`: `{name}` is not one of {}",
                        helios_template::format::SORTS.join(", ")
                    )
                })?;
            let descending = match args.get("descending") {
                None | Some(Value::Null) => current.descending,
                Some(Value::Bool(b)) => *b,
                Some(v) => return Err(format!("`descending`: {v} is not true or false")),
            };
            let sort = helios_vault::Order { by, descending };
            if let Some(state) = self.workspace_state() {
                let mut library = state.bundle.template.library.unwrap_or_default();
                library.sort = crate::library::sort_desc(sort);
                // An empty library section is none at all.
                let library =
                    (library != helios_template::LibraryDesc::default()).then_some(library);
                if library
                    != self
                        .workspace_state()
                        .and_then(|s| s.bundle.template.library)
                {
                    self.edit_workspace(&json!({ "library": library }), origin)?;
                }
            }
            self.focus.library.borrow_mut().set_sort(sort);
        }
        let sort = self.focus.library.borrow().sort();
        Ok(json!({ "by": sort.by.name(), "descending": sort.descending }))
    }

    /// `library.folder`: scopes this window's library to a folder, and
    /// keeps it with the workspace (one undo step) when one is open;
    /// answers the folder in force.
    fn folder_library(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        if let Some(path) = args.get("path") {
            let folder = match path {
                Value::Null => None,
                Value::String(p) if Path::new(p).is_absolute() => Some(p.clone()),
                other => return Err(format!("`path`: {other} is not an absolute folder")),
            };
            if let Some(state) = self.workspace_state() {
                let mut library = state.bundle.template.library.unwrap_or_default();
                library.folder.clone_from(&folder);
                // An empty library section is none at all.
                let library =
                    (library != helios_template::LibraryDesc::default()).then_some(library);
                if library
                    != self
                        .workspace_state()
                        .and_then(|s| s.bundle.template.library)
                {
                    self.edit_workspace(&json!({ "library": library }), origin)?;
                }
            }
            let mut lib = self.focus.library.borrow_mut();
            lib.set_folder(folder.as_deref());
            lib.record_filter();
        }
        Ok(json!({ "path": self.focus.library.borrow().folder() }))
    }

    /// `library.collection`: scopes this window's library to an album, and
    /// keeps it with the workspace (one undo step) when one is open;
    /// answers the album in force.
    fn collection_library(&mut self, args: &Value, origin: &Origin) -> Result<Value, String> {
        if let Some(id) = args.get("id") {
            let id = match id {
                Value::Null => None,
                Value::String(id) => {
                    let known = self
                        .focus
                        .library
                        .borrow()
                        .albums()
                        .is_none_or(|a| a.iter().any(|a| a.id == *id))
                        || id == helios_vault::collections::QUICK;
                    if !known {
                        return Err(format!("`id`: no album `{id}`"));
                    }
                    Some(id.clone())
                }
                other => return Err(format!("`id`: {other} is not an album's id")),
            };
            if let Some(state) = self.workspace_state() {
                let mut library = state.bundle.template.library.unwrap_or_default();
                library.collection.clone_from(&id);
                // An empty library section is none at all.
                let library =
                    (library != helios_template::LibraryDesc::default()).then_some(library);
                if library
                    != self
                        .workspace_state()
                        .and_then(|s| s.bundle.template.library)
                {
                    self.edit_workspace(&json!({ "library": library }), origin)?;
                }
            }
            let mut lib = self.focus.library.borrow_mut();
            lib.set_collection(id.as_deref());
            lib.record_filter();
        }
        Ok(json!({ "id": self.focus.library.borrow().collection() }))
    }

    /// The recent-filters commands (`crate::recent_filters`): the
    /// switcher, the list, apply, pin.
    fn recent_filters(&mut self, command: &'static str, args: &Value) -> Result<Value, String> {
        use crate::recent_filters::{
            APPLY_COMMAND, Kind, PIN_COMMAND, SWITCH_COMMAND, entries, filter_arg, text,
        };
        let smart: Vec<(String, String)> = self
            .focus
            .library
            .borrow()
            .albums()
            .unwrap_or_default()
            .iter()
            .filter(|a| a.is_smart())
            .map(|a| (a.id.clone(), a.name.clone()))
            .collect();
        let history = self
            .focus
            .library
            .borrow()
            .filter_history()
            .cloned()
            .unwrap_or_default();
        if command == APPLY_COMMAND {
            let filter = filter_arg(args)?.ok_or("`query` or `filter` is missing")?;
            let mut lib = self.focus.library.borrow_mut();
            lib.apply_filter(filter);
            return Ok(json!({ "query": text(lib.filter()) }));
        }
        if command == PIN_COMMAND {
            let mut lib = self.focus.library.borrow_mut();
            let filter = filter_arg(args)?.unwrap_or_else(|| lib.filter().clone());
            if filter.is_empty() {
                return Err("no filter to pin".into());
            }
            let pinned = args.get("pinned").and_then(Value::as_bool).unwrap_or(true);
            let changed = lib.pin_filter(&filter, pinned);
            return Ok(json!({ "query": text(&filter), "pinned": pinned, "changed": changed }));
        }
        let listed = entries(&history, &smart);
        if command == SWITCH_COMMAND {
            let options = listed
                .iter()
                .map(|e| {
                    let hint = self.text(match e.kind {
                        Kind::Pinned => Message::new("gui-recent-filters-pinned", "pinned"),
                        Kind::Recent => Message::new("gui-recent-filters-recent", "recent"),
                        Kind::Smart => Message::new("gui-recent-filters-smart", "smart album"),
                    });
                    ChooseOption::new(
                        &e.label,
                        serde_json::to_value(&e.filter).unwrap_or(Value::Null),
                    )
                    .hint(&hint)
                })
                .collect::<Vec<_>>();
            if options.is_empty() {
                self.notify(
                    Level::Info,
                    Message::new(
                        "gui-recent-filters-none",
                        "No recent filter yet: the filters used in the Library are listed here",
                    ),
                );
                return Ok(json!({ "shown": 0 }));
            }
            let title = self.text(Message::new("gui-recent-filters-title", "Recent filters"));
            let apply = CommandId::new(APPLY_COMMAND).map_err(|e| e.to_string())?;
            let shown = options.len();
            self.open_dialog(Choose::with_options(&title, options, apply).value_key("filter"));
            return Ok(json!({ "shown": shown }));
        }
        let of = |kind: Kind| -> Vec<Value> {
            listed
                .iter()
                .filter(|e| e.kind == kind)
                .map(|e| {
                    let mut item = json!({
                        "query": text(&e.filter),
                        "filter": serde_json::to_value(&e.filter).unwrap_or(Value::Null),
                    });
                    if kind == Kind::Smart {
                        item["name"] = json!(e.label);
                    }
                    item
                })
                .collect()
        };
        Ok(json!({
            "pinned": of(Kind::Pinned),
            "recent": of(Kind::Recent),
            "smart": of(Kind::Smart),
            "in_force": text(self.focus.library.borrow().filter()),
        }))
    }

    /// The album commands (`library.collection.*`): each change is queued
    /// for the host, which makes it in the vault as one step of the
    /// library's history (`docs/design/library.md`, L6). A name not given
    /// is asked in a form.
    fn collection_edit(&mut self, command: &'static str, args: &Value) -> Result<Value, String> {
        use crate::library::{
            AlbumEdit, COLLECTION_ADD_COMMAND as ADD, COLLECTION_CREATE_COMMAND as CREATE,
            COLLECTION_DELETE_COMMAND as DELETE, COLLECTION_LIST_COMMAND as LIST,
            COLLECTION_MOVE_COMMAND as MOVE, COLLECTION_REMOVE_COMMAND as REMOVE,
            COLLECTION_RENAME_COMMAND as RENAME, COLLECTION_RULES_COMMAND as RULES,
        };
        use crate::smart_album::SmartForm;
        use helios_vault::collections::{ALBUM, SET, SMART};
        if command == crate::library::COLLECTION_RULES_EDIT_COMMAND {
            return self.rules_edit(args);
        }
        let text = |a: &'_ Value, key: &str| a.get(key).and_then(Value::as_str).map(String::from);
        let albums: Vec<helios_vault::collections::Album> = self
            .focus
            .library
            .borrow()
            .albums()
            .map(<[_]>::to_vec)
            .unwrap_or_default();
        if command == LIST {
            let lib = self.focus.library.borrow();
            let list: Vec<Value> = albums
                .iter()
                .map(|a| {
                    let mut item = json!({
                        "id": a.id,
                        "name": a.name,
                        "kind": a.kind,
                        "count": lib.album_count(a),
                    });
                    if let Some(set) = &a.set {
                        item["set"] = json!(set);
                    }
                    if let Some(rules) = &a.rules {
                        item["rules"] = rules.to_json();
                    }
                    item
                })
                .collect();
            return Ok(json!({
                "albums": list,
                "quick": lib.quick_count(),
                "shown": lib.collection(),
            }));
        }
        let name = text(args, "name")
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty());
        let name = name.as_deref();
        if matches!(
            command,
            crate::library::QUICK_TOGGLE_COMMAND
                | crate::library::QUICK_CLEAR_COMMAND
                | crate::library::QUICK_SAVE_COMMAND
        ) {
            return self.quick_edit(command, args, name, &albums);
        }
        let taken = |name: &str, id: &str| {
            let folded = helios_vault::fields::fold_case(name);
            albums
                .iter()
                .any(|a| a.id != id && helios_vault::fields::fold_case(&a.name) == folded)
        };
        // The images named, else the selection (none: an error, but for a
        // new album, which may start empty).
        let images = |lib: &crate::library::Library, empty_ok: bool| match args.get("images") {
            Some(Value::Array(list)) => Ok(list
                .iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect::<Vec<_>>()),
            Some(other) => Err(format!("`images`: {other} is not a list of ids")),
            None => match lib.targets() {
                Ok(keys) => Ok(keys),
                Err(_) if empty_ok => Ok(Vec::new()),
                Err(e) => Err(e),
            },
        };
        // A smart album's rules: `rules` (a tree), else the form's keys,
        // else none given.
        let rules = || -> Result<Option<helios_vault::rules::Rule>, String> {
            if let Some(tree) = args.get("rules") {
                return helios_vault::rules::Rule::parse(tree).map(Some);
            }
            if SmartForm::in_args(args) {
                return SmartForm::from_args(args)?.rules().map(Some);
            }
            Ok(None)
        };
        if command == CREATE && args.get("kind").and_then(Value::as_str) == Some(SET) {
            let parent = match args.get("set") {
                None | Some(Value::Null) => None,
                Some(Value::String(set)) => {
                    sets::check_target(&albums, set, None)?;
                    Some(set.clone())
                }
                Some(other) => return Err(format!("`set`: {other} is not a set's id")),
            };
            let Some(name) = name else {
                self.set_form(parent.as_deref());
                return Ok(Value::Null);
            };
            if taken(name, "") {
                return Err(format!("an album is already named “{name}”"));
            }
            let id =
                helios_vault::collections::album_key(name, |k| albums.iter().any(|a| a.id == k));
            self.focus
                .library
                .borrow_mut()
                .edit_album(AlbumEdit::CreateSet {
                    id: id.clone(),
                    name: name.to_string(),
                    set: parent.clone(),
                });
            return Ok(json!({ "id": id, "name": name, "kind": SET, "set": parent }));
        }
        if command == CREATE {
            let smart = match args.get("kind").and_then(Value::as_str) {
                None => args.get("rules").is_some() || SmartForm::in_args(args),
                Some(SMART) => true,
                Some(ALBUM) => false,
                Some(other) => return Err(format!("`kind`: `{other}` is album, smart or set")),
            };
            // Checked before a form opens: a fault is said at once.
            let tree = if smart { rules()? } else { None };
            let Some(name) = name else {
                if smart {
                    self.smart_form(None, &SmartForm::of_default());
                } else {
                    self.album_form(CREATE, "");
                }
                return Ok(Value::Null);
            };
            if taken(name, "") {
                return Err(format!("an album is already named “{name}”"));
            }
            let id =
                helios_vault::collections::album_key(name, |k| albums.iter().any(|a| a.id == k));
            if smart {
                let rules = tree.unwrap_or_else(|| {
                    helios_vault::rules::Rule::group(helios_vault::rules::Match::All, Vec::new())
                });
                self.focus
                    .library
                    .borrow_mut()
                    .edit_album(AlbumEdit::CreateSmart {
                        id: id.clone(),
                        name: name.to_string(),
                        rules: rules.clone(),
                    });
                return Ok(
                    json!({ "id": id, "name": name, "kind": SMART, "rules": rules.to_json() }),
                );
            }
            let mut lib = self.focus.library.borrow_mut();
            let images = images(&lib, true)?;
            let count = images.len();
            lib.edit_album(AlbumEdit::Create {
                id: id.clone(),
                name: name.to_string(),
                images,
            });
            return Ok(json!({ "id": id, "name": name, "images": count }));
        }
        // The other commands name an album: given, else the one a form
        // asked a new name for.
        let id = match text(args, "id") {
            Some(id) => id,
            None if command == RENAME => self
                .focus
                .album_renamed
                .clone()
                .ok_or("`id`: no album named")?,
            None if command == RULES => self
                .focus
                .album_ruled
                .clone()
                .ok_or("`id`: no album named")?,
            None => return Err("`id`: no album named".into()),
        };
        let Some(album) = albums.iter().find(|a| a.id == id) else {
            return Err(format!("`id`: no album `{id}`"));
        };
        if album.is_smart() && (command == ADD || command == REMOVE) {
            return Err(format!(
                "“{}” is a smart album: its rules choose its photos",
                album.name
            ));
        }
        if album.is_set() && (command == ADD || command == REMOVE) {
            return Err(format!(
                "“{}” is an album set: its albums hold its photos",
                album.name
            ));
        }
        if command == MOVE {
            let set = match args.get("set") {
                None => {
                    let album = album.clone();
                    self.move_chooser(&album, &albums);
                    return Ok(Value::Null);
                }
                Some(Value::Null) => None,
                Some(Value::String(set)) => {
                    sets::check_target(&albums, set, Some(&id))?;
                    Some(set.clone())
                }
                Some(other) => return Err(format!("`set`: {other} is not a set's id")),
            };
            self.focus.library.borrow_mut().edit_album(AlbumEdit::Move {
                id: id.clone(),
                set: set.clone(),
            });
            return Ok(json!({ "id": id, "set": set }));
        }
        let mut lib = self.focus.library.borrow_mut();
        let edit = match command {
            RENAME => {
                let Some(name) = name else {
                    drop(lib);
                    self.focus.album_renamed = Some(id);
                    let current = album.name.clone();
                    self.album_form(RENAME, &current);
                    return Ok(Value::Null);
                };
                if taken(name, &id) {
                    return Err(format!("an album is already named “{name}”"));
                }
                AlbumEdit::Rename {
                    id,
                    name: name.to_string(),
                }
            }
            DELETE => AlbumEdit::Delete {
                id,
                members: args.get("members").and_then(Value::as_bool) == Some(true),
            },
            RULES => {
                let Some(current) = album.rules.clone() else {
                    return Err(format!(
                        "“{}” is not a smart album: it holds its photos",
                        album.name
                    ));
                };
                let Some(rules) = rules()? else {
                    drop(lib);
                    let editor = crate::rule_editor::RuleEditor::new(&id, &album.name, &current);
                    let state = editor.state();
                    self.focus.rule_editor = Some(editor);
                    return Ok(state);
                };
                AlbumEdit::Rules { id, rules }
            }
            ADD => AlbumEdit::Add {
                images: images(&lib, false)?,
                id,
            },
            REMOVE => AlbumEdit::Remove {
                images: images(&lib, false)?,
                id,
            },
            other => return Err(format!("`{other}` is not an album command")),
        };
        let answer = json!({ "id": edit.id() });
        lib.edit_album(edit);
        Ok(answer)
    }

    /// The library's selection and batch commands (`library.select.*`,
    /// `library.marks.*`): the selection is the focus's, and so are the
    /// marks copied.
    fn library_batch(&mut self, command: &'static str, args: &Value) -> Result<Value, String> {
        use crate::library::{
            CopiedMarks, DELETE_REJECTED_COMMAND, MARK_KINDS, MARKS_COPY_COMMAND,
            MARKS_PASTE_COMMAND, PasteMode, REMOVE_COMMAND, SELECT_INVERT_COMMAND,
            SELECT_NONE_COMMAND, SELECT_SIMILAR_COMMAND, Similar, TRASH_COMMAND,
        };
        let confirmed = args.get("confirmed").and_then(Value::as_bool) == Some(true);
        let text = |key: &str| args.get(key).and_then(Value::as_str);
        let images = || -> Result<Option<Vec<String>>, String> {
            match args.get("images") {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Array(list)) => Ok(Some(
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect(),
                )),
                Some(other) => Err(format!("`images`: {other} is not a list of ids")),
            }
        };
        let mut lib = self.focus.library.borrow_mut();
        match command {
            SELECT_NONE_COMMAND => {
                lib.select_none();
                Ok(json!({ "selected": 0 }))
            }
            SELECT_INVERT_COMMAND => Ok(json!({ "selected": lib.invert_selection() })),
            SELECT_SIMILAR_COMMAND => {
                let by = Similar::parse(text("by").unwrap_or("day"))?;
                Ok(json!({ "selected": lib.select_similar(by)? }))
            }
            MARKS_COPY_COMMAND => lib.copy_marks(text("image")).map(CopiedMarks::to_json),
            MARKS_PASTE_COMMAND => {
                let images = images()?;
                let kinds: Vec<&str> = match args.get("marks") {
                    None | Some(Value::Null) => MARK_KINDS.to_vec(),
                    Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
                    Some(other) => return Err(format!("`marks`: {other} is not a list")),
                };
                let mode = PasteMode::parse(text("mode").unwrap_or("merge"))?;
                let count = lib.paste_marks(images, &kinds, mode)?;
                Ok(json!({ "images": count, "marks": kinds }))
            }
            REMOVE_COMMAND => Ok(json!({ "removed": lib.remove_from_library(images()?)? })),
            TRASH_COMMAND => Ok(json!({ "asked": lib.ask_trash(images()?, confirmed)? })),
            DELETE_REJECTED_COMMAND => Ok(json!({
                "asked": lib.ask_delete_rejected(images()?, confirmed)?
            })),
            other => Err(format!("`{other}` is not a selection command")),
        }
    }

    /// The quick collection's commands (`library.quick.*`), queued for the
    /// host as the albums' are: one step of the library's history each.
    fn quick_edit(
        &mut self,
        command: &'static str,
        args: &Value,
        name: Option<&str>,
        albums: &[helios_vault::collections::Album],
    ) -> Result<Value, String> {
        use crate::library::{AlbumEdit, QUICK_CLEAR_COMMAND, QUICK_TOGGLE_COMMAND};
        let mut lib = self.focus.library.borrow_mut();
        if command == QUICK_TOGGLE_COMMAND {
            let images: Vec<String> = match args.get("images") {
                Some(Value::Array(list)) => list
                    .iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect(),
                Some(other) => return Err(format!("`images`: {other} is not a list of ids")),
                None => lib.targets()?,
            };
            if images.is_empty() {
                return Err("no image selected".into());
            }
            // `add` (a drop on its row) only puts in: those already in
            // stay, and with none missing nothing changes.
            if args.get("add").and_then(Value::as_bool) == Some(true) {
                let missing: Vec<String> = images
                    .into_iter()
                    .filter(|i| !lib.quick().contains(i))
                    .collect();
                let count = missing.len();
                if count > 0 {
                    lib.edit_album(AlbumEdit::QuickToggle { images: missing });
                }
                return Ok(json!({ "added": true, "images": count }));
            }
            // As the vault decides: all in, out; else in.
            let added = !images.iter().all(|i| lib.quick().contains(i));
            let count = images.len();
            lib.edit_album(AlbumEdit::QuickToggle { images });
            return Ok(json!({ "added": added, "images": count }));
        }
        if command == QUICK_CLEAR_COMMAND {
            lib.edit_album(AlbumEdit::QuickClear);
            return Ok(Value::Null);
        }
        if lib.quick_count() == 0 {
            return Err("the quick collection is empty".into());
        }
        let Some(name) = name else {
            drop(lib);
            self.quick_save_form();
            return Ok(Value::Null);
        };
        let folded = helios_vault::fields::fold_case(name);
        if albums
            .iter()
            .any(|a| helios_vault::fields::fold_case(&a.name) == folded)
        {
            return Err(format!("an album is already named “{name}”"));
        }
        let clear = match args.get("clear") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(other) => return Err(format!("`clear`: {other} is not true or false")),
        };
        let id = helios_vault::collections::album_key(name, |k| albums.iter().any(|a| a.id == k));
        let count = lib.quick().len();
        lib.edit_album(AlbumEdit::QuickSave {
            id: id.clone(),
            name: name.to_string(),
            clear,
        });
        Ok(json!({ "id": id, "name": name, "images": count, "clear": clear }))
    }

    /// Asks the name of the album the quick collection is saved as, and
    /// whether to empty it, then runs `library.quick.saveAs`.
    fn quick_save_form(&mut self) {
        use crate::dialogs::Field;
        use helios_values::Descriptor as D;
        let name = self.text(Message::new("gui-library-album-name", "name"));
        let clear = self.text(Message::new(
            "gui-library-quick-save-clear",
            "then clear the quick collection",
        ));
        self.form(crate::dialogs::Form {
            title: self.text(Message::new(
                "gui-library-quick-save-title",
                "Save the quick collection as an album",
            )),
            fields: vec![
                Field {
                    key: "name".into(),
                    descriptor: D::text(&name, ""),
                    value: helios_core::Value::Text(String::new()),
                },
                Field {
                    key: "clear".into(),
                    descriptor: D::bool(&clear, false),
                    value: helios_core::Value::Bool(false),
                },
            ],
            verb: self.text(Message::new("gui-library-quick-save-verb", "Save")),
            command: crate::library::QUICK_SAVE_COMMAND.into(),
        });
    }

    /// `library.collection.rules.edit`: one edit in the open rule editor,
    /// or undo, redo, apply (refused while a condition is not valid, with
    /// its reason), cancel. Answers the editor's state.
    fn rules_edit(&mut self, args: &Value) -> Result<Value, String> {
        let editor = self
            .focus
            .rule_editor
            .as_mut()
            .ok_or("no rule editor is open: run library.collection.rules with an `id`")?;
        match args.get("edit").and_then(Value::as_str) {
            Some("undo") => {
                editor.undo();
            }
            Some("redo") => {
                editor.redo();
            }
            Some("cancel") => {
                self.focus.rule_editor = None;
                return Ok(Value::Null);
            }
            Some("apply") => {
                let rules = editor.tree.rule()?;
                let id = editor.album.clone();
                self.focus.rule_editor = None;
                return self.collection_edit(
                    crate::library::COLLECTION_RULES_COMMAND,
                    &json!({ "id": id, "rules": rules.to_json() }),
                );
            }
            _ => editor.edit(crate::rule_editor::Edit::parse(args)?)?,
        }
        Ok(editor.state())
    }

    /// The rule editor, when open: a modal over this window
    /// (`rule_editor_ui`), its edits run as `library.collection.rules.edit`.
    fn rule_editor_ui(&mut self, ctx: &Egui) {
        let Some(editor) = &self.focus.rule_editor else {
            return;
        };
        let albums: Vec<(String, String)> = self
            .focus
            .library
            .borrow()
            .albums()
            .map(|list| {
                list.iter()
                    .filter(|a| a.id != editor.album)
                    .map(|a| (a.id.clone(), a.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let t = |m: Message| self.text(m);
        let words = crate::rule_editor_ui::Words::new(&t);
        let (asked, rect) = crate::rule_editor_ui::show(ctx, editor, &albums, &words);
        self.focus
            .rects
            .insert(crate::rule_editor_ui::RECT.into(), rect);
        if let Some(args) = asked {
            let _ = self.run(
                crate::library::COLLECTION_RULES_EDIT_COMMAND,
                args,
                Origin::User,
            );
        }
    }

    /// Asks a smart album's conditions (`form` to start from): for a new
    /// one (`id` none) with its name, then runs `library.collection.create`;
    /// else `library.collection.rules` on that album.
    fn smart_form(&mut self, id: Option<String>, form: &crate::smart_album::SmartForm) {
        use crate::dialogs::Field;
        use crate::smart_album::{MATCHES, SmartForm as F};
        use helios_values::Descriptor as D;
        let creating = id.is_none();
        self.focus.album_ruled = id;
        let mut fields = Vec::new();
        let mut field = |key: &str, descriptor: D, value: helios_core::Value| {
            fields.push(Field {
                key: key.into(),
                descriptor,
                value,
            });
        };
        let label = |s: &Self, message: Message| s.text(message);
        let text = |s: &str| helios_core::Value::Text(s.to_string());
        let choice = |s: &str| helios_core::Value::Choice(s.to_string());
        if creating {
            field(
                "name",
                D::text(
                    &label(self, Message::new("gui-library-album-name", "name")),
                    "",
                ),
                text(""),
            );
        }
        let mode = if form.any { "any" } else { "all" };
        field(
            "match",
            D::choice(
                &label(self, Message::new("gui-smart-match", "match")),
                &MATCHES,
                "all",
            ),
            choice(mode),
        );
        field(
            "rating",
            D::int(
                &label(self, Message::new("gui-smart-rating", "at least (stars)")),
                0,
                5,
                0,
            ),
            helios_core::Value::Int(form.rating),
        );
        field(
            "label",
            D::choice(
                &label(self, Message::new("gui-smart-label", "colour label")),
                &F::labels(),
                crate::smart_album::ANY,
            ),
            choice(&form.label),
        );
        field(
            "flag",
            D::choice(
                &label(self, Message::new("gui-smart-flag", "flag")),
                &F::flags(),
                crate::smart_album::ANY,
            ),
            choice(&form.flag),
        );
        for (key, message, value) in [
            ("tag", Message::new("gui-smart-tag", "tag"), &form.tag),
            (
                "camera",
                Message::new("gui-smart-camera", "camera (contains)"),
                &form.camera,
            ),
            (
                "file_type",
                Message::new("gui-smart-file-type", "file type"),
                &form.file_type,
            ),
            (
                "folder",
                Message::new("gui-smart-folder", "folder (absolute)"),
                &form.folder,
            ),
            (
                "taken_from",
                Message::new("gui-smart-taken-from", "taken from (YYYY-MM-DD)"),
                &form.taken_from,
            ),
            (
                "taken_to",
                Message::new("gui-smart-taken-to", "taken to (YYYY-MM-DD)"),
                &form.taken_to,
            ),
        ] {
            field(key, D::text(&label(self, message), ""), text(value));
        }
        let (title, verb, command) = if creating {
            (
                Message::new("gui-smart-new-title", "New smart album"),
                Message::new("gui-library-album-new-verb", "Create"),
                crate::library::COLLECTION_CREATE_COMMAND,
            )
        } else {
            (
                Message::new("gui-smart-rules-title", "The smart album's rules"),
                Message::new("gui-smart-rules-verb", "Apply"),
                crate::library::COLLECTION_RULES_COMMAND,
            )
        };
        self.form(crate::dialogs::Form {
            title: self.text(title),
            fields,
            verb: self.text(verb),
            command: command.into(),
        });
    }

    /// Asks an album's name (`current` to start from), then runs `command`
    /// with it.
    /// The form a new album set's name is asked in, the set `parent`
    /// it goes in kept with the answer.
    fn set_form(&mut self, parent: Option<&str>) {
        let command =
            CommandId::new(crate::library::COLLECTION_CREATE_COMMAND).map_err(|e| e.to_string());
        let command = match command {
            Ok(id) => id,
            Err(e) => return self.warn(e),
        };
        let title = self.text(Message::new("gui-library-set-new-title", "New album set"));
        let verb = self.text(Message::new("gui-library-album-new-verb", "Create"));
        let label = self.text(Message::new("gui-library-album-name", "name"));
        let mut form = dialogs::Form::new(&title, &verb, command).field_at(
            "name",
            crate::dialogs::descriptor(&helios_values::Descriptor::text(&label, "")),
            crate::dialogs::value(&helios_core::Value::Text(String::new())),
        );
        form.args = json!({ "kind": helios_vault::collections::SET, "set": parent });
        self.open_dialog(form);
    }

    /// "Move to set…": the sets `album` may go in (not itself nor one
    /// below it), the top first when it is in one; the choice runs
    /// `library.collection.move` with its `set`.
    fn move_chooser(
        &mut self,
        album: &helios_vault::collections::Album,
        albums: &[helios_vault::collections::Album],
    ) {
        let mut options = Vec::new();
        if album.set.is_some() {
            let top = self.text(Message::new("gui-library-set-top", "No set (the top)"));
            options.push(ChooseOption::new(&top, Value::Null));
        }
        let hint = self.text(Message::new("gui-library-set-hint", "album set"));
        for set in albums.iter().filter(|a| {
            a.is_set()
                && album.set.as_deref() != Some(a.id.as_str())
                && sets::check_target(albums, &a.id, Some(&album.id)).is_ok()
        }) {
            options.push(ChooseOption::new(&set.name, json!(set.id)).hint(&hint));
        }
        if options.is_empty() {
            self.notify(
                Level::Info,
                Message::new(
                    "gui-library-set-none",
                    "No album set to move it to: make one with New set…",
                ),
            );
            return;
        }
        let title = self.text(
            Message::new("gui-library-set-move-title", "Move “{ $name }” to a set")
                .with("name", album.name.as_str()),
        );
        let Ok(command) = CommandId::new(crate::library::COLLECTION_MOVE_COMMAND) else {
            return;
        };
        self.open_dialog(
            Choose::with_options(&title, options, command)
                .args(json!({ "id": album.id }))
                .value_key("set"),
        );
    }

    fn album_form(&mut self, command: &str, current: &str) {
        let (title, verb) = if current.is_empty() {
            (
                Message::new("gui-library-album-new-title", "New album"),
                Message::new("gui-library-album-new-verb", "Create"),
            )
        } else {
            (
                Message::new("gui-library-album-rename-title", "Rename the album"),
                Message::new("gui-library-album-rename-verb", "Rename"),
            )
        };
        self.form(crate::dialogs::Form {
            title: self.text(title),
            fields: vec![crate::dialogs::Field {
                key: "name".into(),
                descriptor: helios_values::Descriptor::text(
                    &self.text(Message::new("gui-library-album-name", "name")),
                    current,
                ),
                value: helios_core::Value::Text(current.to_string()),
            }],
            verb: self.text(verb),
            command: command.into(),
        });
    }

    /// Compares photos side by side: the Compare place on `images` (ids),
    /// else on the selection; two to four photos, the first active.
    fn compare_photos(&mut self, args: &Value) -> Result<Value, String> {
        use egui_pop::views::library::compare::MAX_PANES;
        let mut ids: Vec<String> = match args.get("images").and_then(Value::as_array) {
            Some(list) => list
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            None => self.focus.library.borrow().targets().unwrap_or_default(),
        };
        let mut seen = std::collections::HashSet::new();
        ids.retain(|id| seen.insert(id.clone()));
        {
            let library = self.focus.library.borrow();
            if let Some(unknown) = ids.iter().find(|id| !library.holds(id)) {
                return Err(format!("the library has no photo `{unknown}`"));
            }
        }
        if ids.len() < 2 {
            return Err(self.text(Message::new(
                "gui-compare-too-few",
                "Select two or more photos to compare them",
            )));
        }
        if ids.len() > MAX_PANES {
            ids.truncate(MAX_PANES);
            self.note(
                Message::new(
                    "gui-compare-first",
                    "Compare shows { $count } photos at most: the first { $count } are compared",
                )
                .with("count", MAX_PANES),
            );
        }
        let payload = helios_template::contexts::images_payload(&ids);
        let done = crate::contexts::go(
            self.core.app.handle(),
            crate::contexts::COMPARISON,
            payload,
            &egui_pop::model::commands::Origin::User,
        )?;
        self.follow_comparison();
        Ok(json!({ "images": ids, "place": done }))
    }

    /// The comparison follows the Compare place: its photos are the
    /// place's (none elsewhere: the Compare tab compares the selection);
    /// entering it on photos selects the first, the active one. A
    /// workspace that opens in Compare goes there once its stack is read.
    fn follow_comparison(&mut self) {
        let app = self.core.app.handle().clone();
        if self.focus.open_in_compare && crate::contexts::at_root(&app) {
            self.focus.open_in_compare = false;
            let _ = crate::contexts::go(
                &app,
                crate::contexts::COMPARISON,
                helios_template::contexts::images_payload(&[]),
                &egui_pop::model::commands::Origin::User,
            );
        } else if egui_pop::api::contexts::state(&app).is_some() {
            self.focus.open_in_compare = false;
        }
        let photos: Vec<String> = {
            let library = self.focus.library.borrow();
            crate::contexts::compared(&app)
                .unwrap_or_default()
                .into_iter()
                .filter(|id| library.holds(id))
                .collect()
        };
        let mut cmp = self.focus.comparison.borrow_mut();
        if photos.as_slice() == cmp.photos() {
            return;
        }
        cmp.set_photos(photos);
        if let Some(first) = cmp.photos().first() {
            self.focus
                .library
                .borrow_mut()
                .select_ids(std::slice::from_ref(first));
        }
    }

    /// The workspace's import source, as its template set it.
    pub fn import_source(&self) -> Option<&helios_template::ImportDesc> {
        self.focus.import_source.as_ref()
    }

    /// Whether the workspace proposes stacks after each import.
    pub fn suggests_stacks(&self) -> bool {
        self.focus
            .import_source
            .as_ref()
            .is_some_and(|i| i.suggest_stacks)
    }

    /// The imports asked for since the last call ([`IMPORT_COMMAND`]'s
    /// arguments), for the host to run.
    pub fn take_imports(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.core.imports)
    }

    /// Says whether the host has a library to import into: it runs the
    /// imports asked for as `library.import`, which it registers only
    /// with a library open. Without one, the import window's Import
    /// button is off and every import says why. Open by default (an
    /// embedder or a test running the imports itself).
    pub fn set_library_open(&mut self, open: bool) {
        self.core.no_library = (!open).then(import::no_library);
    }

    /// No library to import into, for the reason `error` gives (it could
    /// not be opened), else for want of one (`None`).
    pub fn set_no_library(&mut self, error: Option<String>) {
        self.core.no_library = Some(error.map_or_else(import::no_library, |e| {
            Message::new(
                "gui-import-no-library-error",
                "No library is open: { $error }",
            )
            .with("error", e)
        }));
    }

    /// Whether a library is open to import into ([`Shell::set_library_open`]).
    pub fn library_open(&self) -> bool {
        self.core.no_library.is_none()
    }

    /// Whether a command of that id is registered.
    pub fn has_command(&self, id: &str) -> bool {
        lock(&self.core.registry).specs().any(|s| s.id == id)
    }

    /// A workspace from a template: the templates to pick from (the
    /// built-in ones and the user's, with their scenario) without one, the
    /// directory and the template's parameters to ask for without one,
    /// else the workspace created and opened in its window.
    fn new_workspace(
        &mut self,
        ctx: &Egui,
        template: Option<String>,
        path: Option<PathBuf>,
        args: &Value,
    ) -> Result<Value, String> {
        let templates = crate::templates::all(&config_dir());
        // The user's templates speak through their own text too (added
        // once: a second time is refused, and nothing changes).
        let _ = crate::templates::add_texts(
            &self.core.i18n,
            &templates[crate::templates::builtin().len()..],
        );
        let name = |s: &Self, b: &helios_template::Bundle| {
            s.text(Message::new(&b.name_key(), &b.template.meta.name))
        };
        let Some(id) = template else {
            let choices = templates
                .iter()
                .map(|b| crate::dialogs::Choice {
                    value: b.template.id.clone(),
                    label: name(self, b),
                    detail: Some(
                        self.text(Message::new(&b.scenario_key(), &b.template.meta.scenario)),
                    ),
                })
                .collect();
            self.pick(crate::dialogs::Picker {
                title: self.text(Message::new(
                    "gui-new-workspace-title",
                    "New workspace from a template",
                )),
                choices,
                command: "workspace.new".into(),
                arg: "template".into(),
            });
            return Ok(Value::Null);
        };
        let bundle = templates
            .iter()
            .find(|b| b.template.id == id)
            .ok_or_else(|| {
                self.text(
                    Message::new("gui-new-workspace-unknown", "no template { $id }")
                        .with("id", &id),
                )
            })?;
        let Some(path) = path else {
            let ids: Vec<&str> = templates.iter().map(|b| b.template.id.as_str()).collect();
            let asked = self.parameter_fields(bundle);
            self.form(crate::dialogs::Form {
                title: self.text(
                    Message::new(
                        "gui-new-workspace-form-title",
                        "New workspace: { $template }",
                    )
                    .with("template", name(self, bundle)),
                ),
                fields: vec![
                    crate::dialogs::Field {
                        key: "template".into(),
                        descriptor: helios_values::Descriptor::choice(
                            &self.text(Message::new("gui-new-workspace-template", "template")),
                            &ids,
                            &id,
                        ),
                        value: helios_core::Value::Choice(id.clone()),
                    },
                    crate::dialogs::Field {
                        key: "path".into(),
                        descriptor: helios_values::Descriptor::path(
                            &self.text(Message::new("gui-new-workspace-directory", "directory")),
                            helios_values::PathKind::Dir,
                            &[],
                        ),
                        value: helios_core::Value::Text(String::new()),
                    },
                ]
                .into_iter()
                .chain(asked)
                .collect(),
                verb: self.text(Message::new("gui-new-workspace-verb", "Create")),
                command: "workspace.new".into(),
            });
            return Ok(Value::Null);
        };
        let catalogue = crate::templates::catalogue(&self.template_panels(), self.specs());
        let answers: BTreeMap<String, Value> = args
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| {
                let id = k.strip_prefix("parameter.")?;
                v.as_str()
                    .filter(|t| !t.is_empty())
                    .map(|t| (id.to_string(), json!(t)))
            })
            .collect();
        let created =
            crate::templates::create_answered(bundle, &answers, &catalogue, &path, &config_dir())?;
        self.note(
            Message::new(
                "gui-new-workspace-created",
                "workspace { $path } created from { $template }",
            )
            .with("path", created.workspace.display().to_string())
            .with("template", name(self, bundle)),
        );
        self.apply(ctx, Action::OpenWorkspace(Some(created.workspace)))
    }

    /// The fields a template's parameters are asked in, each at the
    /// template's own value (`parameter.<id>`).
    fn parameter_fields(&self, bundle: &helios_template::Bundle) -> Vec<crate::dialogs::Field> {
        let json = serde_json::to_value(&bundle.template).unwrap_or_default();
        bundle
            .template
            .parameters
            .iter()
            .map(|q| {
                let label = self.text(Message::new(&bundle.parameter_key(&q.id), &q.name));
                let value = json
                    .pointer(&q.at)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let (descriptor, value) = match q.kind {
                    helios_template::ParameterKind::Choice => {
                        let choices: Vec<&str> = q.choices.iter().map(String::as_str).collect();
                        (
                            helios_values::Descriptor::choice(&label, &choices, &value),
                            helios_core::Value::Choice(value),
                        )
                    }
                    helios_template::ParameterKind::Path => (
                        helios_values::Descriptor::path(&label, helios_values::PathKind::File, &[]),
                        helios_core::Value::Text(value),
                    ),
                    helios_template::ParameterKind::Text => (
                        helios_values::Descriptor::text(&label, &value),
                        helios_core::Value::Text(value),
                    ),
                };
                crate::dialogs::Field {
                    key: format!("parameter.{}", q.id),
                    descriptor,
                    value,
                }
            })
            .collect()
    }

    fn apply(&mut self, ctx: &Egui, action: Action) -> Result<Value, String> {
        match action {
            Action::Palette => {
                return self.run(
                    egui_pop::api::app::CMD_PALETTE.as_str(),
                    Value::Null,
                    Origin::User,
                );
            }
            Action::QuickOpen => self.quick_open(),
            Action::QuickOpenGo(item) => return self.quick_open_go(item),
            Action::Fullscreen => {
                let on = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!on));
            }
            // The frame's steps, which plugins may veto or replace.
            Action::Minimize => frame::request(self.core.app.handle(), ctx, FrameRequest::Minimize),
            Action::Maximize => {
                frame::request(self.core.app.handle(), ctx, FrameRequest::ToggleMaximize);
            }
            Action::Quit => self.quit(ctx),
            Action::QuitConfirmed => self.quit_confirmed(),
            Action::NoticeDetails(i) => self.notice_details(i),
            Action::MainWindow => self.main_window(),
            Action::SystemMenu => {
                // Under the title bar's start, where its menu button is
                // (the window's top left when the system draws the frame).
                self.focus.menu = None;
                let at = HitTable::of(ctx)
                    .get(self.focus.window)
                    .and_then(|r| r.caption)
                    .map_or(egui::pos2(0.0, 0.0), |c| {
                        let ppp = ctx.pixels_per_point();
                        egui::pos2(c.x as f32 / ppp, (c.y + c.h) as f32 / ppp)
                    });
                frame::request(self.core.app.handle(), ctx, FrameRequest::SystemMenu(at));
            }
            Action::Run(id, args, origin) => return self.run(&id, args, origin),
            Action::KeymapPackage(name, on) => return self.keymap_package(&name, on),
            Action::Keybindings => {
                return self.run(
                    egui_pop::api::admin::CMD_OPEN.as_str(),
                    serde_json::json!({ "path": "input/shortcuts" }),
                    Origin::User,
                );
            }
            Action::OpenWorkspace(None) => self.form(crate::dialogs::Form {
                title: self.text(Message::new("gui-open-workspace-title", "Open a workspace")),
                fields: vec![crate::dialogs::Field {
                    key: "path".into(),
                    descriptor: helios_values::Descriptor::path(
                        &self.text(Message::new("gui-open-workspace-directory", "directory")),
                        helios_values::PathKind::Dir,
                        &[],
                    ),
                    value: helios_core::Value::Text(String::new()),
                }],
                verb: self.text(Message::new("gui-open-workspace-verb", "Open")),
                command: "workspace.open".into(),
            }),
            Action::OpenWorkspace(Some(path)) => {
                // One window per workspace: another process opens it,
                // which does not hand it back to this one.
                std::env::current_exe()
                    .and_then(|exe| {
                        std::process::Command::new(exe)
                            .arg(crate::platform::NEW_INSTANCE)
                            .arg(&path)
                            .spawn()
                    })
                    .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
                self.note(Message::new("gui-opening", "opening { $path }").with("path", &path));
            }
            Action::NewWorkspace(template, path, args) => {
                return self.new_workspace(ctx, template, path, &args);
            }
            Action::WorkspaceEdit(id, args, origin) => {
                return self.workspace_command(&id, &args, &origin);
            }
            Action::Theme(name) => {
                let overrides = self.core.theme_overrides.clone();
                self.set_theme(&name, overrides);
            }
            Action::ThemeExport(path) => {
                // The resolved theme as one file, under its name.
                let mut tokens = self.core.theme.resolved().to_dtcg();
                tokens["$name"] = json!(self.core.theme_name);
                let text = serde_json::to_string_pretty(&tokens).expect("tokens serialize");
                path.parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| std::fs::write(&path, text))
                    .map_err(|e| format!("cannot export the theme to {}: {e}", path.display()))?;
                self.note(
                    Message::new("gui-theme-exported", "theme exported to { $path }")
                        .with("path", &path),
                );
            }
            Action::ProfileExport(path, parts, this_machine) => {
                let parts = if parts.is_empty() {
                    helios_profile::Part::ALL.to_vec()
                } else {
                    parts
                };
                // What the shell holds is this machine's view; the
                // profile's variants travel unless asked otherwise.
                let profile = match &self.core.loaded {
                    Some((source, resolved)) if !this_machine => {
                        self.profile().rebase(source, &resolved.profile)
                    }
                    _ => self.profile(),
                };
                let present: Vec<_> = parts.into_iter().filter(|p| profile.has(*p)).collect();
                // No plugin settings yet to keep home.
                profile
                    .export(&present, &|_, _| false)
                    .and_then(|text| {
                        path.parent()
                            .map_or(Ok(()), std::fs::create_dir_all)
                            .and_then(|()| std::fs::write(&path, text))
                            .map_err(|e| format!("{}: {e}", path.display()))
                    })
                    .map_err(|e| format!("profile not exported: {e}"))?;
                self.note(
                    Message::new("gui-profile-exported", "profile exported to { $path }")
                        .with("path", &path),
                );
            }
            Action::ProfileImport(path) => {
                let incoming =
                    Self::read_profile(&path).map_err(|e| format!("profile not imported: {e}"))?;
                let changes = helios_profile::diff(&self.profile(), &incoming);
                let count: usize = changes.iter().map(|d| d.changes.len()).sum();
                self.note(
                    Message::new(
                        "gui-profile-import-changes",
                        "profile { $name }: { $count } changes; profile.import.apply to apply them",
                    )
                    .with("name", &incoming.name)
                    .with("count", count),
                );
                for d in &changes {
                    for c in &d.changes {
                        self.note(format!("{:?} {c}", d.part));
                    }
                }
                self.core.pending_import = Some(incoming);
            }
            Action::ProfileApply => {
                let p = self
                    .core
                    .pending_import
                    .take()
                    .ok_or("no profile read to apply: profile.import first")?;
                let name = p.name.clone();
                self.apply_profile(&p);
                self.note(
                    Message::new("gui-profile-applied", "profile { $name } applied")
                        .with("name", &name),
                );
            }
            Action::ProfileUse(name) => self
                .use_profile(&name)
                .map_err(|e| format!("profile {name}: {e}"))?,
            Action::ScopeMode(mode) => self.focus.scopes.borrow_mut().set_mode(mode),
            Action::ScopeStops(on) => self.focus.scopes.borrow_mut().set_stops(on),
            Action::Decision(request, origin) => return self.answer_decision(request, &origin),
            Action::DecisionDecide(node, values, origin) => {
                let values = {
                    let viewport = self.focus.viewport.borrow();
                    let mut decisions = self.focus.decisions.borrow_mut();
                    decisions.refresh(viewport.graph());
                    decisions.values_of(&node, &values)?
                };
                return self
                    .answer_decision(crate::decisions::Request::Decide { node, values }, &origin);
            }
            Action::DecisionList => {
                let viewport = self.focus.viewport.borrow();
                let mut decisions = self.focus.decisions.borrow_mut();
                decisions.refresh(viewport.graph());
                return Ok(decisions.describe());
            }
            Action::Rate(n) => self
                .focus
                .library
                .borrow_mut()
                .rate(n)
                .map_err(|e| format!("vault.rate: {e}"))?,
            Action::Reject => self
                .focus
                .library
                .borrow_mut()
                .toggle_reject()
                .map_err(|e| format!("vault.reject: {e}"))?,
            Action::Label(l) => self
                .focus
                .library
                .borrow_mut()
                .toggle_label(&l)
                .map_err(|e| format!("vault.label: {e}"))?,
            Action::Step(n) => self.focus.library.borrow_mut().step(n),
            Action::Flag(flag) => self
                .focus
                .library
                .borrow_mut()
                .flag(flag)
                .map_err(|e| format!("vault.flag: {e}"))?,
            Action::AutoAdvance(on) => {
                let mut library = self.focus.library.borrow_mut();
                library.auto_advance = on.unwrap_or(!library.auto_advance);
            }
            Action::Place(frame) => {
                let payload = if frame == crate::contexts::WORKSPACE {
                    Value::from(self.workspace().ok_or("no workspace is open")?)
                } else {
                    Value::Null
                };
                return crate::contexts::go(
                    self.core.app.handle(),
                    frame,
                    payload,
                    &egui_pop::model::commands::Origin::User,
                );
            }
            Action::ComparePhotos(args) => return self.compare_photos(&args),
            Action::Sort(args, origin) => return self.sort_library(&args, &origin),
            Action::Folder(args, origin) => return self.folder_library(&args, &origin),
            Action::Collection(args, origin) => return self.collection_library(&args, &origin),
            Action::CollectionEdit(id, args) => return self.collection_edit(id, &args),
            Action::RecentFilters(id, args) => return self.recent_filters(id, &args),
            Action::ScanFolder(args) => {
                let path = args
                    .get("path")
                    .and_then(Value::as_str)
                    .filter(|p| Path::new(p).is_absolute())
                    .ok_or("`path`: an absolute folder")?;
                let asked = self.focus.library.borrow().scan_folder(Path::new(path));
                return Ok(json!({ "asked": asked }));
            }
            Action::CompareStep(step) => {
                let selection = self.focus.library.borrow().selection();
                let mut cmp = self.focus.comparison.borrow_mut();
                let shown = cmp.shown(&selection);
                if shown.len() < 2 {
                    return Err("no photos are compared".into());
                }
                cmp.step(step, shown.len());
                let active = cmp.active(&shown).cloned();
                if !cmp.photos().is_empty()
                    && let Some(active) = active
                {
                    self.focus.library.borrow_mut().select_ids(&[active]);
                }
            }
            Action::CompareSync(on) => {
                let mut cmp = self.focus.comparison.borrow_mut();
                cmp.synced = on.unwrap_or(!cmp.synced);
                return Ok(json!({ "synced": cmp.synced }));
            }
            Action::EnterSubworkspace(id, origin) => {
                let id = if let Some(id) = id {
                    id
                } else {
                    let node = crate::modules::selected(&lock(&self.core.values))
                        .ok_or("no node is selected: name the sub-workspace (`id`)")?;
                    let shown = crate::contexts::subworkspace_shown(self.core.app.handle());
                    self.focus
                        .subworkspaces
                        .child_holding(shown.as_deref(), &node)
                        .ok_or_else(|| {
                            format!("`{node}` is in no sub-workspace under the place shown")
                        })?
                };
                return crate::contexts::enter_subworkspace(
                    self.core.app.handle(),
                    &self.focus.subworkspaces,
                    &id,
                    &crate::host::to_pop(&origin),
                );
            }
            Action::Culling(on) => {
                return crate::contexts::culling(
                    self.core.app.handle(),
                    on,
                    &egui_pop::model::commands::Origin::User,
                );
            }
            Action::ImportSource(args, origin) => return self.import_from_source(&args, &origin),
            Action::ImportWindow(args) => {
                // The steps import requires first (the library folder).
                if !self.ensure_for_import(&args) {
                    self.open_import_window(&args);
                }
                return Ok(Value::Null);
            }
            Action::InitSetUp => return self.init_setup(),
            Action::InitLibraryFolder(args) => return self.init_library_folder(&args),
            Action::InitApply(args) => return self.init_apply(&args),
            Action::InitChooseFolder => return self.init_choose_folder(),
            Action::PauseAutomaticImports(args) => return self.pause_automatic_imports(&args),
            Action::Calibrate(args) => {
                self.open_calibration(&args);
                return Ok(Value::Null);
            }
            Action::Account(id, args) => return self.account_command(id, &args),
            Action::Health => return self.open_health(),
            Action::HealthFix(args) => return self.health_fix(&args),
            Action::HealthIgnore(args) => return self.health_ignore(&args),
            Action::HealthCheckNow => {
                let started = self.health_check_now();
                return Ok(serde_json::json!({ "started": started }));
            }
            Action::Storage(args) => return self.open_storage(&args),
            Action::ImportFiles(args, origin) => return self.import_files(&args, &origin),
            Action::ImportWith(args, origin) => return self.import_with(&args, &origin),
            Action::ImportHistory(args) => return self.import_history_list(&args),
            Action::ImportAgain(args, origin) => return self.import_again(&args, &origin),
            Action::Export(args, origin) => return self.export(&args, &origin),
            Action::OpenFolder(args) => return Self::open_folder(&args),
            Action::ChooseLibraryFolder(args) => return self.choose_library_folder(&args),
            Action::ToggleStack => self
                .focus
                .library
                .borrow_mut()
                .toggle_stack()
                .map_err(|e| format!("vault.stack.toggle: {e}"))?,
            Action::LibraryBatch(id, args) => {
                return self
                    .library_batch(id, &args)
                    .map_err(|e| format!("{id}: {e}"));
            }
            Action::Zoom(z) => self.focus.viewport.borrow_mut().zoom(z, &self.core.values),
            Action::Fold(area) => {
                let h = self.core.app.handle();
                let folded = h.layout().folded.contains(&area);
                h.fold_area(area, !folded);
            }
            Action::Mask(command, origin) => {
                // A person taking out a group that holds members (the
                // row's menu, the palette, a key) is asked first; agents
                // and scripts are not.
                if let crate::masks::Command::MemberDelete {
                    member,
                    confirmed: false,
                } = &command
                    && origin == Origin::User
                {
                    let ask = {
                        let store = lock(&self.core.values);
                        let viewport = self.focus.viewport.borrow();
                        viewport.graph().and_then(|g| {
                            let net = crate::mask_tree::network(g, &store, member.as_deref());
                            let key = crate::mask_tree::member_or_selected(
                                &net,
                                &store,
                                member.as_deref(),
                            )
                            .ok()?;
                            crate::mask_tree::confirm_take_out(ctx, &net, &key)
                        })
                    };
                    if let Some(ask) = ask {
                        self.confirm(ask);
                        return Ok(json!({ "asked": true }));
                    }
                }
                let mut store = lock(&self.core.values);
                return self
                    .focus
                    .viewport
                    .borrow_mut()
                    .mask(command, origin, &mut store);
            }
            Action::RenameInTree(member) => {
                self.focus.mask_tree.borrow_mut().start_rename(member)?;
            }
            Action::MaskDisplay(mode, origin) => {
                let mut store = lock(&self.core.values);
                let now = match mode {
                    Some(m) => crate::mask_display::set(&mut store, m, &origin)?,
                    None => crate::mask_display::cycle(&mut store, &origin)?,
                };
                // What it shows: the node the selection gives, if the
                // selected module has a mask.
                let target = self
                    .focus
                    .viewport
                    .borrow()
                    .graph()
                    .and_then(|g| crate::mask_display::target(g, &store));
                return Ok(json!({
                    "mode": now.name(),
                    "mask": target.map(|t| t.key),
                }));
            }
            Action::Bypass(node, on, origin) => {
                let mut store = lock(&self.core.values);
                let (node, on) = crate::modules::bypass(&mut store, node.as_deref(), on, &origin)?;
                return Ok(json!({ "node": node, "bypassed": on }));
            }
            Action::Compare => {
                use crate::viewport::COMPARE;
                let mut store = lock(&self.core.values);
                let on = store
                    .get(COMPARE)
                    .and_then(|e| e.value.as_bool())
                    .unwrap_or(false);
                store
                    .set(helios_values::Set::new(
                        COMPARE,
                        helios_values::Change::To(helios_core::Value::Bool(!on)),
                        Origin::User,
                    ))
                    .map_err(|e| e.to_string())?;
            }
            Action::CopyParam(path, text, origin) => {
                let out = crate::clipboard::copy_param(
                    self.core.app.handle(),
                    &self.core.clip.param,
                    &path,
                    text,
                    &to_pop(&origin),
                )?;
                let shown = {
                    let store = lock(&self.core.values);
                    store.get(&path).map(|e| match text {
                        crate::clipboard::param::Text::Value => {
                            crate::clipboard::param::shown(&e.descriptor, &e.value)
                        }
                        crate::clipboard::param::Text::Path => path.clone(),
                    })
                };
                if let Some(t) = shown {
                    self.note(Message::new("gui-copied", "copied { $text }").with("text", &t));
                }
                return Ok(out);
            }
            Action::FitCameraLook(args, origin) => return self.fit_camera_look(&args, origin),
            Action::CopyFiles(origin) => {
                user_only(&origin)?;
                let out = crate::clipboard::copy_files(&self.core.clip.images, &self.core.system)?;
                let count = out["files"].as_array().map_or(0, Vec::len);
                let missing = out["missing"].as_array().map_or(0, Vec::len);
                if missing == 0 {
                    self.note(
                        Message::new("gui-copied-files", "copied { $count } files")
                            .with("count", count),
                    );
                } else {
                    self.warn(
                        Message::new(
                            "gui-copied-files-missing",
                            "copied { $count } files; { $missing } without a file left out",
                        )
                        .with("count", count)
                        .with("missing", missing),
                    );
                }
                return Ok(out);
            }
            Action::PasteFiles(dry_run, origin) => {
                return crate::clipboard::paste_from_system(
                    self.core.app.handle(),
                    &self.core.system,
                    dry_run,
                    &to_pop(&origin),
                );
            }
            Action::CopyImage(origin) => {
                user_only(&origin)?;
                return self.copy_image();
            }
            Action::PasteParam(path, dry_run, origin) => {
                return crate::clipboard::paste_param(
                    self.core.app.handle(),
                    &self.core.clip.param,
                    &path,
                    dry_run,
                    &to_pop(&origin),
                );
            }
            // The workbench's steps: the panel's rules, then any plugin,
            // may refuse.
            Action::MoveTo(p, area) => {
                return workbench_outcome(self.core.app.handle().move_panel(&panel_id(&p)?, area));
            }
            Action::Answer(answer, origin) => self.answer(answer, &origin)?,
            Action::AssumeYes(on) => {
                self.core.assume_yes = on;
                self.note(if on {
                    Message::new("gui-policy-yes", "confirmations are answered yes by policy")
                } else {
                    Message::new("gui-policy-ask", "confirmations are asked again")
                });
            }
            Action::Detach(p) => {
                return workbench_outcome(self.core.app.handle().detach_panel(&panel_id(&p)?));
            }
            Action::Float(p, pos) => {
                let id = panel_id(&p)?;
                let h = self.core.app.handle();
                let pos = pos.unwrap_or_else(|| {
                    egui_pop::model::layout::FloatingGroup::cascade(h.layout().floating.len())
                });
                return workbench_outcome(
                    h.change_workbench(egui_pop::api::workbench::Change::Float(id, pos)),
                );
            }
            Action::Dock(p) => {
                let id = panel_id(&p)?;
                let h = self.core.app.handle();
                let layout = h.layout();
                if layout.auxiliary.iter().any(|a| a.panel == id) {
                    return workbench_outcome(h.attach_panel(&id));
                }
                // A palette docks in its own area.
                if layout.floating_of(&id).is_some() {
                    let area = self
                        .core
                        .panels
                        .iter()
                        .find(|s| s.id == id)
                        .map_or(Area::EndSide, |s| s.area);
                    return workbench_outcome(
                        h.change_workbench(egui_pop::api::workbench::Change::Dock(id, area)),
                    );
                }
            }
            Action::Snapshot { target, path } if target == "viewport.scene" => {
                // The float data, before the display transform.
                self.viewport_scene()
                    .ok_or_else(|| "the viewport shows nothing yet".to_string())
                    .and_then(|scene| snapshot::save_scene(&scene, &path))
                    .map_err(|e| format!("viewport.scene: {e}"))?;
                self.note(
                    Message::new("gui-scene-saved", "viewport.scene saved to { $path }")
                        .with("path", &path),
                );
            }
            Action::Snapshot { target, path } => {
                self.focus.shots.push_back(Shot::Save { target, path });
                self.request_screenshot(ctx);
            }
            Action::RecordStart { target, path, fps } => {
                self.focus.recording = Some((target, Recorder::new(path, fps)));
            }
            Action::RecordStop => {
                if let Some((_, rec)) = self.focus.recording.take() {
                    let p = rec.finish().map_err(|e| format!("recording failed: {e}"))?;
                    self.note(
                        Message::new("gui-recording-saved", "recording saved to { $path }")
                            .with("path", &p),
                    );
                }
            }
        }
        Ok(Value::Null)
    }

    /// Applies what commands queued; an action may run commands that queue
    /// more (a dialog's answer): they apply in the same frame, a bounded
    /// number of rounds.
    fn apply_queue(&mut self, ctx: &Egui) {
        for _ in 0..8 {
            let actions = std::mem::take(&mut *lock(&self.core.queue));
            if actions.is_empty() {
                break;
            }
            for (a, resolver) in actions {
                let result = self.apply(ctx, a);
                if let Err(e) = &result {
                    self.warn(e.clone());
                }
                if let Some(r) = resolver {
                    r.resolve(result);
                }
            }
        }
    }

    /// A target's rectangle in the main window's screenshot: a target in
    /// a detached panel's window is not in it (that window has a scale and
    /// pixels of its own).
    fn pictured_rect(&self, target: &str) -> Result<Rect, String> {
        if self.target_window(target) != self.focus.window {
            return Err(format!(
                "`{target}` is in a detached window, which pictures do not reach yet"
            ));
        }
        self.target_rect(target)
    }

    /// Asks the window for a screenshot of its next frame; it comes back
    /// through `screenshots`, read at the start of a frame.
    fn request_screenshot(&self, ctx: &Egui) {
        let screenshots = Arc::clone(&self.focus.screenshots);
        let repaint = ctx.clone();
        ctx.request_screenshot(move |image| {
            lock(&screenshots).push_back(image);
            repaint.request_repaint();
        });
    }

    fn on_screenshot(&mut self, image: &egui::ColorImage) {
        let full = snapshot::from_color_image(image);
        let Some(shot) = self.focus.shots.pop_front() else {
            return;
        };
        match shot {
            Shot::Save { target, path } => {
                let note = self
                    .pictured_rect(&target)
                    .map(|r| snapshot::crop(&full, r, self.focus.pixels_per_point))
                    .and_then(|img| snapshot::save(&img, &path))
                    .map_or_else(
                        |e| {
                            Message::new("gui-snapshot-failed", "snapshot failed: { $error }")
                                .with("error", e)
                        },
                        |()| {
                            Message::new("gui-snapshot-saved", "{ $target } pictured to { $path }")
                                .with("target", &target)
                                .with("path", &path)
                        },
                    );
                self.note(note);
            }
            Shot::Frame => {
                let target = self.focus.recording.as_ref().map(|(t, _)| t.clone());
                if let Some(target) = target
                    && let rect = self.pictured_rect(&target)
                    && let Some((_, rec)) = &mut self.focus.recording
                {
                    let pushed = rect
                        .map(|r| snapshot::crop(&full, r, self.focus.pixels_per_point))
                        .and_then(|img| rec.push(&img));
                    if let Err(e) = pushed {
                        self.warn(
                            Message::new("gui-recording-stopped", "recording stopped: { $error }")
                                .with("error", e),
                        );
                        self.focus.recording = None;
                    }
                }
            }
        }
    }

    // --- Drawing ---

    /// Follows the window's scale (egui-pop's `scale::changed`, once per
    /// change; `scale.changed` on the journal). The screen is known from
    /// the first frame on, and again after a move to another screen: a
    /// profile with a `hidpi` variant is applied again when the screen's
    /// class changes. The interface zoom changes the scale, not the class.
    fn follow_scale(&mut self, ctx: &Egui) {
        self.focus.pixels_per_point = ctx.pixels_per_point();
        let native = scale::Scale::of(ctx).native;
        let changed = scale::changed(ctx).is_some() || self.focus.native_scale.is_none();
        let was = self.focus.native_scale.replace(native).unwrap_or(1.0);
        if changed
            && (was >= HIDPI) != (native >= HIDPI)
            && let Some((source, _)) = &self.core.loaded
            && source.on.contains_key(&helios_profile::Selector::Hidpi)
        {
            let source = source.clone();
            self.apply_profile(&source);
        }
    }

    /// Draws the shell for one frame.
    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if let Some(rs) = self.core.gpu.take() {
            self.core.gpu_info = Some(egui_pop::viewport::gpu::install(&ctx, &rs));
            crate::start::mark_in_first_frame("viewport GPU path installed");
        }
        self.follow_scale(&ctx);
        // The main window asked back (Helios ran with no window, macOS).
        self.show_reopened(&ctx);
        self.core.now = ctx.input(|i| i.time);
        let screenshots = std::mem::take(&mut *lock(&self.focus.screenshots));
        for image in screenshots {
            self.on_screenshot(&image);
        }
        // What panels asked for during the last frame.
        for r in crate::dialogs::take_requests(&ctx) {
            use crate::dialogs::Request;
            match r {
                Request::Confirm(c) => self.confirm(c),
                Request::Form(f) => self.form(f),
                Request::Picker(p) => self.pick(p),
                Request::Menu { at, items } => self.open_menu(at, items),
                Request::Notify(level, text) => self.notify(level, text),
                Request::Run(id, args, origin) => {
                    let _ = self.run(&id, args, origin);
                }
            }
        }
        self.set_context(&ctx);
        let wanted = crate::display::wanted(crate::display::wide_gamut(self.core.app.handle()));
        self.focus.viewport.borrow_mut().set_wanted(wanted);
        self.core.app.logic(&ctx);
        // The workspace's sub-workspaces; one shown that is gone is left.
        self.follow_subworkspaces();
        // New undo steps are made in the place the stack shows now, and
        // inside a sub-workspace only its nodes may be written.
        crate::contexts::follow_place(
            self.core.app.handle(),
            &mut lock(&self.core.values),
            &self.focus.subworkspaces,
        );
        // The History panel's snapshots: a toast for each restore.
        self.follow_snapshots();
        // The node's endpoint lists and runs the windows' commands.
        if let Some(asks) = self.window_asks.take() {
            crate::window_commands::answer(self, &asks, &ctx);
            self.window_asks = Some(asks);
        }
        self.apply_queue(&ctx);
        // The workspace's files changed outside, and agents' requests.
        self.follow_workspace(&ctx);
        // Keys changed for the workspace in the shortcuts editor.
        self.follow_workspace_keys();
        self.focus
            .follow
            .frame(self.core.app.handle(), &mut lock(&self.core.values));
        self.follow_comparison();
        // An undo or a redo of the workspace's state, put in place.
        self.sync_workspace();
        // The workshop session's record follows the value steps.
        self.follow_session(None);
        // The theme asked for is installed here, before anything is drawn.
        self.core.app.logic(&ctx);
        let spec = self.core.app.handle().theme().spec();
        if self.core.installed.as_ref() != Some(&spec) {
            if self.core.still {
                ctx.all_styles_mut(|style| {
                    style.visuals.text_cursor.blink = false;
                    style.animation_time = 0.0;
                });
            }
            self.follow_theme(&spec);
            self.core.installed = Some(spec);
        }
        self.set_context(&ctx);
        self.follow_language();
        crate::start::mark_in_first_frame("commands run, theme installed");

        self.focus.rects.clear();
        self.focus.rect_windows.clear();
        ctx.data_mut(|d| {
            d.remove::<BTreeMap<String, (Rect, egui::ViewportId)>>(Id::unique("helios.named"));
        });
        self.focus.rects.insert("window".into(), ctx.content_rect());

        self.focus
            .viewport
            .borrow_mut()
            .sync(&mut lock(&self.core.values));
        self.follow_opening(&ctx);
        self.follow_decisions();
        self.status_bar(&ctx);
        self.status_items(&ctx);
        self.health_tick(&ctx);
        self.init_tick();
        // Duplicates an import asks about: said once, answered in the
        // Duplicates panel.
        let asked = self.receive_imports();
        self.select_imported_photos();
        self.receive_exports();
        if asked > 0 {
            self.note(
                Message::new(
                    "gui-duplicates-waiting-note",
                    "{ $count } duplicates wait for an answer in the Duplicates panel",
                )
                .with("count", asked),
            );
        }
        self.focus
            .preview
            .follow(self.focus.viewport.borrow().shown(), &ctx);
        self.core.app.show(ui);
        crate::start::mark_in_first_frame("panels drawn");
        crate::workbench::keep_focus_taken(&ctx);
        self.place_palettes();
        self.graph_edits();
        if self.focus.library.borrow_mut().take_import_asked() {
            let _ = self.run(import_window::COMMAND, Value::Null, Origin::User);
        }
        self.take_dropped_files(&ctx);
        self.import_window_frame(&ctx);
        self.calibration_frame(&ctx);
        let refused = std::mem::take(&mut self.focus.library.borrow_mut().notes);
        for n in refused {
            self.warn(n);
        }
        self.workbench_refusals(&ctx);
        self.save_layout_on_change(&ctx);
        self.engine_notices();
        self.copied_image();
        self.job_notices();
        self.follow_queue(&ctx);
        self.menus_ui(&ctx);
        self.rule_editor_ui(&ctx);
        self.pointer_choice_ui(&ctx);

        let named = ctx.data(|d| {
            d.get_temp::<BTreeMap<String, (Rect, egui::ViewportId)>>(Id::unique("helios.named"))
        });
        for (name, (rect, window)) in named.unwrap_or_default() {
            if window != self.focus.window {
                self.focus.rect_windows.insert(name.clone(), window);
            }
            self.focus.rects.insert(name, rect);
        }
        self.overlay_rects();

        if self.focus.recording.is_some() {
            self.focus.shots.push_back(Shot::Frame);
            self.request_screenshot(&ctx);
            ctx.request_repaint();
        }
    }

    /// Once the areas are drawn, moves the palettes a new layout gathered
    /// at the workbench's end edge to the image's end edge, beside the end
    /// side bar rather than over it (`workbench::beside_the_end_side`).
    fn place_palettes(&mut self) {
        let Some(lead) = self.focus.palettes_to_place.clone() else {
            return;
        };
        let h = self.core.app.handle();
        let Some(centre) = h.area_rect(Area::Center) else {
            return;
        };
        self.focus.palettes_to_place = None;
        let end = [Area::StartSide, Area::EndSide, Area::Bottom]
            .into_iter()
            .filter_map(|a| h.area_rect(a))
            .map(|r| r.right())
            .fold(centre.right(), f32::max);
        let mut layout = h.layout();
        if crate::workbench::beside_the_end_side(&mut layout, &lead, end - centre.right()) {
            let notes = h.set_layout(layout);
            self.notes.extend(notes);
        }
    }

    /// What the Graph panel's edits asked for: its `network.*` commands,
    /// each run as the user's (one undo step each), and what it has to
    /// say.
    fn graph_edits(&mut self) {
        let (commands, notes, reveal) = {
            let mut g = self.focus.graph.borrow_mut();
            (
                g.take_commands(),
                std::mem::take(&mut g.notes),
                g.take_reveal(),
            )
        };
        if reveal {
            self.reveal_params();
        }
        for (id, args) in commands {
            // A refusal is said by `run`.
            let _ = self.run(&id, args, Origin::User);
        }
        for n in notes {
            self.note(n);
        }
    }

    /// A node selected in the graph brings up the Parameters panel: shown
    /// (put back in its area when closed) and made its tabs' front tab,
    /// the keyboard left to the graph. Not when it shares the graph's
    /// tabs (it would hide the graph), nor when it is in front already.
    fn reveal_params(&mut self) {
        let params = PanelId::from_static(crate::params::PANEL);
        if !self.core.panels.iter().any(|p| p.id == params) {
            return;
        }
        let graph = PanelId::from_static(crate::graph::PANEL);
        let layout = self.layout();
        let tabs = |p: &PanelId| layout.areas.iter().find_map(|(_, n)| tabs_holding(n, p));
        if let Some((panels, active)) = tabs(&params) {
            if panels.contains(&graph) || panels.get(active) == Some(&params) {
                return;
            }
        } else if layout.shows(&params) {
            // Floating or in a window of its own: in view already.
            return;
        }
        let _ = self
            .core
            .app
            .handle()
            .change_workbench(egui_pop::api::workbench::Change::Show(params));
    }

    /// The Graph panel's state (its view, selection and folds).
    pub fn graph_panel(&self) -> std::cell::Ref<'_, crate::graph::GraphPanel> {
        self.focus.graph.borrow()
    }

    /// Saves the workspace's layout once it changed (egui-pop's
    /// `layout.save` step), when no button is held (a splitter dragged
    /// is saved where it is let go): a kill loses no arrangement.
    fn save_layout_on_change(&mut self, ctx: &Egui) {
        if self.focus.workspace.get().is_none() || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let layout = self.layout();
        if self.focus.layout_saved.as_ref() == Some(&layout) {
            return;
        }
        if let Err(e) = step_result(self.core.app.save_layout()) {
            self.warn(
                Message::new(
                    "gui-layout-not-saved",
                    "The layout was not saved: { $error }",
                )
                .with("error", e),
            );
        }
        self.focus.layout_saved = Some(layout);
        self.keep_layout_copy();
    }

    /// What the workbench refused this frame, said in the status bar:
    /// from egui-pop's journal (a move asked by a command or a palette's
    /// drop; egui-pop shows the toast), and a tab let go where its panel
    /// may not go. egui-pop shows that one on the drop zone while the tab
    /// is dragged and changes nothing on the drop, so it is the last drop
    /// hint's refusal once the hint is gone with the button up.
    fn workbench_refusals(&mut self, ctx: &Egui) {
        let h = self.core.app.handle().clone();
        let refusal = h.drop_hint().and_then(|hint| hint.refusal);
        let held = ctx.input(|i| i.pointer.primary_down());
        let was = std::mem::replace(&mut self.focus.drag_refusal, refusal);
        if let Some(why) = was
            && self.focus.drag_refusal.is_none()
            && !held
        {
            self.note(Message::plain(why));
        }
        let events = h.events_since(self.core.seen);
        self.core.seen = h.last_event();
        for e in events {
            // egui-pop's Quit (Ctrl+Q, File > Quit), from any window: it
            // closes the window it ran in, the application quits.
            if e.name == "command.done"
                && e.data["command"] == egui_pop::api::app::CMD_QUIT.as_str()
            {
                self.quit(ctx);
            }
            if e.name == "workbench.refused"
                && let Some(why) = e.data["reason"].as_str()
            {
                // Already in the language: Helios's own refusals are said
                // through its catalogues, egui-pop's through its own.
                self.note(Message::plain(why));
            }
        }
    }

    /// Names where egui-pop drew the dialog and the toasts, as it reports
    /// them.
    fn overlay_rects(&mut self) {
        for o in self.core.app.handle().overlay_rects() {
            let name = match o.kind {
                OverlayKind::Dialog => "helios.dialog",
                OverlayKind::Toasts => "helios.toasts",
                OverlayKind::Menu => "helios.menu",
                _ => continue,
            };
            self.focus.rects.insert(name.into(), o.rect);
        }
    }

    /// The first run's "Coming from darktable?" (`pointer_choice`), when
    /// offered and wanted: an area at the bottom of the window over the
    /// panels, not a dialog, so it blocks neither keys nor clicks.
    fn pointer_choice_ui(&mut self, ctx: &Egui) {
        let h = self.core.app.handle().clone();
        if !self.core.pointer_choice || !crate::pointer_choice::wanted(&h) {
            return;
        }
        let shown = egui::Area::new(Id::unique(crate::pointer_choice::RECT))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    crate::pointer_choice::body(ui, &h);
                });
            });
        self.focus
            .rects
            .insert(crate::pointer_choice::RECT.into(), shown.response.rect);
    }

    /// Menus of commands asked for, in egui's popups (the system menu is
    /// egui-pop's, `menus::popup_at`).
    fn menus_ui(&mut self, ctx: &Egui) {
        let h = self.core.app.handle().clone();
        let layer = egui::LayerId::background();
        if let Some((at, items)) = self.focus.menu.take() {
            let mut open = true;
            let mut chosen = None;
            let cx = h.when_context();
            let shown = egui::Popup::new(Id::unique("helios.menu"), ctx.clone(), at, layer)
                .kind(egui::PopupKind::Menu)
                .open_bool(&mut open)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    ui.set_min_width(200.0);
                    egui::containers::menu::menu_style(ui.style_mut());
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                        menu_rows(ui, &h, &cx, &items, &mut chosen);
                    });
                });
            if let Some(r) = shown {
                self.focus
                    .rects
                    .insert("helios.menu".into(), r.response.rect);
            }
            if let Some((id, args)) = chosen {
                let _ = self.run(&id, args, Origin::User);
            } else if open {
                self.focus.menu = Some((at, items));
            }
        }
    }

    /// Offers the first run's choice of darktable's pointer profile
    /// ("Coming from darktable?", `pointer_choice`): shown while no
    /// profile is saved and the question was never answered. The
    /// desktop asks for it; tests and tools do only when they test it.
    pub fn offer_pointer_choice(&mut self) {
        self.core.pointer_choice = true;
    }

    /// Helios's status line, which its block draws in egui-pop's status
    /// bar (beside the jobs item).
    fn status_bar(&self, ctx: &egui::Context) {
        let mut line = self.core.status.borrow_mut();
        line.recording = self.focus.recording.is_some();
        line.note = self.notes.last().cloned();
        line.hint = self.focus.viewport.borrow().masks().hint(ctx);
        line.decisions = self.core.stack_review.borrow().decisions().len();
        {
            let decisions = self.focus.decisions.borrow();
            line.pending_decisions = decisions.pending();
            line.decisions_for_a_person = decisions.for_a_person();
        }
        line.profile.clone_from(&self.core.profile_name);
    }

    /// What the status line says now: the hovered mask handle's
    /// gestures, else the last note.
    pub fn status_text(&self) -> Option<String> {
        let line = self.core.status.borrow();
        line.hint.clone().or_else(|| line.note.clone())
    }

    /// Where background work in this process reports: `job.*` events on
    /// the shell's bus, shown in the jobs centre, cancelled from it
    /// (see `crate::jobs`). An import, an export started from the
    /// desktop, a calibration: each starts its job here.
    pub fn jobs(&self) -> helios_core::jobs::Jobs {
        self.core.jobs.clone()
    }

    /// The library's undo steps: an import records its step there, and
    /// undo and redo reach the vault through what it is given
    /// ([`crate::library_history::LibraryHistory::set_apply`]).
    pub fn library_history(&self) -> crate::library_history::LibraryHistory {
        self.core.library_history.clone()
    }

    /// The jobs that ended since the last frame: said on
    /// the desktop when long and unseen (`job_finished`). The jobs of a
    /// node lost for too long leave the jobs centre.
    fn job_notices(&mut self) {
        let ends = self.core.ended.take(self.core.app.handle());
        for end in ends {
            self.job_finished(&end);
        }
        if self.core.ended.take_backups() {
            self.backup_ended();
        }
        self.core.forgotten.take(self.core.app.handle());
    }

    /// The Queue panel follows the selected node: its queue read again,
    /// the panel closed or opened again as its setting says when empty.
    fn follow_queue(&mut self, ctx: &Egui) {
        if !self
            .core
            .panels
            .iter()
            .any(|p| p.id.as_str() == crate::queue::PANEL)
        {
            return;
        }
        let node = lock(&self.core.values)
            .get(crate::params::SELECTION)
            .and_then(|e| match &e.value {
                helios_core::Value::Text(t) if !t.is_empty() => Some(t.clone()),
                _ => None,
            });
        // The panels are drawn already: a change shows at the next frame.
        if self
            .focus
            .queue_panel
            .borrow_mut()
            .update(self.core.app.handle(), node.as_deref())
        {
            ctx.request_repaint();
        }
    }

    /// The Queue panel (see `crate::queue`).
    pub fn queue_panel(&self) -> std::cell::RefMut<'_, crate::queue::QueuePanel> {
        self.focus.queue_panel.borrow_mut()
    }

    /// Whether the Queue panel shows when the selected node has no queue
    /// (`interface.panels.queue_when_empty`, on by default).
    pub fn set_queue_when_empty(&mut self, on: bool) {
        self.focus.queue_panel.borrow_mut().set_when_empty(on);
    }

    /// Follows a node's jobs in the jobs centre (see `crate::relay`):
    /// its jobs show as `node:<name>/<id>`, Cancel goes to its control
    /// endpoint. A node of the same name followed already is replaced.
    pub fn follow_node(&mut self, link: crate::relay::NodeLink) -> Result<(), String> {
        self.follow_node_with(link, crate::relay::Timing::default())
    }

    /// [`Shell::follow_node`], waiting as `timing` says.
    pub fn follow_node_with(
        &mut self,
        link: crate::relay::NodeLink,
        timing: crate::relay::Timing,
    ) -> Result<(), String> {
        let name = link.name.clone();
        let relay = crate::relay::Relay::start(link, self.core.jobs.bus().clone(), timing)?;
        self.core.relays.borrow_mut().insert(name, relay);
        Ok(())
    }

    /// Drops the link to a followed node, as a network failure would: it
    /// is followed again at the relay's next attempt. Whether the node is
    /// followed.
    pub fn drop_node_link(&self, name: &str) -> bool {
        let relays = self.core.relays.borrow();
        let Some(relay) = relays.get(name) else {
            return false;
        };
        relay.drop_link();
        true
    }
}

/// The rows of a menu asked for: each command with its title and keys,
/// greyed with the reason where it does not apply.
fn menu_rows(
    ui: &mut Ui,
    h: &AppHandle,
    cx: &egui_pop::model::when::WhenContext,
    items: &[MenuItem],
    chosen: &mut Option<(String, Value)>,
) {
    for item in items {
        match item {
            MenuItem::Separator => {
                ui.separator();
            }
            MenuItem::Submenu { label, items } => {
                ui.menu_button(label, |ui| menu_rows(ui, h, cx, items, chosen));
            }
            MenuItem::Command {
                id,
                args,
                label,
                enabled,
            } => {
                let Ok(cid) = CommandId::new(id.as_str()) else {
                    continue;
                };
                let Some(spec) = h.command(&cid) else {
                    continue;
                };
                let title = label.clone().unwrap_or_else(|| h.command_menu_title(&spec));
                let mut button = egui::Button::new(&title);
                if let Some(k) = h.keys_for(&cid).first() {
                    button = button.shortcut_text(k.clone());
                }
                let applies = h.applies(&cid, cx);
                let r = ui.add_enabled(applies && *enabled, button);
                // A menu's row, for screen readers and accesskit tests.
                egui_pop::core::a11y::name(&r, egui::Role::MenuItem, &title);
                // A row its owner greyed (an offline folder's) says
                // nothing more; one whose command does not apply, why.
                if *enabled && !applies {
                    r.on_disabled_hover_text(format!("Does not apply here ({})", spec.when));
                } else if r.clicked() {
                    *chosen = Some((id.clone(), args.clone()));
                    ui.close();
                }
            }
        }
    }
}

/// A panel's id, checked.
fn panel_id(id: &str) -> Result<PanelId, String> {
    PanelId::new(id).map_err(|e| e.to_string())
}

/// A workbench step's outcome as a command's result.
/// The system clipboard is the user's: an agent or a script neither
/// reads it nor writes over what the user copied there.
fn user_only(origin: &Origin) -> Result<(), String> {
    match origin {
        Origin::User => Ok(()),
        _ => Err("only the user copies to other applications".into()),
    }
}

fn workbench_outcome(out: Option<egui_pop::api::steps::Outcome>) -> Result<Value, String> {
    use egui_pop::api::steps::Outcome;
    match out {
        Some(Outcome::Done(r)) => r,
        Some(Outcome::Vetoed { by, reason }) => Err(format!("stopped by `{by}`: {reason}")),
        // Asked while the areas are drawn: done at the frame's end.
        None => Ok(Value::Null),
    }
}

/// Where a picture or a recording goes when no path is given: the user's
/// pictures, named after the target and the time.
fn default_path(target: &str, extension: &str) -> PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    let name: String = target
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    home.join("Pictures/Helios")
        .join(format!("{name}-{secs}.{extension}"))
}

fn recent_file() -> PathBuf {
    config_dir().join("recent-workspaces.json")
}

/// The workspaces opened last, the most recent first.
pub fn recent_workspaces() -> Vec<PathBuf> {
    std::fs::read_to_string(recent_file())
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<PathBuf>>(&t).ok())
        .unwrap_or_default()
}

/// Puts a workspace first among the recent ones (ten are kept).
pub fn remember_workspace(path: &std::path::Path) -> Result<(), String> {
    let mut list = recent_workspaces();
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(10);
    let file = recent_file();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &file,
        serde_json::to_string_pretty(&list).expect("paths serialize"),
    )
    .map_err(|e| format!("{}: {e}", file.display()))
}

/// The user's configuration directory for Helios.
pub fn config_dir() -> PathBuf {
    helios_template::workspace::config_dir()
}

/// Headless rendering, for tests, agents and machines without a display.
pub mod headless {
    use std::sync::{Arc, OnceLock};

    pub use crate::windows::{Drawn, Windows, harness_with_windows};

    use super::{Recorder, RgbaImage, Shell, snapshot};
    use eframe::egui_wgpu::{RenderState, Renderer, RendererOptions, wgpu};
    use egui::mutex::RwLock;
    use egui_kittest::Harness;

    /// The render state on the process's one GPU device
    /// (`helios_gpu::shared`), made by the first harness.
    ///
    /// One device for every harness, never one each: before 1.4.350, the
    /// Vulkan loader drops the drivers without a device when an instance
    /// enumerates its devices, while `vkSetDebugUtilsObjectNameEXT` on
    /// another thread walks those drivers under a different lock. Harnesses
    /// opened on parallel test threads crashed the process that way (see
    /// docs/testing-strategy.md). The device is the one the window and the
    /// engine use in production; when the harness opens it first, it
    /// prefers a software adapter (lavapipe), as `egui_kittest` does, so
    /// pictures do not depend on the machine's GPU.
    fn device() -> &'static RenderState {
        static DEVICE: OnceLock<RenderState> = OnceLock::new();
        DEVICE.get_or_init(|| {
            let gpu = helios_gpu::shared::gpu_choosing(|adapters| {
                adapters
                    .iter()
                    .enumerate()
                    .min_by_key(|(i, a)| {
                        let info = a.get_info();
                        let kind = match info.device_type {
                            wgpu::DeviceType::Cpu => 0,
                            wgpu::DeviceType::DiscreteGpu => 1,
                            _ => 2,
                        };
                        let backend = match info.backend {
                            wgpu::Backend::Metal => 0,
                            wgpu::Backend::Vulkan => 1,
                            wgpu::Backend::Dx12 => 2,
                            _ => 4,
                        };
                        (kind, backend, *i)
                    })
                    .map(|(i, _)| i)
            })
            .expect("a GPU adapter for the headless harness");
            egui_kittest::wgpu::create_render_state(
                super::wgpu_setup(&gpu),
                RendererOptions::PREDICTABLE,
            )
        })
    }

    /// A render state on the process's one GPU device, with a renderer of
    /// its own: the textures and the viewport's GPU objects drawn with it
    /// belong to its user alone (a harness, a test of the viewport).
    pub fn render_state() -> RenderState {
        let mut rs = device().clone();
        rs.renderer = Arc::new(RwLock::new(Renderer::new(
            &rs.device,
            rs.target_format,
            RendererOptions::PREDICTABLE,
        )));
        rs
    }

    /// A shell drawn offscreen at a size, on the wgpu renderer.
    pub fn harness<'a>(shell: Shell, size: egui::Vec2) -> Harness<'a, Shell> {
        harness_at(shell, size, 1.0)
    }

    /// As [`harness`], on a screen of this native scale (physical pixels
    /// per point; egui-pop's test kit runs at `egui_pop_test::SCALES`).
    pub fn harness_at<'a>(shell: Shell, size: egui::Vec2, scale: f32) -> Harness<'a, Shell> {
        harness_parts(shell, size, scale).0
    }

    /// As [`harness_at`], with the render state the harness draws with
    /// (its renderer holds the textures and the viewport's GPU objects).
    pub(crate) fn harness_parts<'a>(
        mut shell: Shell,
        size: egui::Vec2,
        scale: f32,
    ) -> (Harness<'a, Shell>, RenderState) {
        let rs = render_state();
        shell.attach_gpu(&rs);
        shell.core.still = true;
        let title = shell.window_title();
        let mut h = Harness::builder()
            .with_size(size)
            .with_pixels_per_point(scale)
            .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(
                rs.clone(),
            ))
            .build_ui_state(|ui, shell: &mut Shell| shell.ui(ui), shell);
        // The window's title, as a window manager reports it: the frame's
        // title bar shows it.
        h.input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .title = Some(title);
        (h, rs)
    }

    /// A picture of a target as drawn now.
    pub fn snapshot(harness: &mut Harness<'_, Shell>, target: &str) -> Result<RgbaImage, String> {
        harness.run();
        let full = harness.render()?;
        let shell = harness.state();
        let rect = shell.target_rect(target)?;
        Ok(snapshot::crop(&full, rect, shell.pixels_per_point()))
    }

    /// Records `frames` frames of a target, one step each.
    pub fn record(
        harness: &mut Harness<'_, Shell>,
        target: &str,
        frames: u32,
        recorder: &mut Recorder,
    ) -> Result<(), String> {
        for _ in 0..frames {
            harness.step();
            let full = harness.render()?;
            let shell = harness.state();
            let rect = shell.target_rect(target)?;
            recorder.push(&snapshot::crop(&full, rect, shell.pixels_per_point()))?;
        }
        Ok(())
    }

    /// Where the window or modal titled `title` is drawn now, frame and
    /// title bar included: an `egui::Window` by its accessibility node,
    /// an egui-pop modal (the file chooser) by its overlay.
    pub fn window_rect(harness: &Harness<'_, Shell>, title: &str) -> Option<egui::Rect> {
        use egui_kittest::kittest::Queryable as _;
        let window = harness
            .query_by_role_and_label(egui::accesskit::Role::Window, title)
            .map(|n| n.rect());
        window.or_else(|| {
            egui_pop::core::overlay::rects(&harness.ctx)
                .into_iter()
                .find(|(spec, _)| spec.label == title)
                .map(|(_, rect)| rect)
        })
    }

    /// Frames a window is given to show up: an `egui::Window` lays itself
    /// out unseen on its first frame, so it may be missing from the
    /// accessibility tree until the next one.
    pub const WINDOW_APPEARS_WITHIN: usize = 10;

    /// Steps `frames` frames (six at least) and asserts that the window
    /// titled `title` never grows on its own, frame after frame, through a
    /// spacing or a margin its content counts twice. The window is waited
    /// for first (at most [`WINDOW_APPEARS_WITHIN`] frames); from the frame
    /// it shows on, its size never exceeds the one of the frame before, and
    /// from its fifth frame on its rect stays the one it had then.
    pub fn assert_window_stable(harness: &mut Harness<'_, Shell>, title: &str, frames: usize) {
        assert!(frames > 5, "{frames} frames: six at least");
        let mut waited = 0;
        let mut shown = loop {
            harness.step();
            waited += 1;
            if let Some(rect) = window_rect(harness, title) {
                break rect;
            }
            assert!(
                waited < WINDOW_APPEARS_WITHIN,
                "no window titled {title:?} within {WINDOW_APPEARS_WITHIN} frames"
            );
        };
        let mut settled = None;
        for frame in 2..=frames {
            harness.step();
            let rect = window_rect(harness, title)
                .unwrap_or_else(|| panic!("the window {title:?} went at its frame {frame}"));
            assert!(
                rect.width() <= shown.width() && rect.height() <= shown.height(),
                "the window {title:?} grew at its frame {frame}: {:?} after {:?}",
                rect.size(),
                shown.size()
            );
            shown = rect;
            match settled {
                None if frame == 5 => settled = Some(rect),
                Some(settled) => assert_eq!(
                    rect, settled,
                    "the window {title:?} changed between its frame 5 and its frame {frame}"
                ),
                None => {}
            }
        }
    }
}

/// The tabs holding a panel, with the index of the one in front.
fn tabs_holding<'a>(
    node: &'a egui_pop::model::layout::Node,
    panel: &PanelId,
) -> Option<(&'a [PanelId], usize)> {
    use egui_pop::model::layout::Node;
    match node {
        Node::Tabs { panels, active } => panels
            .contains(panel)
            .then_some((panels.as_slice(), *active)),
        Node::Split { a, b, .. } => tabs_holding(a, panel).or_else(|| tabs_holding(b, panel)),
        Node::Grid { children, .. } => children.iter().find_map(|c| tabs_holding(c, panel)),
    }
}

/// Album sets as the shell checks them before a change is queued: the
/// vault checks again when it makes it.
mod sets {
    use helios_vault::collections::Album;

    /// Refuses `set` as where `member` goes (none: a new set): not a set
    /// of the list, or the member itself or a set below it.
    pub(super) fn check_target(
        albums: &[Album],
        set: &str,
        member: Option<&str>,
    ) -> Result<(), String> {
        let find = |id: &str| albums.iter().find(|a| a.id == id);
        match find(set) {
            None => return Err(format!("`set`: no set `{set}`")),
            Some(a) if !a.is_set() => {
                return Err(format!("`set`: “{}” is not an album set", a.name));
            }
            Some(_) => {}
        }
        let Some(member) = member else {
            return Ok(());
        };
        // Up from the set: meeting the member, it would hold itself.
        let mut at = Some(set);
        for _ in 0..=albums.len() {
            let Some(here) = at else {
                break;
            };
            if here == member {
                return Err(if here == set {
                    format!("`set`: a set cannot hold itself (`{member}`)")
                } else {
                    format!("`set`: a set cannot hold itself (`{set}` is inside `{member}`)")
                });
            }
            at = find(here).and_then(|a| a.set.as_deref());
        }
        Ok(())
    }
}
