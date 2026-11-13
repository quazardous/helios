//! Where the Settings app keeps Helios's settings: a store egui-pop calls
//! ([`SettingsStore`]), made of Helios's settings over the node
//! ([`NodeSettingsStore`]) and a file store for everything else
//! ([`composite`]).
//!
//! | setting | scope | kept in |
//! |---|---|---|
//! | Helios's, the person's | `app` | the node's `user` layer |
//! | Helios's, the machine's or a library default | `app` | the machine's file ([`GLOBAL_FILE`], which `helios-node` reads too) |
//! | Helios's, a library default | `library` | the node's `project/<folder>` layer |
//! | the interface's own (egui-pop's) | `app` | [`INTERFACE_FILE`] in the configuration folder |
//! | any | `workspace` | the workspace's [`WORKSPACE_FILE`] |
//!
//! A layer another client of the node changes is heard on the node's bus
//! ([`NodeSettingsStore::follow`]) and told to egui-pop's watch: the
//! Settings window shows the new values, and nothing is written back.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use super::{
    GLOBAL_FILE, LIBRARY, Settings, USER_LAYER, WORKSPACE, from_json, library_values, lock,
};
use egui_pop::api::admin::model::ValueScope;
use egui_pop::api::admin::store::{
    Changes, CompositeStore, FileStore, OnChange, SettingsStore, Values, diff,
};
use helios_config::{Concern, Layer, Schema};

/// The interface's own settings (egui-pop's, not Helios's), in the
/// configuration folder.
pub const INTERFACE_FILE: &str = "interface.json";

/// The settings layers the node keeps, as a client reads and writes them
/// (`helios_node_core::settings::NodeSettings`).
pub trait NodeLayers: Send + Sync + 'static {
    /// A layer's TOML text, `None` when the node keeps none.
    ///
    /// # Errors
    /// The node cannot read it.
    fn read(&self, name: &str) -> Result<Option<String>, String>;

    /// Keeps `text` as the layer `name`.
    ///
    /// # Errors
    /// The node refuses the write.
    fn write(&self, name: &str, text: &str) -> Result<(), String>;
}

impl<S: helios_vault::Store + 'static> NodeLayers for helios_node_core::settings::NodeSettings<S>
where
    Self: Send + Sync,
{
    fn read(&self, name: &str) -> Result<Option<String>, String> {
        helios_node_core::settings::NodeSettings::read(self, name)
    }

    fn write(&self, name: &str, text: &str) -> Result<(), String> {
        helios_node_core::settings::NodeSettings::write(self, name, text)
    }
}

/// Helios's settings (`helios-config`'s), kept in their layers: the
/// person's and the library's through the node, the machine's in its
/// file. Clones share the values.
#[derive(Clone)]
pub struct NodeSettingsStore {
    inner: Arc<Inner>,
}

struct Inner {
    settings: Mutex<Settings>,
    /// The node, when a vault is open: without one the person's and the
    /// library's layers are not kept.
    node: Option<Arc<dyn NodeLayers>>,
    /// The machine's file.
    machine: PathBuf,
    keys: BTreeSet<&'static str>,
    watchers: Mutex<BTreeMap<ValueScope, OnChange>>,
}

impl std::fmt::Debug for NodeSettingsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeSettingsStore")
            .field("machine", &self.inner.machine)
            .finish_non_exhaustive()
    }
}

