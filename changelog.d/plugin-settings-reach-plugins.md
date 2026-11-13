kind: added

Plugins now receive the settings set for them: their own (Settings > Plugins) when they start and with each call, and a source plugin also its account's and its source's. A change reaches a running plugin at once, or restarts it when the setting asks for that. Secrets go from the system keyring to the plugin alone. In the import window, a source's parameters use the same controls as Settings, and a refused value says why.
