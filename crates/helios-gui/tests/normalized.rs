//! The normalized UI's rule, checked: nothing but the shell draws a
//! window, a popup, a menu or a dialog. Panels, widgets, modules and
//! plugins ask the shell (`helios_gui::dialogs`), or name a menu that
//! egui-pop's menus draw (`DECLARED`).

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

/// Where the shell draws them: its menus' popups, the auxiliary windows
/// of its areas and its frame (the `Frame` plugin, whose title bar keeps
/// egui-pop's menu button). Dialogs, toasts, the palette and the
/// settings window are egui-pop's.
const SHELL: &[&str] = &[
    "crates/helios-gui/src/shell.rs",
    "crates/helios-gui/src/workbench.rs",
    "crates/helios-gui/src/plugins.rs",
];

/// Declared menus, allowed outside the shell: a view of egui-pop's given
/// a menu id (`TreeView::context_menu(app, menu)`) opens that menu on a
/// right click and egui-pop's menus draw it, as they draw the shell's.
/// The panel only names the menu; its entries are contributions (commands,
/// lists) like any other menu's, so nothing of the panel's own is drawn.
/// Textually the call is a `.context_menu(` like egui's own
/// `Response::context_menu`, which draws whatever its closure adds: each
/// declared one is listed here, by file and by the call's text, and the
/// test fails when a listed call is gone, so the list cannot go stale.
const DECLARED: &[(&str, &str)] = &[(
    "crates/helios-gui/src/mask_tree.rs",
    ".context_menu(app, ROW_MENU)",
)];

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
    let mut declared_seen = vec![false; DECLARED.len()];
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
            if let Some(i) = DECLARED
                .iter()
                .position(|(file, call)| *file == rel && line.contains(call))
            {
                declared_seen[i] = true;
                continue;
            }
            for pattern in FORBIDDEN {
                if line.contains(pattern) {
                    offenders.push(format!("{rel}:{}: {pattern}", n + 1));
                }
            }
        }
    }
    let gone: Vec<String> = DECLARED
        .iter()
        .zip(&declared_seen)
        .filter(|(_, seen)| !**seen)
        .map(|((file, call), _)| format!("{file}: {call}"))
        .collect();
    assert!(
        gone.is_empty(),
        "declared menus allowed but no longer called (remove them from DECLARED):\n{}",
        gone.join("\n")
    );
    assert!(
        offenders.is_empty(),
        "draw these through the shell (helios_gui::dialogs):\n{}",
        offenders.join("\n")
    );
}
