//! The desktop shell: an egui-pop application (`egui_pop::api::App`) with
//! Helios's plugins, around Helios's own frame, areas and status bar.
//!
//! egui-pop draws what every application shares, from declarations: the
//! menu bar and menus, the command palette, dialogs, toasts, the settings
//! window with its shortcuts editor, the window's place, and the keys,
//! resolved by its keymap. Helios declares its commands there and runs
//! them on its own kernel (see `host`), draws its title bar and status
//! bar, and its areas (docked panels and auxiliary windows, see
//! `workbench`) as a block of egui-pop's main zone.
//!
//! Everything the user can do goes through a command, so a key, the
//! palette, a menu, a script and an agent reach the same actions.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use egui::{Context as Egui, Event, Id, Rect, Ui};
use egui_pop::api::app::OverlayKind;
use egui_pop::api::dialogs::{self, Choose, ChooseOption, DIALOGS, Dialogs};
use egui_pop::api::host::{Reply, Resolver};
use egui_pop::api::settings_window::{SETTINGS_WINDOW, SHORTCUTS};
use egui_pop::api::toasts::{TOASTS, Toast};
use egui_pop::api::{App, AppHandle, DefaultPlugins, palette};
use egui_pop::model::ids::{CommandId, MenuId, OwnerId};
use egui_pop::model::keymap::Layer;
use egui_pop::model::menus::{Item, MenuItem as PopItem, Shown, show_items};
use helios_core::Bus;
use helios_shell::commands::{CommandSpec, Origin, Registry, Run};
use helios_shell::layout::Layout;
use helios_shell::panels::{Area, PanelSpec, Panels};
use image::RgbaImage;
use serde_json::{Value, json};

use crate::dialogs::{Answer, Level, MenuItem, Notice};
use crate::host::{HeliosHost, Later, to_helios, to_pop};
use crate::plugins::{
    self, Declarations, ListSource, SYSTEM, SYSTEM_PROFILES, SYSTEM_RECENT, SYSTEM_THEMES,
    menu_title,
};
use crate::snapshot::{self, Recorder};
use crate::workbench::Workbench;

pub use crate::workbench::Content;

/// Pixels per point from which a screen is `hidpi` for profile variants.
const HIDPI: f32 = 1.5;

/// What commands ask the shell to do; they are queued and applied at the
/// next frame, where the shell's state is at hand. The command shows as
/// running until then (see `host::Later`).
enum Action {
    Palette,
    Theme(String),
    ThemeExport(PathBuf),
    /// Where, which parts, only this machine's view.
    ProfileExport(PathBuf, Vec<helios_profile::Part>, bool),
    ProfileImport(PathBuf),
    ProfileApply,
    ProfileUse(String),
    ScopeMode(crate::scopes::Mode),
    ScopeStops(bool),
    Rate(i64),
    Reject,
    Label(String),
    Step(i64),
    Fullscreen,
    Minimize,
    Maximize,
    Close,
    Focus(String),
    Toggle(String),
    Detach(String),
    MoveTo(String, Area),
    Zoom(Zoom),
    Copy(String),
    SystemMenu,
    /// A command, its arguments, as whom.
    Run(String, Value, Origin),
    About,
    Keybindings,
    OpenWorkspace(Option<PathBuf>),
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

/// What the viewport's zoom commands ask for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Zoom {
    Fit,
    Actual,
    By(f64),
}

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
/// that name (`gui.snapshot {target: name}`).
pub fn name_rect(ctx: &Egui, name: &str, rect: Rect) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<BTreeMap<String, Rect>>(Id::new("helios.named"))
            .insert(name.to_string(), rect);
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

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Shell {
    app: App,
    registry: Arc<Mutex<Registry>>,
    lists: Rc<RefCell<ListSource>>,
    workbench: Rc<RefCell<Workbench>>,
    queue: Queue,
    rects: BTreeMap<String, Rect>,
    pixels_per_point: f32,
    shots: VecDeque<Shot>,
    recording: Option<(String, Recorder)>,
    /// What the shell has to say: command results, layout repairs.
    pub notes: Vec<String>,
    title: String,
    /// The window's frame is drawn by Helios (see `frame`).
    framed: bool,
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
    params: Rc<RefCell<crate::params::Params>>,
    /// Every value widgets and agents edit (`param.*` commands).
    pub values: Arc<Mutex<helios_values::Store>>,
    /// Headless runs: no cursor blink and no animation, so a frame settles
    /// and a test's `run` ends.
    still: bool,
}

