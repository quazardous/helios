//! What tests and tools build a [`Persist`] with, and reach its vault
//! by. The desktop's own code goes through the node's client and its
//! [`Library`] instead (`cargo xtask check`, the desktop's rule).

use std::sync::Arc;

use helios_core::Bus;
use helios_node_core::desktop::Library;
use helios_vault::{Store, Vault};

use super::Persist;

impl<S: Store + 'static> Persist<S> {
    /// The link to a vault opened by its owner, whose changes are
    /// announced on `bus`.
    pub fn on(vault: Arc<Vault<S>>, bus: Arc<Bus>) -> Self {
        Self::on_library(Library::on(vault, bus))
    }

    /// The vault under the node's library.
    pub fn vault(&self) -> &Arc<Vault<S>> {
        self.library().vault()
    }
}
