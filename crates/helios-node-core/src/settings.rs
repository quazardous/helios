//! The settings layers the node keeps: the person's (`user`) and each
//! library's (`project/<library folder>`), in its vault
//! (docs/design/node-client-split.md, gap 13).
//!
//! Every client of one node reads and writes these two layers through it
//! ([`NodeSettings`], from [`crate::node::LocalNode::settings`]), so two
//! clients of one node see the same values. A write that changes a layer
//! is announced on the node's bus ([`LayerChanged`]) for its other
//! clients to hear.
//!
//! What stays on the client is classified by the setting's declared
//! concern (`helios_config::Concern`): the machine's settings (`global`)
//! live in each machine's file, and the interface's own (display, input,
//! window geometry) in the client's interface settings. The node refuses
//! any other layer name ([`is_node_layer`]).

use std::sync::Arc;

use helios_core::{Bus, Event};
use helios_vault::{Store, Vault};
use serde::Serialize;

/// The source the node's settings events are published under.
pub const SOURCE: &str = "node.settings";

/// The vault's name for the person's layer.
pub const USER: &str = "user";

/// The prefix of a library's project layer: `project/<library folder>`.
pub const PROJECT_PREFIX: &str = "project/";

/// Whether `name` is a layer the node keeps: the person's, or a library's.
pub fn is_node_layer(name: &str) -> bool {
    name == USER
        || name
            .strip_prefix(PROJECT_PREFIX)
            .is_some_and(|f| !f.is_empty())
}

/// A layer the node keeps changed: its name. Clients read its new text
/// from the node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LayerChanged {
    pub layer: String,
}

impl Event for LayerChanged {
    const KIND: &'static str = "settings.layerChanged";
}

/// A client's way to the settings layers the node keeps.
pub struct NodeSettings<S: Store + 'static> {
    vault: Arc<Vault<S>>,
    bus: Arc<Bus>,
}

impl<S: Store + 'static> Clone for NodeSettings<S> {
    fn clone(&self) -> Self {
        Self {
            vault: self.vault.clone(),
            bus: self.bus.clone(),
        }
    }
}

impl<S: Store + 'static> std::fmt::Debug for NodeSettings<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeSettings").finish_non_exhaustive()
    }
}

impl<S: Store + 'static> NodeSettings<S> {
    pub(crate) fn new(vault: Arc<Vault<S>>, bus: Arc<Bus>) -> Self {
        Self { vault, bus }
    }

    /// A layer's TOML text, `None` when the node keeps none under that
    /// name.
    ///
    /// # Errors
    /// `name` is not a layer the node keeps, or the vault cannot read it.
    pub fn read(&self, name: &str) -> Result<Option<String>, String> {
        refuse_others(name)?;
        self.vault.config_layer(name)
    }

    /// Keeps `text` as the layer `name`; when it changed, the change is
    /// announced on the node's bus.
    ///
    /// # Errors
    /// `name` is not a layer the node keeps, or the vault refuses the
    /// write (closed, or failing).
    pub fn write(&self, name: &str, text: &str) -> Result<(), String> {
        refuse_others(name)?;
        if self.vault.config_layer(name)?.as_deref() == Some(text) {
            return Ok(());
        }
        self.vault.set_config_layer(name, Some(text))?;
        self.bus.publish(
            SOURCE,
            None,
            &LayerChanged {
                layer: name.to_string(),
            },
        );
        Ok(())
    }
}

fn refuse_others(name: &str) -> Result<(), String> {
    if is_node_layer(name) {
        Ok(())
    } else {
        Err(format!(
            "the node keeps only the `{USER}` and `{PROJECT_PREFIX}<folder>` settings layers, not `{name}`"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::LocalNode;
    use helios_vault_sqlite::SqliteStore;
    use std::time::Duration;

    /// Two clients of one node see the same user setting change, and hear
    /// it; a client-only layer is not the node's to keep.
    #[test]
    fn two_clients_of_one_node_share_the_user_and_project_layers() {
        let dir = std::env::temp_dir().join(format!("helios-node-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = SqliteStore::open(&dir.join("vault.db")).unwrap();
        let bus = Arc::new(Bus::new());
        let node = LocalNode::open(store, bus.clone(), Duration::from_hours(1));
        let (one, two) = (node.settings(), node.settings());
        let heard = bus.subscribe();

        assert_eq!(two.read(USER).unwrap(), None);
        let text = "[interface.graph]\nzoom = \"detail\"\n";
        one.write(USER, text).unwrap();
        assert_eq!(two.read(USER).unwrap().as_deref(), Some(text));
        let event = heard.recv_timeout(Duration::from_secs(10)).expect("heard");
        assert_eq!(event.kind, LayerChanged::KIND);
        assert_eq!(event.payload["layer"], USER);

        // The same text again changes nothing, and says nothing.
        one.write(USER, text).unwrap();
        assert!(heard.try_recv().is_err(), "no change, no event");

        let project = "project//photos";
        two.write(project, "[project.decisions]\nmode = \"human\"\n")
            .unwrap();
        assert!(one.read(project).unwrap().unwrap().contains("human"));
        let event = heard.recv_timeout(Duration::from_secs(10)).expect("heard");
        assert_eq!(event.payload["layer"], project);

        // The machine's layer and the interface's stay on the client.
        for other in ["global", "interface", "project/", "run"] {
            assert!(one.write(other, "a = 1").is_err(), "{other} refused");
            assert!(one.read(other).is_err(), "{other} refused");
        }
        node.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
