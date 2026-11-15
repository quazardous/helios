kind: fixed

Closing the Settings in their own window (Health check included) can no longer leave a window behind without a word: egui-pop closes every application window one way, and its paint guard now watches each window, not only the main one. A window that paints nothing says which it is in the log, once, and shows a page with a Close button; a closed window the system still shows is hidden. The first-user journey closes the Settings' Health check window and checks the main window is left alone; the `settings-close` bench job closes it by every way, on labwc or mutter.
