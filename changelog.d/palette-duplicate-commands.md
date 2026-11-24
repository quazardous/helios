kind: changed

The command palette no longer lists four commands twice: "Show all commands", "Toggle full screen", "Fold the end side bar" and "Fold the bottom panel" are gone in favour of the library's own Command palette, Full screen, Toggle secondary side bar and Toggle panel. The darktable keymap's Ctrl+Shift+R and Ctrl+Shift+B now run those; a script calling `workbench.palette` or `workbench.fullscreen` calls `app.palette` or `app.fullscreen`.
