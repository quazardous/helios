kind: internal

The viewport engine's notices (a GPU fallen back to the CPU, a GPU lost) and the settings layers the desktop's node says changed now reach their readers as named listeners of the application's bus: the two notices are relayed into its journal while the engine's per-render events stay on its own bus, and the settings' listener is registered before the settings are read, so a layer changed in between is still read again. A layer changed on a remote node is no longer taken for one of the desktop's.
