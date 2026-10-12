//! The desktop shell drawn with egui: areas docked from the workspace's
//! layout, detached panels in their own windows, the command palette, keys
//! resolved by the keymap, and pictures of any named part of it.
//!
//! Everything the user can do goes through a command of the registry, so
//! a key, the palette, a script and an agent reach the same actions.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{Align2, Context as Egui, Event, Id, Key, Rect, Ui, ViewportBuilder, ViewportId};
use egui_dock::{DockArea, DockState, TabViewer};
use helios_shell::commands::{CommandSpec, Origin, Registry};
use helios_shell::keymap::{Chord, Keymap, Keys, Layer, Resolution};
use helios_shell::layout::{Auxiliary, Layout, Node};
use helios_shell::panels::{Area, PanelSpec, Panels};
use helios_shell::when::Context;
use image::RgbaImage;
use serde_json::{Value, json};

use crate::dock::{from_dock, to_dock};
use crate::keys::chord;
use crate::snapshot::{self, Recorder};

/// The screen auxiliary windows are said to be on, until egui names
/// monitors.
const SCREEN: &str = "primary";

/// Pixels per point from which a screen is `hidpi` for profile variants.
const HIDPI: f32 = 1.5;

/// What commands ask the shell to do; they are queued and applied at the
/// next frame, where the shell's state is at hand.
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
    RunCommand(String),
    About,
    Keybindings,
    OpenWorkspace(Option<PathBuf>),
    Answer(crate::dialogs::Answer, Origin),
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

type Queue = Arc<Mutex<Vec<Action>>>;

/// What the viewport's zoom commands ask for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Zoom {
    Fit,
    Actual,
    By(f64),
}

/// How a command turns its arguments into an action.
type ActionOf = Box<dyn Fn(&Value) -> Result<Action, String> + Send + Sync>;

/// A picture asked of the window, waiting for egui's screenshot.
enum Shot {
    Save { target: String, path: PathBuf },
    Frame,
}

struct Palette {
    query: String,
    selected: usize,
    opened: bool,
}

/// Draws a panel's content.
pub type Content = Box<dyn FnMut(&mut Ui)>;

/// Names a widget's rectangle for this frame, so it can be pictured by
/// that name (`gui.snapshot {target: name}`).
pub fn name_rect(ctx: &Egui, name: &str, rect: Rect) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<BTreeMap<String, Rect>>(Id::new("helios.named"))
            .insert(name.to_string(), rect);
    });
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent switches (framed, pending theme, auto-answer, still), not states of one machine"
)]
pub struct Shell {
    pub registry: Registry,
    pub keymap: Keymap,
    panels: Panels,
    docks: Vec<(Area, DockState<String>)>,
    auxiliary: Vec<Auxiliary>,
    /// Panels the user closed: a layout keeps them closed.
    hidden: Vec<String>,
    queue: Queue,
    palette: Option<Palette>,
    pending: Vec<Chord>,
    focused: Option<String>,
    cx: Context,
    rects: BTreeMap<String, Rect>,
    /// Where each area was drawn last frame (the editors always are).
    area_rects: BTreeMap<Area, Rect>,
    /// The window's content, where hidden areas offer a drop zone.
    content_rect: Rect,
    /// The panel whose tab is being dragged, the area it comes from and
    /// where the pointer was last seen.
    dragging: Option<(String, Area, egui::Pos2)>,
    pixels_per_point: f32,
    contents: BTreeMap<String, Content>,
    shots: VecDeque<Shot>,
    recording: Option<(String, Recorder)>,
    /// What the shell has to say: command results, layout repairs.
    pub notes: Vec<String>,
    title: String,
    /// The window's frame is drawn by Helios (see `frame`).
    framed: bool,
    themes: crate::theme::Themes,
    theme: crate::theme::Tokens,
    theme_overrides: BTreeMap<String, serde_json::Value>,
    /// The theme changed and egui has not been told yet.
    theme_pending: bool,
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
    scopes: std::rc::Rc<std::cell::RefCell<crate::scopes::Scopes>>,
    library: std::rc::Rc<std::cell::RefCell<crate::library::Library>>,
    /// Dialogs waiting to be answered, the first one shown.
    dialogs: VecDeque<crate::dialogs::Open>,
    /// What `dialog.current` answers: the shown dialog, described.
    dialog_view: Arc<Mutex<Option<Value>>>,
    /// Confirmations are answered yes by policy (a script's `--yes`).
    assume_yes: bool,
    /// An open menu: where, its entries, the selected one.
    menu: Option<(egui::Pos2, MenuSource, usize)>,
    /// The menus contributed so far, by id (`system`).
    pub menus: helios_shell::menus::Menus,
    /// The theme in use, by name.
    theme_name: String,
    /// Every notice so far; the status bar shows the last, toasts the
    /// recent warnings and errors.
    notices: Vec<crate::dialogs::Notice>,
    dismissed: Vec<usize>,
    /// The GUI's clock, in seconds, as of this frame.
    now: f64,
    viewport: std::rc::Rc<std::cell::RefCell<crate::viewport::Viewport>>,
    params: std::rc::Rc<std::cell::RefCell<crate::params::Params>>,
    /// Every value widgets and agents edit (`param.*` commands).
    pub values: std::sync::Arc<std::sync::Mutex<helios_values::Store>>,
    /// Where path fields browse: the system's chooser or Helios's own
    /// picker, and the recent workspaces.
    places: crate::dialogs::Places,
    /// Headless runs: no cursor blink and no animation, so a frame settles
    /// and a test's `run` ends.
    still: bool,
}