impl Shell {
    /// A shell over declared panels, in a saved layout (repaired) or the
    /// default one.
    pub fn new(panels: Panels, layout: Option<Layout>) -> Shell {
        Shell::build(panels, layout, None)
    }

    /// As [`Shell::new`], keeping the interface's settings (window place,
    /// focus mode, the user's keys) in `ui_settings`.
    pub fn build(panels: Panels, layout: Option<Layout>, ui_settings: Option<PathBuf>) -> Shell {
        let registry = Arc::new(Mutex::new(Registry::new()));
        let bus = Arc::new(Bus::new());
        // Helios's values: what widgets and agents edit, and what Edit >
        // Undo walks back through the host.
        let values: Arc<Mutex<helios_values::Store>> = Arc::default();
        let host =
            HeliosHost::new(registry.clone(), bus, Arc::default()).with_values(values.clone());
        let later = host.later();
        let specs: Vec<PanelSpec> = panels.all().to_vec();
        let (workbench, notes) = Workbench::new(panels, layout);
        let workbench = Rc::new(RefCell::new(workbench));
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
        Shell::declare_commands(&wiring, &specs);
        // The param commands first: the menus take their titles.
        helios_values::register(&mut lock(&registry), wiring.values.clone())
            .expect("param commands are declared once");
        Shell::declare_menus(&wiring);
        let Wiring {
            queue,
            dialogs,
            resolved_view,
            values,
            ..
        } = wiring;
        let mut builder = App::builder()
            .host(host)
            .themes(themes.clone())
            .theme(crate::theme::ThemeSpec::new(crate::theme::DEFAULT))
            .plugins(DefaultPlugins)
            .plugin(plugins::Commands {
                registry: registry.clone(),
                declarations: declarations.clone(),
            })
            .plugin(plugins::Lists {
                source: lists.clone(),
            })
            .plugin(plugins::History {
                values: values.clone(),
            })
            .plugin(plugins::Areas {
                workbench: workbench.clone(),
            });
        if let Some(dir) = ui_settings {
            builder = builder.persist(dir);
        }
        let app = builder
            .build(&Egui::default())
            .unwrap_or_else(|e| panic!("Helios's application builds: {e}"));
        if let Some(d) = app.handle().service(&DIALOGS) {
            let _ = dialogs.set((*d).clone());
        }
        let mut shell = Shell {
            app,
            registry,
            lists,
            workbench,
            queue,
            rects: BTreeMap::new(),
            pixels_per_point: 1.0,
            shots: VecDeque::new(),
            recording: None,
            notes,
            title: String::new(),
            framed: false,
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
            dialogs,
            assume_yes: false,
            menu: None,
            notices: Vec::new(),
            now: 0.0,
            viewport: Rc::default(),
            params: Rc::default(),
            values,
            still: false,
        };
        for d in shell.app.handle().diagnostics() {
            shell.notes.push(format!("{}: {}", d.code, d.message));
        }
        {
            let mut store = lock(&shell.values);
            crate::viewport::declare(&mut store);
            store.declare(
                crate::params::SELECTION,
                helios_values::Descriptor::text("selected node", ""),
            );
        }
        let has = |shell: &Shell, panel: &str| shell.workbench.borrow().panels.get(panel).is_some();
        if has(&shell, "helios.params") {
            shell
                .params
                .borrow_mut()
                .set_app(shell.app.handle().clone());
            let (params, values) = (shell.params.clone(), shell.values.clone());
            shell.set_content("helios.params", move |ui| {
                let selection = lock(&values)
                    .get(crate::params::SELECTION)
                    .and_then(|e| match &e.value {
                        helios_core::Value::Text(t) if !t.is_empty() => Some(t.clone()),
                        _ => None,
                    });
                params
                    .borrow_mut()
                    .panel_ui(ui, &values, selection.as_deref());
            });
        }
        if has(&shell, "helios.viewport") {
            let (viewport, values) = (shell.viewport.clone(), shell.values.clone());
            shell.set_content("helios.viewport", move |ui| {
                viewport.borrow_mut().ui(ui, &values);
            });
        }
        if has(&shell, "helios.scopes") {
            let scopes = shell.scopes.clone();
            shell.set_content("helios.scopes", move |ui| scopes.borrow_mut().ui(ui));
        }
        for (panel, strip) in [("helios.vault", false), ("helios.filmstrip", true)] {
            if has(&shell, panel) {
                let library = shell.library.clone();
                shell.set_content(panel, move |ui| {
                    let mut library = library.borrow_mut();
                    if strip {
                        library.filmstrip(ui);
                    } else {
                        library.grid(ui);
                    }
                });
            }
        }
        shell
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

    /// Declares a command on Helios's registry, and to the interface.
    pub fn register(
        &mut self,
        spec: CommandSpec,
        handler: impl Fn(&Value, &Origin) -> Result<Value, String> + Send + Sync + 'static,
    ) -> Result<(), String> {
        lock(&self.registry).register(spec, handler)?;
        self.redeclare(&plugins::COMMANDS);
        Ok(())
    }

    /// Every command Helios declares, by id.
    pub fn specs(&self) -> Vec<CommandSpec> {
        lock(&self.registry).specs().cloned().collect()
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

    /// Connects a plugin again, so what it declares is read again.
    fn redeclare(&mut self, plugin: &egui_pop::model::ids::PluginId) {
        self.app.disconnect(plugin);
        if let Err(e) = self.app.connect(plugin) {
            self.notes.push(format!("{plugin}: {e}"));
        }
        self.app.rebuild();
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

    /// Where thumbnails are sent, by image id, from any thread.
    pub fn library_thumbnails(&self) -> std::sync::mpsc::Sender<(String, helios_core::Image)> {
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
        self.viewport.borrow_mut().set_source(graph, node, port)
    }

    /// Gives the viewport the GPU objects it draws with, on the render
    /// state the window (or a test harness) draws with: the process's one
    /// device ([`wgpu_setup`]), never one of the window's own.
    pub fn attach_gpu(&mut self, rs: &eframe::egui_wgpu::RenderState) {
        debug_assert!(
            helios_gpu::shared::owns(&rs.instance),
            "the window opened a wgpu instance of its own: hand it wgpu_setup()"
        );
        crate::viewport::install(rs);
    }

    /// What the viewport's render thread did.
    pub fn viewport_stats(&self) -> Arc<crate::viewport::Stats> {
        self.viewport.borrow().stats.clone()
    }

    /// The float data the viewport shows, before the display transform.
    pub fn viewport_scene(&self) -> Option<helios_core::Image> {
        self.viewport.borrow().scene().cloned()
    }

    /// The layout as it stands, to be saved with the workspace.
    pub fn layout(&self) -> Layout {
        self.workbench.borrow().layout()
    }

    /// Gives a panel its content; panels without one show a placeholder.
    pub fn set_content(&mut self, panel: &str, content: impl FnMut(&mut Ui) + 'static) {
        self.workbench
            .borrow_mut()
            .contents
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
            self.warn(format!(
                "theme {name}: no theme `{name}`: {} is used instead",
                crate::theme::DEFAULT
            ));
        }
        let resolved = if known { name } else { crate::theme::DEFAULT };
        self.theme = crate::theme::resolve(&self.themes, resolved, &self.theme_overrides);
        let mut spec = crate::theme::ThemeSpec::new(resolved);
        spec.overrides.clone_from(&self.theme_overrides);
        self.app.handle().theme().set(spec);
        let problems: Vec<String> = self
            .theme
            .resolved()
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        for p in problems {
            self.warn(format!("theme {name}: {p}"));
        }
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

    /// The user's own key bindings: keys, command, `when`.
    fn user_bindings(&self) -> Vec<(String, String, String)> {
        self.app
            .keymap()
            .export(&OwnerId::USER, Layer::User)
            .bindings
            .into_iter()
            .filter_map(|b| {
                let c = b.command.filter(|c| !c.starts_with('-'))?;
                Some((b.keys, c, b.when))
            })
            .collect()
    }

    /// What the shell holds as a profile: theme, user keys, layout.
    pub fn profile(&self) -> helios_profile::Profile {
        use helios_profile::{Binding, Profile, System, ThemeChoice, portable_keys};
        let keymap = self
            .user_bindings()
            .into_iter()
            .map(|(keys, command, when)| Binding {
                keys: portable_keys(&keys.to_lowercase(), System::current()),
                command,
                when,
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
            self.note(format!(
                "profile {}: {} values from variants: {}",
                p.name,
                from.len(),
                from.join(", ")
            ));
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
        m.hidpi |= self.pixels_per_point >= HIDPI;
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
            let handle = self.app.handle().clone();
            for (_, command, _) in self.user_bindings() {
                if let Ok(id) = CommandId::new(command) {
                    handle.reset_keys(id);
                }
            }
            for b in keys {
                // egui-pop's user keymap takes a command's keys, not a
                // `when` of their own: the command's applies.
                let local = local_keys(&b.keys, System::current());
                match CommandId::new(b.command.as_str()) {
                    Ok(id) => handle.rebind(id, Some(local)),
                    Err(e) => self.warn(format!("profile {}: {e}", p.name)),
                }
            }
            self.app.process_requests();
        }
        if let Some(layout) = &p.layout {
            match Layout::from_json(&layout.to_string()) {
                Ok(l) => self.set_layout(l),
                Err(e) => self.warn(format!("profile {}: layout: {e}", p.name)),
            }
        }
    }

    /// Arranges the panels as a layout says, repaired to today's panels.
    pub fn set_layout(&mut self, layout: Layout) {
        let notes = self.workbench.borrow_mut().set_layout(layout);
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

    /// The window's title, shown in the frame Helios draws.
    pub fn set_title(&mut self, title: &str) {
        self.title = title.into();
    }

    /// Draws the window's frame (title bar, buttons, resize edges) instead
    /// of the system's; the window is then opened without decorations.
    pub fn draw_frame(&mut self, on: bool) {
        self.framed = on;
    }

    /// The panel that has the focus.
    pub fn focused(&self) -> Option<String> {
        self.workbench.borrow().focused.clone()
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
            CommandSpec::new("workbench.palette", "Show all commands", "View"),
            Box::new(|_, _| Ok(Action::Palette)),
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
        for p in panels {
            let id = p.id.clone();
            let (focus, toggle) = (id.clone(), id.clone());
            add(
                CommandSpec::new(
                    &format!("{id}.focus"),
                    &format!("Focus {}", p.title),
                    "View",
                ),
                Box::new(move |_, _| Ok(Action::Focus(focus.clone()))),
            );
            add(
                CommandSpec::new(
                    &format!("{id}.toggle"),
                    &format!("Show or hide {}", p.title),
                    "View",
                ),
                Box::new(move |_, _| Ok(Action::Toggle(toggle.clone()))),
            );
            let move_to = id.clone();
            add(
                CommandSpec::new(
                    &format!("{id}.moveTo"),
                    &format!("Move {} to an area (`area`)", p.title),
                    "View",
                ),
                Box::new(move |a, _| {
                    let area = a
                        .get("area")
                        .cloned()
                        .ok_or("`area` is missing: editors, primary_side_bar, secondary_side_bar or bottom_panel")?;
                    let area: Area =
                        serde_json::from_value(area).map_err(|e| format!("`area`: {e}"))?;
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
                "theme.export",
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
            ("viewport.zoom.fit", "Image: fit to the view", Zoom::Fit),
            (
                "viewport.zoom.100",
                "Image: actual pixels (100 %)",
                Zoom::Actual,
            ),
            ("viewport.zoom.in", "Image: zoom in", Zoom::By(1.25)),
            ("viewport.zoom.out", "Image: zoom out", Zoom::By(0.8)),
        ] {
            add(
                CommandSpec::new(id, title, "View"),
                Box::new(move |_, _| Ok(Action::Zoom(zoom))),
            );
        }
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
        d.keys.push((
            "Alt+Space".into(),
            "workbench.systemMenu".into(),
            String::new(),
        ));
        // As in darktable's lighttable, while the library has the focus
        // (egui-pop leaves plain keys to a text field being typed in).
        let library = "focusedPanel == 'helios.vault' || focusedPanel == 'helios.filmstrip'";
        for n in 0..=5 {
            d.keys
                .push((n.to_string(), format!("vault.rate.{n}"), library.into()));
        }
        for (k, command) in [
            ("R", "vault.reject"),
            ("Right", "vault.next"),
            ("Left", "vault.previous"),
        ] {
            d.keys.push((k.into(), command.into(), library.into()));
        }
        // egui-pop's palette is the palette; Helios's name for it stays
        // for scripts.
        d.hidden.insert("workbench.palette".into());
        for name in crate::theme::choosable(w.themes) {
            d.checked
                .insert(format!("theme.{name}"), format!("heliosTheme == '{name}'"));
        }
    }

    /// The menus Helios contributes: its system menu, and its commands in
    /// egui-pop's menu bar.
    fn declare_menus(w: &Wiring<'_>) {
        use egui_pop::api::app::{MENU_FILE, MENU_HELP};
        let command = |menu: MenuId, group: &str, order: i32, id: &str, title: &str| {
            PopItem::command(
                menu,
                group,
                order,
                CommandId::new(id).expect("Helios's command ids are valid"),
            )
            .label(&menu_title(title))
        };
        let specs: BTreeMap<String, String> = lock(w.registry)
            .specs()
            .map(|s| (s.id.clone(), s.title.clone()))
            .collect();
        let title = |id: &str| specs.get(id).cloned().unwrap_or_else(|| id.to_string());
        let mut items = vec![
            command(
                SYSTEM,
                "1_workspace",
                0,
                "workspace.open",
                &title("workspace.open"),
            ),
            PopItem::submenu(SYSTEM, "1_workspace", 1, SYSTEM_RECENT, "Recent workspaces"),
            PopItem::submenu(SYSTEM, "2_preferences", 0, SYSTEM_PROFILES, "Profiles"),
            PopItem::submenu(SYSTEM, "2_preferences", 1, SYSTEM_THEMES, "Themes"),
            command(
                SYSTEM,
                "2_preferences",
                2,
                "workbench.keybindings",
                &title("workbench.keybindings"),
            ),
            command(
                SYSTEM,
                "4_about",
                0,
                "workbench.about",
                &title("workbench.about"),
            ),
            command(
                SYSTEM,
                "9_quit",
                0,
                "workbench.quit",
                &title("workbench.quit"),
            ),
            command(
                SYSTEM_PROFILES,
                "2",
                0,
                "profile.export",
                &title("profile.export"),
            ),
            command(
                SYSTEM_PROFILES,
                "2",
                1,
                "profile.import",
                &title("profile.import"),
            ),
            command(
                SYSTEM_THEMES,
                "2",
                0,
                "theme.export",
                &title("theme.export"),
            ),
            // A parameter's context menu: its widget gives the path.
            command(plugins::PARAM, "1", 0, "param.reset", &title("param.reset")),
            command(
                plugins::PARAM,
                "2",
                0,
                "param.copyPath",
                &title("param.copyPath"),
            ),
            command(
                plugins::PARAM,
                "2",
                1,
                "param.copyValue",
                &title("param.copyValue"),
            ),
            // The same commands in the menu bar.
            command(
                MENU_FILE,
                "1_workspace",
                0,
                "workspace.open",
                &title("workspace.open"),
            ),
            command(
                MENU_HELP,
                "4_about",
                0,
                "workbench.about",
                &title("workbench.about"),
            ),
        ];
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

    /// Shows a notice: the status bar always, a toast unless it is plain
    /// information. Kept in `notes` too.
    pub fn notify(&mut self, level: Level, text: impl Into<String>) {
        let text = text.into();
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
            action: None,
            at: self.now,
        });
    }

    fn note(&mut self, text: impl Into<String>) {
        self.notify(Level::Info, text);
    }

    fn warn(&mut self, text: impl Into<String>) {
        self.notify(Level::Warning, text);
    }

    fn open_dialog(&mut self, dialog: impl Into<dialogs::Dialog>) {
        match self.dialogs.get() {
            Some(d) => {
                d.open(dialog);
            }
            None => self.warn("no dialogs to show this in"),
        }
    }

    /// Asks before running a command. Under the yes policy it runs at once,
    /// and says so.
    pub fn confirm(&mut self, c: crate::dialogs::Confirm) {
        if self.assume_yes {
            let text = format!("{}: confirmed by policy", c.verb);
            self.notify(Level::Warning, text);
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

    /// Opens a menu of commands at a position.
    pub fn open_menu(&mut self, at: egui::Pos2, items: Vec<MenuItem>) {
        egui_pop::api::menus::close_popup(self.app.handle());
        self.menu = Some((at, items));
    }

    /// Opens the system menu under a rectangle (the logo's).
    fn open_system_menu(&mut self, under: Rect) {
        self.menu = None;
        egui_pop::api::menus::popup_at(self.app.handle(), &SYSTEM, under);
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
                                    .map_or_else(|| id.to_string(), |s| menu_title(&s.title))
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
            self.workbench.borrow().focused.as_deref().unwrap_or(""),
        );
        h.set_context("paletteVisible", flag(palette::is_open(h)));
        h.set_context(
            "fullScreen",
            flag(ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))),
        );
        h.set_context("heliosTheme", &self.theme_name);
        h.set_context("heliosProfile", &self.profile_name);
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
            Action::Fullscreen => {
                let on = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!on));
            }
            Action::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            Action::Maximize => {
                let on = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!on));
            }
            Action::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::SystemMenu => {
                let under = self.rects.get("helios.logo").copied().unwrap_or_else(|| {
                    Rect::from_min_size(
                        egui::pos2(8.0, crate::frame::TITLE_HEIGHT),
                        egui::Vec2::ZERO,
                    )
                });
                self.open_system_menu(under);
            }
            Action::Run(id, args, origin) => return self.run(&id, args, origin),
            Action::About => self.open_dialog(dialogs::Info::new(
                &format!("Helios {}", env!("CARGO_PKG_VERSION")),
                "A node-based photo developer.\n\
                 Free software under the GNU General Public License, version 3 or later.\n\
                 Source: https://github.com/quazardous/helios",
            )),
            Action::Keybindings => {
                let w = self
                    .app
                    .handle()
                    .service(&SETTINGS_WINDOW)
                    .ok_or("no settings window")?;
                w.open(Some(SHORTCUTS));
            }
            Action::OpenWorkspace(None) => self.form(crate::dialogs::Form {
                title: "Open a workspace".into(),
                fields: vec![crate::dialogs::Field {
                    key: "path".into(),
                    descriptor: helios_values::Descriptor::path(
                        "directory",
                        helios_values::PathKind::Dir,
                        &[],
                    ),
                    value: helios_core::Value::Text(String::new()),
                }],
                verb: "Open".into(),
                command: "workspace.open".into(),
            }),
            Action::OpenWorkspace(Some(path)) => {
                // One window per workspace: another process opens it.
                std::env::current_exe()
                    .and_then(|exe| std::process::Command::new(exe).arg(&path).spawn())
                    .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
                self.note(format!("opening {}", path.display()));
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
                self.note(format!("theme exported to {}", path.display()));
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
                self.note(format!("profile exported to {}", path.display()));
            }
            Action::ProfileImport(path) => {
                let incoming =
                    Self::read_profile(&path).map_err(|e| format!("profile not imported: {e}"))?;
                let changes = helios_profile::diff(&self.profile(), &incoming);
                let count: usize = changes.iter().map(|d| d.changes.len()).sum();
                self.note(format!(
                    "profile {}: {count} changes; profile.import.apply to apply them",
                    incoming.name
                ));
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
                self.note(format!("profile {name} applied"));
            }
            Action::ProfileUse(name) => self
                .use_profile(&name)
                .map_err(|e| format!("profile {name}: {e}"))?,
            Action::ScopeMode(mode) => self.scopes.borrow_mut().set_mode(mode),
            Action::ScopeStops(on) => self.scopes.borrow_mut().set_stops(on),
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
            Action::Focus(p) => self.workbench.borrow_mut().focus(&p),
            Action::Toggle(p) => self.workbench.borrow_mut().toggle(&p),
            Action::Zoom(z) => self.zoom(z),
            Action::Copy(text) => {
                ctx.copy_text(text.clone());
                self.note(format!("copied {text}"));
            }
            Action::MoveTo(p, area) => self.workbench.borrow_mut().move_to(&p, area, None)?,
            Action::Answer(answer, origin) => self.answer(answer, &origin)?,
            Action::AssumeYes(on) => {
                self.assume_yes = on;
                self.note(if on {
                    "confirmations are answered yes by policy"
                } else {
                    "confirmations are asked again"
                });
            }
            Action::Detach(p) => {
                let was = self.rects.get(&p).copied();
                self.workbench.borrow_mut().detach(&p, was);
            }
            Action::Dock(p) => {
                let mut wb = self.workbench.borrow_mut();
                if wb.auxiliary.iter().any(|a| a.panel == p) {
                    wb.dock(&p);
                }
            }
            Action::Snapshot { target, path } if target == "viewport.scene" => {
                // The float data, before the display transform.
                self.viewport_scene()
                    .ok_or_else(|| "the viewport shows nothing yet".to_string())
                    .and_then(|scene| snapshot::save_scene(&scene, &path))
                    .map_err(|e| format!("viewport.scene: {e}"))?;
                self.note(format!("viewport.scene saved to {}", path.display()));
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
                    self.note(format!("recording saved to {}", p.display()));
                }
            }
        }
        Ok(Value::Null)
    }

