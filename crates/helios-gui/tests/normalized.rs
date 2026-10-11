//! The normalized UI's rule, checked: nothing but the shell draws a
//! window, a popup, a menu or a dialog. Panels, widgets, modules and
//! plugins ask the shell (`helios_gui::dialogs`).

use std::path::{Path, PathBuf};

/// What draws a window, a popup or a menu of its own.
const FORBIDDEN: &[&str] = &[
    "Window::new",
    "Modal::new",
    "Popup::",
    "Area::new",
    ".context_menu(",
    "menu_button(",
    "show_viewport",
];

/// Where the shell draws them: its dialogs, its own windows (the palette,
/// auxiliary windows) and its frame.
const SHELL: &[&str] = &[
    "crates/helios-gui/src/dialogs.rs",
    "crates/helios-gui/src/shell.rs",
    "crates/helios-gui/src/frame.rs",
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn only_the_shell_draws_windows_popups_and_menus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for group in ["crates", "modules"] {
        let Ok(entries) = std::fs::read_dir(root.join(group)) else {
            continue;
        };
        for e in entries.flatten() {
            sources(&e.path().join("src"), &mut files);
        }
    }
    assert!(files.len() > 20, "found {} sources", files.len());
    let mut offenders = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(&root)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/");
        if SHELL.contains(&rel.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(f).unwrap();
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for pattern in FORBIDDEN {
                if line.contains(pattern) {
                    offenders.push(format!("{rel}:{}: {pattern}", n + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "draw these through the shell (helios_gui::dialogs):\n{}",
        offenders.join("\n")
    );
}
