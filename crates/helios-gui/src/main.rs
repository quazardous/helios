//! `helios-gui [workspace] [--vault FILE] [--image FILE]`: opens a
//! workspace (a directory, the current one by default) in a window; with a
//! vault, its images fill the library, and what the user rates or labels
//! there is written back; with an image, the viewport shows it through an
//! exposure node whose parameters are values (`node/exposure/*`); with
//! both, the image is imported into the vault, opens with its saved edit,
//! and each edit step is saved there as a version.
//! `--snapshot <target> <file.png>` draws it offscreen instead and saves a
//! picture of the target.

use std::path::{Path, PathBuf};

use helios_gui::{Shell, headless, helios_panels};
use helios_shell::layout::Layout;

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
    Ok((g, exp))
}

struct App {
    shell: Shell,
    file: Option<PathBuf>,
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
        if let Some(file) = &self.file {
            // The workspace remembers its profile beside its layout.
            let pointer = serde_json::json!({ "profile": self.shell.profile_name() });
            let saved = file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| {
                    std::fs::write(file.with_file_name("workspace.json"), pointer.to_string())
                })
                .and_then(|()| std::fs::write(file, self.shell.layout().to_json()));
            if let Err(e) = saved {
                eprintln!(
                    "helios-gui: cannot save the layout to {}: {e}",
                    file.display()
                );
            }
        }
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
    let mut shell = Shell::new(helios_panels(), load(file.as_deref()));
    let name = workspace.file_name().map_or_else(
        || workspace.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    shell.set_title(&name);
    shell.draw_frame(true);
    // The workspace's profile, else the Default one; a saved layout wins
    // over the profile's.
    let profile = file
        .as_ref()
        .and_then(|f| std::fs::read_to_string(f.with_file_name("workspace.json")).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["profile"].as_str().map(String::from))
        .unwrap_or_else(|| "default".into());
    if let Err(e) = shell.use_profile(&profile) {
        shell.notes.push(format!("profile {profile}: {e}"));
    }
    if let Some(saved) = load(file.as_deref()) {
        shell.set_layout(saved);
    }
    let bus = std::sync::Arc::new(helios_core::Bus::new());
    let mut persist = match vault_file {
        Some(path) => {
            let persist = Persist::new(helios_vault_sqlite::SqliteStore::open(&path)?, bus);
            fill_library(&mut shell, &persist)?;
            Some(persist)
        }
        None => None,
    };
    if let Some(path) = image_file {
        let (graph, node) = image_graph(&path)?;
        match &mut persist {
            // With a vault, the image's edit is loaded and saved there.
            Some(persist) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let image = persist.vault().import_file(&path, now)?;
                persist.open_image(&mut shell, image, graph, node, "output")?;
            }
            None => shell.set_view_source(graph, node, "output")?,
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("{name} — Helios"))
            .with_app_id("helios-gui")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0])
            // Helios draws its own frame (see `frame.rs`).
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "helios-gui",
        options,
        Box::new(|cc| {
            if let Some(rs) = cc.wgpu_render_state.as_ref() {
                shell.attach_gpu(rs);
            }
            Ok(Box::new(App {
                shell,
                file,
                persist,
            }))
        }),
    )
    .map_err(|e| e.to_string())
}
