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

use egui_pop::model::layout::Layout;
use egui_pop::platform::instance::{self, Instance, Launch, Single};
use helios_gui::platform::{self, Open};
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
                    if sink.send((id, image)).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(e) => eprintln!("helios-gui: no thumbnail for {}: {e}", path.display()),
            }
        }
    });
    Ok(())
}

/// A picture (or a raw file's embedded preview) through an exposure node:
/// the network and the node the viewport shows.
fn image_graph(path: &Path) -> Result<(helios_core::Graph, helios_core::NodeId), String> {
    let image = match helios_load::Kind::of(path) {
        helios_load::Kind::Picture => helios_load::load(path)?.image,
        helios_load::Kind::Raw => helios_load::preview(path)?
            .ok_or_else(|| format!("{}: no preview to show", path.display()))?,
    };
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
    shell: Shell,
    persist: Option<Persist>,
}

impl App {
    /// Opens what a later launch or a link asked for: an image in the
    /// viewport, a workspace in a window of its own (none for the one
    /// this window shows).
    fn open(&mut self, open: Open) {
        let done = match open {
            Open::Image(path) => image_graph(&path)
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shell.ui(ui);
        for open in self.shell.take_opens() {
            self.open(open);
        }
        if let Some(persist) = &mut self.persist {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |d| d.as_secs_f64());
            persist.frame(&mut self.shell, now);
            // While a gesture goes on, come back to save it in time.
            if self.shell.values.lock().is_ok_and(|v| v.uncommitted()) {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(
                        helios_gui::persist::FLUSH_SECONDS,
                    ));
            }
        }
    }

    fn on_exit(&mut self) {
        // egui-pop's exit: the workspace saved (`save_workspace`), then
        // the `app.exit` step saves the interface's settings and keys.
        if let Err(e) = self.shell.exit() {
            eprintln!("helios-gui: cannot save the interface's settings: {e}");
        }
    }
}

/// Saves the profile the workspace uses as the application closes
/// (egui-pop's `on_exit`); egui-pop's `app.exit` step saves its layout.
fn save_workspace(dir: &Path, app: &egui_pop::api::AppHandle) {
    let profile = app.context().get("heliosProfile").to_string();
    let pointer = serde_json::json!({ "profile": profile });
    let file = dir.join("workspace.json");
    let saved =
        std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&file, pointer.to_string()));
    if let Err(e) = saved {
        eprintln!("helios-gui: cannot save {}: {e}", file.display());
    }
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
    let image = image_file
        .map(|path| image_graph(&path).map(|g| (path, g)))
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
            .with_decorations(false),
        ..Default::default()
    };
    options.wgpu_options.wgpu_setup = helios_gui::shell::wgpu_setup(&gpu);
    // egui-pop opens the window where it was last (repaired) and builds
    // the application on its context.
    builder
        .run_native("helios-gui", options, move |mut shell, cc| {
            if let Some(rs) = cc.wgpu_render_state.as_ref() {
                shell.attach_gpu(rs);
            }
            shell.set_title(&name);
            if let Err(e) = shell.use_profile(&profile) {
                shell.notes.push(format!("profile {profile}: {e}"));
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
            Box::new(App { shell, persist })
        })
        .map_err(|e| e.to_string())
}
