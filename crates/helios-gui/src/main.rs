//! `helios-gui [workspace] [--vault FILE] [--image FILE]`: opens a
//! workspace (a directory, the current one by default) in a window; with a
//! vault, its images fill the library, and what the user rates or labels
//! there is written back; with an image, the viewport shows it through an
//! exposure node whose parameters are values (`node/exposure/*`).
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

type Vault = helios_vault::Vault<helios_vault_sqlite::SqliteStore>;

/// Fills the library from the vault, and reads the thumbnails behind it,
/// one image at a time, from the first copy still present.
fn fill_library(shell: &mut Shell, vault: &Vault) -> Result<(), String> {
    use helios_gui::library::Entry;
    use helios_vault::fields::{LABELS, Query, RATING};
    let cursor = vault.open_cursor(Query::default())?;
    let rows = vault.fetch(cursor.id, 0, cursor.count, &[RATING, LABELS])?;
    vault.close_cursor(cursor.id);
    let mut entries = Vec::new();
    let mut files = Vec::new();
    for row in rows {
        let path = vault
            .locations(&row.id)?
            .into_iter()
            .find(|l| l.present)
            .map(|l| l.path);
        let id = row.id.hex();
        entries.push(Entry {
            id: id.clone(),
            name: path
                .as_ref()
                .and_then(|p| p.file_name())
                .map_or_else(|| id[..12].to_string(), |n| n.to_string_lossy().into()),
            rating: row.fields.get(RATING).and_then(|v| v.as_i64()).unwrap_or(0),
            labels: row
                .fields
                .get(LABELS)
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|l| l.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        });
        if let Some(path) = path {
            files.push((id, path));
        }
    }
    shell.set_library(entries);
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

/// Shows a picture (or a raw file's embedded preview) in the viewport,
/// through an exposure node.
fn show_image(shell: &mut Shell, path: &Path) -> Result<(), String> {
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
    shell.set_view_source(g, exp, "output")
}

struct App {
    shell: Shell,
    file: Option<PathBuf>,
    vault: Option<Vault>,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shell.ui(ui);
        let changes = self.shell.take_library_changes();
        if let (Some(vault), false) = (&self.vault, changes.is_empty()) {
            let commands: Result<Vec<_>, String> = changes
                .into_iter()
                .map(|c| {
                    Ok(helios_vault::fields::Command {
                        image: helios_vault::ImageId::from_hex(&c.image)?,
                        field: c.field,
                        value: c.value,
                    })
                })
                .collect();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            if let Err(e) = commands.and_then(|c| vault.apply(&c, "user", None, now)) {
                self.shell.notes.push(format!("vault: {e}"));
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
                .and_then(|_| {
                    std::fs::write(file.with_file_name("workspace.json"), pointer.to_string())
                })
                .and_then(|_| std::fs::write(file, self.shell.layout().to_json()));
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
    if let Some(path) = image_file {
        show_image(&mut shell, &path)?;
    }
    let vault = match vault_file {
        Some(path) => {
            let vault = helios_vault::Vault::new(helios_vault_sqlite::SqliteStore::open(&path)?);
            fill_library(&mut shell, &vault)?;
            Some(vault)
        }
        None => None,
    };
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
            Ok(Box::new(App { shell, file, vault }))
        }),
    )
    .map_err(|e| e.to_string())
}
