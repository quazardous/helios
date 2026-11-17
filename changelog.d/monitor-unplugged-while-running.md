kind: fixed

A monitor unplugged while Helios runs (X11, Windows) no longer leaves windows off-screen: the main window and the import and settings windows on it move onto a screen left, and a detached panel's window on it docks back. On Wayland, where Helios never knows a window's position, nothing moves.
