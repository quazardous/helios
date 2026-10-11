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

/// What commands ask the shell to do; they are queued and applied at the
/// next frame, where the shell's state is at hand.
enum Action {
    Palette,
    Theme(String),
    ThemeExport(PathBuf),
    ProfileExport(PathBuf, Vec<helios_profile::Part>),
    ProfileImport(PathBuf),
    ProfileApply,
    ProfileUse(String),
    Fullscreen,
    Minimize,
    Maximize,
    Close,
    Focus(String),
    Toggle(String),
    Detach(String),
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
    pixels_per_point: f32,
    contents: BTreeMap<String, Content>,
    shots: VecDeque<Shot>,
    recording: Option<(String, Recorder)>,
    /// What the shell has to say: command results, layout repairs.
    pub notes: Vec<String>,
    title: String,
    /// The window's frame is drawn by Helios (see `frame`).
    framed: bool,
    themes: helios_theme::Themes,
    theme: helios_theme::Resolved,
    theme_overrides: BTreeMap<String, serde_json::Value>,
    /// The theme changed and egui has not been told yet.
    theme_pending: bool,
    /// Where profiles are kept (`<name>.helios-profile`).
    profiles_dir: PathBuf,
    profile_name: String,
    /// A profile read and shown as a diff, waiting for `profile.import.apply`.
    pending_import: Option<helios_profile::Profile>,
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
            pixels_per_point: 1.0,
            contents: BTreeMap::new(),
            shots: VecDeque::new(),
            recording: None,
            notes,
            panels,
            title: String::new(),
            framed: false,
            themes: helios_theme::Themes::default(),
            theme: helios_theme::Themes::default().resolve(helios_theme::BASE, &BTreeMap::new()),
            theme_overrides: BTreeMap::new(),
            theme_pending: true,
            profiles_dir: config_dir().join("profiles"),
            profile_name: "default".into(),
            pending_import: None,
        };
        shell.docks = layout
            .areas
            .iter()
            .map(|(area, node)| (*area, to_dock(node)))
            .collect();
        shell.declare_commands();
        shell
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
        self.theme_overrides = overrides;
        self.theme = self.themes.resolve(name, &self.theme_overrides);
        self.theme_pending = true;
        for p in &self.theme.problems {
            self.notes.push(format!("theme {name}: {p}"));
        }
    }

    /// The theme in use.
    pub fn theme(&self) -> &helios_theme::Resolved {
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
                name: self.theme.name.clone(),
                overrides: self.theme_overrides.clone(),
            }),
            layout: serde_json::from_str(&self.layout().to_json()).ok(),
            ..Profile::default()
        }
    }

    /// Applies a profile's parts the shell holds; the others stay.
    pub fn apply_profile(&mut self, p: &helios_profile::Profile) {
        use helios_profile::{System, local_keys};
        self.profile_name = p.name.clone();
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
                    self.notes.push(format!("profile {}: {e}", p.name));
                }
            }
        }
        if let Some(layout) = &p.layout {
            match Layout::from_json(&layout.to_string()) {
                Ok(l) => self.set_layout(l),
                Err(e) => self.notes.push(format!("profile {}: layout: {e}", p.name)),
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

    fn read_profile(&self, path: &std::path::Path) -> Result<helios_profile::Profile, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        helios_profile::Profile::import(&text)
    }

    /// Uses a named profile, the parts it leaves out from the Default
    /// profile (`default.helios-profile`, when there is one).
    pub fn use_profile(&mut self, name: &str) -> Result<(), String> {
        let default = self
            .read_profile(&self.profile_file("default"))
            .unwrap_or_default();
        let p = if name == "default" {
            default.clone()
        } else {
            self.read_profile(&self.profile_file(name))?
        };
        let mut p = p.over(&default);
        p.name = name.to_string();
        self.apply_profile(&p);
        Ok(())
    }

    /// Adds a theme (a user's file, a plugin's) to those that can be chosen.
    pub fn add_theme(&mut self, theme: helios_theme::Theme) {
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
            self.notes.push(e.clone());
        }
        result
    }

    fn declare_commands(&mut self) {
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
        let names: Vec<String> = self.themes.names().iter().map(|n| n.to_string()).collect();
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
        add(
            CommandSpec::new(
                "profile.export",
                "Export the profile (all parts, or `parts`)",
                "Preferences",
            ),
            Box::new(move |a| {
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
                Ok(Action::ProfileExport(path, parts))
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
            ("ctrl+shift+p", "workbench.palette"),
            ("f1", "workbench.palette"),
            ("f11", "workbench.fullscreen"),
        ] {
            self.keymap
                .bind(keys, command, "", Layer::Default, "shell")
                .expect("shell keys parse");
        }
    }

    fn context(&self, ctx: &Egui) -> Context {
        let mut cx = Context::new()
            .flag("textInputFocus", ctx.egui_wants_keyboard_input())
            .flag("paletteVisible", self.palette.is_some());
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
        match self.docks.iter_mut().find(|(a, _)| *a == area) {
            Some((_, dock)) => dock.push_to_first_leaf(panel.to_string()),
            None => {
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
            Action::Theme(name) => {
                let overrides = self.theme_overrides.clone();
                self.set_theme(&name, overrides);
            }
            Action::ThemeExport(path) => {
                let text = serde_json::to_string_pretty(&self.theme.to_tokens())
                    .expect("tokens serialize");
                let saved = path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|_| std::fs::write(&path, text));
                self.notes.push(match saved {
                    Ok(()) => format!("theme exported to {}", path.display()),
                    Err(e) => format!("cannot export the theme to {}: {e}", path.display()),
                });
            }
            Action::ProfileExport(path, parts) => {
                let parts = if parts.is_empty() {
                    helios_profile::Part::ALL.to_vec()
                } else {
                    parts
                };
                let profile = self.profile();
                let present: Vec<_> = parts.into_iter().filter(|p| profile.has(*p)).collect();
                // No plugin settings yet to keep home.
                let written = profile.export(&present, &|_, _| false).and_then(|text| {
                    path.parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|_| std::fs::write(&path, text))
                        .map_err(|e| format!("{}: {e}", path.display()))
                });
                self.notes.push(match written {
                    Ok(()) => format!("profile exported to {}", path.display()),
                    Err(e) => format!("profile not exported: {e}"),
                });
            }
            Action::ProfileImport(path) => match self.read_profile(&path) {
                Ok(incoming) => {
                    let changes = helios_profile::diff(&self.profile(), &incoming);
                    let count: usize = changes.iter().map(|d| d.changes.len()).sum();
                    self.notes.push(format!(
                        "profile {}: {count} changes; profile.import.apply to apply them",
                        incoming.name
                    ));
                    for d in &changes {
                        for c in &d.changes {
                            self.notes.push(format!("{:?} {c}", d.part));
                        }
                    }
                    self.pending_import = Some(incoming);
                }
                Err(e) => self.notes.push(format!("profile not imported: {e}")),
            },
            Action::ProfileApply => match self.pending_import.take() {
                Some(p) => {
                    let name = p.name.clone();
                    self.apply_profile(&p);
                    self.notes.push(format!("profile {name} applied"));
                }
                None => self
                    .notes
                    .push("no profile read to apply: profile.import first".into()),
            },
            Action::ProfileUse(name) => {
                if let Err(e) = self.use_profile(&name) {
                    self.notes.push(format!("profile {name}: {e}"));
                }
            }
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
            Action::Snapshot { target, path } => {
                self.shots.push_back(Shot::Save { target, path });
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
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
                    self.notes.push(note);
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
                    .map(|_| format!("{target} pictured to {}", path.display()))
                    .unwrap_or_else(|e| format!("snapshot failed: {e}"));
                self.notes.push(note);
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
                        self.notes.push(format!("recording stopped: {e}"));
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
        self.pixels_per_point = ctx.pixels_per_point();
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
                } if self.palette.is_none() || key == Key::F1 || modifiers.ctrl => {
                    self.on_key(key, modifiers)
                }
                Event::Screenshot { image, .. } => self.on_screenshot(&image),
                _ => {}
            }
        }
        let actions: Vec<Action> = std::mem::take(&mut *self.queue.lock().expect("queue"));
        for a in actions {
            self.apply(&ctx, a);
        }
        if self.theme_pending {
            crate::theme::apply(&ctx, &self.theme);
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
        self.areas(ui);
        self.windows(&ctx);
        self.palette_ui(&ctx);

        let named = ctx.data(|d| d.get_temp::<BTreeMap<String, Rect>>(Id::new("helios.named")));
        self.rects.extend(named.unwrap_or_default());

        if self.recording.is_some() {
            self.shots.push_back(Shot::Frame);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
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
            ..
        } = self;
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
            panel.resizable(true).show(ui, |ui| {
                show_dock(ui, &id, dock, &mut viewer);
            });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(dock) = editors {
                show_dock(ui, "helios.area.Editors", dock, &mut viewer);
            }
        });
    }

    fn windows(&mut self, ctx: &Egui) {
        let mut back = Vec::new();
        for aux in &mut self.auxiliary {
            let title = self
                .panels
                .get(&aux.panel)
                .map(|p| p.title.clone())
                .unwrap_or_else(|| aux.panel.clone());
            let builder = ViewportBuilder::default()
                .with_title(format!("{title} — Helios"))
                .with_inner_size([aux.rect[2], aux.rect[3]]);
            let mut content = self.contents.get_mut(&aux.panel);
            let rects = &mut self.rects;
            let panel = aux.panel.clone();
            ctx.show_viewport_immediate(ViewportId::from_hash_of(&panel), builder, |ui, _| {
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
                let keys = self.keymap.keys_for(&s.id).first().map(|k| k.to_string());
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
                    let row = ui.horizontal(|ui| {
                        let r = ui.selectable_label(i == palette.selected, title);
                        if let Some(k) = keys {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.weak(k);
                                },
                            );
                        }
                        r
                    });
                    if row.inner.clicked() {
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
            .map(|p| p.title.clone())
            .unwrap_or_else(|| tab.clone())
            .into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut String) {
        let rect = ui.max_rect();
        self.rects.insert(tab.clone(), rect);
        if ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(rect) {
            *self.focused = Some(tab.clone());
        }
        match self.contents.get_mut(tab) {
            Some(c) => c(ui),
            None => {
                let title = self.panels.get(tab).map_or(tab.as_str(), |p| &p.title);
                placeholder(ui, title)
            }
        }
    }

    fn allowed_in_windows(&self, _tab: &mut String) -> bool {
        // Detached panels get real windows, through `<panel>.detach`.
        false
    }
}

/// Headless rendering, for tests, agents and machines without a display.
pub mod headless {
    use super::*;
    use egui_kittest::Harness;

    /// A shell drawn offscreen at a size, on the wgpu renderer.
    pub fn harness<'a>(shell: Shell, size: egui::Vec2) -> Harness<'a, Shell> {
        Harness::builder()
            .with_size(size)
            .wgpu()
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
