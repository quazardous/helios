kind: fixed

The `window-audit` and `import-close` bench probes wait for the main window to be listed before opening the import window. The agent socket can answer before the main window is mapped; the probe then took the main window for the import window and closed it with the compositor, so `close-with-picker` failed while the import window itself behaved.
