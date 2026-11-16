kind: changed

Helios's own windows are on egui-pop's bus (ADR 0029): the Library, Activity and Calibration windows and the import window open as their owner's `window.open` request and close their state with their window's `window.close`, whatever closed it (its title bar, a command, Escape, the compositor, the main window closing), in the same pass. A window closed by the person brings the main window forward through `window.activate`.
