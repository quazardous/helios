kind: internal

Storage is a crate of its own, `helios-storage-ui`: the Storage and Backup pages, their commands, a backup's end and the cards safe to format. helios-gui mounts it and the rest of the desktop hears it through the bus. Its page tests run in its own test binary on `helios-feature-host`, a test-support crate that runs one feature without the desktop.
