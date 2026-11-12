kind: added

The person's and the library's settings follow live: when another client of the node changes them, the desktop reads them again and the Settings app shows the new values without a restart. `helios-node --vault` reads and writes these settings through its control endpoints (`helios control settings-get LAYER`, `helios control settings-set LAYER TOML-FILE`, the `settings.layer.get` and `settings.layer.set` requests, also on the batch endpoint), each change announced on its bus as `settings.layerChanged`.
