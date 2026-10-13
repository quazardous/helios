//! `helios-gui [workspace] [--vault FILE] [--image FILE]`: opens a
//! workspace (a directory, the current one by default) in a window; with a
//! vault, its images fill the library, and what the user rates or labels
//! there is written back; with an image, the viewport shows it through an
//! exposure node whose parameters are values (`node/exposure/*`), applied
//! through a mask whose shapes the viewport draws (`node/mask/*`); with
//! both, the image is imported into the vault, opens with its saved edit,
//! and each edit step is saved there as a version.
//! `--snapshot <target> <file.png>` draws it offscreen instead and saves a
//! picture of the target.
//!
//! Helios runs once per user: a later launch hands its files, workspaces
//! and `helios://` links to the running one and exits; `--new-instance`
//! opens a window of its own (see `helios_gui::platform`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui_pop::model::layout::Layout;
use egui_pop::platform::instance::{self, Instance, Launch, Single};
use helios_gui::platform::{self, Open};
use helios_gui::quit;
use helios_gui::{Shell, headless, helios_panels};

/// Where a workspace's own files are kept (the profile it uses): in the
/// user's configuration, not in the workspace. Its layout is egui-pop's
/// document (`workspaces/<key>/layout` under the interface's settings).
fn workspace_dir(workspace: &Path) -> PathBuf {
    helios_gui::shell::config_dir()
        .join("workspaces")
        .join(helios_gui::host::workspace_key(workspace))
}

/// A layout an earlier Helios saved beside the workspace's profile, read
/// once when egui-pop has none for the workspace yet.
fn earlier_layout(workspace: &Path, ui_settings: &Path) -> Option<Layout> {
    let key = helios_gui::host::workspace_key(workspace);
    let current = ui_settings.join(format!("workspaces/{key}/layout.json"));
    if current.exists() {
        return None;
    }
    let text = std::fs::read_to_string(workspace_dir(workspace).join("layout.json")).ok()?;
    Layout::from_json(&text).ok()
}

type Persist = helios_gui::persist::Persist<helios_vault_sqlite::SqliteStore>;

/// A problem worth knowing: on the terminal, and in the log file once the
/// window's logs are set up.
fn warn(text: &str) {
    eprintln!("helios-gui: {text}");
    log::warn!(target: "helios", "{text}");
}