    fn zoom(&mut self, z: Zoom) {
        use crate::viewport::{CENTER_X, CENTER_Y, FIT, ZOOM};
        use helios_core::Value as V;
        let current = self.viewport.borrow().effective_zoom;
        let mut store = lock(&self.values);
        let set = |store: &mut helios_values::Store, path: &str, v: V| {
            let _ = store.set(helios_values::Set::new(
                path,
                helios_values::Change::To(v),
                Origin::User,
            ));
        };
        let fitted = store
            .get(FIT)
            .and_then(|e| e.value.as_bool())
            .unwrap_or(true);
        match z {
            Zoom::Fit => set(&mut store, FIT, V::Bool(true)),
            Zoom::Actual | Zoom::By(_) => {
                if fitted {
                    // Leaving fit keeps the image where it was.
                    let extent = self.viewport.borrow().graph_extent();
                    if let Some((w, h)) = extent {
                        set(&mut store, CENTER_X, V::Float(w / 2.0));
                        set(&mut store, CENTER_Y, V::Float(h / 2.0));
                    }
                }
                let zoom = match z {
                    Zoom::Actual => 1.0 / f64::from(self.pixels_per_point),
                    Zoom::By(f) => current * f,
                    Zoom::Fit => unreachable!(),
                };
                set(&mut store, ZOOM, V::Float(zoom));
                set(&mut store, FIT, V::Bool(false));
            }
        }
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

    fn on_screenshot(&mut self, image: &egui::ColorImage) {
        let full = snapshot::from_color_image(image);
        let Some(shot) = self.shots.pop_front() else {
            return;
        };
        match shot {
            Shot::Save { target, path } => {
                let note = self
                    .target_rect(&target)
                    .map(|r| snapshot::crop(&full, r, self.pixels_per_point))
                    .and_then(|img| snapshot::save(&img, &path))
                    .map_or_else(
                        |e| format!("snapshot failed: {e}"),
                        |()| format!("{target} pictured to {}", path.display()),
                    );
                self.note(note);
            }
            Shot::Frame => {
                if let Some((target, rec)) = &mut self.recording {
                    let pushed = self
                        .rects
                        .get(target.as_str())
                        .ok_or_else(|| format!("nothing named `{target}` is shown"))
                        .map(|r| snapshot::crop(&full, *r, self.pixels_per_point))
                        .and_then(|img| rec.push(&img));
                    if let Err(e) = pushed {
                        self.warn(format!("recording stopped: {e}"));
                        self.recording = None;
                    }
                }
            }
        }
    }

    // --- Drawing ---

    /// Draws the shell for one frame.
    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let was_hidpi = self.pixels_per_point >= HIDPI;
        self.pixels_per_point = ctx.pixels_per_point();
        // The screen is known from the first frame on: a profile applied
        // before, with a `hidpi` variant, is applied again.
        if was_hidpi != (self.pixels_per_point >= HIDPI)
            && let Some((source, _)) = &self.loaded
            && source.on.contains_key(&helios_profile::Selector::Hidpi)
        {
            let source = source.clone();
            self.apply_profile(&source);
        }
        self.now = ctx.input(|i| i.time);
        if self.framed {
            crate::frame::resize_handles(&ctx);
        }
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
        self.app.logic(&ctx);
        self.apply_queue(&ctx);
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
            self.installed = Some(spec);
        }
        self.set_context(&ctx);

