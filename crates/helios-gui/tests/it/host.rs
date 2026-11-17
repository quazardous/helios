//! What the desktop's host does that the tests do in its place: it hears
//! the workspace's network changed (`darkroom.network`, on
//! `workspace.applied`) and passes the image shown through it.

use std::cell::Cell;

use helios_gui::Shell;

thread_local! {
    static CHANGED: Cell<usize> = const { Cell::new(0) };
}

/// Listens for the workspace's network changed on `shell`'s bus, as the
/// host does: counted, for [`network_changed`].
pub fn listen_network(shell: &Shell) {
    shell
        .listen_network(|| CHANGED.with(|c| c.set(c.get() + 1)))
        .unwrap();
}

/// How many times the network changed since the last call.
pub fn network_changed() -> usize {
    CHANGED.with(|c| c.replace(0))
}