/// Fills the library from the vault, and reads the thumbnails behind it,
/// one image at a time, from the first copy still present.
fn fill_library(shell: &mut Shell, persist: &Persist) -> Result<(), String> {
    let files = persist.fill_library(shell)?;
    let sink = shell.library_thumbnails();
    std::thread::spawn(move || {
        for (id, path) in files {
            let image = match helios_load::Kind::of(&path) {
                helios_load::Kind::Picture => helios_load::load(&path).map(|l| Some(l.image)),
                helios_load::Kind::Raw => helios_load::preview(&path),
            };
            match image {
                Ok(Some(image)) => {
                    // Scaled here: the window only converts a small one.
                    let small = helios_gui::library::small(&image);
                    if sink.send((id, small)).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(e) => warn(&format!("no thumbnail for {}: {e}", path.display())),
            }
        }
    });
    Ok(())
}

/// Who ran a command, as the vault records it.
fn who(origin: &helios_shell::commands::Origin) -> String {
    use helios_shell::commands::Origin;
    match origin {
        Origin::User => "user".into(),
        Origin::Agent(id) => format!("agent:{id}"),
        Origin::Script(id) => format!("script:{id}"),
        Origin::Midi => "midi".into(),
        Origin::Remote(id) => format!("remote:{id}"),
    }
}

/// The time in seconds since the epoch, as the vault's link counts it.
fn seconds_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The library operations as commands (see `helios_library`): an import
/// runs as a job on the shell's jobs (`job.*`, in the jobs centre, where
/// it is cancelled), the library filled again when it ends, its
/// duplicates listed in the Duplicates panel; stack suggestions are
/// recorded, and applied once accepted (the Stacks panel answers them).
fn library_commands(
    shell: &mut Shell,
    persist: &Persist,
    refill: &Arc<AtomicBool>,
    ctx: &egui::Context,
) -> Result<(), String> {
    use helios_library::import::{Duplicates, Options, Placement};
    use helios_library::stacks;
    use helios_shell::commands::CommandSpec;
    use serde_json::{Value, json};

    let text = |args: &Value, key: &str| args.get(key).and_then(Value::as_str).map(String::from);
    let (vault, jobs) = (persist.vault().clone(), shell.jobs());
    let (done, ctx) = (refill.clone(), ctx.clone());
    // Each file's line goes to the Duplicates panel with the import's
    // arguments, so a duplicate kept is imported again the same way.
    let lines = shell.duplicates_sink();
    shell.register(
        CommandSpec::new(
            "library.import",
            "Import a folder or a card (`path`; `copy`, `into`, `rename`, `duplicates`)",
            "Library",
        ),
        move |args, _| {
            let path = text(args, "path").ok_or("`path` is missing")?;
            let mut options = Options::default();
            if args.get("copy").and_then(Value::as_bool).unwrap_or(false)
                || args.get("into").is_some()
            {
                options.placement = Placement::Copy {
                    into: text(args, "into").map(Into::into),
                };
            }
            if let Some(pattern) = text(args, "rename") {
                options.rename = Some(helios_library::rename::Pattern::parse(&pattern)?);
            }
            if let Some(policy) = text(args, "duplicates") {
                options.duplicates = Duplicates::parse(&policy)?;
            }
            let (vault, jobs, done, ctx) = (vault.clone(), jobs.clone(), done.clone(), ctx.clone());
            let (lines, asked) = (lines.clone(), args.clone());
            std::thread::spawn(move || {
                let sources = [std::path::PathBuf::from(path)];
                // The job's events say how it went; the lines feed the
                // Duplicates panel.
                let ended = helios_library::import::import(
                    vault.as_ref(),
                    &jobs,
                    &sources,
                    &options,
                    seconds(),
                    &mut |line: &Value| {
                        let _ = lines.send((asked.clone(), line.clone()));
                    },
                );
                if let Err(e) = ended {
                    warn(&format!("import: {e}"));
                }
                done.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            });
            Ok(json!({ "queued": true }))
        },
    )?;
    let (vault, bus) = (persist.vault().clone(), persist.bus().clone());
    shell.register(
        CommandSpec::new(
            "library.stacks.suggest",
            "Suggest stacks (`images`: ids, every image when absent; `resolver`)",
            "Library",
        ),
        move |args, _| {
            let images = args
                .get("images")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .map(|i| helios_vault::ImageId::from_hex(i.as_str().unwrap_or("")))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?
                .unwrap_or_default();
            let resolver = text(args, "resolver").unwrap_or_else(|| stacks::RULES.into());
            let resolvers = stacks::Resolvers::default();
            stacks::suggest(
                vault.as_ref(),
                Some(bus.as_ref()),
                &resolvers,
                &resolver,
                &images,
                seconds(),
            )
            .map(|d| stacks::to_json(&d))
        },
    )?;
    let (vault, bus) = (persist.vault().clone(), persist.bus().clone());
    shell.register(
        CommandSpec::new(
            "library.stacks.accept",
            "Accept a stack suggestion (`decision`; `groups`: the ones taken, all when absent)",
            "Library",
        ),
        move |args, origin| {
            let id = text(args, "decision").ok_or("`decision` is missing")?;
            let groups: Option<Vec<usize>> =
                args.get("groups").and_then(Value::as_array).map(|g| {
                    g.iter()
                        .filter_map(Value::as_u64)
                        .map(|n| n as usize)
                        .collect()
                });
            let who = who(origin);
            stacks::accept(
                vault.as_ref(),
                Some(bus.as_ref()),
                &id,
                groups.as_deref(),
                &who,
                seconds(),
            )
            .map(|d| stacks::to_json(&d))
        },
    )?;
    let (vault, bus) = (persist.vault().clone(), persist.bus().clone());
    shell.register(
        CommandSpec::new(
            "library.stacks.decline",
            "Decline a stack suggestion (`decision`)",
            "Library",
        ),
        move |args, origin| {
            let id = text(args, "decision").ok_or("`decision` is missing")?;
            let who = who(origin);
            stacks::decline(vault.as_ref(), Some(bus.as_ref()), &id, &who, seconds())
                .map(|d| stacks::to_json(&d))
        },
    )?;
    let vault = persist.vault().clone();
    shell.register(
        CommandSpec::new("library.stacks.list", "List the stacks", "Library"),
        move |_, _| {
            vault
                .stacks()
                .map(|s| serde_json::to_value(s).expect("stacks serialize"))
        },
    )?;
    let vault = persist.vault().clone();
    shell.register(
        CommandSpec::new(
            "library.stacks.dissolve",
            "Dissolve a stack, keeping its images (`stack`)",
            "Library",
        ),
        move |args, _| {
            let id = text(args, "stack").ok_or("`stack` is missing")?;
            vault
                .dissolve_stack(&id)
                .map(|s| serde_json::to_value(s).expect("stacks serialize"))
        },
    )?;
    Ok(())
}

/// A picture (or a raw file's embedded preview) through the network of
/// the template the workspace was made from, else through an exposure
/// node: the network and the node the viewport shows.
fn image_graph(
    path: &Path,
    template: Option<&helios_template::Bundle>,
) -> Result<(helios_core::Graph, helios_core::NodeId), String> {
    let image = match helios_load::Kind::of(path) {
        helios_load::Kind::Picture => helios_load::load(path)?.image,
        helios_load::Kind::Raw => helios_load::preview(path)?
            .ok_or_else(|| format!("{}: no preview to show", path.display()))?,
    };
    if let Some(t) = template {
        let built = t.build_with(&helios_gui::templates::modules(), &image, None)?;
        return Ok((built.graph, built.view.0));
    }
    let mut g = helios_core::Graph::new();
    let src = g.add_source("source", image);
    let exp = g
        .add_module(
            "exposure",
            helios_core::ModuleRef::new(helios_module_exposure::Exposure),
            &helios_core::Params::new(),
        )
        .map_err(|e| e.to_string())?;
    g.connect(src, "image", exp, "input")
        .map_err(|e| e.to_string())?;
    // The exposure through a mask the viewport draws in: with no shape
    // yet the mask is 1 everywhere, so the exposure applies fully.
    let mask = g
        .add_module(
            "mask",
            helios_core::ModuleRef::new(helios_module_masks::MaskCombine),
            &helios_core::Params::new(),
        )
        .map_err(|e| e.to_string())?;
    let blend = g
        .add_module(
            "blend",
            helios_core::ModuleRef::new(helios_module_masks::MaskBlend),
            &helios_core::Params::new(),
        )
        .map_err(|e| e.to_string())?;
    for (from, out, port) in [
        (src, "image", "input"),
        (exp, "output", "processed"),
        (mask, "mask", "mask"),
    ] {
        g.connect(from, out, blend, port)
            .map_err(|e| e.to_string())?;
    }
    Ok((g, blend))
}

struct App {
    // Fields drop in this order; the marks time the quit's steps.
    persist: Option<Persist>,
    _persist_dropped: quit::MarkOnDrop,
    shell: Shell,
    _shell_dropped: quit::MarkOnDrop,
    /// The template the workspace was made from: an image opened goes
    /// through its network.
    template: Option<helios_template::Bundle>,
    /// Set when an import ended: the library is filled again.
    refill: Arc<AtomicBool>,
}

impl Drop for App {
    fn drop(&mut self) {
        quit::mark("application dropping");
    }
}

impl App {
    /// Opens an image the library activated (Enter, a double click) in
    /// the viewport, from its first copy still present.
    fn open_from_library(&mut self, id: &str) -> Result<(), String> {
        let persist = self.persist.as_mut().ok_or("no vault open")?;
        let image = helios_vault::ImageId::from_hex(id)?;
        let path = persist
            .vault()
            .locations(&image)?
            .into_iter()
            .find(|l| l.present)
            .map(|l| l.path)
            .ok_or_else(|| format!("no copy of image {id} is present"))?;
        let (graph, node) = image_graph(&path, self.template.as_ref())?;
        persist
            .open_image(&mut self.shell, image, graph, node, "output")
            .map(|_| ())
    }

    /// Opens what a later launch or a link asked for: an image in the
    /// viewport, a workspace in a window of its own (none for the one
    /// this window shows).
    fn open(&mut self, open: Open) {
        let done = match open {
            Open::Image(path) => image_graph(&path, self.template.as_ref())
                .and_then(|g| show_image(&mut self.shell, self.persist.as_mut(), path, g)),
            Open::Workspace(dir) => {
                let dir = dir.canonicalize().unwrap_or(dir);
                if self.shell.workspace() == Some(helios_gui::host::workspace_key(&dir)) {
                    Ok(())
                } else {
                    self.shell
                        .run(
                            "workspace.open",
                            serde_json::json!({ "path": dir }),
                            helios_shell::commands::Origin::User,
                        )
                        .map(|_| ())
                }
            }
        };
        if let Err(e) = done {
            self.shell.notify(helios_gui::dialogs::Level::Error, e);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            // The window closes after this frame: nothing to draw (work
            // waiting for a frame, thumbnails or tiles, would hold the
            // window up), only what the user changed to write.
            quit::requested();
            if let Some(persist) = &mut self.persist {
                persist.frame(&mut self.shell, seconds_f64());
                quit::mark("vault written (closing frame)");
            }
            return;
        }
        self.shell.ui(ui);
        // The GPU path is installed by now: the image's own surface
        // follows the Wide-gamut preview setting.
        let frame: &eframe::Frame = frame;
        if let Some(rs) = frame.wgpu_render_state() {
            self.shell.follow_image_surface(ui.ctx(), rs, frame);
        }
        for open in self.shell.take_opens() {
            self.open(open);
        }
        // Duplicates the user chose to keep, imported again.
        let imports = self.shell.duplicates_mut().take_imports();
        for args in imports {
            let _ = self
                .shell
                .run("library.import", args, helios_shell::commands::Origin::User);
        }
        let activated = self.shell.library_mut().take_activated();
        if let Some(id) = activated
            && let Err(e) = self.open_from_library(&id)
        {
            self.shell.notify(helios_gui::dialogs::Level::Error, e);
        }
        if self.refill.swap(false, Ordering::Relaxed)
            && let Some(persist) = &self.persist
            && let Err(e) = fill_library(&mut self.shell, persist)
        {
            warn(&format!("cannot fill the library: {e}"));
        }
        if let Some(persist) = &mut self.persist {
            persist.frame(&mut self.shell, seconds_f64());
            // While a gesture goes on, come back to save it in time.
            if self.shell.values.lock().is_ok_and(|v| v.uncommitted()) {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(
                        helios_gui::persist::FLUSH_SECONDS,
                    ));
            }
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        helios_gui::display::CLEAR_COLOR.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self) {
        quit::mark("on_exit");
        // egui-pop's exit: the workspace saved (`save_workspace`), then
        // the `app.exit` step saves the interface's settings and keys.
        if let Err(e) = self.shell.exit() {
            warn(&format!("cannot save the interface's settings: {e}"));
        }
        quit::mark("interface settings, keys and layout saved");
    }
}

/// Saves the profile the workspace uses as the application closes
/// (egui-pop's `on_exit`), keeping what else the file says (the template
/// it was made from); egui-pop's `app.exit` step saves its layout.
fn save_workspace(dir: &Path, app: &egui_pop::api::AppHandle) {
    let profile = app.context().get("heliosProfile").to_string();
    let file = dir.join("workspace.json");
    let mut pointer = helios_gui::templates::read_json(&file);
    pointer["profile"] = serde_json::json!(profile);
    let saved =
        std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&file, pointer.to_string()));
    if let Err(e) = saved {
        warn(&format!("cannot save {}: {e}", file.display()));
    }
    quit::mark("workspace profile saved");
}

