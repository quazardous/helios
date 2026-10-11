kind: added

A first Helios window: `helios-gui [workspace]` opens a workspace in a
VS Code-like shell, with side bars, editors and a bottom panel docked from
the workspace's saved layout, panels that can be moved to their own
window, and a command palette (Ctrl+Shift+P or F1) running every action.
The interface can picture any panel or named widget, and record a video
of it, by itself (`gui.snapshot`, `gui.record.start`), or headless with
`helios-gui --snapshot <target> <file.png>`. `packaging/linux/install.sh
--dev` adds a "Helios Dev" launcher running a source checkout's build.
