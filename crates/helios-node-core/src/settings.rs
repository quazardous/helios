//! The settings layers the node keeps: the person's (`user`) and each
//! library's (`project/<library folder>`), in its vault
//! (docs/design/node-client-split.md, gap 13).
//!
//! Beside the person's layer, the node keeps the person's documents
//! (`user/<name>`): what is theirs but not a setting's value, such as the
//! import window's saved sources (`user/import-sources`), in TOML too.
//!
//! Every client of one node reads and writes these layers through it
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
//!
//! **The schema.** Every setting is declared by the crate that owns it, as
//! its own set; [`SETS`] gathers them in one place and [`schema`] builds
//! the one schema both the node and the desktop read the layers with (the
//! node keeps the person's layer, which the desktop writes, so both must
//! know every key in it).

use std::sync::Arc;

use helios_command::schema::text;
use helios_config::{Schema, Setting};
use helios_core::{Bus, Event};
use helios_vault::{Store, Vault};
use serde::Serialize;

use crate::commands::{CommandSpec, Need, object};

mod declared;

pub use declared::SETTINGS;

/// Every set of settings Helios knows, each declared by the crate that
/// owns it. `helios_config::SETTINGS` holds the only ones declared apart
/// from their owner (the render cache's size, read by
/// `helios_config::Config::cache_budget` for the engine below it).
pub const SETS: &[&[Setting]] = &[
    helios_config::SETTINGS,
    helios_thumbs::settings::SETTINGS,
    helios_shell::settings::SETTINGS,
    helios_storage::settings::SETTINGS,
    helios_library::settings::SETTINGS,
    SETTINGS,
    helios_decision::settings::SETTINGS,
];

/// Settings another component keeps rather than the layers, declared by
/// that component so a form draws them; never in the [`schema`].
pub const KEPT_ELSEWHERE: &[&[Setting]] = &[helios_library::settings::KEPT_ELSEWHERE];

/// The settings the node and the desktop know: every set of [`SETS`].
pub fn schema() -> Schema {
    Schema::new(SETS)
}

/// A setting declared in [`KEPT_ELSEWHERE`].
pub fn kept_elsewhere(key: &str) -> Option<&'static Setting> {
    KEPT_ELSEWHERE
        .iter()
        .flat_map(|set| set.iter())
        .find(|s| s.key == key)
}

/// The settings layers' node verbs: read and write a layer the node keeps
/// ([`NodeSettings`]).
pub fn verbs() -> Vec<CommandSpec> {
    let layer = || text("`user` or `project/<library folder>`");
    vec![
        CommandSpec::new("settings.layer.get", "Read a settings layer", "Settings")
            .params(object(&[("layer", layer(), true)]))
            .needs(Need::Vault)
            .read_only(),
        CommandSpec::new("settings.layer.set", "Write a settings layer", "Settings")
            .params(object(&[
                ("layer", layer(), true),
                ("text", text("TOML"), true),
            ]))
            .needs(Need::Vault),
    ]
}

/// The source the node's settings events are published under.
pub const SOURCE: &str = "node.settings";

/// The vault's name for the person's layer.
pub const USER: &str = "user";

/// The prefix of a library's project layer: `project/<library folder>`.
pub const PROJECT_PREFIX: &str = "project/";

/// The prefix of the person's documents: `user/<name>`.
pub const USER_DOCUMENT_PREFIX: &str = "user/";

/// Whether `name` is a layer the node keeps: the person's, one of the
/// person's documents, or a library's.
pub fn is_node_layer(name: &str) -> bool {
    name == USER
        || [PROJECT_PREFIX, USER_DOCUMENT_PREFIX]
            .iter()
            .any(|prefix| name.strip_prefix(prefix).is_some_and(|f| !f.is_empty()))
}

/// A layer the node keeps changed: its name. Clients read its new text
/// from the node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LayerChanged {
    pub layer: String,
}

impl Event for LayerChanged {
    const KIND: &'static str = "helios.settings.layerChanged";
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
            "the node keeps only the `{USER}`, `{USER_DOCUMENT_PREFIX}<name>` and `{PROJECT_PREFIX}<folder>` settings layers, not `{name}`"
        ))
    }
}

#[cfg(test)]
mod tests {
    /// Every key the settings held when they were all declared in
    /// `helios-config`, before each moved to the crate that owns it: a
    /// saved layer still loads only if none is lost or renamed.
    const BEFORE_THE_MOVE: &[&str] = &[
        "system.cache.budget",
        "system.cache.share",
        "system.cache.states",
        "system.cache.thumbnails",
        "system.cache.textures",
        "system.map.exclude",
        "system.map.defaults",
        "system.map.record",
        "system.map.limit",
        "system.map.keepMissing",
        "system.scheduler.workers",
        "system.ai.calls",
        "system.events.journal",
        "system.workflow.keep_days",
        "project.decisions.mode",
        "interface.panels.queue_when_empty",
        "library.double_click",
        "interface.graph.zoom",
        "import.automatic.paused",
        "import.automatic.ask_files",
        "import.automatic.ask_gb",
        "import.show_in_library",
        "interface.experimental.workshops",
    ];

    #[test]
    fn every_setting_is_declared_once_by_one_set() {
        for key in BEFORE_THE_MOVE {
            let sets = super::SETS
                .iter()
                .filter(|set| set.iter().any(|s| s.key == *key))
                .count();
            let times = super::SETS
                .iter()
                .flat_map(|set| set.iter())
                .filter(|s| s.key == *key)
                .count();
            assert_eq!((sets, times), (1, 1), "`{key}`");
        }
        let mut now: Vec<&str> = super::schema().iter().map(|s| s.key).collect();
        let mut before = BEFORE_THE_MOVE.to_vec();
        now.sort_unstable();
        before.sort_unstable();
        assert_eq!(now, before, "the schema holds exactly the keys it held");
    }

    #[test]
    fn kept_settings_do_not_collide_with_the_schema() {
        let schema = super::schema();
        let kept: Vec<&str> = super::KEPT_ELSEWHERE
            .iter()
            .flat_map(|set| set.iter())
            .map(|s| s.key)
            .collect();
        assert_eq!(kept, ["library.folder"]);
        for key in kept {
            assert!(schema.get(key).is_none(), "`{key}` is in the schema");
            assert!(super::kept_elsewhere(key).is_some());
        }
    }

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

        // One of the person's documents is the node's too.
        let doc = "user/import-sources";
        one.write(doc, "[[source]]\nconnector = \"folder\"\n")
            .unwrap();
        assert!(two.read(doc).unwrap().unwrap().contains("folder"));
        let event = heard.recv_timeout(Duration::from_secs(10)).expect("heard");
        assert_eq!(event.payload["layer"], doc);

        // The machine's layer and the interface's stay on the client.
        for other in ["global", "interface", "project/", "user/", "run"] {
            assert!(one.write(other, "a = 1").is_err(), "{other} refused");
            assert!(one.read(other).is_err(), "{other} refused");
        }
        node.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
