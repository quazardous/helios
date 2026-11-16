kind: fixed

A window kept on a monitor that is no longer plugged in (X11, Windows) opens on a screen left: Helios now tells egui-pop the screens, read at start and again when a window's scale changes or a window moves, by their names so a monitor plugged in again is recognised. The main window, detached panels and the import and settings windows are all brought back. On Wayland, where Helios never knows a window's position, nothing moves.