/// Fills the library from the vault and shows the image asked for (its
/// edit loaded from the vault, and saved there, when there is one).
fn open(
    shell: &mut Shell,
    persist: Option<&mut Persist>,
    image: Option<(PathBuf, (helios_core::Graph, helios_core::NodeId))>,
) -> Result<(), String> {
    if let Some(persist) = &persist {
        fill_library(shell, persist)?;
    }
    match image {
        Some((path, graph)) => show_image(shell, persist, path, graph),
        None => Ok(()),
    }
}

/// Shows an image: imported into the vault and opened with its saved edit
/// when there is one, else on its own.
fn show_image(
    shell: &mut Shell,
    persist: Option<&mut Persist>,
    path: PathBuf,
    (graph, node): (helios_core::Graph, helios_core::NodeId),
) -> Result<(), String> {
    match persist {
        Some(persist) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let image = persist.vault().import_file(&path, now)?;
            persist
                .open_image(shell, image, graph, node, "output")
                .map(|_| ())
        }
        None => shell.set_view_source(graph, node, "output"),
    }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, target, out] = args.as_slice()
        && flag == "--snapshot"
    {
        let shell = Shell::new(helios_panels(), None);
        let mut harness = headless::harness(shell, egui::vec2(1280.0, 800.0));
        let image = headless::snapshot(&mut harness, target)?;
        return helios_gui::snapshot::save(&image, Path::new(out));
    }
    // One Helios per user: a later launch hands over and exits.
    let instance = if args.iter().any(|a| a == platform::NEW_INSTANCE) {
        Instance::detached(Launch::current())
    } else {
        match instance::single(platform::APP_ID) {
            Ok(Single::Handed) => {
                eprintln!("helios-gui: handed over to the running Helios");
                return Ok(());
            }
            Ok(Single::First(first)) => first,
            Err(e) => {
                eprintln!("helios-gui: no single instance ({e}), running on its own");
                Instance::detached(Launch::current())
            }
        }
    };
    if args.last().is_some_and(|a| a == "--image") {
        return Err("--image takes a FILE".into());
    }
    let cwd = std::env::current_dir().ok();
    let asked = platform::opens(&args, cwd.as_deref());
    // The last image named, the first workspace (else the current
    // directory).
    let image_file = asked.iter().rev().find_map(|o| match o {
        Open::Image(p) => Some(p.clone()),
        Open::Workspace(_) => None,
    });
    let vault_file = args
        .iter()
        .position(|a| a == "--vault")
        .map(|i| {
            args.get(i + 1)
                .map(PathBuf::from)
                .ok_or("--vault takes a FILE")
        })
        .transpose()?;
    let workspace = asked
        .iter()
        .find_map(|o| match o {
            Open::Workspace(p) => Some(p.clone()),
            Open::Image(_) => None,
        })
        .or(cwd)
        .and_then(|p| p.canonicalize().ok())
        .ok_or("no workspace directory")?;
    if let Err(e) = helios_gui::shell::remember_workspace(&workspace) {
        eprintln!("helios-gui: cannot remember the workspace: {e}");
    }
    let dir = workspace_dir(&workspace);
    // The interface's settings (window place, focus mode, the user's
    // keys) are the user's, beside Helios's other settings.
    let ui_settings = helios_gui::shell::config_dir().join("ui");
    let name = workspace.file_name().map_or_else(
        || workspace.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    // The workspace's profile, else the Default one; the workspace's
    // saved layout wins over the profile's.
    let profile = std::fs::read_to_string(dir.join("workspace.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["profile"].as_str().map(String::from))
        .unwrap_or_else(|| "default".into());
    let bus = std::sync::Arc::new(helios_core::Bus::new());
    let persist = match vault_file {
        Some(path) => Some(Persist::new(
            helios_vault_sqlite::SqliteStore::open(&path)?,
            bus,
        )),
        None => None,
    };
    // Held here, the vault outlives the window: it closes once the
    // window is gone (see `helios_gui::quit`).
    let vault = persist.as_ref().map(|p| p.vault().clone());
    // The template the workspace was made from, if it was.
    let template = match helios_gui::templates::workspace_template(
        &helios_gui::shell::config_dir(),
        &workspace,
    ) {
        Some(Ok(t)) => Some(t),
        Some(Err(e)) => {
            warn(&format!("the workspace's template: {e}"));
            None
        }
        None => None,
    };
    let image = image_file
        .map(|path| image_graph(&path, template.as_ref()).map(|g| (path, g)))
        .transpose()?;
    let earlier = earlier_layout(&workspace, &ui_settings);
    let builder = Shell::builder(helios_panels(), Some(ui_settings))
        .workspace(&workspace)
        .on_exit(move |app| save_workspace(&dir, app))
        .desktop(instance);
    // The window draws with the process's one device, which the engine
    // computes on (see `helios_gui::shell::wgpu_setup`).
    let gpu = helios_gpu::shared::gpu().map_err(|e| format!("no GPU to draw with: {e}"))?;
    let title = format!("{name} — Helios");
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title.clone())
            .with_app_id("helios-gui")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0])
            // egui-pop's frame is Helios's by default (its `window.frame`
            // setting gives the system's back, and the window follows).
            .with_decorations(false)
            // The image's own surface on Wayland shows through the
            // interface; the interface itself stays opaque (`clear_color`).
            .with_transparent(helios_gui::display::TRANSPARENT_WINDOW),
        ..Default::default()
    };
    options.wgpu_options.wgpu_setup = helios_gui::shell::wgpu_setup(&gpu);
    // egui-pop opens the window where it was last (repaired) and builds
    // the application on its context.
    let ran = builder
        .run_native("helios-gui", options, move |mut shell, cc| {
            if let Some(rs) = cc.wgpu_render_state.as_ref() {
                shell.attach_gpu(rs);
            }
            shell.set_title(&name);
            if let Err(e) = shell.use_profile(&profile) {
                shell.notes.push(format!("profile {profile}: {e}"));
            }
            // The template's keys, in the workspace's keymap layer.
            if let Some(t) = &template {
                shell.apply_template_tools(&t.template.meta.name, &t.template.tools);
            }
            // The workspace's layout wins over the profile's: egui-pop's
            // document is loaded again (or an earlier Helios's, once).
            if let Some(saved) = earlier {
                shell.set_layout(saved);
            } else if let Err(e) = shell.load_layout() {
                shell.notes.push(format!("layout: {e}"));
            }
            let mut persist = persist;
            if let Err(e) = open(&mut shell, persist.as_mut(), image) {
                shell.notify(helios_gui::dialogs::Level::Error, e);
            }
            let refill = Arc::new(AtomicBool::new(false));
            if let Some(p) = &persist
                && let Err(e) = library_commands(&mut shell, p, &refill, &cc.egui_ctx)
            {
                shell.notes.push(format!("library commands: {e}"));
            }
            Box::new(App {
                persist,
                _persist_dropped: quit::MarkOnDrop("vault link dropped"),
                shell,
                _shell_dropped: quit::MarkOnDrop("shell dropped"),
                template,
                refill,
            })
        })
        .map_err(|e| e.to_string());
    quit::mark("window closed (eframe returned)");
    // The window is gone: what is left runs against its deadline.
    let mut finisher = quit::Finisher::default();
    if let Some(vault) = vault {
        finisher.spawn("vault closed", quit::VAULT_DEADLINE, move || drop(vault));
    }
    for left in finisher.finish() {
        warn(&format!("quit: {left} was left unfinished"));
    }
    ran
}