impl NodeSettingsStore {
    /// The settings as resolved at the start, the node keeping the
    /// person's and the library's layers, and the folder holding the
    /// machine's file.
    pub fn new(
        settings: Settings,
        node: Option<Arc<dyn NodeLayers>>,
        config_dir: &Path,
    ) -> NodeSettingsStore {
        NodeSettingsStore {
            inner: Arc::new(Inner {
                settings: Mutex::new(settings),
                node,
                machine: config_dir.join(GLOBAL_FILE),
                keys: Schema::core().iter().map(|s| s.key).collect(),
                watchers: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    /// The settings as kept now.
    pub fn settings(&self) -> Settings {
        lock(&self.inner.settings).clone()
    }

    /// Follows the node's bus (`heard`, from `Bus::subscribe`): a layer
    /// it says changed is read again and what moved is told to the
    /// watchers. A change this store wrote itself reads as no change.
    /// Ends with the bus.
    pub fn follow(&self, heard: Receiver<helios_core::Envelope>) {
        let me = self.clone();
        let picked = std::thread::Builder::new()
            .name("helios-settings-follow".into())
            .spawn(move || {
                let kind = <helios_node_core::settings::LayerChanged as helios_core::Event>::KIND;
                while let Ok(e) = heard.recv() {
                    if e.kind != kind {
                        continue;
                    }
                    if let Some(name) = e.payload["layer"].as_str() {
                        me.layer_changed(name);
                    }
                }
            });
        if let Err(e) = picked {
            log::warn!("settings: the node's layers are not followed: {e}");
        }
    }

    /// Reads a layer the node said changed again, and tells the watchers
    /// what moved.
    fn layer_changed(&self, name: &str) {
        let Some(node) = &self.inner.node else { return };
        let (app, library) = {
            let mut settings = lock(&self.inner.settings);
            let Some(layer) = settings.node_layer(name) else {
                return;
            };
            let text = match node.read(name) {
                Ok(t) => t,
                Err(e) => {
                    log::warn!("settings: the {} layer not read again: {e}", layer.name());
                    return;
                }
            };
            let before = (app_values(&settings), library_values(&settings));
            settings.reload(layer, name, text.as_deref());
            (
                diff(&before.0, &app_values(&settings)),
                diff(&before.1, &library_values(&settings)),
            )
        };
        let watchers = lock(&self.inner.watchers).clone();
        for (scope, changes) in [(ValueScope::App, app), (LIBRARY.scope, library)] {
            if changes.is_empty() {
                continue;
            }
            if let Some(w) = watchers.get(&scope) {
                w(scope, changes);
            }
        }
    }

    /// Writes the layers whose values changed.
    fn write_changed(&self, settings: &mut Settings) -> Result<(), String> {
        let project = settings.project().map(String::from);
        let mut errors = Vec::new();
        for (layer, text) in settings.take_changed() {
            let written = match layer {
                Layer::Global => {
                    let file = &self.inner.machine;
                    file.parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|()| std::fs::write(file, &text))
                        .map_err(|e| format!("{}: {e}", file.display()))
                }
                Layer::User | Layer::Project => {
                    let name = if layer == Layer::User {
                        Some(USER_LAYER.to_string())
                    } else {
                        project.clone()
                    };
                    match (&self.inner.node, name) {
                        (Some(node), Some(name)) => node.write(&name, &text),
                        _ => Ok(()),
                    }
                }
                _ => Ok(()),
            };
            if let Err(e) = written {
                errors.push(format!("the {} settings: {e}", layer.name()));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

/// Every Helios setting's `app` value, by id.
fn app_values(settings: &Settings) -> Values {
    super::app_values(settings)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

impl SettingsStore for NodeSettingsStore {
    fn scopes(&self) -> Vec<ValueScope> {
        let mut out = vec![ValueScope::App];
        if lock(&self.inner.settings).project().is_some() {
            out.push(LIBRARY.scope);
        }
        out
    }

    fn claims(&self, setting: &str) -> bool {
        self.inner.keys.contains(setting)
    }

    fn load(&self, scope: ValueScope) -> Result<Values, String> {
        let settings = lock(&self.inner.settings);
        if scope == ValueScope::App {
            Ok(app_values(&settings))
        } else if scope == LIBRARY.scope {
            Ok(library_values(&settings))
        } else {
            Err(format!("Helios keeps no {} settings", scope.name()))
        }
    }

    fn save(&self, scope: ValueScope, changes: &Changes) -> Result<(), String> {
        let schema = Schema::core();
        let mut settings = lock(&self.inner.settings);
        let mut errors = Vec::new();
        for (key, value) in changes {
            let Some(s) = schema.get(key) else { continue };
            let layer = if scope == ValueScope::App {
                Settings::home(s.concern)
            } else if scope == LIBRARY.scope && s.concern == Concern::Project {
                Layer::Project
            } else {
                errors.push(format!("`{key}` is not kept per {}", scope.name()));
                continue;
            };
            let value = value.as_ref().and_then(|j| from_json(&s.kind, j));
            if let Err(e) = settings.set(layer, key, value) {
                errors.push(format!("{key}: {e}"));
            }
        }
        if let Err(e) = self.write_changed(&mut settings) {
            errors.push(e);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn watch(&self, scope: ValueScope, on_change: OnChange) -> Result<(), String> {
        lock(&self.inner.watchers).insert(scope, on_change);
        Ok(())
    }
}

/// The store the Settings app is given: Helios's settings through
/// `node`, everything else in a file store, the interface's own in
/// [`INTERFACE_FILE`] in `config_dir` and the open workspace's in its
/// file.
pub fn composite(
    node: &NodeSettingsStore,
    config_dir: &Path,
    workspace: Option<&Path>,
) -> CompositeStore {
    let mut files = FileStore::new().file(ValueScope::App, config_dir.join(INTERFACE_FILE));
    if let Some(file) = workspace {
        files = files.file(WORKSPACE.scope, file);
    }
    CompositeStore::new(files).handler(node.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Sources, to_json};
    use serde_json::{Value as Json, json};
    use std::time::Duration;

    /// The node's layers, kept in memory, with every write recorded.
    #[derive(Default)]
    struct Layers(Mutex<BTreeMap<String, String>>);

    impl NodeLayers for Layers {
        fn read(&self, name: &str) -> Result<Option<String>, String> {
            Ok(lock(&self.0).get(name).cloned())
        }

        fn write(&self, name: &str, text: &str) -> Result<(), String> {
            lock(&self.0).insert(name.into(), text.into());
            Ok(())
        }
    }

    /// A setting's value in a Helios layer, as JSON.
    fn layer_value(settings: &Settings, layer: Layer, key: &str) -> Option<Json> {
        let s = Schema::core().get(key)?;
        Some(to_json(&s.kind, &settings.config().value_in(layer, key)?))
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("helios-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_composite_routes_helios_settings_to_the_node_and_the_rest_to_the_file() {
        let dir = temp("routes");
        let layers = Arc::new(Layers::default());
        let node = NodeSettingsStore::new(
            Settings::resolve(&Sources::default()),
            Some(layers.clone()),
            &dir,
        );
        let store = composite(&node, &dir, Some(&dir.join("workspace.json")));
        assert_eq!(store.owner("interface.graph.zoom"), Some(0));
        assert_eq!(store.owner("project.decisions.mode"), Some(0));
        assert_eq!(store.owner("system.ai.calls"), Some(0));
        assert_eq!(store.owner("display.wide-gamut-preview"), None);
        assert!(store.problems().is_empty());

        let changes: Changes = [
            ("interface.graph.zoom".to_string(), Some(json!("overview"))),
            ("system.ai.calls".to_string(), Some(json!(6))),
            ("display.wide-gamut-preview".to_string(), Some(json!(true))),
        ]
        .into();
        store.save(ValueScope::App, &changes).unwrap();
        // The person's setting in the node's `user` layer.
        let user = layers.read(USER_LAYER).unwrap().unwrap();
        assert!(user.contains("overview"), "{user}");
        // The machine's in its file.
        let machine = std::fs::read_to_string(dir.join(GLOBAL_FILE)).unwrap();
        assert!(machine.contains("calls = 6"), "{machine}");
        // The interface's own in the file store, at the path given.
        let file: Json =
            serde_json::from_str(&std::fs::read_to_string(dir.join(INTERFACE_FILE)).unwrap())
                .unwrap();
        assert_eq!(file, json!({ "display.wide-gamut-preview": true }));
        // A load merges what each keeps.
        let loaded = store.load(ValueScope::App).unwrap();
        assert_eq!(loaded["interface.graph.zoom"], json!("overview"));
        assert_eq!(loaded["system.ai.calls"], json!(6));
        assert_eq!(loaded["display.wide-gamut-preview"], json!(true));
        // A workspace value of the interface's goes to its file.
        store
            .save(
                WORKSPACE.scope,
                &[("display.wide-gamut-preview".to_string(), Some(json!(false)))].into(),
            )
            .unwrap();
        assert!(dir.join("workspace.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A library default round-trips through an in-process node, and a
    /// change another client makes there is told to the watcher.
    #[test]
    fn a_library_setting_round_trips_through_the_node_and_its_changes_are_watched() {
        use helios_node_core::node::LocalNode;
        use helios_vault_sqlite::SqliteStore;

        let dir = temp("node");
        std::fs::create_dir_all(&dir).unwrap();
        let sqlite = SqliteStore::open(&dir.join("vault.db")).unwrap();
        let bus = Arc::new(helios_core::Bus::new());
        let node = LocalNode::open(sqlite, bus.clone(), Duration::from_hours(1));
        let project = "project//photos";
        let sources = |node: &LocalNode<SqliteStore>| {
            let layers = node.settings();
            crate::config::read(&dir, &|n| layers.read(n), Some(project.into()), &[]).0
        };

        let mine = NodeSettingsStore::new(
            Settings::resolve(&sources(&node)),
            Some(Arc::new(node.settings())),
            &dir,
        );
        assert_eq!(mine.scopes(), [ValueScope::App, LIBRARY.scope]);
        mine.save(
            LIBRARY.scope,
            &[("project.decisions.mode".to_string(), Some(json!("human")))].into(),
        )
        .unwrap();
        let kept = node.settings().read(project).unwrap().unwrap();
        assert!(kept.contains("human"), "{kept}");
        // A store started afresh reads it back from the node.
        let again = NodeSettingsStore::new(
            Settings::resolve(&sources(&node)),
            Some(Arc::new(node.settings())),
            &dir,
        );
        assert_eq!(
            again.load(LIBRARY.scope).unwrap()["project.decisions.mode"],
            json!("human")
        );
        // An app-wide setting is refused a library value.
        assert!(
            mine.save(
                LIBRARY.scope,
                &[("system.ai.calls".to_string(), Some(json!(3)))].into()
            )
            .is_err()
        );

        // Watched: another client's change comes in, this store's own
        // write does not.
        mine.follow(bus.subscribe());
        let (tell, heard) = std::sync::mpsc::channel();
        let tell = Mutex::new(tell);
        mine.watch(
            LIBRARY.scope,
            Arc::new(move |scope, changes| {
                let _ = lock(&tell).send((scope, changes));
            }),
        )
        .unwrap();
        mine.save(
            LIBRARY.scope,
            &[("project.decisions.mode".to_string(), Some(json!("full_ai")))].into(),
        )
        .unwrap();
        node.settings()
            .write(project, "[project.decisions]\nmode = \"human\"\n")
            .unwrap();
        let (scope, changes) = heard.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(scope, LIBRARY.scope);
        assert_eq!(
            changes,
            [("project.decisions.mode".to_string(), Some(json!("human")))].into()
        );
        assert!(
            heard.recv_timeout(Duration::from_millis(200)).is_err(),
            "its own write is no change"
        );
        assert_eq!(
            layer_value(&mine.settings(), Layer::Project, "project.decisions.mode"),
            Some(json!("human"))
        );
        node.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