impl Shell {
    /// A shell over declared panels, in a saved layout (repaired) or the
    /// default one.
    pub fn new(panels: Panels, layout: Option<Layout>) -> Shell {
        let (layout, notes) = match layout {
            Some(l) => l.repair(&panels, &[SCREEN]),
            None => (Layout::default_for(&panels), Vec::new()),
        };
        let mut shell = Shell {
            registry: Registry::new(),
            keymap: Keymap::new(),
            docks: Vec::new(),
            auxiliary: layout.auxiliary.clone(),
            hidden: layout.hidden.clone(),
            queue: Arc::default(),
            palette: None,
            pending: Vec::new(),
            focused: None,
            cx: Context::new(),
            rects: BTreeMap::new(),
            area_rects: BTreeMap::new(),
            content_rect: Rect::NOTHING,
            dragging: None,
            pixels_per_point: 1.0,
            contents: BTreeMap::new(),
            shots: VecDeque::new(),
            recording: None,
            notes,
            panels,
            title: String::new(),
            framed: false,
            themes: crate::theme::themes(),
            theme: crate::theme::resolve(
                &crate::theme::themes(),
                crate::theme::DEFAULT,
                &BTreeMap::new(),
            ),
            theme_overrides: BTreeMap::new(),
            theme_pending: true,
            profiles_dir: config_dir().join("profiles"),
            profile_name: "default".into(),
            pending_import: None,
            machine: helios_profile::Machine::here(&config_dir().join("machine.toml")),
            loaded: None,
            resolved_view: Arc::new(Mutex::new(Value::Null)),
            scopes: std::rc::Rc::default(),
            library: std::rc::Rc::default(),
            values: Arc::default(),
            viewport: std::rc::Rc::default(),
            params: std::rc::Rc::default(),
            dialogs: VecDeque::new(),
            dialog_view: Arc::default(),
            assume_yes: false,
            still: false,
            places: crate::dialogs::Places {
                native: std::env::var_os("HELIOS_NO_NATIVE_DIALOGS").is_none(),
                recent: recent_workspaces(),
            },
            menu: None,
            menus: helios_shell::menus::Menus::new(),
            theme_name: crate::theme::DEFAULT.into(),
            notices: Vec::new(),
            dismissed: Vec::new(),
            now: 0.0,
        };
        shell.docks = layout
            .areas
            .iter()
            .map(|(area, node)| (*area, to_dock(node)))
            .collect();
        shell.declare_commands();
        shell.declare_menus();
        helios_values::register(&mut shell.registry, shell.values.clone())
            .expect("param commands are declared once");
        {
            let mut store = shell.values.lock().expect("value store lock");
            crate::viewport::declare(&mut store);
            store.declare(
                crate::params::SELECTION,
                helios_values::Descriptor::text("selected node", ""),
            );
        }
        if shell.panels.get("helios.params").is_some() {
            let (params, values) = (shell.params.clone(), shell.values.clone());
            shell.set_content("helios.params", move |ui| {
                let selection = values
                    .lock()
                    .expect("value store lock")
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
        if shell.panels.get("helios.viewport").is_some() {
            let (viewport, values) = (shell.viewport.clone(), shell.values.clone());
            shell.set_content("helios.viewport", move |ui| {
                viewport.borrow_mut().ui(ui, &values);
            });
        }
        if shell.panels.get("helios.scopes").is_some() {
            let scopes = shell.scopes.clone();
            shell.set_content("helios.scopes", move |ui| scopes.borrow_mut().ui(ui));
        }
        for (panel, strip) in [("helios.vault", false), ("helios.filmstrip", true)] {
            if shell.panels.get(panel).is_some() {
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
            let mut store = self.values.lock().expect("value store lock");
            for id in graph.ids() {
                helios_values::bind_node(&mut store, &graph, id)?;
            }
            store.take_dirty();
        }
        self.viewport.borrow_mut().set_source(graph, node, port)
    }

    /// Gives the viewport the GPU objects it draws with, on the render
    /// state the window (or a test harness) draws with.
    pub fn attach_gpu(&mut self, rs: &eframe::egui_wgpu::RenderState) {
        crate::viewport::install(rs);
    }

    /// What the viewport's render thread did.
    pub fn viewport_stats(&self) -> std::sync::Arc<crate::viewport::Stats> {
        self.viewport.borrow().stats.clone()
    }

    /// The float data the viewport shows, before the display transform.
    pub fn viewport_scene(&self) -> Option<helios_core::Image> {
        self.viewport.borrow().scene().cloned()
    }

    /// The layout as it stands, to be saved with the workspace.
    pub fn layout(&self) -> Layout {
        Layout {
            areas: self
                .docks
                .iter()
                .filter_map(|(area, dock)| from_dock(dock).map(|n| (*area, n)))
                .collect(),
            auxiliary: self.auxiliary.clone(),
            hidden: self.hidden.clone(),
        }
    }

    /// Gives a panel its content; panels without one show a placeholder.
    pub fn set_content(&mut self, panel: &str, content: impl FnMut(&mut Ui) + 'static) {
        self.contents.insert(panel.into(), Box::new(content));
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
        self.theme_pending = true;
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

    /// What the shell holds as a profile: theme, user keys, layout.
    pub fn profile(&self) -> helios_profile::Profile {
        use helios_profile::{Binding, Profile, System, ThemeChoice, portable_keys};
        let keymap = self
            .keymap
            .bindings(Layer::User)
            .into_iter()
            .map(|b| Binding {
                keys: portable_keys(&b.keys.to_string().to_lowercase(), System::current()),
                command: b.command.clone(),
                when: b.when.clone(),
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
        *self.resolved_view.lock().expect("resolved view") = json!({
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
            self.keymap.clear(Layer::User);
            for b in keys {
                let local = local_keys(&b.keys, System::current());
                if let Err(e) =
                    self.keymap
                        .bind(&local, &b.command, &b.when, Layer::User, "profile")
                {
                    self.warn(format!("profile {}: {e}", p.name));
                }
            }
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
        let (l, notes) = layout.repair(&self.panels, &[SCREEN]);
        self.notes.extend(notes);
        self.docks = l.areas.iter().map(|(a, n)| (*a, to_dock(n))).collect();
        self.auxiliary = l.auxiliary;
        self.hidden = l.hidden;
    }

    /// Where profiles are read and written.
    pub fn set_profiles_dir(&mut self, dir: PathBuf) {
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

    /// Adds a theme (a user's file, a plugin's) to those that can be chosen.
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
    pub fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    pub fn palette_open(&self) -> bool {
        self.palette.is_some()
    }

    /// Where a target was drawn at the last frame: `window`, a panel id or
    /// a name given by [`name_rect`].
    pub fn target_rect(&self, target: &str) -> Result<Rect, String> {
        self.rects
            .get(target)
            .copied()
            .ok_or_else(|| format!("nothing named `{target}` is shown"))
    }

    pub fn pixels_per_point(&self) -> f32 {
        self.pixels_per_point
    }

    /// Runs a command as `origin`, in the context of the last frame.
    pub fn run(&mut self, id: &str, args: Value, origin: Origin) -> Result<Value, String> {
        let result = self.registry.run(id, args, origin, &self.cx);
        if let Err(e) = &result {
            self.warn(e.clone());
        }
        result
    }

    fn declare_commands(&mut self) {
        let view = self.dialog_view.clone();
        self.registry
            .register(
                CommandSpec::new(
                    "dialog.current",
                    "Describe the dialog waiting for an answer",
                    "Window",
                ),
                move |_, _| {
                    Ok(view
                        .lock()
                        .expect("dialog view")
                        .clone()
                        .unwrap_or(Value::Null))
                },
            )
            .expect("shell commands are declared once");
        let view = self.resolved_view.clone();
        self.registry
            .register(
                CommandSpec::new(
                    "profile.resolved",
                    "Describe the profile in use as resolved for this machine: which values came from which variant",
                    "Preferences",
                ),
                move |_, _| Ok(view.lock().expect("resolved view").clone()),
            )
            .expect("shell commands are declared once");
        let q = self.queue.clone();
        self.registry
            .register(
                CommandSpec::new(
                    "dialog.answer",
                    "Answer the dialog (`answer`: primary or cancel; `values` for a form; `choice` for a picker)",
                    "Window",
                ),
                move |args, origin| {
                    use crate::dialogs::Answer;
                    let answer = if let Some(c) = args.get("choice") {
                        Answer::Values(c.clone())
                    } else if let Some(v) = args.get("values") {
                        Answer::Values(v.clone())
                    } else {
                        match args.get("answer").and_then(Value::as_str) {
                            Some("primary") => Answer::Primary,
                            Some("cancel") => Answer::Cancel,
                            _ => return Err("`answer` is primary or cancel".into()),
                        }
                    };
                    q.lock()
                        .expect("queue")
                        .push(Action::Answer(answer, origin.clone()));
                    Ok(json!({"queued": true}))
                },
            )
            .expect("shell commands are declared once");
        let q = self.queue.clone();
        let push = move |a: Action| q.lock().expect("queue").push(a);
        let mut add = |spec: CommandSpec, action: ActionOf| {
            let push = push.clone();
            self.registry
                .register(spec, move |args, _| {
                    push(action(args)?);
                    Ok(json!({"queued": true}))
                })
                .expect("shell commands are declared once");
        };
        add(
            CommandSpec::new(
                "dialog.assumeYes",
                "Answer confirmations yes by policy (`on`)",
                "Window",
            ),
            Box::new(|a| {
                Ok(Action::AssumeYes(
                    a.get("on").and_then(Value::as_bool).unwrap_or(true),
                ))
            }),
        );
        add(
            CommandSpec::new("workbench.systemMenu", "Show the Helios menu", "Window"),
            Box::new(|_| Ok(Action::SystemMenu)),
        );
        add(
            CommandSpec::new("workbench.run", "Run a command by id (`command`)", "Window"),
            Box::new(|a| {
                a.get("command")
                    .and_then(Value::as_str)
                    .map(|c| Action::RunCommand(c.into()))
                    .ok_or_else(|| "`command` is missing".to_string())
            }),
        );
        add(
            CommandSpec::new("workbench.about", "About Helios", "Help"),
            Box::new(|_| Ok(Action::About)),
        );
        add(
            CommandSpec::new("workbench.keybindings", "Keyboard shortcuts", "Preferences"),
            Box::new(|_| Ok(Action::Keybindings)),
        );
        add(
            CommandSpec::new("workbench.quit", "Quit Helios", "Window"),
            Box::new(|_| Ok(Action::Close)),
        );
        add(
            CommandSpec::new(
                "workspace.open",
                "Open a workspace in a new window (`path`)",
                "Workspace",
            ),
            Box::new(|a| {
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
            Box::new(|_| Ok(Action::Palette)),
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
            Box::new(move |a| {
                let target = string(a, "target").unwrap_or_else(|_| "window".into());
                let path =
                    string(a, "path").map_or_else(|_| default_path(&target, "png"), Into::into);
                Ok(Action::Snapshot { target, path })
            }),
        );
        add(
            CommandSpec::new("gui.record.start", "Start recording a video", "Developer"),
            Box::new(move |a| {
                let target = string(a, "target").unwrap_or_else(|_| "window".into());
                let path =
                    string(a, "path").map_or_else(|_| default_path(&target, "mp4"), Into::into);
                let fps = a.get("fps").and_then(Value::as_u64).unwrap_or(30) as u32;
                Ok(Action::RecordStart { target, path, fps })
            }),
        );
        add(
            CommandSpec::new("gui.record.stop", "Stop recording", "Developer"),
            Box::new(|_| Ok(Action::RecordStop)),
        );
        let specs: Vec<PanelSpec> = self.panels.all().to_vec();
        for p in specs {
            let id = p.id.clone();
            let (focus, toggle) = (id.clone(), id.clone());
            add(
                CommandSpec::new(
                    &format!("{id}.focus"),
                    &format!("Focus {}", p.title),
                    "View",
                ),
                Box::new(move |_| Ok(Action::Focus(focus.clone()))),
            );
            add(
                CommandSpec::new(
                    &format!("{id}.toggle"),
                    &format!("Show or hide {}", p.title),
                    "View",
                ),
                Box::new(move |_| Ok(Action::Toggle(toggle.clone()))),
            );
            let move_to = id.clone();
            add(
                CommandSpec::new(
                    &format!("{id}.moveTo"),
                    &format!("Move {} to an area (`area`)", p.title),
                    "View",
                ),
                Box::new(move |a| {
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
                Box::new(move |_| Ok(Action::Detach(detach.clone()))),
            );
            add(
                CommandSpec::new(
                    &format!("{id}.dock"),
                    &format!("Dock {} back in its area", p.title),
                    "View",
                ),
                Box::new(move |_| Ok(Action::Dock(id.clone()))),
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
                Box::new(move |_| Ok(action())),
            );
        }
        let names = crate::theme::choosable(&self.themes);
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
                Box::new(move |_| Ok(Action::Theme(picked.clone()))),
            );
        }
        add(
            CommandSpec::new("theme.select", "Use a theme by name", "Preferences"),
            Box::new(move |a| {
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
            Box::new(move |a| {
                // By default next to the user's settings, where themes are read.
                let path = a.get("path").and_then(Value::as_str).map_or_else(
                    || {
                        let config = std::env::var_os("XDG_CONFIG_HOME")
                            .map(PathBuf::from)
                            .or_else(|| {
                                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
                            })
                            .unwrap_or_else(|| PathBuf::from("."));
                        config.join("helios/themes/exported.tokens.json")
                    },
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
                Box::new(move |_| Ok(Action::ScopeMode(mode))),
            );
        }
        add(
            CommandSpec::new(
                "vault.rate",
                "Rate the selected image (`stars`, 0 to 5)",
                "Vault",
            ),
            Box::new(|a| {
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
                Box::new(move |_| Ok(Action::Rate(n))),
            );
        }
        add(
            CommandSpec::new(
                "vault.reject",
                "Reject the selected image, or take it back",
                "Vault",
            ),
            Box::new(|_| Ok(Action::Reject)),
        );
        for label in crate::library::LABELS {
            let title = format!("Toggle the {label} label on the selected image");
            add(
                CommandSpec::new(&format!("vault.label.{label}"), &title, "Vault"),
                Box::new(move |_| Ok(Action::Label(label.into()))),
            );
        }
        add(
            CommandSpec::new("vault.next", "Select the next image", "Vault"),
            Box::new(|_| Ok(Action::Step(1))),
        );
        add(
            CommandSpec::new("vault.previous", "Select the previous image", "Vault"),
            Box::new(|_| Ok(Action::Step(-1))),
        );
        for (id, title, value) in [
            ("param.copyPath", "Copy a value's path (`path`)", false),
            ("param.copyValue", "Copy a value (`path`)", true),
        ] {
            let values = self.values.clone();
            add(
                CommandSpec::new(id, title, "Values"),
                Box::new(move |a| {
                    let path = string(a, "path")?;
                    if !value {
                        return Ok(Action::Copy(path));
                    }
                    let store = values.lock().expect("value store lock");
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
                Box::new(move |_| Ok(Action::Zoom(zoom))),
            );
        }
        add(
            CommandSpec::new("scopes.stops", "Scopes: in stops (log)", "View"),
            Box::new(|_| Ok(Action::ScopeStops(true))),
        );
        add(
            CommandSpec::new("scopes.linear", "Scopes: linear", "View"),
            Box::new(|_| Ok(Action::ScopeStops(false))),
        );
        add(
            CommandSpec::new(
                "profile.export",
                "Export the profile (all parts, or `parts`; `this_machine` flattens its variants)",
                "Preferences",
            ),
            Box::new(move |a| {
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
            Box::new(move |a| {
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
            Box::new(|_| Ok(Action::ProfileApply)),
        );
        add(
            CommandSpec::new("profile.use", "Use a saved profile by name", "Preferences"),
            Box::new(move |a| {
                a.get("name")
                    .and_then(Value::as_str)
                    .map(|n| Action::ProfileUse(n.to_string()))
                    .ok_or_else(|| "`name` is missing".to_string())
            }),
        );
        for (keys, command) in [
            ("alt+space", "workbench.systemMenu"),
            ("ctrl+shift+p", "workbench.palette"),
            ("f1", "workbench.palette"),
            ("f11", "workbench.fullscreen"),
        ] {
            self.keymap
                .bind(keys, command, "", Layer::Default, "shell")
                .expect("shell keys parse");
        }
        // As in darktable's lighttable, while the library has the focus.
        let library = "(focusedPanel == 'helios.vault' || focusedPanel == 'helios.filmstrip') && !textInputFocus";
        let mut keys: Vec<(String, String)> = (0..=5)
            .map(|n| (n.to_string(), format!("vault.rate.{n}")))
            .collect();
        keys.push(("r".into(), "vault.reject".into()));
        keys.push(("right".into(), "vault.next".into()));
        keys.push(("left".into(), "vault.previous".into()));
        for (k, command) in keys {
            self.keymap
                .bind(&k, &command, library, Layer::Default, "shell")
                .expect("library keys parse");
        }
    }

    fn context(&self, ctx: &Egui) -> Context {
        let mut cx = Context::new()
            .flag("textInputFocus", ctx.egui_wants_keyboard_input())
            .flag("paletteVisible", self.palette.is_some())
            .flag(
                "fullScreen",
                ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)),
            );
        if let Some(p) = &self.focused {
            cx.set("focusedPanel", p);
        }
        cx
    }

    // --- Layout changes ---

    fn shown(&self, panel: &str) -> bool {
        self.docks
            .iter()
            .any(|(_, d)| d.find_tab(&panel.to_string()).is_some())
            || self.auxiliary.iter().any(|a| a.panel == panel)
    }

    fn undock(&mut self, panel: &str) {
        for (_, dock) in &mut self.docks {
            if let Some(path) = dock.find_tab(&panel.to_string()) {
                dock.remove_tab(path);
            }
        }
        self.docks.retain(|(_, d)| from_dock(d).is_some());
        if self.focused.as_deref() == Some(panel) {
            self.focused = None;
        }
    }

    /// Puts a panel back in its default area.
    fn dock(&mut self, panel: &str) {
        let Some(area) = self.panels.get(panel).map(|p| p.area) else {
            return;
        };
        self.auxiliary.retain(|a| a.panel != panel);
        self.hidden.retain(|h| h != panel);
        if let Some((_, dock)) = self.docks.iter_mut().find(|(a, _)| *a == area) {
            dock.push_to_first_leaf(panel.to_string());
        } else {
            self.docks.push((
                area,
                to_dock(&Node::Tabs {
                    panels: vec![panel.to_string()],
                    active: 0,
                }),
            ));
            self.docks.sort_by_key(|(a, _)| *a);
        }
    }

    // --- Dialogs, menus and notices (see `dialogs`) ---

    /// Shows a notice: the status bar always, a toast unless it is plain
    /// information. Kept in `notes` too.
    pub fn notify(&mut self, level: crate::dialogs::Level, text: impl Into<String>) {
        let text = text.into();
        self.notes.push(text.clone());
        self.notices.push(crate::dialogs::Notice {
            level,
            text,
            action: None,
            at: self.now,
        });
    }

    fn note(&mut self, text: impl Into<String>) {
        self.notify(crate::dialogs::Level::Info, text);
    }

    fn warn(&mut self, text: impl Into<String>) {
        self.notify(crate::dialogs::Level::Warning, text);
    }

    fn open(&mut self, dialog: crate::dialogs::Dialog) {
        self.dialogs.push_back(crate::dialogs::Open::new(dialog));
        self.refresh_dialog_view();
    }

    /// Asks before running a command. Under the yes policy it runs at once,
    /// and says so.
    pub fn confirm(&mut self, c: crate::dialogs::Confirm) {
        if self.assume_yes {
            let text = format!("{}: confirmed by policy", c.verb);
            self.notify(crate::dialogs::Level::Warning, text);
            let _ = self.run(&c.command, c.args, Origin::Script("policy".into()));
            return;
        }
        self.open(crate::dialogs::Dialog::Confirm(c));
    }

    /// Where path fields browse. `headless::harness` turns the system's
    /// chooser off.
    pub fn set_places(&mut self, places: crate::dialogs::Places) {
        self.places = places;
    }

    /// Asks for values, then runs the form's command with them.
    pub fn form(&mut self, f: crate::dialogs::Form) {
        self.open(crate::dialogs::Dialog::Form(f));
    }

    /// Asks for a choice, then runs the picker's command with it.
    pub fn pick(&mut self, p: crate::dialogs::Picker) {
        self.open(crate::dialogs::Dialog::Picker(p));
    }

    /// Opens a menu of commands at a position.
    pub fn open_menu(&mut self, at: egui::Pos2, items: Vec<crate::dialogs::MenuItem>) {
        self.menu = Some((at, MenuSource::Items(items), 0));
    }

    /// Opens a declared menu (see `helios_shell::menus`) at a position.
    pub fn open_declared_menu(&mut self, at: egui::Pos2, menu: &str) {
        self.menu = Some((at, MenuSource::Declared(menu.into()), 0));
    }

    /// A declared menu as data, as it would open now: each entry's
    /// command, title, keys, whether it applies and whether it is ticked;
    /// submenus nested.
    pub fn menu_items(&self, menu: &str) -> Value {
        fn rows(list: &[crate::dialogs::Row]) -> Value {
            use crate::dialogs::Row;
            Value::Array(
                list.iter()
                    .map(|r| match r {
                        Row::Separator => json!({ "kind": "separator" }),
                        Row::Submenu(label, sub) => {
                            json!({ "kind": "submenu", "label": label, "items": rows(sub) })
                        }
                        Row::Entry(e) => json!({
                            "kind": "command",
                            "id": e.id,
                            "args": e.args,
                            "title": e.title,
                            "keys": e.keys,
                            "enabled": e.disabled.is_none(),
                            "checked": e.checked,
                        }),
                    })
                    .collect(),
            )
        }
        rows(&self.declared_rows(menu))
    }

    /// A declared menu's rows in the current context.
    fn declared_rows(&self, menu: &str) -> Vec<crate::dialogs::Row> {
        use crate::dialogs::{Entry, Row};
        use helios_shell::menus::{Item, Shown};
        let mut rows = Vec::new();
        for shown in self.menus.show(menu, &self.cx) {
            match shown {
                Shown::Separator => rows.push(Row::Separator),
                Shown::Item {
                    item: Item::Command { id, args },
                    enabled,
                    checked,
                } => {
                    let Some(spec) = self.registry.spec(&id) else {
                        continue;
                    };
                    rows.push(Row::Entry(Entry {
                        title: menu_title(&spec.title),
                        keys: self
                            .keymap
                            .keys_for(&id)
                            .first()
                            .map(std::string::ToString::to_string),
                        disabled: (!enabled).then(|| format!("Not available here ({})", spec.when)),
                        checked,
                        id,
                        args,
                    }));
                }
                Shown::Item {
                    item: Item::Submenu { menu, label },
                    ..
                } => rows.push(Row::Submenu(label, self.declared_rows(&menu))),
                Shown::Item {
                    item: Item::List { name },
                    ..
                } => rows.extend(self.list_rows(&name)),
            }
        }
        rows
    }

    /// The entries of a list the shell fills when a menu opens.
    fn list_rows(&self, name: &str) -> Vec<crate::dialogs::Row> {
        use crate::dialogs::{Entry, Row};
        let entry = |id: &str, args: Value, title: String, checked: bool| {
            Row::Entry(Entry {
                id: id.into(),
                args,
                title,
                keys: None,
                disabled: None,
                checked,
            })
        };
        match name {
            "themes" => crate::theme::choosable(&self.themes)
                .into_iter()
                .map(|t| {
                    let checked = t == self.theme_name;
                    entry("theme.select", json!({ "name": t }), t, checked)
                })
                .collect(),
            "profiles" => {
                let mut names: Vec<String> = std::fs::read_dir(&self.profiles_dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| {
                        e.file_name()
                            .to_str()?
                            .strip_suffix(".helios-profile")
                            .map(String::from)
                    })
                    .collect();
                if !names.contains(&"default".to_string()) {
                    names.push("default".into());
                }
                names.sort();
                names
                    .into_iter()
                    .map(|n| {
                        let checked = n == self.profile_name;
                        entry("profile.use", json!({ "name": n }), n, checked)
                    })
                    .collect()
            }
            "recent" => {
                let recent = recent_workspaces();
                if recent.is_empty() {
                    return vec![Row::Entry(Entry {
                        id: "workspace.open".into(),
                        args: Value::Null,
                        title: "No recent workspace".into(),
                        keys: None,
                        disabled: Some("Nothing opened yet".into()),
                        checked: false,
                    })];
                }
                recent
                    .into_iter()
                    .map(|p| {
                        entry(
                            "workspace.open",
                            json!({ "path": p }),
                            p.display().to_string(),
                            false,
                        )
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// The menus Helios itself contributes.
    fn declare_menus(&mut self) {
        use helios_shell::menus::MenuItem as M;
        for item in [
            M::command("system", "1_workspace", 0, "workspace.open"),
            M::submenu(
                "system",
                "1_workspace",
                1,
                "system.recent",
                "Recent workspaces",
            ),
            M::submenu("system", "2_preferences", 0, "system.profiles", "Profiles"),
            M::submenu("system", "2_preferences", 1, "system.themes", "Themes"),
            M::command("system", "2_preferences", 2, "workbench.keybindings"),
            M::command("system", "4_about", 0, "workbench.about"),
            M::command("system", "9_quit", 0, "workbench.quit"),
            M::list("system.recent", "1", 0, "recent"),
            M::list("system.profiles", "1", 0, "profiles"),
            M::command("system.profiles", "2", 0, "profile.export"),
            M::command("system.profiles", "2", 1, "profile.import"),
            M::list("system.themes", "1", 0, "themes"),
            M::command("system.themes", "2", 0, "theme.export"),
        ] {
            self.menus
                .contribute(item)
                .expect("the shell's menus parse");
        }
    }

    /// The notices so far, oldest first.
    pub fn notices(&self) -> &[crate::dialogs::Notice] {
        &self.notices
    }

    fn refresh_dialog_view(&mut self) {
        *self.dialog_view.lock().expect("dialog view") =
            self.dialogs.front().map(crate::dialogs::Open::describe);
    }

    fn answer(&mut self, answer: crate::dialogs::Answer, origin: Origin) {
        let Some(open) = self.dialogs.front() else {
            self.warn("no dialog to answer");
            return;
        };
        match open.resolve(&answer) {
            Ok(run) => {
                self.dialogs.pop_front();
                self.refresh_dialog_view();
                if let Some((command, args)) = run
                    && let Err(e) = self.run(&command, args, origin)
                {
                    self.warn(format!("{command}: {e}"));
                }
            }
            Err(e) => self.warn(e),
        }
    }

    /// A menu's items resolved against the registry, the keymap and the
    /// context.
    fn menu_rows(&self, items: &[crate::dialogs::MenuItem]) -> Vec<crate::dialogs::Row> {
        use crate::dialogs::{Entry, MenuItem, Row};
        items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Separator => Some(Row::Separator),
                MenuItem::Submenu { label, items } => {
                    Some(Row::Submenu(label.clone(), self.menu_rows(items)))
                }
                MenuItem::Command { id, args } => {
                    let spec = self.registry.spec(id)?;
                    let applies = self
                        .registry
                        .available(&self.cx)
                        .iter()
                        .any(|s| s.id == *id);
                    Some(Row::Entry(Entry {
                        id: id.clone(),
                        args: args.clone(),
                        title: menu_title(&spec.title),
                        keys: self
                            .keymap
                            .keys_for(id)
                            .first()
                            .map(std::string::ToString::to_string),
                        disabled: (!applies).then(|| format!("Not available here ({})", spec.when)),
                        checked: false,
                    }))
                }
            })
            .collect()
    }

    /// Dialogs, menus and toasts, over everything else.
    fn overlays(&mut self, ctx: &Egui) {
        use crate::dialogs::{Request, take_requests};
        for r in take_requests(ctx) {
            match r {
                Request::Confirm(c) => self.confirm(c),
                Request::Form(f) => self.form(f),
                Request::Picker(p) => self.pick(p),
                Request::Menu { at, items } => self.open_menu(at, items),
                Request::Notify(level, text) => self.notify(level, text),
            }
        }
        let look = crate::dialogs::Look::of(&self.theme);
        if let Some(open) = self.dialogs.front_mut() {
            let answer = crate::dialogs::dialog_ui(ctx, &look, open, &self.places);
            self.refresh_dialog_view();
            if let Some(a) = answer {
                self.answer(a, Origin::User);
            }
        } else if let Some((at, source, mut selected)) = self.menu.take() {
            let rows = match &source {
                MenuSource::Items(items) => self.menu_rows(items),
                MenuSource::Declared(id) => self.declared_rows(id),
            };
            match crate::dialogs::menu_ui(ctx, at, &rows, &mut selected) {
                Some(Some((id, args))) => {
                    let _ = self.run(&id, args, Origin::User);
                }
                Some(None) => {}
                None => self.menu = Some((at, source, selected)),
            }
        }
        if let Some(command) =
            crate::dialogs::toasts_ui(ctx, &look, &self.notices, &mut self.dismissed)
        {
            let _ = self.run(&command, Value::Null, Origin::User);
        }
    }

    /// Moves a panel to an area, into the tabs under `at` when given,
    /// else the area's first tabs; refused when the panel may not be there.
    fn move_to(&mut self, panel: &str, area: Area, at: Option<egui::Pos2>) -> Result<(), String> {
        let spec = self
            .panels
            .get(panel)
            .ok_or_else(|| format!("no panel `{panel}`"))?;
        if !spec.allows(area) {
            return Err(format!("{} stays out of {}", spec.title, area.name()));
        }
        self.undock(panel);
        self.auxiliary.retain(|a| a.panel != panel);
        self.hidden.retain(|h| h != panel);
        let tab = panel.to_string();
        if let Some((_, dock)) = self.docks.iter_mut().find(|(a, _)| *a == area) {
            let leaf = at.and_then(|at| {
                dock.main_surface()
                    .iter()
                    .position(|n| matches!(n, egui_dock::Node::Leaf(l) if l.rect.contains(at)))
            });
            match leaf {
                Some(i) => {
                    let tree = dock.main_surface_mut();
                    let node = egui_dock::NodeIndex(i);
                    tree.set_focused_node(node);
                    tree.push_to_focused_leaf(tab);
                }
                None => dock.push_to_first_leaf(tab),
            }
        } else {
            self.docks.push((
                area,
                to_dock(&Node::Tabs {
                    panels: vec![tab],
                    active: 0,
                }),
            ));
            self.docks.sort_by_key(|(a, _)| *a);
        }
        self.focused = Some(panel.to_string());
        Ok(())
    }

    /// The area a drop at `at` goes to: the one drawn there, else the
    /// zone of a hidden side or bottom area along the window's edges.
    fn area_at(&self, at: egui::Pos2) -> Option<Area> {
        let shown = |a: Area| self.docks.iter().any(|(d, _)| *d == a);
        let edge = 48.0;
        let c = self.content_rect;
        if c.is_positive() {
            if !shown(Area::PrimarySideBar) && at.x < c.left() + edge {
                return Some(Area::PrimarySideBar);
            }
            if !shown(Area::SecondarySideBar) && at.x > c.right() - edge {
                return Some(Area::SecondarySideBar);
            }
            if !shown(Area::BottomPanel) && at.y > c.bottom() - edge {
                return Some(Area::BottomPanel);
            }
        }
        self.area_rects
            .iter()
            .filter(|(a, _)| **a != Area::Editors)
            .find(|(_, r)| r.contains(at))
            .map(|(a, _)| *a)
            .or_else(|| {
                self.area_rects
                    .get(&Area::Editors)
                    .filter(|r| r.contains(at))
                    .map(|_| Area::Editors)
            })
    }

    /// The tab egui is dragging, if it is one of the docks' tabs.
    fn dragged_tab(&self, ctx: &Egui) -> Option<(String, Area)> {
        let id = ctx.dragged_id()?;
        self.docks.iter().find_map(|(area, dock)| {
            let base = Id::new(format!("helios.area.{area:?}"));
            dock.iter_all_tabs().find_map(|(path, tab)| {
                // How egui_dock names a tab's drag (`show/leaf.rs`).
                let tab_id = base
                    .with((path.surface, "surface"))
                    .with((path.node_path().node, "node"))
                    .with((path.tab.0, "tab"));
                (tab_id == id).then(|| (tab.clone(), *area))
            })
        })
    }

    /// Follows a tab drag; when it is dropped over another area, `egui_dock`
    /// (which only sees its own area) leaves it where it was, and the shell
    /// moves it, or says why it may not go there.
    fn follow_drag(&mut self, ctx: &Egui) {
        let pos = ctx.input(|i| i.pointer.latest_pos());
        if let Some((panel, from)) = self.dragged_tab(ctx) {
            let at = pos.or(self.dragging.as_ref().map(|d| d.2));
            if let Some(at) = at {
                self.dragging = Some((panel, from, at));
            }
            return;
        }
        let Some((panel, from, last)) = self.dragging.take() else {
            return;
        };
        let at = pos.unwrap_or(last);
        let Some(to) = self.area_at(at).filter(|a| *a != from) else {
            return;
        };
        if let Err(e) = self.move_to(&panel, to, Some(at)) {
            self.warn(e);
        }
    }

    /// While a tab is dragged over another area, shows whether it may go
    /// there.
    fn drop_hint(&mut self, ctx: &Egui) {
        let Some((panel, from, at)) = self.dragging.clone() else {
            return;
        };
        let Some(to) = self.area_at(at).filter(|a| *a != from) else {
            return;
        };
        let Some(spec) = self.panels.get(&panel) else {
            return;
        };
        let c = self.content_rect;
        let edge = 48.0;
        let rect = self.area_rects.get(&to).copied().unwrap_or(match to {
            Area::PrimarySideBar => {
                Rect::from_min_max(c.min, egui::pos2(c.left() + edge, c.bottom()))
            }
            Area::SecondarySideBar => {
                Rect::from_min_max(egui::pos2(c.right() - edge, c.top()), c.max)
            }
            Area::BottomPanel => Rect::from_min_max(egui::pos2(c.left(), c.bottom() - edge), c.max),
            Area::Editors => c,
        });
        let visuals = ctx.global_style().visuals.clone();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            Id::new("helios.drop"),
        ));
        if spec.allows(to) {
            let fill = visuals.selection.bg_fill.gamma_multiply(0.25);
            painter.rect_filled(rect, 4.0, fill);
            painter.rect_stroke(
                rect,
                4.0,
                visuals.selection.stroke,
                egui::StrokeKind::Inside,
            );
        } else {
            let fill = visuals.error_fg_color.gamma_multiply(0.15);
            painter.rect_filled(rect, 4.0, fill);
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!("{} stays out of {}", spec.title, to.name()),
                egui::FontId::proportional(13.0),
                visuals.error_fg_color,
            );
        }
        name_rect(ctx, "helios.drop", rect);
    }

    fn focus(&mut self, panel: &str) {
        if !self.shown(panel) {
            self.dock(panel);
        }
        for (_, dock) in &mut self.docks {
            if let Some(path) = dock.find_tab(&panel.to_string()) {
                let _ = dock.set_active_tab(path);
                dock.set_focused_node_and_surface(path.node_path());
            }
        }
        self.focused = Some(panel.to_string());
    }

    fn apply(&mut self, ctx: &Egui, action: Action) {
        match action {
            Action::Palette => {
                self.palette = match self.palette {
                    Some(_) => None,
                    None => Some(Palette {
                        query: String::new(),
                        selected: 0,
                        opened: false,
                    }),
                }
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
                let at = self
                    .rects
                    .get("helios.logo")
                    .map_or(egui::pos2(8.0, crate::frame::TITLE_HEIGHT), |r| {
                        r.left_bottom()
                    });
                self.open_declared_menu(at, "system");
            }
            Action::RunCommand(id) => {
                let _ = self.run(&id, Value::Null, Origin::User);
            }
            Action::About => self.open(crate::dialogs::Dialog::Info(crate::dialogs::Info {
                title: format!("Helios {}", env!("CARGO_PKG_VERSION")),
                lines: vec![
                    "A node-based photo developer.".into(),
                    "Free software under the GNU General Public License, version 3 or later."
                        .into(),
                    "Source: https://github.com/quazardous/helios".into(),
                ],
            })),
            Action::Keybindings => {
                let mut choices: Vec<crate::dialogs::Choice> = self
                    .registry
                    .available(&self.cx)
                    .into_iter()
                    .filter_map(|spec| {
                        // Every key bound to it.
                        let keys: Vec<String> = self
                            .keymap
                            .keys_for(&spec.id)
                            .iter()
                            .map(std::string::ToString::to_string)
                            .collect();
                        if keys.is_empty() {
                            return None;
                        }
                        let keys = keys.join(" · ");
                        Some(crate::dialogs::Choice {
                            value: spec.id.clone(),
                            label: format!("{}: {}", spec.category, spec.title),
                            detail: Some(keys),
                        })
                    })
                    .collect();
                choices.sort_by(|a, b| a.label.cmp(&b.label));
                self.pick(crate::dialogs::Picker {
                    title: "Keyboard shortcuts".into(),
                    choices,
                    command: "workbench.run".into(),
                    arg: "command".into(),
                });
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
                let opened = std::env::current_exe()
                    .map_err(|e| e.to_string())
                    .and_then(|exe| {
                        std::process::Command::new(exe)
                            .arg(&path)
                            .spawn()
                            .map_err(|e| e.to_string())
                    });
                match opened {
                    Ok(_) => self.note(format!("opening {}", path.display())),
                    Err(e) => self.warn(format!("cannot open {}: {e}", path.display())),
                }
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
                let saved = path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| std::fs::write(&path, text));
                self.note(match saved {
                    Ok(()) => format!("theme exported to {}", path.display()),
                    Err(e) => format!("cannot export the theme to {}: {e}", path.display()),
                });
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
                let written = profile.export(&present, &|_, _| false).and_then(|text| {
                    path.parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|()| std::fs::write(&path, text))
                        .map_err(|e| format!("{}: {e}", path.display()))
                });
                self.note(match written {
                    Ok(()) => format!("profile exported to {}", path.display()),
                    Err(e) => format!("profile not exported: {e}"),
                });
            }
            Action::ProfileImport(path) => match Self::read_profile(&path) {
                Ok(incoming) => {
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
                Err(e) => self.warn(format!("profile not imported: {e}")),
            },
            Action::ProfileApply => match self.pending_import.take() {
                Some(p) => {
                    let name = p.name.clone();
                    self.apply_profile(&p);
                    self.note(format!("profile {name} applied"));
                }
                None => self
                    .notes
                    .push("no profile read to apply: profile.import first".into()),
            },
            Action::ProfileUse(name) => {
                if let Err(e) = self.use_profile(&name) {
                    self.warn(format!("profile {name}: {e}"));
                }
            }
            Action::ScopeMode(mode) => self.scopes.borrow_mut().set_mode(mode),
            Action::ScopeStops(on) => self.scopes.borrow_mut().set_stops(on),
            Action::Rate(n) => {
                let result = self.library.borrow_mut().rate(n);
                if let Err(e) = result {
                    self.warn(format!("vault.rate: {e}"));
                }
            }
            Action::Reject => {
                let result = self.library.borrow_mut().toggle_reject();
                if let Err(e) = result {
                    self.warn(format!("vault.reject: {e}"));
                }
            }
            Action::Label(l) => {
                let result = self.library.borrow_mut().toggle_label(&l);
                if let Err(e) = result {
                    self.warn(format!("vault.label: {e}"));
                }
            }
            Action::Step(n) => self.library.borrow_mut().step(n),
            Action::Focus(p) => self.focus(&p),
            Action::Toggle(p) => {
                if self.shown(&p) {
                    self.undock(&p);
                    self.auxiliary.retain(|a| a.panel != p);
                    self.hidden.push(p);
                } else {
                    self.dock(&p);
                }
            }
            Action::Zoom(z) => {
                use crate::viewport::{CENTER_X, CENTER_Y, FIT, ZOOM};
                use helios_core::Value as V;
                let current = self.viewport.borrow().effective_zoom;
                let mut store = self.values.lock().expect("value store lock");
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
            Action::Copy(text) => {
                ctx.copy_text(text.clone());
                self.note(format!("copied {text}"));
            }
            Action::MoveTo(p, area) => {
                if let Err(e) = self.move_to(&p, area, None) {
                    self.warn(e);
                }
            }
            Action::Answer(answer, origin) => self.answer(answer, origin),
            Action::AssumeYes(on) => {
                self.assume_yes = on;
                self.note(if on {
                    "confirmations are answered yes by policy"
                } else {
                    "confirmations are asked again"
                });
            }
            Action::Detach(p) => {
                if !self.auxiliary.iter().any(|a| a.panel == p) {
                    let r = self.rects.get(&p).copied().unwrap_or(Rect::from_min_size(
                        egui::pos2(80.0, 80.0),
                        egui::vec2(480.0, 360.0),
                    ));
                    self.undock(&p);
                    self.auxiliary.push(Auxiliary {
                        panel: p,
                        screen: SCREEN.into(),
                        rect: [r.min.x, r.min.y, r.width(), r.height()],
                    });
                }
            }
            Action::Dock(p) => {
                if self.auxiliary.iter().any(|a| a.panel == p) {
                    self.dock(&p);
                }
            }
            Action::Snapshot { target, path } if target == "viewport.scene" => {
                // The float data, before the display transform.
                let saved = self
                    .viewport_scene()
                    .ok_or_else(|| "the viewport shows nothing yet".to_string())
                    .and_then(|scene| snapshot::save_scene(&scene, &path));
                match saved {
                    Ok(()) => self.note(format!("viewport.scene saved to {}", path.display())),
                    Err(e) => self.warn(format!("viewport.scene: {e}")),
                }
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
                    let note = match rec.finish() {
                        Ok(p) => format!("recording saved to {}", p.display()),
                        Err(e) => format!("recording failed: {e}"),
                    };
                    self.note(note);
                }
            }
        }
    }

    // --- Input ---

    fn on_key(&mut self, key: Key, modifiers: egui::Modifiers) {
        let c = chord(key, modifiers);
        let mut keys = self.pending.clone();
        keys.push(c.clone());
        match self.keymap.resolve(&Keys(keys.clone()), &self.cx) {
            Resolution::Run(command) => {
                self.pending.clear();
                let _ = self.run(&command, Value::Null, Origin::User);
            }
            Resolution::Pending => self.pending = keys,
            Resolution::Nothing => {
                // A chord that breaks a sequence may start another one.
                let was_pending = !self.pending.is_empty();
                self.pending.clear();
                if was_pending {
                    self.on_key(key, modifiers);
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
        self.cx = self.context(&ctx);
        if self.framed {
            crate::frame::resize_handles(&ctx);
        }

        for event in ctx.input(|i| i.events.clone()) {
            match event {
                Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if self.dialogs.is_empty()
                    && self.menu.is_none()
                    && (self.palette.is_none() || key == Key::F1 || modifiers.ctrl) =>
                {
                    self.on_key(key, modifiers);
                }
                Event::Screenshot { image, .. } => self.on_screenshot(&image),
                _ => {}
            }
        }
        // An action may run commands that queue more (a dialog's answer):
        // they apply in the same frame, a bounded number of rounds.
        for _ in 0..8 {
            let actions: Vec<Action> = std::mem::take(&mut *self.queue.lock().expect("queue"));
            if actions.is_empty() {
                break;
            }
            for a in actions {
                self.apply(&ctx, a);
            }
        }
        if self.theme_pending {
            egui_pop::theme::install_tokens(&ctx, self.theme.clone());
            if self.still {
                ctx.all_styles_mut(|style| {
                    style.visuals.text_cursor.blink = false;
                    style.animation_time = 0.0;
                });
            }
            self.theme_pending = false;
        }

        self.rects.clear();
        ctx.data_mut(|d| d.remove::<BTreeMap<String, Rect>>(Id::new("helios.named")));
        self.rects.insert("window".into(), ctx.content_rect());

        let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        if self.framed && !fullscreen {
            self.title_bar(ui);
        }
        self.status_bar(ui);
        self.viewport
            .borrow_mut()
            .sync(&mut self.values.lock().expect("value store lock"));
        self.areas(ui);
        self.windows(&ctx);
        self.palette_ui(&ctx);
        self.overlays(&ctx);

        let named = ctx.data(|d| d.get_temp::<BTreeMap<String, Rect>>(Id::new("helios.named")));
        self.rects.extend(named.unwrap_or_default());

        if self.recording.is_some() {
            self.shots.push_back(Shot::Frame);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            ctx.request_repaint();
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
            Some(crate::frame::Request::SystemMenu(at)) => {
                self.open_declared_menu(at, "system");
                return;
            }
            None => return,
        };
        let _ = self.run(command, Value::Null, Origin::User);
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        egui::Panel::bottom("helios.status")
            .exact_size(22.0)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if !self.pending.is_empty() {
                        ui.label(format!("{} …", Keys(self.pending.clone())));
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

    fn areas(&mut self, ui: &mut Ui) {
        let Shell {
            docks,
            panels,
            contents,
            rects,
            focused,
            area_rects,
            content_rect,
            ..
        } = self;
        *content_rect = ui.max_rect();
        area_rects.clear();
        let mut viewer = Viewer {
            panels,
            contents,
            rects,
            focused,
        };
        // Side areas first, the editors take what is left.
        let mut editors = None;
        for (area, dock) in docks.iter_mut() {
            let id = format!("helios.area.{area:?}");
            let panel = match area {
                Area::Editors => {
                    editors = Some(dock);
                    continue;
                }
                Area::PrimarySideBar => egui::Panel::left(id.clone()).default_size(260.0),
                Area::SecondarySideBar => egui::Panel::right(id.clone()).default_size(260.0),
                Area::BottomPanel => egui::Panel::bottom(id.clone()).default_size(200.0),
            };
            let shown = panel.resizable(true).show(ui, |ui| {
                show_dock(ui, &id, dock, &mut viewer);
            });
            area_rects.insert(*area, shown.response.rect);
        }
        let shown = egui::CentralPanel::default().show(ui, |ui| {
            if let Some(dock) = editors {
                show_dock(ui, "helios.area.Editors", dock, &mut viewer);
            }
        });
        area_rects.insert(Area::Editors, shown.response.rect);
        let ctx = ui.ctx().clone();
        self.follow_drag(&ctx);
        self.drop_hint(&ctx);
    }

    fn windows(&mut self, ctx: &Egui) {
        let mut back = Vec::new();
        for aux in &mut self.auxiliary {
            let title = self
                .panels
                .get(&aux.panel)
                .map_or_else(|| aux.panel.clone(), |p| p.title.clone());
            // The same title and, when Helios draws the main window's
            // frame, the same frame.
            let window_title = format!("{title} — Helios");
            let framed = self.framed;
            let builder = ViewportBuilder::default()
                .with_title(window_title.clone())
                .with_decorations(!framed)
                .with_inner_size([aux.rect[2], aux.rect[3]]);
            let mut content = self.contents.get_mut(&aux.panel);
            let rects = &mut self.rects;
            let panel = aux.panel.clone();
            ctx.show_viewport_immediate(ViewportId::from_hash_of(&panel), builder, |ui, _| {
                if framed {
                    let vctx = ui.ctx().clone();
                    crate::frame::resize_handles(&vctx);
                    let request = egui::Panel::top("helios.titlebar")
                        .exact_size(crate::frame::TITLE_HEIGHT)
                        .frame(egui::Frame::NONE.fill(ui.visuals().panel_fill))
                        .show(ui, |ui| crate::frame::title_bar(ui, &window_title))
                        .inner;
                    match request {
                        Some(crate::frame::Request::Minimize) => {
                            vctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                        Some(crate::frame::Request::ToggleMaximize) => {
                            let on = vctx.input(|i| i.viewport().maximized.unwrap_or(false));
                            vctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!on));
                        }
                        Some(crate::frame::Request::Close) => back.push(panel.clone()),
                        Some(crate::frame::Request::SystemMenu(_)) | None => {}
                    }
                }
                egui::CentralPanel::default().show(ui, |ui| {
                    rects.insert(panel.clone(), ui.max_rect());
                    match content.as_deref_mut() {
                        Some(c) => c(ui),
                        None => placeholder(ui, &title),
                    }
                });
                if ui.input(|i| i.viewport().close_requested()) {
                    back.push(panel.clone());
                }
            });
        }
        for p in back {
            self.dock(&p);
        }
    }

    fn palette_ui(&mut self, ctx: &Egui) {
        let Some(mut palette) = self.palette.take() else {
            return;
        };
        let mut run = None;
        let mut close = false;
        let matches: Vec<(String, String, Option<String>)> = self
            .registry
            .search(&palette.query, &self.cx)
            .into_iter()
            .filter(|s| s.id != "workbench.palette")
            .take(12)
            .map(|s| {
                let keys = self
                    .keymap
                    .keys_for(&s.id)
                    .first()
                    .map(std::string::ToString::to_string);
                (s.id.clone(), format!("{}: {}", s.category, s.title), keys)
            })
            .collect();
        egui::Window::new("Commands")
            .id(Id::new("helios.palette"))
            .title_bar(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, [0.0, 48.0])
            .fixed_size([520.0, 0.0])
            .show(ctx, |ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut palette.query)
                        .hint_text("Type a command")
                        .desired_width(f32::INFINITY),
                );
                if !palette.opened {
                    edit.request_focus();
                    palette.opened = true;
                }
                if edit.changed() {
                    palette.selected = 0;
                }
                let (down, up, enter, escape) = ui.input(|i| {
                    (
                        i.key_pressed(Key::ArrowDown),
                        i.key_pressed(Key::ArrowUp),
                        i.key_pressed(Key::Enter),
                        i.key_pressed(Key::Escape),
                    )
                });
                let n = matches.len();
                if n > 0 {
                    if down {
                        palette.selected = (palette.selected + 1) % n;
                    }
                    if up {
                        palette.selected = (palette.selected + n - 1) % n;
                    }
                }
                ui.separator();
                for (i, (id, title, keys)) in matches.iter().enumerate() {
                    let row =
                        crate::dialogs::list_row(ui, i == palette.selected, title, keys.as_deref());
                    if row.clicked() {
                        run = Some(id.clone());
                    }
                }
                if n == 0 {
                    ui.weak("No command matches");
                }
                if enter {
                    run = run
                        .take()
                        .or_else(|| matches.get(palette.selected).map(|m| m.0.clone()));
                }
                close = escape;
            });
        if let Some(id) = run {
            let _ = self.run(&id, Value::Null, Origin::User);
        } else if !close {
            self.palette = Some(palette);
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

/// A command's title as a menu shows it: without the hint of its
/// arguments (`Open a workspace (`path`)`), which the palette keeps.
fn menu_title(title: &str) -> String {
    match title.rfind(" (`") {
        Some(i) if title.ends_with(')') => title[..i].to_string(),
        _ => title.to_string(),
    }
}

/// What a menu shows: items given by the caller, or a declared menu.
enum MenuSource {
    Items(Vec<crate::dialogs::MenuItem>),
    Declared(String),
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

fn placeholder(ui: &mut Ui, title: &str) {
    ui.centered_and_justified(|ui| {
        ui.weak(title);
    });
}

fn show_dock(ui: &mut Ui, id: &str, dock: &mut DockState<String>, viewer: &mut Viewer) {
    DockArea::new(dock)
        .id(Id::new(id))
        .show_close_buttons(false)
        .show_add_buttons(false)
        .show_leaf_close_all_buttons(false)
        .show_leaf_collapse_buttons(false)
        .show_inside(ui, viewer);
}

struct Viewer<'a> {
    panels: &'a Panels,
    contents: &'a mut BTreeMap<String, Content>,
    rects: &'a mut BTreeMap<String, Rect>,
    focused: &'a mut Option<String>,
}

impl TabViewer for Viewer<'_> {
    type Tab = String;

    fn id(&mut self, tab: &mut String) -> Id {
        Id::new(("helios.panel", tab.as_str()))
    }

    fn title(&mut self, tab: &mut String) -> egui::WidgetText {
        self.panels
            .get(tab)
            .map_or_else(|| tab.clone(), |p| p.title.clone())
            .into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut String) {
        let rect = ui.max_rect();
        self.rects.insert(tab.clone(), rect);
        if ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(rect) {
            *self.focused = Some(tab.clone());
        }
        if let Some(c) = self.contents.get_mut(tab) {
            c(ui);
        } else {
            let title = self.panels.get(tab).map_or(tab.as_str(), |p| &p.title);
            placeholder(ui, title);
        }
    }

    fn allowed_in_windows(&self, _tab: &mut String) -> bool {
        // Detached panels get real windows, through `<panel>.detach`; a tab
        // dropped over another area is moved there by the shell.
        false
    }
}

/// Headless rendering, for tests, agents and machines without a display.
pub mod headless {
    use super::{Recorder, RgbaImage, Shell, snapshot};
    use egui_kittest::Harness;

    /// A shell drawn offscreen at a size, on the wgpu renderer.
    pub fn harness<'a>(mut shell: Shell, size: egui::Vec2) -> Harness<'a, Shell> {
        // Our own render state, so the viewport's GPU objects live on it.
        let rs = egui_kittest::wgpu::create_render_state(
            egui_kittest::wgpu::default_wgpu_setup(),
            eframe::egui_wgpu::RendererOptions::PREDICTABLE,
        );
        shell.attach_gpu(&rs);
        // Never a window of the system's from a headless run.
        shell.places.native = false;
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