        self.rects.clear();
        ctx.data_mut(|d| d.remove::<BTreeMap<String, Rect>>(Id::new("helios.named")));
        self.rects.insert("window".into(), ctx.content_rect());

        let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        if self.framed && !fullscreen {
            self.title_bar(ui);
        }
        self.status_bar(ui);
        self.viewport.borrow_mut().sync(&mut lock(&self.values));
        self.app.show(ui);
        let warnings = std::mem::take(&mut self.workbench.borrow_mut().warnings);
        for w in warnings {
            self.warn(w);
        }
        self.workbench.borrow_mut().windows(&ctx, self.framed);
        self.menus_ui(&ctx);

        let named = ctx.data(|d| d.get_temp::<BTreeMap<String, Rect>>(Id::new("helios.named")));
        self.rects.extend(named.unwrap_or_default());
        self.overlay_rects();

        if self.recording.is_some() {
            self.shots.push_back(Shot::Frame);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            ctx.request_repaint();
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

    fn title_bar(&mut self, ui: &mut Ui) {
        let request = egui::Panel::top("helios.titlebar")
            .exact_size(crate::frame::TITLE_HEIGHT)
            .frame(egui::Frame::NONE.fill(ui.visuals().panel_fill))
            .show(ui, |ui| crate::frame::title_bar(ui, &self.title))
            .inner;
        let command = match request {
            Some(crate::frame::Request::Minimize) => "window.minimize",
            Some(crate::frame::Request::ToggleMaximize) => "window.maximize",
            Some(crate::frame::Request::Close) => "window.close",
            Some(crate::frame::Request::SystemMenu(_)) => {
                let named = ui
                    .ctx()
                    .data(|d| d.get_temp::<BTreeMap<String, Rect>>(Id::new("helios.named")));
                let logo = named.and_then(|n| n.get("helios.logo").copied());
                if let Some(logo) = logo {
                    self.rects.insert("helios.logo".into(), logo);
                    self.open_system_menu(logo);
                }
                return;
            }
            None => return,
        };
        let _ = self.run(command, Value::Null, Origin::User);
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let pending = self.app.handle().pending_keys();
        egui::Panel::bottom("helios.status")
            .exact_size(22.0)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if let Some(keys) = &pending {
                        ui.label(format!("{keys} …"));
                        ui.separator();
                    }
                    if self.recording.is_some() {
                        ui.colored_label(ui.visuals().error_fg_color, "● REC");
                        ui.separator();
                    }
                    if let Some(note) = self.notes.last() {
                        ui.label(note);
                    }
                });
            });
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
                let mut button = egui::Button::new(menu_title(&spec.title));
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
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("helios")
}

/// Headless rendering, for tests, agents and machines without a display.
pub mod headless {
    use std::sync::{Arc, OnceLock};

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
    pub fn harness<'a>(mut shell: Shell, size: egui::Vec2) -> Harness<'a, Shell> {
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
        Harness::builder()
            .with_size(size)
            .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs))
            .build_ui_state(|ui, shell: &mut Shell| shell.ui(ui), shell)
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
