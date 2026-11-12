kind: fixed

The Import window no longer freezes while the library is busy or a plugin is slow to stop: its accounts are read in the background, and allowing a plugin's permissions stops and restarts it off the interface's thread. Finishing or cancelling an import in a window that took the main window's place (the main window closed first) now brings the library back instead of quitting Helios.
