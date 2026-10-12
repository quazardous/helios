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

use std::path::{Path, PathBuf};

use egui_pop::model::layout::Layout;
use helios_gui::{Shell, headless, helios_panels};

/// Where a workspace's layout is kept: in the user's configuration, not
/// in the workspace.
fn layout_file(workspace: &Path) -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let key: String = workspace
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(
        config
            .join("helios/workspaces")
            .join(key)
            .join("layout.json"),
    )
}

/// A workspace's saved layout: egui-pop's layout document, which also
/// reads (and migrates) the layouts Helios saved before.
fn load(file: Option<&Path>) -> Option<Layout> {
    let text = std::fs::read_to_string(file?).ok()?;
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shell.ui(ui);
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

/// Saves the workspace's layout and, beside it, the profile it uses,
/// as the application closes (egui-pop's `on_exit`).
fn save_workspace(file: &Path, app: &egui_pop::api::AppHandle) {
    let profile = app.context().get("heliosProfile").to_string();
    let pointer = serde_json::json!({ "profile": profile });
    let saved = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(file.with_file_name("workspace.json"), pointer.to_string()))
        .and_then(|()| std::fs::write(file, app.layout().to_json()));
    if let Err(e) = saved {
        eprintln!(
            "helios-gui: cannot save the layout to {}: {e}",
            file.display()
        );
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
    let Some((path, (graph, node))) = image else {
        return Ok(());
    };
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
    let image_file = args
        .iter()
        .position(|a| a == "--image")
        .map(|i| {
            args.get(i + 1)
                .map(PathBuf::from)
                .ok_or("--image takes a FILE")
        })
        .transpose()?;
    let vault_file = args
        .iter()
        .position(|a| a == "--vault")
        .map(|i| {
            args.get(i + 1)
                .map(PathBuf::from)
                .ok_or("--vault takes a FILE")
        })
        .transpose()?;
    let workspace = args
        .iter()
        .enumerate()
        .find(|(i, a)| {
            !a.starts_with('-')
                && (*i == 0 || !["--vault", "--image"].contains(&args[i - 1].as_str()))
        })
        .map(|(_, a)| a)
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .and_then(|p| p.canonicalize().ok())
        .ok_or("no workspace directory")?;
    if let Err(e) = helios_gui::shell::remember_workspace(&workspace) {
        eprintln!("helios-gui: cannot remember the workspace: {e}");
    }
    let file = layout_file(&workspace);
    // The interface's settings (window place, focus mode, the user's
    // keys) are the user's, beside Helios's other settings.
    let ui_settings = helios_gui::shell::config_dir().join("ui");
    let name = workspace.file_name().map_or_else(
        || workspace.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    // The workspace's profile, else the Default one; a saved layout wins
    // over the profile's.
    let profile = file
        .as_ref()
        .and_then(|f| std::fs::read_to_string(f.with_file_name("workspace.json")).ok())
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
    let mut builder = Shell::builder(helios_panels(), Some(ui_settings));
    if let Some(file) = file.clone() {
        builder = builder.on_exit(move |app| save_workspace(&file, app));
    }
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
            if let Some(saved) = load(file.as_deref()) {
                shell.set_layout(saved);
            }
            let mut persist = persist;
            if let Err(e) = open(&mut shell, persist.as_mut(), image) {
                shell.notify(helios_gui::dialogs::Level::Error, e);
            }
            Box::new(App { shell, persist })
        })
        .map_err(|e| e.to_string())
}
