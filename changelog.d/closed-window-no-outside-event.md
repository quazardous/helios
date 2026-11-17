kind: fixed

A closed Settings window no longer stays on screen frozen until another window is brought forward (GNOME under Wayland), and detached panels' windows no longer freeze while they cover the main window: when the system withholds the main window's redraw (a window it does not show gets none), Helios's event loop paints it itself, and that pass lets the closed window go and draws the detached panels.
