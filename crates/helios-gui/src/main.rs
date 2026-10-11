//! `helios-gui [workspace]`: opens a workspace (a directory, the current
//! one by default) in a window. `--snapshot <target> <file.png>` draws it
//! offscreen instead and saves a picture of the target.

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

struct App {
    shell: Shell,
    file: Option<PathBuf>,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shell.ui(ui);
    }

    fn on_exit(&mut self) {
        if let Some(file) = &self.file {
            let saved = file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
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
    let workspace = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .and_then(|p| p.canonicalize().ok())
        .ok_or("no workspace directory")?;
    let file = layout_file(&workspace);
    let mut shell = Shell::new(helios_panels(), load(file.as_deref()));
    let name = workspace.file_name().map_or_else(
        || workspace.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    shell.set_title(&name);
    shell.draw_frame(true);
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
        Box::new(|_| Ok(Box::new(App { shell, file }))),
    )
    .map_err(|e| e.to_string())
}
