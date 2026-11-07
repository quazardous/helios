kind: added

The default library: without `--vault`, Helios opens `library.db` in its data folder (`~/.local/share/helios` on Linux, `%LOCALAPPDATA%\helios` on Windows, `~/Library/Application Support/helios` on macOS), made on the first launch, so File > Import… works from the desktop entry. A library that cannot be opened is said in a toast, and the import window then shows why with its Import button off.
