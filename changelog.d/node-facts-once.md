kind: internal

A node's relayable facts now reach the desktop once: the node listener table hears them as one named listener of their point on the application bus, the event stream no longer copies them on the shell bus, and a restarted node says one resync instead of two. The desktop's remaining hand-rolled bus subscriptions are listed with their reasons in `xtask/allow/bus.toml`, which `cargo xtask bus --check` holds.
