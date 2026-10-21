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
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use egui::{Context as Egui, Event, Id, Rect, Ui};
use egui_pop::api::app::OverlayKind;
use egui_pop::api::dialogs::{self, Choose, ChooseOption, DIALOGS, Dialogs};
use egui_pop::api::frame::{self, FramePlugin, SETTING_FRAME};
use egui_pop::api::host::{Reply, Resolver};
use egui_pop::api::settings_window::{SETTINGS_WINDOW, SHORTCUTS};
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
pub use workspace_edit::STATE as WORKSPACE_STATE;

mod network_edit;
mod workspace_edit;
mod workspace_live;

/// The screen's own scale (physical pixels per point, the interface zoom
/// left out) from which it is `hidpi` for profile variants.
const HIDPI: f32 = 1.5;

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
    /// Import from the workspace's import source: this card or folder, or
    /// asked.
    ImportSource(Option<PathBuf>),
    Fullscreen,
    Minimize,
    Maximize,
    Close,
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
    /// Goes to a place of the context stack at the workspace's top: the
    /// library, or the workspace's network.
    Place(crate::contexts::FrameId),
    /// Culling on, off, or toggled.
    Culling(Option<bool>),
    /// A drawn-mask command, as whom.
    Mask(crate::masks::Command, Origin),
    /// Rename a node of the Masks panel (its selected one by default) in
    /// place.
    RenameInTree(Option<String>),
    Copy(String),
    SystemMenu,
    /// A command, its arguments, as whom.
    Run(String, Value, Origin),
    About,
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
        d.get_temp_mut_or_default::<BTreeMap<String, (Rect, egui::ViewportId)>>(Id::new(
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
    workspace: Workspace,
    /// The catalogues of the application's translation plugin.
    i18n: egui_pop::i18n::I18n,
    opens: crate::platform::Opens,
    jobs: helios_core::jobs::Jobs,
    status: crate::jobs::Status,
    relays: crate::jobs::Relays,
    /// What checks a restored darkroom's image is still there.
    images: crate::contexts::Images,
}

/// A shell whose egui-pop application is still to build: in a test
/// harness's context, or in a window by egui-pop's `run_native_with`.
pub struct ShellBuilder {
    app: AppBuilder,
    parts: Parts,
}

impl ShellBuilder {
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
        self.app.run_native_with(title, options, move |app, cc| {
            Ok(make(Shell::assemble(parts, app), cc))
        })
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

pub struct Shell {
    app: App,
    registry: Arc<Mutex<Registry>>,
    lists: Rc<RefCell<ListSource>>,
    /// Keys, menu entries and ticks of Helios's commands.
    declarations: Rc<RefCell<Declarations>>,
    /// The panels declared, and what each draws.
    panels: Vec<PanelSpec>,
    contents: Contents,
    /// The journal read up to here, for the workbench's refusals.
    seen: u64,
    queue: Queue,
    rects: BTreeMap<String, Rect>,
    /// The window of each named rectangle drawn outside the main one.
    rect_windows: BTreeMap<String, egui::ViewportId>,
    pixels_per_point: f32,
    /// The screen's own scale, as of the last frame (none before it).
    native_scale: Option<f32>,
    shots: VecDeque<Shot>,
    recording: Option<(String, Recorder)>,
    /// What the shell has to say: command results, layout repairs.
    pub notes: Vec<String>,
    /// The workspace's name, which the window's title shows.
    title: String,
    themes: crate::theme::Themes,
    theme: crate::theme::Tokens,
    theme_name: String,
    theme_overrides: BTreeMap<String, serde_json::Value>,
    /// The theme egui-pop last installed, as seen by the shell.
    installed: Option<crate::theme::ThemeSpec>,
    /// Where profiles are kept (`<name>.helios-profile`).
    profiles_dir: PathBuf,
    profile_name: String,
    /// A profile read and shown as a diff, waiting for `profile.import.apply`.
    pending_import: Option<helios_profile::Profile>,
    /// This machine, as profile variants select it.
    machine: helios_profile::Machine,
    /// The profile in use as read, with its variants, and as resolved for
    /// this machine.
    loaded: Option<(helios_profile::Profile, helios_profile::Resolved)>,
    /// What `profile.resolved` answers.
    resolved_view: Arc<Mutex<Value>>,
    scopes: Rc<RefCell<crate::scopes::Scopes>>,
    library: Rc<RefCell<crate::library::Library>>,
    /// The Stacks panel's proposals.
    stack_review: Rc<RefCell<crate::stack_review::Review>>,
    /// The open image's decisions, and the Decisions panel's cards.
    decisions: Rc<RefCell<crate::decisions::Decisions>>,
    /// The duplicates imports found (the Duplicates panel).
    duplicates: Rc<RefCell<crate::duplicates::Duplicates>>,
    /// Where the workspace imports from, and how (its template's).
    import_source: Option<helios_template::ImportDesc>,
    /// Imports asked for (`library.import`'s arguments), for the host.
    imports: Vec<Value>,
    /// The library's images on disk, by id, for quick open.
    image_files: BTreeMap<String, PathBuf>,
    /// egui-pop's dialogs, once the application is built (`dialog.current`).
    dialogs: Arc<OnceLock<Dialogs>>,
    /// Confirmations are answered yes by policy (a script's `--yes`).
    assume_yes: bool,
    /// A menu of commands asked for, open at a point.
    menu: Option<(egui::Pos2, Vec<MenuItem>)>,
    /// Every notice so far; the status bar shows the last, toasts the
    /// warnings and errors.
    notices: Vec<Notice>,
    /// The GUI's clock, in seconds, as of this frame.
    now: f64,
    viewport: Rc<RefCell<crate::viewport::Viewport>>,
    /// A small render of what the viewport shows, for the scopes and the
    /// navigator.
    preview: Rc<crate::preview::Preview>,
    /// The render state the viewport's GPU path waits to be installed on.
    gpu: Option<eframe::egui_wgpu::RenderState>,
    /// What installing it found.
    gpu_info: Option<egui_pop::viewport::gpu::GpuInfo>,
    /// The image's own surface (Wayland), following the Wide-gamut
    /// preview setting.
    image_surface: crate::display::ImageSurfaceSwitch,
    params: Rc<RefCell<crate::params::Params>>,
    /// The Masks panel: the selected module's mask network as a tree.
    mask_tree: Rc<RefCell<crate::mask_tree::MaskTree>>,
    /// The Graph panel: the workspace's network in egui-pop's editor.
    graph: Rc<RefCell<crate::graph::GraphPanel>>,
    /// Every value widgets and agents edit (`param.*` commands).
    pub values: Arc<Mutex<helios_values::Store>>,
    /// Headless runs: no cursor blink and no animation, so a frame settles
    /// and a test's `run` ends.
    still: bool,
    /// The workspace open, whose layout egui-pop keeps.
    workspace: Workspace,
    /// The layout as last saved to the workspace's document: saved again
    /// once it changes, so a kill keeps it.
    layout_saved: Option<Layout>,
    /// The workspace's state, as edited (`workspace_edit`).
    edit: Option<workspace_edit::Edited>,
    /// The catalogues the shell's text is shown through.
    i18n: egui_pop::i18n::I18n,
    /// What later launches and links asked to open (`platform`).
    opens: crate::platform::Opens,
    /// The viewport engine's events read up to here, and the modules whose
    /// GPU fallback was already said.
    engine_seen: u64,
    fell_back: BTreeSet<String>,
    /// Where background work reports (`job.*` on the shell's bus).
    jobs: helios_core::jobs::Jobs,
    /// Helios's status line, drawn in egui-pop's status bar.
    status: crate::jobs::Status,
    /// The jobs' ends read up to here, for the desktop notification.
    ended: crate::jobs::Ended,
    /// The nodes followed, whose jobs show in the jobs centre.
    relays: crate::jobs::Relays,
    /// The lost nodes' jobs to take out of the jobs centre.
    forgotten: crate::jobs::Forgotten,
    /// The Queue panel: the selected node's queue.
    queue_panel: Rc<RefCell<crate::queue::QueuePanel>>,
    /// The image's compare and mask tool, kept with their modes.
    follow: crate::contexts::Follow,
    /// Helios's status bar items, published when what they say changes.
    status_items: crate::status_items::Items,
    /// The first run's "Coming from darktable?" is offered (the desktop
    /// only, `offer_pointer_choice`).
    pointer_choice: bool,
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
        let host =
            HeliosHost::new(registry.clone(), bus, Arc::default()).with_values(values.clone());
        let later = host.later();
        let workspace = host.workspace();
        let images = crate::contexts::Images::default();
        let contents = Contents::default();
        let themes = crate::theme::themes();
        let declarations = Rc::new(RefCell::new(Declarations::default()));
        let lists = Rc::new(RefCell::new(ListSource {
            profiles_dir: config_dir().join("profiles"),
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
        Shell::declare_menus(&wiring);
        let i18n_plugin = egui_pop::i18n::I18nPlugin::new()
            .catalogues(helios_i18n::CATALOGUES)
            .env(helios_i18n::ENV)
            // A shell without the user's settings (tests, pictures,
            // tools) speaks English; the desktop follows the language
            // setting, the system's by default.
            .language(if ui_settings.is_some() {
                egui_pop::i18n::plugin::SYSTEM
            } else {
                "en"
            });
        let i18n = i18n_plugin.i18n();
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
            .themes(themes.clone())
            .theme(crate::theme::ThemeSpec::new(crate::theme::DEFAULT))
            .plugins(DefaultPlugins)
            // Lightroom's habits by default: the wheel scrolls the panel
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
            })
            .plugin(plugins::History {
                values: values.clone(),
                i18n: i18n.clone(),
            })
            // egui-pop's data views (the Masks panel's tree).
            .plugin(egui_pop::views::ViewsPlugin)
            // The photo library's steps and commands (the library panels).
            .plugin(egui_pop::views::library::LibraryPlugin)
            // The image tools' steps: the histogram's drag, curves, colour
            // wheels, the navigator.
            .plugin(egui_pop::imaging::ImagingPlugin)
            // The window's frame: Helios's own on the desktop, the
            // system's elsewhere (tests, pictures, tools) until asked.
            .plugin(if ui_settings.is_some() {
                FramePlugin::own()
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
            // egui-pop's theme editor, as a page of the settings window
            // (Appearance), on the theme in use.
            .plugin(egui_pop::api::theme_editor::ThemeEditorPlugin)
            .plugin(crate::theme_editor::Page)
            .plugin(crate::platform::Platform::new(opens.clone()))
            .plugin(crate::workbench::Panels {
                specs: panels.clone(),
                contents: contents.clone(),
            })
            // Places and modes (egui-pop's context stack), kept beside
            // the workspace's layout.
            .plugin(crate::contexts::stack(workspace.clone(), images.clone()))
            .plugin(crate::contexts::Frames(workspace.clone()))
            // egui-pop's status bar: the breadcrumb and the mode chips,
            // then the status items (Helios's by place, the jobs'); no
            // activity bar.
            .plugin(egui_pop::api::strips::StripsPlugin::status_only())
            .plugin(i18n_plugin);
        if let Some(dir) = ui_settings {
            builder = builder.persist(dir);
        }
        ShellBuilder {
            app: builder,
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
                workspace,
                i18n,
                opens,
                jobs,
                status,
                relays,
                images,
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
            workspace,
            i18n,
            opens,
            jobs,
            status,
            relays,
            images: _,
        } = parts;
        if let Some(d) = app.handle().service(&DIALOGS) {
            let _ = dialogs.set((*d).clone());
        }
        let viewport: Rc<RefCell<crate::viewport::Viewport>> = Rc::default();
        let preview = Rc::new(crate::preview::Preview::new(
            viewport.borrow().engine().clone(),
        ));
        let mut shell = Shell {
            app,
            registry,
            lists,
            declarations,
            panels,
            contents,
            seen: 0,
            queue,
            rects: BTreeMap::new(),
            rect_windows: BTreeMap::new(),
            pixels_per_point: 1.0,
            native_scale: None,
            shots: VecDeque::new(),
            recording: None,
            notes: Vec::new(),
            title: String::new(),
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
            scopes: Rc::default(),
            library: Rc::default(),
            stack_review: Rc::default(),
            decisions: Rc::default(),
            duplicates: Rc::default(),
            import_source: None,
            imports: Vec::new(),
            image_files: BTreeMap::new(),
            dialogs,
            assume_yes: false,
            menu: None,
            notices: Vec::new(),
            now: 0.0,
            viewport,
            preview,
            gpu: None,
            gpu_info: None,
            image_surface: crate::display::ImageSurfaceSwitch::default(),
            params: Rc::default(),
            mask_tree: Rc::default(),
            graph: Rc::default(),
            values,
            workspace,
            layout_saved: None,
            edit: None,
            still: false,
            i18n,
            engine_seen: 0,
            fell_back: BTreeSet::new(),
            opens,
            jobs,
            status,
            ended: crate::jobs::Ended::default(),
            relays,
            forgotten: crate::jobs::Forgotten::default(),
            queue_panel: Rc::default(),
            follow: crate::contexts::Follow::default(),
            status_items: crate::status_items::Items::default(),
            pointer_choice: false,
        };
        if let Err(e) = crate::pointer_choice::declare(shell.app.handle()) {
            shell.notes.push(format!("pointer choice: {e}"));
        }
        for d in shell.app.handle().diagnostics() {
            shell.notes.push(format!("{}: {}", d.code, d.message));
        }
        // The built-in templates' text, each under its own keys, once
        // the translation plugin has loaded Helios's catalogues.
        if let Err(e) = crate::templates::add_texts(&shell.i18n, &crate::templates::builtin()) {
            shell.notes.push(e);
        }
        shell.seen = shell.app.handle().last_event();
        shell.ended = crate::jobs::Ended::after(shell.seen);
        shell.forgotten = crate::jobs::Forgotten::after(shell.seen);
        {
            let mut store = lock(&shell.values);
            crate::viewport::declare(&mut store);
            crate::mask_display::declare(&mut store);
            crate::masks::declare(&mut store);
            crate::graph::declare(&mut store);
            store.declare(
                crate::params::SELECTION,
                // Which node one looks at is not an edit: never undone.
                helios_values::Descriptor::text("selected node", "").transient(),
            );
        }
        let has = |shell: &Shell, panel: &str| shell.panels.iter().any(|p| p.id.as_str() == panel);
        if has(&shell, "helios.params") {
            shell
                .params
                .borrow_mut()
                .set_app(shell.app.handle().clone());
            let (params, viewport, values) = (
                shell.params.clone(),
                shell.viewport.clone(),
                shell.values.clone(),
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
        if has(&shell, crate::queue::PANEL) {
            let (panel, app) = (shell.queue_panel.clone(), shell.app.handle().clone());
            shell.set_content(crate::queue::PANEL, move |ui| {
                panel.borrow_mut().ui(ui, &app);
            });
        }
        if has(&shell, "helios.viewport") {
            let (viewport, values, app) = (
                shell.viewport.clone(),
                shell.values.clone(),
                shell.app.handle().clone(),
            );
            shell.set_content("helios.viewport", move |ui| {
                let leave = {
                    let mut viewport = viewport.borrow_mut();
                    viewport.ui(ui, &values);
                    viewport.take_leave()
                };
                // The darktable profile's double click on the image: back
                // where the darkroom was opened from.
                if leave {
                    crate::contexts::leave_darkroom(&app, &egui_pop::model::commands::Origin::User);
                }
            });
        }
        if has(&shell, crate::graph::PANEL) {
            shell.graph.borrow_mut().set_app(shell.app.handle().clone());
            let (graph, viewport, values) = (
                shell.graph.clone(),
                shell.viewport.clone(),
                shell.values.clone(),
            );
            shell.set_content(crate::graph::PANEL, move |ui| {
                graph.borrow_mut().ui(ui, &values, &viewport);
            });
        }
        if has(&shell, crate::mask_tree::PANEL) {
            shell
                .mask_tree
                .borrow_mut()
                .set_app(shell.app.handle().clone());
            let (tree, viewport, values) = (
                shell.mask_tree.clone(),
                shell.viewport.clone(),
                shell.values.clone(),
            );
            shell.set_content(crate::mask_tree::PANEL, move |ui| {
                tree.borrow_mut().ui(ui, &viewport, &values);
            });
        }
        if has(&shell, crate::navigator::PANEL) {
            let mut panel = crate::navigator::NavigatorPanel::default();
            let (viewport, preview, values, app) = (
                shell.viewport.clone(),
                shell.preview.clone(),
                shell.values.clone(),
                shell.app.handle().clone(),
            );
            shell.set_content(crate::navigator::PANEL, move |ui| {
                panel.ui(ui, &viewport, &preview, &values, &app);
            });
        }
        if has(&shell, "helios.scopes") {
            let (scopes, preview, values, app) = (
                shell.scopes.clone(),
                shell.preview.clone(),
                shell.values.clone(),
                shell.app.handle().clone(),
            );
            shell.set_content("helios.scopes", move |ui| {
                let mut scopes = scopes.borrow_mut();
                scopes.follow(&preview);
                scopes.ui_with(ui, Some((&app, &*values)));
            });
        }
        shell
            .library
            .borrow_mut()
            .set_app(shell.app.handle().clone());
        if has(&shell, crate::duplicates::PANEL) {
            let duplicates = shell.duplicates.clone();
            shell.set_content(crate::duplicates::PANEL, move |ui| {
                duplicates.borrow_mut().ui(ui);
            });
        }
        if has(&shell, crate::stack_review::PANEL) {
            let (review, library) = (shell.stack_review.clone(), shell.library.clone());
            shell.set_content(crate::stack_review::PANEL, move |ui| {
                let show = review.borrow_mut().ui(ui, &library.borrow());
                if let Some(ids) = show {
                    library.borrow_mut().select_ids(&ids);
                }
            });
        }
        if has(&shell, crate::decisions::PANEL) {
            let decisions = shell.decisions.clone();
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
                let library = shell.library.clone();
                shell.set_content(panel, move |ui| {
                    let mut library = library.borrow_mut();
                    match view {
                        0 => library.grid(ui),
                        1 => library.filmstrip(ui),
                        _ => library.compare_ui(ui),
                    }
                });
            }
        }
        shell
    }

    /// A long export or batch ended: said on the desktop when no Helios
    /// window has the focus and the `notifications.jobs` setting is on
    /// (see `platform::job_finished`); whether it was.
    pub fn job_finished(&self, end: &crate::platform::JobEnd) -> bool {
        let app = self.app.handle();
        let focused = crate::platform::window_focused(&app.egui());
        crate::platform::job_finished(app, end, focused, |m| self.text(m))
    }

    /// What later launches and `helios:` links asked to open since the
    /// last call (the window opens them).
    pub fn take_opens(&self) -> Vec<crate::platform::Open> {
        self.opens.take()
    }

    /// The egui-pop application (its handle: services, menus, history).
    pub fn app(&self) -> &App {
        &self.app
    }

    /// Saves the interface's settings and the user's keys (when built
    /// with a place for them).
    pub fn save(&self) -> Result<(), String> {
        self.app.save().map_err(|e| e.to_string())
    }

    /// The application closes (egui-pop's `App::exit`): the builder's
    /// `on_exit` functions, then the `app.exit` step, whose default saves
    /// the interface's settings, the user's keys and the layout.
    pub fn exit(&mut self) -> Result<(), String> {
        use egui_pop::api::steps::Outcome;
        // Agents write the workspace's files again from now on.
        self.release_workspace();
        match self.app.exit() {
            Outcome::Done(r) => r.map(|_| ()),
            Outcome::Vetoed { by, reason } => Err(format!("exit stopped by `{by}`: {reason}")),
        }
    }

    /// Declares a command on Helios's registry, and to the interface at
    /// once (egui-pop's `App::declare`): its menus, palette entry and keys
    /// follow, no plugin reconnected.
    pub fn register(
        &mut self,
        spec: CommandSpec,
        handler: impl Fn(&Value, &Origin) -> Result<Value, String> + Send + Sync + 'static,
    ) -> Result<(), String> {
        let declared =
            plugins::declared(&spec, &self.declarations.borrow()).map_err(|e| e.to_string())?;
        lock(&self.registry).register(spec, handler)?;
        self.app
            .declare(&OwnerId::from(&plugins::COMMANDS), declared)
            .map_err(|e| e.to_string())
    }

    /// Every command Helios declares, by id.
    pub fn specs(&self) -> Vec<CommandSpec> {
        lock(&self.registry).specs().cloned().collect()
    }

    /// The commands egui-pop's workbench declares for Helios's panels
    /// with texts of their own (`<panel>.detach`, `<panel>.moveTo`...),
    /// run in the interface. A panel's `focus` and `toggle` commands are
    /// left out: they read texts egui-pop shares between every panel.
    pub fn panel_commands(&self) -> Vec<CommandSpec> {
        let owner = egui_pop::model::ids::OwnerId::from(&crate::workbench::PANELS);
        let ctx = self.app.handle().egui();
        self.app
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
        &self.panels
    }

    /// The panels a workspace template may name: Helios's, and those
    /// egui-pop's plugins declare (the jobs centre, `egui_pop.jobs`).
    pub fn template_panels(&self) -> Vec<PanelSpec> {
        let mut all = self.panels.clone();
        for (spec, owner) in self.app.handle().declared_panels() {
            if owner != OwnerId::FRAMEWORK && !all.iter().any(|p| p.id == spec.id) {
                all.push(spec);
            }
        }
        all
    }

    /// The last command runs, Helios's and the interface's, oldest first.
    pub fn history(&self) -> Vec<Run> {
        self.app
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
        self.scopes.borrow_mut().set_image(image);
    }

    /// The images the library shows (the vault's, in its order); the
    /// selection follows its image when it is still there.
    pub fn set_library(&mut self, entries: Vec<crate::library::Entry>) {
        self.library.borrow_mut().set_entries(entries);
    }

    /// Where the library's images are on disk, by id: quick open opens
    /// them from there.
    pub fn set_image_files(&mut self, files: impl IntoIterator<Item = (String, PathBuf)>) {
        self.image_files = files.into_iter().collect();
    }

    /// Where thumbnails are sent, by image id, from any thread.
    pub fn library_thumbnails(&self) -> crate::library::ThumbnailSink {
        self.library.borrow().thumbnails()
    }

    /// What the user changed in the library since the last call, for the
    /// host to apply to the vault.
    pub fn take_library_changes(&mut self) -> Vec<crate::library::Change> {
        self.library.borrow_mut().take_changes()
    }

    /// The library's state, to read.
    pub fn library(&self) -> std::cell::Ref<'_, crate::library::Library> {
        self.library.borrow()
    }

    /// The library's state, to show what the vault holds.
    pub fn library_mut(&self) -> std::cell::RefMut<'_, crate::library::Library> {
        self.library.borrow_mut()
    }

    /// Where an import's lines go, with its arguments (`library.import`'s),
    /// from any thread: its duplicates fill the Duplicates panel.
    pub fn duplicates_sink(&self) -> std::sync::mpsc::Sender<(Value, Value)> {
        self.duplicates.borrow().sink()
    }

    /// The duplicates imports found, to read.
    pub fn duplicates(&self) -> std::cell::Ref<'_, crate::duplicates::Duplicates> {
        self.duplicates.borrow()
    }

    /// The duplicates imports found, to answer them.
    pub fn duplicates_mut(&self) -> std::cell::RefMut<'_, crate::duplicates::Duplicates> {
        self.duplicates.borrow_mut()
    }

    /// The Stacks panel's proposals, to read.
    pub fn stack_review(&self) -> std::cell::Ref<'_, crate::stack_review::Review> {
        self.stack_review.borrow()
    }

    /// The Stacks panel's proposals, to show the vault's and answer them.
    pub fn stack_review_mut(&self) -> std::cell::RefMut<'_, crate::stack_review::Review> {
        self.stack_review.borrow_mut()
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
            let mut store = lock(&self.values);
            for id in graph.ids() {
                helios_values::bind_node(&mut store, &graph, id)?;
            }
            store.take_dirty();
        }
        let mut viewport = self.viewport.borrow_mut();
        viewport.set_source(graph, node, port)?;
        // Another image: its own decisions, the decision nodes of its
        // network waiting for a person until an edit's book says more.
        let mut decisions = self.decisions.borrow_mut();
        decisions.set_book(helios_decision::Book::new());
        if let Some(g) = viewport.graph() {
            decisions.adopt(g);
        }
        decisions.refresh(viewport.graph());
        Ok(())
    }

    /// The open image's decisions and their cards, to read.
    pub fn decisions(&self) -> std::cell::Ref<'_, crate::decisions::Decisions> {
        self.decisions.borrow()
    }

    /// The Decisions panel, to act as the user would (its requests are
    /// answered at the next frame).
    pub fn decisions_mut(&self) -> std::cell::RefMut<'_, crate::decisions::Decisions> {
        self.decisions.borrow_mut()
    }

    /// The open image's edit's decisions (a loaded edit's book): their
    /// answers are applied to the graph shown, which renders again; the
    /// decision nodes the book does not know wait for a person.
    pub fn set_decisions(&mut self, book: helios_decision::Book) -> Result<(), String> {
        let mut viewport = self.viewport.borrow_mut();
        let mut decisions = self.decisions.borrow_mut();
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
        let mut viewport = self.viewport.borrow_mut();
        let region = viewport.shown().map(|s| s.extent);
        let engine = viewport.engine().clone();
        let mut decisions = self.decisions.borrow_mut();
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
        let requests = self.decisions.borrow_mut().take_requests();
        for r in requests {
            if let Err(e) = self.answer_decision(r, &Origin::User) {
                self.warn(e);
            }
        }
        let viewport = self.viewport.borrow();
        self.decisions.borrow_mut().refresh(viewport.graph());
    }

    /// Gives the viewport the GPU path it draws with, on the render state
    /// the window (or a test harness) draws with: the process's one device
    /// ([`wgpu_setup`]), never one of the window's own. Installed on the
    /// next frame, in its context.
    pub fn attach_gpu(&mut self, rs: &eframe::egui_wgpu::RenderState) {
        debug_assert!(
            helios_gpu::shared::owns(&rs.instance),
            "the window opened a wgpu instance of its own: hand it wgpu_setup()"
        );
        self.gpu = Some(rs.clone());
    }

    /// The viewport's GPU path, once installed: `None` draws on the CPU.
    pub fn viewport_gpu(&self) -> Option<egui_pop::viewport::gpu::GpuInfo> {
        self.gpu_info
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
        let on = crate::display::wide_gamut(self.app.handle());
        match self.image_surface.step(on, self.gpu_info.is_some()) {
            SurfaceStep::Install => {
                let offer = crate::display::install_image_surface(ctx, rs, window);
                if let Some(why) = offer.unavailable {
                    log::info!(target: "helios", "no image surface, the image stays sRGB: {why}");
                }
            }
            SurfaceStep::Down => crate::display::take_down_image_surface(ctx),
            SurfaceStep::Keep | SurfaceStep::Up => {}
        }
    }

    /// What the viewport's render threads did.
    pub fn viewport_stats(&self) -> Arc<crate::engine_source::Stats> {
        self.viewport.borrow().stats.clone()
    }

    /// The viewport: its egui-pop state, its readout, its scene.
    pub fn viewport(&self) -> std::cell::Ref<'_, crate::viewport::Viewport> {
        self.viewport.borrow()
    }

    /// The Masks panel: its tree's state, the network it shows.
    pub fn mask_tree(&self) -> std::cell::Ref<'_, crate::mask_tree::MaskTree> {
        self.mask_tree.borrow()
    }

    /// The viewport, to set its hooks.
    pub fn viewport_mut(&self) -> std::cell::RefMut<'_, crate::viewport::Viewport> {
        self.viewport.borrow_mut()
    }

    /// The preview of what the viewport shows (the pipeline's output,
    /// small), which the scopes measure and the navigator draws.
    pub fn preview(&self) -> &crate::preview::Preview {
        &self.preview
    }

    /// The image the scopes measure now.
    pub fn scope_image(&self) -> Option<helios_core::Image> {
        self.scopes.borrow().image.clone()
    }

    /// The float data the viewport shows, before the display transform.
    pub fn viewport_scene(&self) -> Option<helios_core::Image> {
        self.viewport.borrow().scene()
    }

    /// The layout as it stands, to be saved with the workspace.
    pub fn layout(&self) -> Layout {
        self.app.handle().layout()
    }

    /// Gives a panel its content; panels without one show a placeholder.
    pub fn set_content(&mut self, panel: &str, content: impl FnMut(&mut Ui) + 'static) {
        self.contents
            .borrow_mut()
            .insert(panel.into(), Box::new(content));
    }

    /// Switches to a theme, with the user's overrides; problems (an
    /// unknown base, a wrong token) fall back and are noted, never fatal.
    pub fn set_theme(&mut self, name: &str, overrides: BTreeMap<String, serde_json::Value>) {
        self.theme_name = name.into();
        self.theme_overrides = overrides;
        // An unknown theme falls back to Helios's default, not to the
        // egui-pop base egui-pop itself would pick.
        let known = self.themes.get(name).is_some();
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
        self.theme = crate::theme::resolve(&self.themes, resolved, &self.theme_overrides);
        let mut spec = crate::theme::ThemeSpec::new(resolved);
        spec.overrides.clone_from(&self.theme_overrides);
        // What the user chose across themes stays: the text size, the
        // fonts (Settings › Appearance › Fonts) and the accent colour.
        let service = self.app.handle().theme();
        let kept = service.spec();
        spec.text_size = kept.text_size;
        spec.font_ui = kept.font_ui;
        spec.font_mono = kept.font_mono;
        spec.accent = kept.accent;
        service.set(spec);
        let problems: Vec<String> = self
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
        let (images, picked, rejected) = self.library.borrow().counts();
        let zoom = self.viewport.borrow().state().mapping().map(|m| m.zoom);
        let history = lock(&self.values).undo_steps().len();
        let measure = crate::status_items::Measure {
            images,
            picked,
            rejected,
            zoom,
            history,
        };
        self.status_items.frame(self.app.handle(), ctx, &measure);
    }

    /// Takes a theme installed from elsewhere (the theme editor) as the
    /// one in use: its base and overrides become what a profile saves and
    /// what `theme.exportMerged` writes. The theme the shell installed
    /// itself, or an unknown name it fell back from, changes nothing.
    fn follow_theme(&mut self, spec: &crate::theme::ThemeSpec) {
        let ours = if self.themes.get(&self.theme_name).is_some() {
            self.theme_name.as_str()
        } else {
            crate::theme::DEFAULT
        };
        if spec.base == ours && spec.overrides == self.theme_overrides {
            return;
        }
        self.theme_name.clone_from(&spec.base);
        self.theme_overrides.clone_from(&spec.overrides);
        self.theme = crate::theme::resolve(&self.themes, &spec.base, &self.theme_overrides);
    }

    /// The name of the theme in use, as chosen.
    pub fn theme_name(&self) -> &str {
        &self.theme_name
    }

    /// The theme in use.
    pub fn theme(&self) -> &crate::theme::Tokens {
        &self.theme
    }

    /// Binds keys to a command in the user's keymap (what a profile
    /// carries), from the next frame.
    pub fn bind_user_keys(&mut self, keys: &str, command: &str) -> Result<(), String> {
        let id = CommandId::new(command).map_err(|e| e.to_string())?;
        self.app.handle().rebind(id, Some(keys.to_string()));
        self.app.process_requests();
        Ok(())
    }

    /// What the shell holds as a profile: theme, user keys, layout. The
    /// keys are the user's layer as egui-pop exports it
    /// (`AppHandle::export_keymap`): each binding with its `when`, and the
    /// default keys a rebinding took away (`-<command>`, no keys).
    pub fn profile(&self) -> helios_profile::Profile {
        use helios_profile::{Binding, Profile, System, ThemeChoice, portable_keys};
        let keymap = self
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
            name: self.profile_name.clone(),
            keymap: Some(keymap),
            theme: Some(ThemeChoice {
                name: self.theme_name.clone(),
                overrides: self.theme_overrides.clone(),
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
        *lock(&self.resolved_view) = json!({
            "profile": p.name,
            "machine": machine,
            "from": resolved.from,
        });
        self.apply_resolved(&resolved.profile);
        self.loaded = Some((p.clone(), resolved));
    }

    /// This machine, its screen as of the last frame.
    fn machine_now(&self) -> helios_profile::Machine {
        let mut m = self.machine.clone();
        m.hidpi |= self.native_scale.unwrap_or(1.0) >= HIDPI;
        m
    }

    /// Sets this machine as profile variants see it (the host reads it
    /// from `machine.toml` in the config directory); applies at the next
    /// profile.
    pub fn set_machine(&mut self, machine: helios_profile::Machine) {
        self.machine = machine;
    }

    /// Applies a resolved profile's parts the shell holds.
    fn apply_resolved(&mut self, p: &helios_profile::Profile) {
        use helios_profile::{System, local_keys};
        self.profile_name.clone_from(&p.name);
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
            match self.app.load_keymap(&doc, Layer::User) {
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

    /// The open workspace's key (see `host::workspace_key`), if any.
    pub fn workspace(&self) -> Option<String> {
        self.workspace.get()
    }

    /// Switches to a workspace directory: the layout as it stands is
    /// saved to the workspace left, then the one `dir` saved is loaded
    /// (egui-pop's `layout.save` and `layout.load` steps). A workspace
    /// with no layout saved yet keeps the layout as it is.
    pub fn switch_workspace(&mut self, dir: &Path) -> Result<(), String> {
        step_result(self.app.save_layout())?;
        self.workspace.set(Some(workspace_key(dir)));
        self.load_layout()?;
        // Its places and modes, kept beside its layout.
        egui_pop::api::contexts::reload(self.app.handle());
        Ok(())
    }

    /// Loads the open workspace's layout again (egui-pop's `layout.load`
    /// step), repaired to today's panels; the repairs are noted. With no
    /// layout saved yet the layout stays as it is.
    pub fn load_layout(&mut self) -> Result<(), String> {
        let loaded = step_result(self.app.load_layout())?;
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
        let notes = self.app.handle().set_layout(layout);
        self.notes.extend(notes);
    }

    /// Where profiles are read and written.
    pub fn set_profiles_dir(&mut self, dir: PathBuf) {
        self.lists.borrow_mut().profiles_dir.clone_from(&dir);
        self.profiles_dir = dir;
    }

    pub fn profile_name(&self) -> &str {
        &self.profile_name
    }

    fn profile_file(&self, name: &str) -> PathBuf {
        self.profiles_dir.join(format!("{name}.helios-profile"))
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
        self.themes.add(theme);
    }

    /// The workspace's name, for the window's title.
    pub fn set_title(&mut self, title: &str) {
        self.title = title.into();
    }

    /// The window's title: the workspace, then Helios. The frame's title
    /// bar shows it.
    pub fn window_title(&self) -> String {
        if self.title.is_empty() {
            "Helios".into()
        } else {
            format!("{} — Helios", self.title)
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
        let done = lock(&self.app.handle().store()).set(set).map(|_| ());
        if let Err(e) = done {
            self.warn(
                Message::new("gui-window-frame-error", "window frame: { $error }")
                    .with("error", e.to_string()),
            );
        }
    }

    /// The panel that has the focus.
    pub fn focused(&self) -> Option<String> {
        self.app.handle().focused_panel().map(|p| p.to_string())
    }

    pub fn palette_open(&self) -> bool {
        palette::is_open(self.app.handle())
    }

    /// Where a target was drawn at the last frame: `window`, a panel id or
    /// a name given by [`name_rect`]; `helios.dialog`, `helios.toasts` and
    /// `helios.menu` for what egui-pop and the shell show over the rest.
    pub fn target_rect(&self, target: &str) -> Result<Rect, String> {
        self.rects
            .get(target)
            .copied()
            .ok_or_else(|| format!("nothing named `{target}` is shown"))
    }

    /// The window a target was drawn in at the last frame: the main one
    /// (`ViewportId::ROOT`) unless its panel is detached.
    pub fn target_window(&self, target: &str) -> egui::ViewportId {
        self.rect_windows
            .get(target)
            .copied()
            .unwrap_or(egui::ViewportId::ROOT)
    }

    /// The main window's physical pixels per point (a detached panel's
    /// window has its own: `ui.ctx().pixels_per_point()` where it draws).
    pub fn pixels_per_point(&self) -> f32 {
        self.pixels_per_point
    }

    /// Runs a command as `origin`, in the context of the last frame. A
    /// command whose work waits for the next frame answers
    /// `{"queued": true}`; its result goes to the history and the bus.
    pub fn run(&mut self, id: &str, args: Value, origin: Origin) -> Result<Value, String> {
        let result = match CommandId::new(id) {
            Ok(c) => match self.app.run(&c, args, to_pop(&origin)) {
                Reply::Done(r) => r,
                Reply::Later(_) => Ok(json!({ "queued": true })),
            },
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = &result {
            self.warn(e.clone());
        }
        result
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
            CommandSpec::new("workbench.about", "About Helios", "Help"),
            Box::new(|_, _| Ok(Action::About)),
        );
        add(
            CommandSpec::new("workbench.keybindings", "Keyboard shortcuts", "Preferences"),
            Box::new(|_, _| Ok(Action::Keybindings)),
        );
        add(
            CommandSpec::new("workbench.quit", "Quit Helios", "Window"),
            Box::new(|_, _| Ok(Action::Close)),
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
        {
            let id = *id;
            add(
                CommandSpec::new(id, title, "Workspace"),
                Box::new(move |a, origin| {
                    Ok(Action::WorkspaceEdit(id.into(), a.clone(), origin.clone()))
                }),
            );
        }
        add(
            CommandSpec::new("workbench.palette", "Show all commands", "View"),
            Box::new(|_, _| Ok(Action::Palette)),
        );
        add(
            CommandSpec::new(
                crate::quick_open::COMMAND,
                "Go to a module, an image or a workspace",
                "View",
            ),
            Box::new(|_, _| Ok(Action::QuickOpen)),
        );
        add(
            CommandSpec::new(
                crate::quick_open::GO,
                "Go to what quick open offers (`item`: module:<node>, image:<id> or workspace:<path>)",
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
            ("window.close", "Close the window", || Action::Close),
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
                "Vault",
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
                CommandSpec::new(&format!("vault.rate.{n}"), &title, "Vault"),
                Box::new(move |_, _| Ok(Action::Rate(n))),
            );
        }
        add(
            CommandSpec::new(
                "vault.reject",
                "Reject the selected image, or take it back",
                "Vault",
            ),
            Box::new(|_, _| Ok(Action::Reject)),
        );
        for label in crate::library::LABELS {
            let title = format!("Toggle the {label} label on the selected image");
            add(
                CommandSpec::new(&format!("vault.label.{label}"), &title, "Vault"),
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
                    CommandSpec::new(id, title, "Vault"),
                    Box::new(move |_, _| Ok(Action::Flag(flag))),
                );
            }
        }
        add(
            CommandSpec::new(
                "view.library",
                "Go to the library: the vault's images to browse, rate and cull",
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
                "Vault",
            ),
            Box::new(|a, _| Ok(Action::AutoAdvance(a.get("on").and_then(Value::as_bool)))),
        );
        add(
            CommandSpec::new(
                "vault.stack.toggle",
                "Expand or collapse the stack of the selected image",
                "Vault",
            ),
            Box::new(|_, _| Ok(Action::ToggleStack)),
        );
        add(
            CommandSpec::new(
                "library.importSource",
                "Import from the workspace's import source (`path`: the card or folder; asked when absent)",
                "Vault",
            ),
            Box::new(|a, _| {
                Ok(Action::ImportSource(
                    a.get("path")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(PathBuf::from),
                ))
            }),
        );
        add(
            CommandSpec::new("vault.next", "Select the next image", "Vault"),
            Box::new(|_, _| Ok(Action::Step(1))),
        );
        add(
            CommandSpec::new("vault.previous", "Select the previous image", "Vault"),
            Box::new(|_, _| Ok(Action::Step(-1))),
        );
        for (id, title, value) in [
            ("param.copyPath", "Copy a value's path (`path`)", false),
            ("param.copyValue", "Copy a value (`path`)", true),
        ] {
            let values = w.values.clone();
            add(
                CommandSpec::new(id, title, "Values"),
                Box::new(move |a, _| {
                    let path = string(a, "path")?;
                    if !value {
                        return Ok(Action::Copy(path));
                    }
                    let store = lock(&values);
                    let e = store
                        .get(&path)
                        .ok_or_else(|| format!("no value at `{path}`"))?;
                    Ok(Action::Copy(
                        helios_values::value_to_json(&e.value).to_string(),
                    ))
                }),
            );
        }
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
        for name in crate::theme::choosable(w.themes) {
            d.checked
                .insert(format!("theme.{name}"), format!("heliosTheme == '{name}'"));
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
        use egui_pop::api::app::{MENU_FILE, MENU_HELP};
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
            command(SYSTEM, "4_about", 0, "workbench.about"),
            command(SYSTEM, "9_quit", 0, "workbench.quit"),
            command(SYSTEM_PROFILES, "2", 0, "profile.export"),
            command(SYSTEM_PROFILES, "2", 1, "profile.import"),
            command(SYSTEM_THEMES, "2", 0, "theme.exportMerged"),
            // A parameter's context menu: its widget gives the path.
            command(plugins::PARAM, "1", 0, "param.reset"),
            command(plugins::PARAM, "2", 0, "param.copyPath"),
            command(plugins::PARAM, "2", 1, "param.copyValue"),
            // The same commands in the menu bar.
            command(MENU_FILE, "1_workspace", 0, "workspace.open"),
            // The workspace's own edits.
            command(MENU_FILE, "2_edit", 0, "workspace.edit"),
            command(MENU_FILE, "2_edit", 1, "workspace.library"),
            command(MENU_FILE, "2_edit", 2, "workspace.importParts"),
            command(MENU_FILE, "2_edit", 3, "workspace.saveAsTemplate"),
            command(MENU_FILE, "2_edit", 4, "workspace.reset"),
            command(MENU_HELP, "4_about", 0, "workbench.about"),
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
        helios_i18n::message(&self.i18n, &m.into())
    }

    /// Shows a notice, in the shell's language: the status bar always, a
    /// toast unless it is plain information. Kept in `notes` too.
    pub fn notify(&mut self, level: Level, text: impl Into<Message>) {
        let message = text.into();
        let text = self.text(message.clone());
        self.notes.push(text.clone());
        let toast = match level {
            Level::Info => None,
            Level::Warning => Some(Toast::warning(&text)),
            Level::Error => Some(Toast::error(&text)),
        };
        if let (Some(t), Some(toasts)) = (toast, self.app.handle().service(&TOASTS)) {
            toasts.notify(t);
        }
        self.notices.push(Notice {
            level,
            text,
            key: message.key,
            action: None,
            at: self.now,
        });
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
        let engine = self.viewport.borrow().engine().clone();
        for e in engine.bus().since(self.engine_seen) {
            self.engine_seen = e.seq;
            let message = || serde_json::from_value::<Message>(e.payload["message"].clone()).ok();
            match e.kind {
                "engine.gpu_fell_back" => {
                    let module = e.payload["module"].as_str().unwrap_or_default().to_string();
                    if self.fell_back.insert(module)
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
        match self.dialogs.get() {
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
        if self.assume_yes {
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
        let mut d = dialogs::Confirm::new(&c.title, &c.verb, command)
            .detail(&c.detail)
            .args(c.args);
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
            .library
            .borrow()
            .entries
            .iter()
            .map(|e| (e.id.clone(), e.name.clone()))
            .collect();
        let sources = crate::quick_open::Sources {
            modules,
            images,
            workspaces: recent_workspaces(),
        };
        let choices = crate::quick_open::choices(&sources, |m| self.text(m));
        let title = self.text(Message::new(
            "gui-quick-open-title",
            "Go to a module, an image or a workspace",
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
                    .viewport
                    .borrow()
                    .graph()
                    .is_some_and(|g| g.find(&node).is_some());
                if !known {
                    return Err(format!("no module `{node}` in the network shown"));
                }
                lock(&self.values)
                    .set(helios_values::Set::new(
                        crate::params::SELECTION,
                        helios_values::Change::To(helios_core::Value::Text(node.clone())),
                        Origin::User,
                    ))
                    .map_err(|e| e.to_string())?;
                if self.panels.iter().any(|p| p.id.as_str() == "helios.params") {
                    self.run("helios.params.focus", Value::Null, Origin::User)?;
                }
                Ok(json!({ "module": node }))
            }
            Item::Image(id) => {
                let path = self
                    .image_files
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| format!("image `{id}` has no file at hand"))?;
                self.opens.push(crate::platform::Open::Image(path.clone()));
                Ok(json!({ "image": id, "path": path }))
            }
            Item::Workspace(dir) => {
                let ctx = self.app.handle().egui();
                self.apply(&ctx, Action::OpenWorkspace(Some(dir.clone())))?;
                Ok(json!({ "workspace": dir }))
            }
        }
    }

    /// Opens a menu of commands at a position.
    pub fn open_menu(&mut self, at: egui::Pos2, items: Vec<MenuItem>) {
        egui_pop::api::menus::close_popup(self.app.handle());
        self.menu = Some((at, items));
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
        self.set_context(&self.app.handle().egui());
        let h = self.app.handle().clone();
        let cx = h.when_context();
        match MenuId::new(menu) {
            Ok(id) => rows(&h, &id, &cx),
            Err(_) => Value::Array(Vec::new()),
        }
    }

    /// The notices so far, oldest first.
    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    /// Answers the dialog shown, as `origin` (applied at the next frame).
    fn answer(&mut self, answer: Answer, origin: &Origin) -> Result<(), String> {
        let d = self.dialogs.get().ok_or("no dialogs")?.clone();
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

    /// Sets the context keys commands and keys are tested against.
    fn set_context(&self, ctx: &Egui) {
        let h = self.app.handle();
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
            let store = lock(&self.values);
            h.set_context("maskTool", &crate::masks::tool(&store));
            h.set_context("maskPoint", flag(crate::masks::point(&store).is_some()));
            h.set_context("maskDisplay", crate::mask_display::mode(&store).name());
        }
        h.set_context("heliosTheme", &self.theme_name);
        h.set_context("heliosProfile", &self.profile_name);
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
        let mut doc = self.app.handle().export_keymap(Layer::User);
        doc.bindings
            .retain(|b| !ours.iter().any(|o| crate::keymaps::same(o, b)));
        if on {
            doc.bindings.splice(0..0, ours);
        }
        let diff = self.app.load_keymap(&doc, Layer::User)?;
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
        if let Err(e) = self.app.load_keymap(&doc, Layer::Workspace) {
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
    /// Vault panel then offers to import from it) and culling's
    /// auto-advance.
    pub fn apply_template_library(&mut self, library: Option<&helios_template::LibraryDesc>) {
        let Some(l) = library else {
            return;
        };
        self.import_source.clone_from(&l.import);
        let mut lib = self.library.borrow_mut();
        lib.auto_advance = l.auto_advance;
        lib.import_offered = l.import.is_some();
    }

    /// The workspace's import source, as its template set it.
    pub fn import_source(&self) -> Option<&helios_template::ImportDesc> {
        self.import_source.as_ref()
    }

    /// Whether the workspace proposes stacks after each import.
    pub fn suggests_stacks(&self) -> bool {
        self.import_source
            .as_ref()
            .is_some_and(|i| i.suggest_stacks)
    }

    /// The imports asked for since the last call (`library.import`'s
    /// arguments), for the host to run.
    pub fn take_imports(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.imports)
    }

    /// Imports a card or folder as the workspace's source says (folder,
    /// referenced, duplicates skipped without one); the path is asked
    /// when absent.
    fn import_from_source(&mut self, path: Option<PathBuf>) -> Value {
        let Some(path) = path else {
            self.form(crate::dialogs::Form {
                title: self.text(Message::new(
                    "gui-import-form-title",
                    "Import a card or a folder",
                )),
                fields: vec![crate::dialogs::Field {
                    key: "path".into(),
                    descriptor: helios_values::Descriptor::path(
                        &self.text(Message::new("gui-import-path", "card or folder")),
                        helios_values::PathKind::Dir,
                        &[],
                    ),
                    value: helios_core::Value::Text(String::new()),
                }],
                verb: self.text(Message::new("gui-import-verb", "Import")),
                command: "library.importSource".into(),
            });
            return Value::Null;
        };
        let mut args = json!({ "path": path });
        if let Some(source) = &self.import_source {
            if source.placement == "copy" {
                args["copy"] = json!(true);
                if let Some(rename) = &source.rename {
                    args["rename"] = json!(rename);
                }
            }
            args["duplicates"] = json!(source.duplicates);
        }
        self.imports.push(args.clone());
        json!({ "queued": args })
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
            &self.i18n,
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
            Action::Minimize => frame::request(self.app.handle(), ctx, FrameRequest::Minimize),
            Action::Maximize => {
                frame::request(self.app.handle(), ctx, FrameRequest::ToggleMaximize);
            }
            Action::Close => frame::request(self.app.handle(), ctx, FrameRequest::Close),
            Action::SystemMenu => {
                // Under the title bar's start, where its menu button is
                // (the window's top left when the system draws the frame).
                self.menu = None;
                let at = HitTable::of(ctx)
                    .get(egui::ViewportId::ROOT)
                    .and_then(|r| r.caption)
                    .map_or(egui::pos2(0.0, 0.0), |c| {
                        let ppp = ctx.pixels_per_point();
                        egui::pos2(c.x as f32 / ppp, (c.y + c.h) as f32 / ppp)
                    });
                frame::request(self.app.handle(), ctx, FrameRequest::SystemMenu(at));
            }
            Action::Run(id, args, origin) => return self.run(&id, args, origin),
            Action::About => {
                let mut text = String::from(
                    "A node-based photo developer.\n\
                     Free software under the GNU General Public License, version 3 or later.\n\
                     Source: https://github.com/quazardous/helios",
                );
                // Where the logs and crash reports are, on the desktop.
                let files = crate::platform::about(self.app.handle(), |m| self.text(m));
                if !files.is_empty() {
                    text.push_str("\n\n");
                    text.push_str(&files.join("\n"));
                }
                self.open_dialog(dialogs::Info::new(
                    &format!("Helios {}", env!("CARGO_PKG_VERSION")),
                    &text,
                ));
            }
            Action::KeymapPackage(name, on) => return self.keymap_package(&name, on),
            Action::Keybindings => {
                let w = self
                    .app
                    .handle()
                    .service(&SETTINGS_WINDOW)
                    .ok_or("no settings window")?;
                w.open(Some(SHORTCUTS));
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
                let overrides = self.theme_overrides.clone();
                self.set_theme(&name, overrides);
            }
            Action::ThemeExport(path) => {
                // The resolved theme as one file, under its name.
                let mut tokens = self.theme.resolved().to_dtcg();
                tokens["$name"] = json!(self.theme_name);
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
                let profile = match &self.loaded {
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
                self.pending_import = Some(incoming);
            }
            Action::ProfileApply => {
                let p = self
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
            Action::ScopeMode(mode) => self.scopes.borrow_mut().set_mode(mode),
            Action::ScopeStops(on) => self.scopes.borrow_mut().set_stops(on),
            Action::Decision(request, origin) => return self.answer_decision(request, &origin),
            Action::DecisionDecide(node, values, origin) => {
                let values = {
                    let viewport = self.viewport.borrow();
                    let mut decisions = self.decisions.borrow_mut();
                    decisions.refresh(viewport.graph());
                    decisions.values_of(&node, &values)?
                };
                return self
                    .answer_decision(crate::decisions::Request::Decide { node, values }, &origin);
            }
            Action::DecisionList => {
                let viewport = self.viewport.borrow();
                let mut decisions = self.decisions.borrow_mut();
                decisions.refresh(viewport.graph());
                return Ok(decisions.describe());
            }
            Action::Rate(n) => self
                .library
                .borrow_mut()
                .rate(n)
                .map_err(|e| format!("vault.rate: {e}"))?,
            Action::Reject => self
                .library
                .borrow_mut()
                .toggle_reject()
                .map_err(|e| format!("vault.reject: {e}"))?,
            Action::Label(l) => self
                .library
                .borrow_mut()
                .toggle_label(&l)
                .map_err(|e| format!("vault.label: {e}"))?,
            Action::Step(n) => self.library.borrow_mut().step(n),
            Action::Flag(flag) => self
                .library
                .borrow_mut()
                .flag(flag)
                .map_err(|e| format!("vault.flag: {e}"))?,
            Action::AutoAdvance(on) => {
                let mut library = self.library.borrow_mut();
                library.auto_advance = on.unwrap_or(!library.auto_advance);
            }
            Action::Place(frame) => {
                let payload = if frame == crate::contexts::WORKSPACE {
                    Value::from(self.workspace().ok_or("no workspace is open")?)
                } else {
                    Value::Null
                };
                return crate::contexts::go(
                    self.app.handle(),
                    frame,
                    payload,
                    &egui_pop::model::commands::Origin::User,
                );
            }
            Action::Culling(on) => {
                return crate::contexts::culling(
                    self.app.handle(),
                    on,
                    &egui_pop::model::commands::Origin::User,
                );
            }
            Action::ImportSource(path) => return Ok(self.import_from_source(path)),
            Action::ToggleStack => self
                .library
                .borrow_mut()
                .toggle_stack()
                .map_err(|e| format!("vault.stack.toggle: {e}"))?,
            Action::Zoom(z) => self.viewport.borrow_mut().zoom(z, &self.values),
            Action::Fold(area) => {
                let h = self.app.handle();
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
                        let store = lock(&self.values);
                        let viewport = self.viewport.borrow();
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
                let mut store = lock(&self.values);
                return self.viewport.borrow_mut().mask(command, origin, &mut store);
            }
            Action::RenameInTree(member) => {
                self.mask_tree.borrow_mut().start_rename(member)?;
            }
            Action::MaskDisplay(mode, origin) => {
                let mut store = lock(&self.values);
                let now = match mode {
                    Some(m) => crate::mask_display::set(&mut store, m, &origin)?,
                    None => crate::mask_display::cycle(&mut store, &origin)?,
                };
                // What it shows: the node the selection gives, if the
                // selected module has a mask.
                let target = self
                    .viewport
                    .borrow()
                    .graph()
                    .and_then(|g| crate::mask_display::target(g, &store));
                return Ok(json!({
                    "mode": now.name(),
                    "mask": target.map(|t| t.key),
                }));
            }
            Action::Compare => {
                use crate::viewport::COMPARE;
                let mut store = lock(&self.values);
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
            Action::Copy(text) => {
                ctx.copy_text(text.clone());
                self.note(Message::new("gui-copied", "copied { $text }").with("text", &text));
            }
            // The workbench's steps: the panel's rules, then any plugin,
            // may refuse.
            Action::MoveTo(p, area) => {
                return workbench_outcome(self.app.handle().move_panel(&panel_id(&p)?, area));
            }
            Action::Answer(answer, origin) => self.answer(answer, &origin)?,
            Action::AssumeYes(on) => {
                self.assume_yes = on;
                self.note(if on {
                    Message::new("gui-policy-yes", "confirmations are answered yes by policy")
                } else {
                    Message::new("gui-policy-ask", "confirmations are asked again")
                });
            }
            Action::Detach(p) => {
                return workbench_outcome(self.app.handle().detach_panel(&panel_id(&p)?));
            }
            Action::Float(p, pos) => {
                let id = panel_id(&p)?;
                let h = self.app.handle();
                let pos = pos.unwrap_or_else(|| {
                    egui_pop::model::layout::FloatingGroup::cascade(h.layout().floating.len())
                });
                return workbench_outcome(
                    h.change_workbench(egui_pop::api::workbench::Change::Float(id, pos)),
                );
            }
            Action::Dock(p) => {
                let id = panel_id(&p)?;
                let h = self.app.handle();
                let layout = h.layout();
                if layout.auxiliary.iter().any(|a| a.panel == id) {
                    return workbench_outcome(h.attach_panel(&id));
                }
                // A palette docks in its own area.
                if layout.floating_of(&id).is_some() {
                    let area = self
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
                self.shots.push_back(Shot::Save { target, path });
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            Action::RecordStart { target, path, fps } => {
                self.recording = Some((target, Recorder::new(path, fps)));
            }
            Action::RecordStop => {
                if let Some((_, rec)) = self.recording.take() {
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
            let actions = std::mem::take(&mut *lock(&self.queue));
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
        if self.target_window(target) != egui::ViewportId::ROOT {
            return Err(format!(
                "`{target}` is in a detached window, which pictures do not reach yet"
            ));
        }
        self.target_rect(target)
    }

    fn on_screenshot(&mut self, image: &egui::ColorImage) {
        let full = snapshot::from_color_image(image);
        let Some(shot) = self.shots.pop_front() else {
            return;
        };
        match shot {
            Shot::Save { target, path } => {
                let note = self
                    .pictured_rect(&target)
                    .map(|r| snapshot::crop(&full, r, self.pixels_per_point))
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
                let target = self.recording.as_ref().map(|(t, _)| t.clone());
                if let Some(target) = target
                    && let rect = self.pictured_rect(&target)
                    && let Some((_, rec)) = &mut self.recording
                {
                    let pushed = rect
                        .map(|r| snapshot::crop(&full, r, self.pixels_per_point))
                        .and_then(|img| rec.push(&img));
                    if let Err(e) = pushed {
                        self.warn(
                            Message::new("gui-recording-stopped", "recording stopped: { $error }")
                                .with("error", e),
                        );
                        self.recording = None;
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
        self.pixels_per_point = ctx.pixels_per_point();
        let native = scale::Scale::of(ctx).native;
        let changed = scale::changed(ctx).is_some() || self.native_scale.is_none();
        let was = self.native_scale.replace(native).unwrap_or(1.0);
        if changed
            && (was >= HIDPI) != (native >= HIDPI)
            && let Some((source, _)) = &self.loaded
            && source.on.contains_key(&helios_profile::Selector::Hidpi)
        {
            let source = source.clone();
            self.apply_profile(&source);
        }
    }

    /// Draws the shell for one frame.
    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if let Some(rs) = self.gpu.take() {
            self.gpu_info = Some(egui_pop::viewport::gpu::install(&ctx, &rs));
            crate::start::mark_in_first_frame("viewport GPU path installed");
        }
        self.follow_scale(&ctx);
        self.now = ctx.input(|i| i.time);
        for event in ctx.input(|i| i.events.clone()) {
            if let Event::Screenshot { image, .. } = event {
                self.on_screenshot(&image);
            }
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
            }
        }
        self.set_context(&ctx);
        let wanted = crate::display::wanted(crate::display::wide_gamut(self.app.handle()));
        self.viewport.borrow_mut().set_wanted(wanted);
        self.app.logic(&ctx);
        self.apply_queue(&ctx);
        // The workspace's files changed outside, and agents' requests.
        self.follow_workspace(&ctx);
        // Keys changed for the workspace in the shortcuts editor.
        self.follow_workspace_keys();
        self.follow
            .frame(self.app.handle(), &mut lock(&self.values));
        // An undo or a redo of the workspace's state, put in place.
        self.sync_workspace();
        // The theme asked for is installed here, before anything is drawn.
        self.app.logic(&ctx);
        let spec = self.app.handle().theme().spec();
        if self.installed.as_ref() != Some(&spec) {
            if self.still {
                ctx.all_styles_mut(|style| {
                    style.visuals.text_cursor.blink = false;
                    style.animation_time = 0.0;
                });
            }
            self.follow_theme(&spec);
            self.installed = Some(spec);
        }
        self.set_context(&ctx);
        crate::start::mark_in_first_frame("commands run, theme installed");

        self.rects.clear();
        self.rect_windows.clear();
        ctx.data_mut(|d| {
            d.remove::<BTreeMap<String, (Rect, egui::ViewportId)>>(Id::new("helios.named"));
        });
        self.rects.insert("window".into(), ctx.content_rect());

        self.viewport.borrow_mut().sync(&mut lock(&self.values));
        self.follow_decisions();
        self.status_bar(&ctx);
        self.status_items(&ctx);
        // Duplicates an import asks about: said once, answered in the
        // Duplicates panel.
        let asked = self.duplicates.borrow_mut().receive();
        if asked > 0 {
            self.note(
                Message::new(
                    "gui-duplicates-waiting-note",
                    "{ $count } duplicates wait for an answer in the Duplicates panel",
                )
                .with("count", asked),
            );
        }
        self.preview.follow(self.viewport.borrow().shown(), &ctx);
        self.app.show(ui);
        crate::start::mark_in_first_frame("panels drawn");
        self.graph_edits();
        if self.library.borrow_mut().take_import_asked() {
            let _ = self.run("library.importSource", Value::Null, Origin::User);
        }
        let refused = std::mem::take(&mut self.library.borrow_mut().notes);
        for n in refused {
            self.warn(n);
        }
        self.workbench_refusals();
        self.save_layout_on_change(&ctx);
        self.engine_notices();
        self.job_notices();
        self.follow_queue();
        self.menus_ui(&ctx);
        self.pointer_choice_ui(&ctx);

        let named = ctx.data(|d| {
            d.get_temp::<BTreeMap<String, (Rect, egui::ViewportId)>>(Id::new("helios.named"))
        });
        for (name, (rect, window)) in named.unwrap_or_default() {
            if window != egui::ViewportId::ROOT {
                self.rect_windows.insert(name.clone(), window);
            }
            self.rects.insert(name, rect);
        }
        self.overlay_rects();

        if self.recording.is_some() {
            self.shots.push_back(Shot::Frame);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            ctx.request_repaint();
        }
    }

    /// What the Graph panel's edits asked for: its `network.*` commands,
    /// each run as the user's (one undo step each), and what it has to
    /// say.
    fn graph_edits(&mut self) {
        let (commands, notes) = {
            let mut g = self.graph.borrow_mut();
            (g.take_commands(), std::mem::take(&mut g.notes))
        };
        for (id, args) in commands {
            // A refusal is said by `run`.
            let _ = self.run(&id, args, Origin::User);
        }
        for n in notes {
            self.note(n);
        }
    }

    /// The Graph panel's state (its view, selection and folds).
    pub fn graph_panel(&self) -> std::cell::Ref<'_, crate::graph::GraphPanel> {
        self.graph.borrow()
    }

    /// Saves the workspace's layout once it changed (egui-pop's
    /// `layout.save` step), when no button is held (a splitter dragged
    /// is saved where it is let go): a kill loses no arrangement.
    fn save_layout_on_change(&mut self, ctx: &Egui) {
        if self.workspace.get().is_none() || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let layout = self.layout();
        if self.layout_saved.as_ref() == Some(&layout) {
            return;
        }
        if let Err(e) = step_result(self.app.save_layout()) {
            self.warn(
                Message::new(
                    "gui-layout-not-saved",
                    "The layout was not saved: { $error }",
                )
                .with("error", e),
            );
        }
        self.layout_saved = Some(layout);
    }

    /// What the workbench refused this frame (a tab dropped where its
    /// panel may not go), from egui-pop's journal: said in the status bar
    /// (egui-pop shows the toast).
    fn workbench_refusals(&mut self) {
        let h = self.app.handle().clone();
        let events = h.events_since(self.seen);
        self.seen = h.last_event();
        for e in events {
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
        for o in self.app.handle().overlay_rects() {
            let name = match o.kind {
                OverlayKind::Dialog => "helios.dialog",
                OverlayKind::Toasts => "helios.toasts",
                OverlayKind::Menu => "helios.menu",
                _ => continue,
            };
            self.rects.insert(name.into(), o.rect);
        }
    }

    /// The first run's "Coming from darktable?" (`pointer_choice`), when
    /// offered and wanted: an area at the bottom of the window over the
    /// panels, not a dialog, so it blocks neither keys nor clicks.
    fn pointer_choice_ui(&mut self, ctx: &Egui) {
        let h = self.app.handle().clone();
        if !self.pointer_choice || !crate::pointer_choice::wanted(&h) {
            return;
        }
        let shown = egui::Area::new(Id::new(crate::pointer_choice::RECT))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    crate::pointer_choice::body(ui, &h);
                });
            });
        self.rects
            .insert(crate::pointer_choice::RECT.into(), shown.response.rect);
    }

    /// Menus of commands asked for, in egui's popups (the system menu is
    /// egui-pop's, `menus::popup_at`).
    fn menus_ui(&mut self, ctx: &Egui) {
        let h = self.app.handle().clone();
        let layer = egui::LayerId::background();
        if let Some((at, items)) = self.menu.take() {
            let mut open = true;
            let mut chosen = None;
            let cx = h.when_context();
            let shown = egui::Popup::new(Id::new("helios.menu"), ctx.clone(), at, layer)
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
                self.rects.insert("helios.menu".into(), r.response.rect);
            }
            if let Some((id, args)) = chosen {
                let _ = self.run(&id, args, Origin::User);
            } else if open {
                self.menu = Some((at, items));
            }
        }
    }

    /// Offers the first run's choice of darktable's pointer profile
    /// ("Coming from darktable?", `pointer_choice`): shown while no
    /// profile is saved and the question was never answered. The
    /// desktop asks for it; tests and tools do only when they test it.
    pub fn offer_pointer_choice(&mut self) {
        self.pointer_choice = true;
    }

    /// Helios's status line, which its block draws in egui-pop's status
    /// bar (beside the jobs item).
    fn status_bar(&self, ctx: &egui::Context) {
        let mut line = self.status.borrow_mut();
        line.recording = self.recording.is_some();
        line.note = self.notes.last().cloned();
        line.hint = self.viewport.borrow().masks().hint(ctx);
        line.decisions = self.stack_review.borrow().decisions().len();
        {
            let decisions = self.decisions.borrow();
            line.pending_decisions = decisions.pending();
            line.decisions_for_a_person = decisions.for_a_person();
        }
        line.profile.clone_from(&self.profile_name);
    }

    /// What the status line says now: the hovered mask handle's
    /// gestures, else the last note.
    pub fn status_text(&self) -> Option<String> {
        let line = self.status.borrow();
        line.hint.clone().or_else(|| line.note.clone())
    }

    /// Where background work in this process reports: `job.*` events on
    /// the shell's bus, shown in the jobs centre, cancelled from it
    /// (see `crate::jobs`). An import, an export started from the
    /// desktop, a calibration: each starts its job here.
    pub fn jobs(&self) -> helios_core::jobs::Jobs {
        self.jobs.clone()
    }

    /// The exports and batches that ended since the last frame: said on
    /// the desktop when long and unseen (`job_finished`). The jobs of a
    /// node lost for too long leave the jobs centre.
    fn job_notices(&mut self) {
        let ends = self.ended.take(self.app.handle());
        for end in ends {
            self.job_finished(&end);
        }
        self.forgotten.take(self.app.handle());
    }

    /// The Queue panel follows the selected node: its queue read again,
    /// the panel closed or opened again as its setting says when empty.
    fn follow_queue(&mut self) {
        if !self
            .panels
            .iter()
            .any(|p| p.id.as_str() == crate::queue::PANEL)
        {
            return;
        }
        let node = lock(&self.values)
            .get(crate::params::SELECTION)
            .and_then(|e| match &e.value {
                helios_core::Value::Text(t) if !t.is_empty() => Some(t.clone()),
                _ => None,
            });
        self.queue_panel
            .borrow_mut()
            .update(self.app.handle(), node.as_deref());
    }

    /// The Queue panel (see `crate::queue`).
    pub fn queue_panel(&self) -> std::cell::RefMut<'_, crate::queue::QueuePanel> {
        self.queue_panel.borrow_mut()
    }

    /// Whether the Queue panel shows when the selected node has no queue
    /// (`interface.panels.queue_when_empty`, on by default).
    pub fn set_queue_when_empty(&mut self, on: bool) {
        self.queue_panel.borrow_mut().set_when_empty(on);
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
        let relay = crate::relay::Relay::start(link, self.jobs.bus().clone(), timing)?;
        self.relays.borrow_mut().insert(name, relay);
        Ok(())
    }

    /// Drops the link to a followed node, as a network failure would: it
    /// is followed again at the relay's next attempt. Whether the node is
    /// followed.
    pub fn drop_node_link(&self, name: &str) -> bool {
        let relays = self.relays.borrow();
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
            MenuItem::Command { id, args } => {
                let Ok(cid) = CommandId::new(id.as_str()) else {
                    continue;
                };
                let Some(spec) = h.command(&cid) else {
                    continue;
                };
                let mut button = egui::Button::new(h.command_menu_title(&spec));
                if let Some(k) = h.keys_for(&cid).first() {
                    button = button.shortcut_text(k.clone());
                }
                let applies = h.applies(&cid, cx);
                let r = ui.add_enabled(applies, button);
                if !applies {
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
        // The shared device, with a renderer of our own: the textures and
        // the viewport's GPU objects belong to this harness alone.
        let mut rs = device().clone();
        rs.renderer = Arc::new(RwLock::new(Renderer::new(
            &rs.device,
            rs.target_format,
            RendererOptions::PREDICTABLE,
        )));
        shell.attach_gpu(&rs);
        shell.still = true;
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
}
